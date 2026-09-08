//! Validated, idempotent hosts blocks with conflict-aware recovery.
use crate::files::{atomic_write, read_text, sha256};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::Path;

const START: &str = "# --- tandem-vpn zapret start ---";
const END: &str = "# --- tandem-vpn zapret end ---";
const LIMIT: u64 = 2 * 1024 * 1024;
const JOURNAL: &str = "hosts-recovery.json";

pub fn allowed_domain(domain: &str, suffixes: &[String]) -> bool {
    let domain = domain.to_ascii_lowercase();
    crate::config::valid_domain(&domain)
        && suffixes.iter().any(|suffix| {
            let suffix = suffix.to_ascii_lowercase();
            domain == suffix || domain.ends_with(&format!(".{suffix}"))
        })
}

/// Conservative public-address policy. Special/private/documentation ranges
/// are rejected; IPv4-mapped IPv6 is rejected to avoid policy bypasses.
pub fn public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
                || (a == 100 && (64..=127).contains(&b))
                || (a == 198 && (b == 18 || b == 19))
                || (a == 192 && b == 0 && (c == 0 || c == 2))
                || (a == 192 && b == 88 && c == 99)
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            // Only ordinary global unicast 2000::/3; exclude special ranges,
            // documentation, 6to4, Teredo, benchmarking and ORCHID.
            s[0] & 0xe000 == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

pub fn validate_entries(text: &str, suffixes: &[String]) -> Result<Vec<(String, IpAddr)>> {
    if text.len() as u64 > LIMIT {
        return Err(Error::Invalid("Hosts input exceeds 2 MiB".into()));
    }
    let mut entries = BTreeMap::new();
    for (index, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        if line.trim() == START || line.trim() == END {
            return Err(Error::Security(
                "Remote hosts input contains management markers".into(),
            ));
        }
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let address = parts
            .next()
            .unwrap_or_default()
            .parse::<IpAddr>()
            .map_err(|_| Error::Invalid(format!("Invalid hosts address at line {}", index + 1)))?;
        if !public_ip(address) {
            return Err(Error::Security(format!(
                "Non-public hosts address at line {}",
                index + 1
            )));
        }
        let names: Vec<_> = parts.collect();
        if names.is_empty() {
            return Err(Error::Invalid("Hosts entry has no hostname".into()));
        }
        for name in names {
            let name = name.to_ascii_lowercase();
            if !allowed_domain(&name, suffixes) {
                return Err(Error::Security(format!(
                    "Hostname is outside the explicit allowlist: {name}"
                )));
            }
            if let Some(previous) = entries.insert(name.clone(), address) {
                if previous != address {
                    return Err(Error::Invalid(format!("Conflicting addresses for {name}")));
                }
            }
            if entries.len() > 10000 {
                return Err(Error::Invalid("Too many hosts entries".into()));
            }
        }
    }
    if entries.is_empty() {
        return Err(Error::Invalid("Hosts input has no allowed entries".into()));
    }
    Ok(entries.into_iter().collect())
}

/// Preserve all unmanaged bytes (including CRLF, comments and blank lines).
pub fn merge(current: &str, remote: &str, suffixes: &[String]) -> Result<String> {
    let entries = validate_entries(remote, suffixes)?;
    let mut start = None;
    let mut end = None;
    let mut offset = 0;
    for line in current.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']).trim() == START {
            if start.is_some() || end.is_some() {
                return Err(Error::Invalid("Duplicate/nested hosts start marker".into()));
            }
            start = Some(offset);
        }
        if line.trim_end_matches(['\r', '\n']).trim() == END {
            if start.is_none() || end.is_some() {
                return Err(Error::Invalid(
                    "Unmatched/duplicate hosts end marker".into(),
                ));
            }
            end = Some(offset + line.len());
        }
        offset += line.len();
    }
    if start.is_some() != end.is_some() {
        return Err(Error::Invalid(
            "Unbalanced hosts markers; file was not modified".into(),
        ));
    }
    let newline = if current.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut block = format!("{START}{newline}");
    for (domain, address) in entries {
        block.push_str(&format!("{address}\t{domain}{newline}"));
    }
    block.push_str(END);
    block.push_str(newline);
    let merged = if let (Some(start), Some(end)) = (start, end) {
        format!("{}{}{}", &current[..start], block, &current[end..])
    } else {
        let separator = if current.is_empty() || current.ends_with('\n') {
            ""
        } else {
            newline
        };
        format!("{current}{separator}{block}")
    };
    if merged.len() as u64 > LIMIT {
        return Err(Error::Invalid("Resulting hosts file exceeds limit".into()));
    }
    Ok(merged)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recovery {
    schema_version: u32,
    original: String,
    original_sha256: String,
    applied_sha256: String,
}

/// Caller holds the protected-root operation lock and administrator privileges.
pub fn apply(path: &Path, root: &Path, remote: &str, suffixes: &[String]) -> Result<usize> {
    let current = read_text(path, LIMIT)?;
    let merged = merge(&current, remote, suffixes)?;
    let entries = validate_entries(remote, suffixes)?.len();
    if merged == current {
        return Ok(entries);
    }
    let journal_path = root.join(JOURNAL);
    // Restore before applying a second revision: one recovery point is explicit,
    // never silently overwritten, and external changes cannot be lost.
    if journal_path.exists() {
        return Err(Error::Operation("Hosts recovery point already exists; restore it before applying a different hosts revision".into()));
    }
    let recovery = Recovery {
        schema_version: 1,
        original: current.clone(),
        original_sha256: sha256(current.as_bytes()),
        applied_sha256: sha256(merged.as_bytes()),
    };
    atomic_write(&journal_path, &serde_json::to_vec_pretty(&recovery)?)?;
    if read_text(path, LIMIT)? != current {
        return Err(Error::Operation("Hosts changed while preparing the update. Journal retained; no hosts replacement was performed".into()));
    }
    atomic_write(path, merged.as_bytes())?;
    Ok(entries)
}

pub fn restore(path: &Path, root: &Path) -> Result<bool> {
    let journal_path = root.join(JOURNAL);
    if !journal_path.exists() {
        return Ok(false);
    }
    let recovery: Recovery =
        serde_json::from_slice(&crate::files::read_bounded(&journal_path, LIMIT * 6)?)?;
    if recovery.schema_version != 1
        || sha256(recovery.original.as_bytes()) != recovery.original_sha256
    {
        return Err(Error::Security("Hosts recovery record is invalid".into()));
    }
    let current = read_text(path, LIMIT)?;
    let current_hash = sha256(current.as_bytes());
    if current_hash != recovery.applied_sha256 && current_hash != recovery.original_sha256 {
        return Err(Error::Operation("Hosts was edited outside Tandem. Automatic restore refused; review the recovery record manually".into()));
    }
    if current_hash != recovery.original_sha256 {
        atomic_write(path, recovery.original.as_bytes())?;
    }
    std::fs::remove_file(journal_path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn allow() -> Vec<String> {
        vec!["discord.com".into()]
    }
    #[test]
    fn merge_is_idempotent_and_preserves_unmanaged_bytes() {
        let before = "# local\r\n127.0.0.1 localhost\r\n\r\n";
        let first = merge(before, "1.1.1.1 discord.com", &allow()).unwrap();
        assert!(first.starts_with(before));
        assert_eq!(
            merge(&first, "1.1.1.1 discord.com", &allow()).unwrap(),
            first
        );
    }
    #[test]
    fn malformed_markers_fail_closed() {
        for current in [
            format!("{START}\nprivate content\n"),
            format!("{END}\n"),
            format!("{START}\n{START}\n{END}\n"),
        ] {
            assert!(merge(&current, "1.1.1.1 discord.com", &allow()).is_err());
        }
    }
    #[test]
    fn rejects_poisoning_and_suffix_confusion() {
        for remote in [
            "127.0.0.1 discord.com",
            "1.1.1.1 evildiscord.com",
            "1.1.1.1 example.org",
            "192.168.0.1 discord.com",
            "::ffff:127.0.0.1 discord.com",
        ] {
            assert!(validate_entries(remote, &allow()).is_err());
        }
    }
    #[test]
    fn restore_preserves_exact_original_and_refuses_conflicts() {
        let root = crate::files::test_dir("hosts");
        let path = root.join("hosts");
        atomic_write(&path, b"127.0.0.1 localhost").unwrap();
        apply(&path, &root, "1.1.1.1 discord.com", &allow()).unwrap();
        let applied = read_text(&path, LIMIT).unwrap();
        atomic_write(&path, b"outside edit").unwrap();
        assert!(restore(&path, &root).is_err());
        atomic_write(&path, applied.as_bytes()).unwrap();
        assert!(restore(&path, &root).unwrap());
        assert_eq!(read_text(&path, LIMIT).unwrap(), "127.0.0.1 localhost");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn public_policy() {
        for address in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(public_ip(address.parse().unwrap()));
        }
        for address in [
            "100.64.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "0.0.0.0",
            "2001:db8::1",
            "fc00::1",
            "::1",
            "2002:7f00:1::",
        ] {
            assert!(!public_ip(address.parse().unwrap()));
        }
    }
}
