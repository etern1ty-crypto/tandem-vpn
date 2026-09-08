//! Bounded I/O, atomic replacement, path policy and interprocess exclusion.
use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn validate_digest(value: &str) -> Result<String> {
    if value.len() != 64 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::Invalid("SHA-256 must contain exactly 64 hexadecimal characters from an independently trusted source".into()));
    }
    Ok(value.to_ascii_lowercase())
}

pub fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    reject_links(path)?;
    let file = File::open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > limit {
        return Err(Error::Invalid(format!(
            "Not a regular file or exceeds {limit} bytes: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::Invalid(
            "Input exceeded size limit while reading".into(),
        ));
    }
    Ok(bytes)
}

pub fn read_text(path: &Path, limit: u64) -> Result<String> {
    String::from_utf8(read_bounded(path, limit)?)
        .map_err(|_| Error::Invalid(format!("Expected UTF-8: {}", path.display())))
}

/// Validate a single Windows filename even when tests run on Unix.
pub fn safe_component(name: &str) -> Result<()> {
    let reserved = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 180
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c.is_control() || "\\/:<>\"|?*".contains(c))
        || matches!(
            reserved.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        )
        || ["COM¹", "COM²", "COM³", "LPT¹", "LPT²", "LPT³"].contains(&reserved.as_str())
        || (reserved.len() == 4
            && (reserved.starts_with("COM") || reserved.starts_with("LPT"))
            && matches!(reserved.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(Error::Security(format!(
            "Unsafe Windows path component: {name:?}"
        )));
    }
    Ok(())
}

/// Reject symlinks, junctions and reparse points in every existing component.
pub fn reject_links(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(Error::Security("Parent traversal is forbidden".into()));
        }
        current.push(component);
        if matches!(
            component,
            std::path::Component::Prefix(_) | std::path::Component::RootDir
        ) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(Error::Security("Symbolic links are forbidden".into()));
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err(Error::Security(
                            "Windows reparse points are forbidden".into(),
                        ));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Write beside the destination, sync, then replace without a truncate window.
/// On Windows ReplaceFileW preserves the destination's ACL; new files inherit.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    reject_links(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("File has no parent directory".into()))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".tandem-{}-{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    let guard = Temporary(tmp.clone());
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    #[cfg(windows)]
    crate::platform::replace_file(&tmp, path)?;
    #[cfg(not(windows))]
    {
        fs::rename(&tmp, path)?;
        File::open(parent)?.sync_all()?;
    }
    drop(guard);
    Ok(())
}

/// Keep the lock file, not a stale PID. The OS releases its lock on crash/exit.
pub struct OperationLock {
    _file: File,
}
impl OperationLock {
    pub fn acquire(root: &Path) -> Result<Self> {
        let path = root.join("operation.lock");
        reject_links(&path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(fs::TryLockError::WouldBlock) => Err(Error::Busy),
            Err(fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }
}

#[cfg(test)]
pub(crate) fn test_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tandem-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_windows_special_names() {
        for value in [
            "../a", "C:evil", "x\\a", "NUL", "con.txt", "COM1", "x.", "a ", "..", "a\0b",
        ] {
            assert!(safe_component(value).is_err(), "{value}");
        }
        for value in ["general (ALT2).bat", ".service", "ipset-all.txt"] {
            safe_component(value).unwrap();
        }
    }
    #[test]
    fn atomic_replace_and_limit() {
        let root = test_dir("atomic");
        let path = root.join("a");
        atomic_write(&path, b"before").unwrap();
        atomic_write(&path, b"after").unwrap();
        assert_eq!(read_bounded(&path, 10).unwrap(), b"after");
        assert!(read_bounded(&path, 2).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn lock_releases_when_dropped() {
        let root = test_dir("lock");
        let first = OperationLock::acquire(&root).unwrap();
        assert!(matches!(OperationLock::acquire(&root), Err(Error::Busy)));
        drop(first);
        drop(OperationLock::acquire(&root).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn digest_vector() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
