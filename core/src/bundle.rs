//! Checksum-pinned ZIP import with explicit path policy, staging and recovery.
//! The archive never writes directly into the active engine directory.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

const MAX_EXPANDED: u64 = 256 * 1024 * 1024;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 2048;
const MANIFEST: &str = ".tandem-manifest.json";
const JOURNAL: &str = "deploy-recovery.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub tag: String,
    pub sha256: String,
    pub file_count: usize,
    pub expanded_bytes: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Recovery {
    Install { had_engine: bool },
    Rollback { target_sha256: String },
}

fn archive_path(name: &str) -> Result<PathBuf> {
    if name.len() > 200 || name.contains('\\') || name.starts_with('/') {
        return Err(Error::Security("Unsafe archive path".into()));
    }
    let name = name.strip_suffix('/').unwrap_or(name);
    let mut result = PathBuf::new();
    for component in name.split('/') {
        crate::files::safe_component(component)?;
        result.push(component);
    }
    if result.as_os_str().is_empty() {
        return Err(Error::Security("Empty archive path".into()));
    }
    Ok(result)
}

fn checked_archive<'a>(
    bytes: &'a [u8],
    sha256: &str,
) -> Result<(zip::ZipArchive<Cursor<&'a [u8]>>, usize, u64)> {
    let expected = crate::files::validate_digest(sha256)?;
    if bytes.is_empty()
        || bytes.len() as u64 > crate::network::MAX_ARCHIVE
        || crate::files::sha256(bytes) != expected
    {
        return Err(Error::Security(
            "Archive SHA-256/size does not match the approved artifact".into(),
        ));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| Error::Invalid(format!("Invalid ZIP: {e}")))?;
    if archive.len() == 0 || archive.len() > MAX_ENTRIES {
        return Err(Error::Invalid("ZIP entry-count limit exceeded".into()));
    }
    let mut seen = HashSet::new();
    let mut total = 0u64;
    let mut files = 0;
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|e| Error::Invalid(format!("ZIP entry error: {e}")))?;
        let path = archive_path(file.name())?;
        if !seen.insert(path.to_string_lossy().to_ascii_lowercase()) {
            return Err(Error::Security("Duplicate/case-colliding ZIP entry".into()));
        }
        if file
            .unix_mode()
            .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000 | 0o040000))
        {
            return Err(Error::Security(
                "Links and special files in ZIP are forbidden".into(),
            ));
        }
        if path
            .file_name()
            .is_some_and(|name| name == MANIFEST || name == "ipset-source.txt")
        {
            return Err(Error::Security(
                "ZIP contains a reserved Tandem metadata filename".into(),
            ));
        }
        total = total
            .checked_add(file.size())
            .ok_or_else(|| Error::Invalid("ZIP size overflow".into()))?;
        if file.size() > MAX_FILE || total > MAX_EXPANDED {
            return Err(Error::Invalid("ZIP expanded-size limit exceeded".into()));
        }
        if !file.is_dir() {
            files += 1;
        }
    }
    Ok((archive, files, total))
}

pub fn inspect(bytes: &[u8], sha256: &str, tag: &str) -> Result<Manifest> {
    crate::zapret::validate_tag(tag)?;
    let (_, file_count, expanded_bytes) = checked_archive(bytes, sha256)?;
    Ok(Manifest {
        schema_version: 1,
        tag: tag.into(),
        sha256: crate::files::validate_digest(sha256)?,
        file_count,
        expanded_bytes,
    })
}

fn remove_directory(path: &Path) -> Result<()> {
    crate::files::reject_links(path)?;
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}
fn payload_root(staging: &Path) -> Result<PathBuf> {
    if staging.join("bin/winws.exe").is_file() {
        return Ok(staging.to_path_buf());
    }
    let mut entries = fs::read_dir(staging)?.collect::<std::io::Result<Vec<_>>>()?;
    if entries.len() == 1 {
        let candidate = entries.remove(0);
        if candidate.file_type()?.is_dir() && candidate.path().join("bin/winws.exe").is_file() {
            return Ok(candidate.path());
        }
    }
    Err(Error::Invalid(
        "ZIP must contain bin/winws.exe at root or under one wrapper directory".into(),
    ))
}
fn validate_payload(path: &Path) -> Result<()> {
    for name in ["bin/winws.exe", "bin/WinDivert.dll", "bin/WinDivert64.sys"] {
        let bytes = crate::files::read_bounded(&path.join(name), MAX_FILE)?;
        if bytes.len() < 64 || !bytes.starts_with(b"MZ") {
            return Err(Error::Invalid(format!(
                "Missing or invalid Windows PE file: {name}"
            )));
        }
    }
    let lists = path.join("lists");
    if !lists.is_dir() {
        return Err(Error::Invalid("Bundle has no lists/ directory".into()));
    }
    // Upstream service.bat creates these empty optional files. We implement
    // that behavior directly instead of executing the upstream batch script.
    for name in [
        "list-general-user.txt",
        "list-exclude-user.txt",
        "ipset-exclude-user.txt",
    ] {
        if !lists.join(name).exists() {
            crate::files::atomic_write(&lists.join(name), b"")?;
        }
    }
    let manager = crate::zapret::ZapretManager::new(path);
    if manager.list_strategies()?.is_empty() {
        return Err(Error::Invalid("Bundle has no strategy .bat files".into()));
    }
    // List mode starts as Loaded; copying source is non-destructive.
    manager.apply_ipset(crate::config::IpsetFilter::Loaded)?;
    // Strategies may depend on a later supported parser revision. They are
    // validated again on explicit preview/install and are never run as scripts.
    Ok(())
}

pub fn manifest(engine: &Path) -> Result<Option<Manifest>> {
    let path = engine.join(MANIFEST);
    if !path.exists() {
        return Ok(None);
    }
    let manifest: Manifest =
        serde_json::from_slice(&crate::files::read_bounded(&path, 16 * 1024)?)?;
    if manifest.schema_version != 1 {
        return Err(Error::Invalid("Unknown bundle manifest schema".into()));
    }
    crate::zapret::validate_tag(&manifest.tag)?;
    crate::files::validate_digest(&manifest.sha256)?;
    Ok(Some(manifest))
}

/// Caller must hold operation.lock and ensure the service is absent.
pub fn install(root: &Path, bytes: &[u8], sha256: &str, tag: &str) -> Result<Manifest> {
    if root.join(JOURNAL).exists() {
        return Err(Error::Operation(
            "Deployment recovery is required before importing another bundle".into(),
        ));
    }
    let receipt = inspect(bytes, sha256, tag)?;
    let staging = root.join("engine.unpack");
    let next = root.join("engine.next");
    remove_directory(&staging)?;
    remove_directory(&next)?;
    fs::create_dir(&staging)?;
    let staged: Result<()> = (|| {
        let (mut archive, _, _) = checked_archive(bytes, sha256)?;
        let mut expanded = 0u64;
        for index in 0..archive.len() {
            let mut file = archive
                .by_index(index)
                .map_err(|e| Error::Invalid(e.to_string()))?;
            let destination = staging.join(archive_path(file.name())?);
            if destination.to_string_lossy().encode_utf16().count() > 240 {
                return Err(Error::Invalid(
                    "Expanded path is too long for the Windows engine".into(),
                ));
            }
            if file.is_dir() {
                fs::create_dir_all(&destination)?;
                continue;
            }
            let parent = destination
                .parent()
                .ok_or_else(|| Error::Security("Archive entry has no parent".into()))?;
            fs::create_dir_all(parent)?;
            let mut output = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&destination)?;
            let mut buffer = [0u8; 32768];
            let mut written = 0u64;
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                written += count as u64;
                expanded += count as u64;
                if written > MAX_FILE || expanded > MAX_EXPANDED {
                    return Err(Error::Invalid("Decompressed data exceeded policy".into()));
                }
                output.write_all(&buffer[..count])?;
            }
            if written != file.size() {
                return Err(Error::Invalid("ZIP size metadata mismatch".into()));
            }
            output.sync_all()?;
        }
        let payload = payload_root(&staging)?;
        validate_payload(&payload)?;
        crate::files::atomic_write(
            &payload.join(MANIFEST),
            &serde_json::to_vec_pretty(&receipt)?,
        )?;
        fs::rename(&payload, &next)?;
        if staging.exists() {
            fs::remove_dir_all(&staging)?;
        }
        Ok(())
    })();
    if let Err(error) = staged {
        let cleanup = remove_directory(&staging).and_then(|_| remove_directory(&next));
        return Err(Error::Operation(format!(
            "Bundle staging failed: {error}; cleanup: {}",
            if cleanup.is_ok() {
                "complete".into()
            } else {
                cleanup.unwrap_err().to_string()
            }
        )));
    }
    let engine = root.join("engine");
    let previous = root.join("engine.previous");
    remove_directory(&previous)?;
    crate::files::atomic_write(
        &root.join(JOURNAL),
        &serde_json::to_vec(&Recovery::Install {
            had_engine: engine.exists(),
        })?,
    )?;
    let promoted = (|| -> Result<()> {
        if engine.exists() {
            fs::rename(&engine, &previous)?;
        }
        fs::rename(&next, &engine)?;
        Ok(())
    })();
    if let Err(error) = promoted {
        return match recover(root) {
            Ok(_) => Err(Error::Operation(format!(
                "Deployment failed: {error}; previous installation restored"
            ))),
            Err(recovery) => Err(Error::Operation(format!(
                "Deployment failed: {error}; recovery failed: {recovery}; journal retained"
            ))),
        };
    }
    fs::remove_file(root.join(JOURNAL))?;
    Ok(receipt)
}

pub fn rollback(root: &Path) -> Result<()> {
    if root.join(JOURNAL).exists() {
        return Err(Error::Operation("Run deployment recovery first".into()));
    }
    let previous = manifest(&root.join("engine.previous"))?
        .ok_or_else(|| Error::Operation("No previous managed bundle is available".into()))?;
    crate::files::atomic_write(
        &root.join(JOURNAL),
        &serde_json::to_vec(&Recovery::Rollback {
            target_sha256: previous.sha256,
        })?,
    )?;
    recover(root)?;
    Ok(())
}

pub fn recover(root: &Path) -> Result<bool> {
    let journal = root.join(JOURNAL);
    if !journal.exists() {
        return Ok(false);
    }
    let record: Recovery =
        serde_json::from_slice(&crate::files::read_bounded(&journal, 16 * 1024)?)?;
    let engine = root.join("engine");
    let previous = root.join("engine.previous");
    let next = root.join("engine.next");
    match record {
        Recovery::Install { had_engine } => {
            if had_engine && previous.exists() {
                remove_directory(&engine)?;
                fs::rename(&previous, &engine)?;
            } else if !had_engine {
                remove_directory(&engine)?;
            } else if !engine.exists() {
                return Err(Error::Operation(
                    "Neither active nor previous engine exists; manual recovery is required".into(),
                ));
            }
            remove_directory(&next)?;
        }
        Recovery::Rollback { target_sha256 } => {
            crate::files::validate_digest(&target_sha256)?;
            let swap = root.join("engine.swap");
            let already_active =
                manifest(&engine)?.is_some_and(|manifest| manifest.sha256 == target_sha256);
            if !already_active {
                if engine.exists() {
                    if swap.exists() {
                        return Err(Error::Operation(
                            "Ambiguous rollback directories; manual review required".into(),
                        ));
                    }
                    fs::rename(&engine, &swap)?;
                }
                if !previous.exists() {
                    return Err(Error::Operation("Rollback source is missing".into()));
                }
                fs::rename(&previous, &engine)?;
            }
            if swap.exists() {
                if previous.exists() {
                    return Err(Error::Operation(
                        "Rollback backup collision; no files deleted".into(),
                    ));
                }
                fs::rename(swap, &previous)?;
            }
            if !manifest(&engine)?.is_some_and(|manifest| manifest.sha256 == target_sha256) {
                return Err(Error::Security(
                    "Rollback result does not match expected bundle".into(),
                ));
            }
        }
    }
    fs::remove_file(journal)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_paths_are_platform_independent() {
        for path in [
            "../evil",
            "/root",
            "C:/evil",
            "x\\evil",
            "x/CON.txt",
            "a//b",
            "a/../b",
        ] {
            assert!(archive_path(path).is_err(), "{path}");
        }
        assert!(archive_path("release/general (ALT).bat").is_ok());
    }
    #[test]
    fn digest_is_checked_before_parsing() {
        assert!(inspect(b"not a zip", &"0".repeat(64), "1.0").is_err());
    }
    #[test]
    fn duplicate_case_collision_fails() {
        let mut data = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut data);
            for name in ["a.txt", "A.txt"] {
                writer
                    .start_file(name, zip::write::FileOptions::default())
                    .unwrap();
                writer.write_all(b"x").unwrap();
            }
            writer.finish().unwrap();
        }
        let bytes = data.into_inner();
        assert!(inspect(&bytes, &crate::files::sha256(&bytes), "1.0").is_err());
    }
    #[test]
    fn interrupted_promotion_restores_previous() {
        let root = crate::files::test_dir("deploy");
        fs::create_dir(root.join("engine.previous")).unwrap();
        crate::files::atomic_write(&root.join("engine.previous/a"), b"old").unwrap();
        crate::files::atomic_write(
            &root.join(JOURNAL),
            &serde_json::to_vec(&Recovery::Install { had_engine: true }).unwrap(),
        )
        .unwrap();
        assert!(recover(&root).unwrap());
        assert_eq!(fs::read(root.join("engine/a")).unwrap(), b"old");
        fs::remove_dir_all(root).unwrap();
    }
}
