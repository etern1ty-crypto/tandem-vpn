//! File-backed engine configuration with reversible IPSet selection.
pub mod strategy;
pub use crate::config::{GameFilter, IpsetFilter};
use crate::{Error, Result};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

pub struct ZapretManager {
    engine: PathBuf,
}
impl ZapretManager {
    pub fn new(engine: impl Into<PathBuf>) -> Self {
        Self {
            engine: engine.into(),
        }
    }
    pub fn engine_dir(&self) -> &Path {
        &self.engine
    }
    pub fn list_strategies(&self) -> Result<Vec<String>> {
        let mut result = Vec::new();
        let entries = match std::fs::read_dir(&self.engine) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let lower = name.to_ascii_lowercase();
            if lower.ends_with(".bat")
                && !lower.starts_with("service")
                && entry.file_type()?.is_file()
            {
                crate::files::safe_component(&name)?;
                crate::files::reject_links(&entry.path())?;
                result.push(name);
            }
        }
        result.sort_by_key(|name| name.to_ascii_lowercase());
        Ok(result)
    }
    pub fn plan(&self, name: &str, game: GameFilter) -> Result<strategy::StrategyPlan> {
        crate::files::safe_component(name)?;
        if !self
            .list_strategies()?
            .iter()
            .any(|candidate| candidate == name)
        {
            return Err(Error::Invalid(
                "Strategy must be selected from the installed bundle".into(),
            ));
        }
        let source = crate::files::read_text(&self.engine.join(name), 128 * 1024)?;
        strategy::parse_and_render(&source, &self.engine, game, true)
    }
    pub fn driver_present(&self) -> bool {
        self.engine.join("bin/WinDivert64.sys").is_file()
    }

    /// Validate first, persist an immutable source list, then derive active data.
    /// Runtime changes are only made with the service stopped/absent by caller.
    pub fn import_ipset(&self, text: &str, mode: IpsetFilter) -> Result<usize> {
        let normalized = normalize_ipset(text)?;
        let count = normalized.lines().filter(|line| !line.is_empty()).count();
        crate::files::atomic_write(
            &self.engine.join("lists/ipset-source.txt"),
            normalized.as_bytes(),
        )?;
        self.apply_ipset(mode)?;
        Ok(count)
    }
    pub fn apply_ipset(&self, mode: IpsetFilter) -> Result<()> {
        let lists = self.engine.join("lists");
        let source = lists.join("ipset-source.txt");
        let active = lists.join("ipset-all.txt");
        if !source.exists() {
            let original = crate::files::read_text(&active, 8 * 1024 * 1024)?;
            crate::files::atomic_write(&source, normalize_ipset(&original)?.as_bytes())?;
        }
        let contents = match mode {
            IpsetFilter::None => String::new(),
            IpsetFilter::Any => "0.0.0.0/0\n::/0\n".into(),
            IpsetFilter::Loaded => {
                normalize_ipset(&crate::files::read_text(&source, 8 * 1024 * 1024)?)?
            }
        };
        crate::files::atomic_write(&active, contents.as_bytes())
    }
}

pub fn normalize_ipset(text: &str) -> Result<String> {
    if text.len() > 8 * 1024 * 1024 {
        return Err(Error::Invalid("IPSet exceeds 8 MiB".into()));
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = String::new();
    for (index, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let value = line.split('#').next().unwrap_or_default().trim();
        if value.is_empty() {
            continue;
        }
        let (address, prefix) = value
            .split_once('/')
            .map_or((value, None), |(a, p)| (a, Some(p)));
        let address: IpAddr = address
            .parse()
            .map_err(|_| Error::Invalid(format!("Invalid IP address at line {}", index + 1)))?;
        let normalized = if let Some(prefix) = prefix {
            let prefix: u8 = prefix
                .parse()
                .map_err(|_| Error::Invalid("Invalid CIDR prefix".into()))?;
            if prefix > if address.is_ipv4() { 32 } else { 128 } {
                return Err(Error::Invalid("CIDR prefix is out of range".into()));
            }
            format!("{address}/{prefix}")
        } else {
            address.to_string()
        };
        if seen.insert(normalized.clone()) {
            out.push_str(&normalized);
            out.push('\n');
        }
        if seen.len() > 200_000 {
            return Err(Error::Invalid("IPSet has too many entries".into()));
        }
    }
    Ok(out)
}

pub fn validate_tag(tag: &str) -> Result<()> {
    if tag.is_empty()
        || tag.len() > 64
        || !tag
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        || tag.contains("..")
        || !tag.bytes().any(|c| c.is_ascii_digit())
    {
        return Err(Error::Invalid(
            "Release tag must be 1..64 letters, digits, dots, dashes or underscores".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalize_validates_and_deduplicates() {
        assert_eq!(
            normalize_ipset("1.1.1.1\n# comment\n1.1.1.1\n2001:4860::/32").unwrap(),
            "1.1.1.1\n2001:4860::/32\n"
        );
    }
    #[test]
    fn invalid_ranges_fail() {
        for v in ["1.1.1.1/33", "::/129", "not-ip", "1.2.3.4/24/5"] {
            assert!(normalize_ipset(v).is_err());
        }
    }
    #[test]
    fn ipset_round_trip_preserves_source() {
        let root = crate::files::test_dir("ipset");
        std::fs::create_dir_all(root.join("lists")).unwrap();
        crate::files::atomic_write(&root.join("lists/ipset-all.txt"), b"1.1.1.1\n").unwrap();
        let manager = ZapretManager::new(&root);
        for mode in [IpsetFilter::None, IpsetFilter::Any, IpsetFilter::Loaded] {
            manager.apply_ipset(mode).unwrap();
        }
        assert_eq!(
            crate::files::read_text(&root.join("lists/ipset-all.txt"), 100).unwrap(),
            "1.1.1.1\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn strategy_path_traversal_rejected() {
        assert!(ZapretManager::new("/tmp")
            .plan("../evil.bat", GameFilter::Disabled)
            .is_err());
    }
    #[test]
    fn tag_paths_rejected() {
        for tag in ["", "../../main", "v1?x=y", "v1\r\n"] {
            assert!(validate_tag(tag).is_err());
        }
    }
}
