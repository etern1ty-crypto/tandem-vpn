//! Versioned configuration. Unknown keys are errors, not silently ignored.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameFilter {
    #[default]
    Disabled,
    Tcp,
    Udp,
    All,
}

impl GameFilter {
    pub fn tcp(self) -> &'static str {
        if matches!(self, Self::Tcp | Self::All) {
            "1024-65535"
        } else {
            "12"
        }
    }
    pub fn udp(self) -> &'static str {
        if matches!(self, Self::Udp | Self::All) {
            "1024-65535"
        } else {
            "12"
        }
    }
    pub fn combined(self) -> &'static str {
        if self == Self::Disabled {
            "12"
        } else {
            "1024-65535"
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpsetFilter {
    None,
    #[default]
    Loaded,
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub game_filter: GameFilter,
    pub ipset_filter: IpsetFilter,
    pub check_updates_on_start: bool,
    pub request_timeout_secs: u64,
    pub targets: Vec<String>,
    pub hosts_allowed_suffixes: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            game_filter: GameFilter::Disabled,
            ipset_filter: IpsetFilter::Loaded,
            check_updates_on_start: false,
            request_timeout_secs: 8,
            targets: vec![
                "https://www.youtube.com/".into(),
                "https://discord.com/".into(),
                "https://web.telegram.org/".into(),
            ],
            hosts_allowed_suffixes: vec![
                "discord.com".into(),
                "discord.gg".into(),
                "discord.media".into(),
                "discordapp.net".into(),
                "web.telegram.org".into(),
            ],
        }
    }
}

pub fn valid_domain(domain: &str) -> bool {
    domain.len() <= 253
        && domain.contains('.')
        && domain.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

/// Only HTTPS DNS names, default port and no credentials, query or fragment.
/// Addresses are resolved and checked for public routing again before probes.
pub fn validate_target(value: &str) -> Result<url::Url> {
    let url = url::Url::parse(value).map_err(|e| Error::Invalid(e.to_string()))?;
    if value.len() > 2048
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|p| p != 443)
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Invalid(
            "Targets must be HTTPS URLs without credentials, custom ports, queries or fragments"
                .into(),
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| Error::Invalid("Target has no host".into()))?;
    if !valid_domain(host)
        || host.parse::<std::net::IpAddr>().is_ok()
        || [
            ".localhost",
            ".local",
            ".internal",
            ".home",
            ".test",
            ".invalid",
        ]
        .iter()
        .any(|suffix| host.ends_with(suffix))
    {
        return Err(Error::Invalid("Target must use a public DNS name".into()));
    }
    Ok(url)
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err(Error::Invalid(
                "Unsupported configuration schema_version".into(),
            ));
        }
        if !(1..=30).contains(&self.request_timeout_secs) {
            return Err(Error::Invalid("request_timeout_secs must be 1..30".into()));
        }
        if self.targets.is_empty() || self.targets.len() > 12 {
            return Err(Error::Invalid("Configure between 1 and 12 targets".into()));
        }
        let mut seen = HashSet::new();
        for target in &self.targets {
            let normalized = validate_target(target)?.to_string();
            if !seen.insert(normalized) {
                return Err(Error::Invalid("Duplicate target".into()));
            }
        }
        if self.hosts_allowed_suffixes.is_empty() || self.hosts_allowed_suffixes.len() > 32 {
            return Err(Error::Invalid("Configure 1..32 hosts suffixes".into()));
        }
        for suffix in &self.hosts_allowed_suffixes {
            if !valid_domain(suffix)
                || suffix.split('.').count() < 2
                || suffix.parse::<std::net::IpAddr>().is_ok()
            {
                return Err(Error::Invalid(format!("Invalid hosts suffix: {suffix}")));
            }
        }
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self> {
        let config = if path.exists() {
            serde_json::from_slice(&crate::files::read_bounded(path, 64 * 1024)?)?
        } else {
            Self::default()
        };
        config.validate()?;
        Ok(config)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        crate::files::atomic_write(path, &serde_json::to_vec_pretty(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_validate() {
        Config::default().validate().unwrap();
    }
    #[test]
    fn unknown_keys_fail() {
        assert!(serde_json::from_str::<Config>(r#"{"auto_updat":true}"#).is_err());
    }
    #[test]
    fn bad_targets_fail() {
        for target in [
            "http://example.org",
            "https://user:pass@example.org",
            "https://127.0.0.1",
            "https://host.local",
            "https://example.org:8443",
            "https://example.org?token=x",
        ] {
            assert!(validate_target(target).is_err(), "{target}");
        }
    }
    #[test]
    fn duplicate_targets_fail() {
        let mut c = Config::default();
        c.targets = vec!["https://example.org".into(), "https://example.org/".into()];
        assert!(c.validate().is_err());
    }
    #[test]
    fn filter_uses_upstream_disabled_sentinel() {
        assert_eq!(GameFilter::Disabled.tcp(), "12");
        assert_eq!(GameFilter::Tcp.udp(), "12");
        assert_eq!(GameFilter::Udp.udp(), "1024-65535");
    }
}
