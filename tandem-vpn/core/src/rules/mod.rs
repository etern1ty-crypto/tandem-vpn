//! Routing-rule buckets: RU-throttled-but-not-blocked foreign services route
//! through `warp`; services that geo-block Russia route through `goida`;
//! everything else (RU-domestic sites, games, anything unmatched) falls
//! through to `direct` via [`crate::engine::RouteInputs`]'s
//! `route.final = "direct"` — no explicit rule needed for that bucket at
//! all, which is what keeps latency-sensitive/unclassified traffic off any
//! tunnel by default.
//!
//! Both community buckets are sourced from
//! [1andrevich/Re-filter-lists](https://github.com/1andrevich/Re-filter-lists),
//! an actively maintained dump of the actual Roskomnadzor block list plus a
//! community-contributed "blocks Russia" list — not hand-curated here:
//! * `refilter_domains` (RKN-blocked) ships as a ready sing-box `.srs`
//!   binary rule-set, referenced remotely so sing-box downloads/caches it
//!   itself — verified against the project's own published sing-box config
//!   example, not guessed.
//! * `community.lst` (blocks-Russia) is plain text only (no pre-built
//!   rule-set), so it's fetched and turned into a plain `domain_suffix`
//!   rule the same way the earlier WARP seed list was.
//!
//! User overrides (force one domain to a specific bucket) are emitted
//! before the bucket rules, so they always win under sing-box's
//! first-matching-rule semantics.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Remote sing-box rule-set for RKN-blocked domains (the RU-throttled
/// bucket). Config shape verified against Re-filter-lists' own published
/// sing-box example via `sing-box check`.
pub const REFILTER_DOMAINS_RULESET_URL: &str = "https://github.com/1andrevich/Re-filter-lists/releases/latest/download/ruleset-domain-refilter_domains.srs";
/// Plain-text domain list for the "blocks Russia" bucket.
pub const COMMUNITY_LIST_URL: &str =
    "https://raw.githubusercontent.com/1andrevich/Re-filter-lists/main/community.lst";

const REFILTER_TAG: &str = "refilter_domains";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bucket {
    Direct,
    Warp,
    Goida,
}

/// A user override: always route `domain` (a bare suffix, e.g.
/// `"example.com"`) to `bucket`, regardless of what the community lists say.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Override {
    pub domain: String,
    pub bucket: Bucket,
}

/// Parse `community.lst`-style plain text (one domain per line, `#`
/// comments, blank lines ignored) into a suffix list.
pub fn parse_domain_list(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect()
}

/// Build the `route.rule_set` definitions array (just the remote
/// refilter_domains binary rule-set — the community list has no pre-built
/// rule-set and is passed as plain `domain_suffix` rules instead).
pub fn build_rule_set_defs() -> Vec<Value> {
    vec![json!({
        "tag": REFILTER_TAG,
        "type": "remote",
        "format": "binary",
        "url": REFILTER_DOMAINS_RULESET_URL,
        "download_detour": "direct"
    })]
}

/// Build `route.rules`: user overrides first (so they always win under
/// first-match semantics), then the RKN-blocked bucket (if `warp_tag` is
/// set — omitted otherwise, e.g. no WARP profile generated yet), then the
/// blocks-Russia bucket (if `goida_tag` is set and the community domain
/// list is non-empty). An override into a bucket with no tag set falls back
/// to `direct` — it just has no effect until that engine is set up.
pub fn build_rules(
    overrides: &[Override],
    warp_tag: Option<&str>,
    goida_tag: Option<&str>,
    goida_community_domains: &[String],
) -> Vec<Value> {
    let mut rules = Vec::new();

    for o in overrides {
        let outbound = match o.bucket {
            Bucket::Direct => "direct",
            Bucket::Warp => warp_tag.unwrap_or("direct"),
            Bucket::Goida => goida_tag.unwrap_or("direct"),
        };
        rules.push(json!({ "domain_suffix": [o.domain.clone()], "outbound": outbound }));
    }

    if let Some(warp_tag) = warp_tag {
        rules.push(json!({ "rule_set": REFILTER_TAG, "outbound": warp_tag }));
    }

    if let Some(goida_tag) = goida_tag {
        if !goida_community_domains.is_empty() {
            rules.push(json!({ "domain_suffix": goida_community_domains, "outbound": goida_tag }));
        }
    }

    rules
}

/// Persists user overrides as JSON in the engine install directory.
pub struct OverrideStore {
    path: PathBuf,
}

impl OverrideStore {
    pub fn new(install_dir: impl AsRef<Path>) -> Self {
        Self {
            path: install_dir.as_ref().join("overrides.json"),
        }
    }

    pub fn load(&self) -> crate::Result<Vec<Override>> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| crate::Error::Other(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, overrides: &[Override]) -> crate::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes =
            serde_json::to_vec_pretty(overrides).map_err(|e| crate::Error::Other(e.to_string()))?;
        std::fs::write(&self.path, bytes)?;
        Ok(())
    }

    /// Add or replace (by domain) an override, then persist.
    pub fn set(&self, domain: &str, bucket: Bucket) -> crate::Result<Vec<Override>> {
        let mut overrides = self.load()?;
        overrides.retain(|o| o.domain != domain);
        overrides.push(Override {
            domain: domain.to_string(),
            bucket,
        });
        self.save(&overrides)?;
        Ok(overrides)
    }

    /// Remove an override by domain, then persist.
    pub fn remove(&self, domain: &str) -> crate::Result<Vec<Override>> {
        let mut overrides = self.load()?;
        overrides.retain(|o| o.domain != domain);
        self.save(&overrides)?;
        Ok(overrides)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_domain_list_skipping_comments_and_blanks() {
        let raw = "example.com\n# a comment\n\nanother.example\n";
        assert_eq!(
            parse_domain_list(raw),
            vec!["example.com".to_string(), "another.example".to_string()]
        );
    }

    #[test]
    fn build_rules_orders_overrides_before_buckets() {
        let overrides = vec![Override {
            domain: "force-direct.example".into(),
            bucket: Bucket::Direct,
        }];
        let rules = build_rules(
            &overrides,
            Some("warp"),
            Some("goida"),
            &["blocked.example".to_string()],
        );
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0]["domain_suffix"][0], "force-direct.example");
        assert_eq!(rules[0]["outbound"], "direct");
        assert_eq!(rules[1]["rule_set"], "refilter_domains");
        assert_eq!(rules[1]["outbound"], "warp");
        assert_eq!(rules[2]["domain_suffix"][0], "blocked.example");
        assert_eq!(rules[2]["outbound"], "goida");
    }

    #[test]
    fn build_rules_omits_bucket_without_tag() {
        let rules = build_rules(&[], None, None, &["blocked.example".to_string()]);
        assert!(rules.is_empty());
    }

    #[test]
    fn override_into_unset_bucket_falls_back_to_direct() {
        let overrides = vec![Override {
            domain: "example.com".into(),
            bucket: Bucket::Warp,
        }];
        let rules = build_rules(&overrides, None, None, &[]);
        assert_eq!(rules[0]["outbound"], "direct");
    }

    #[test]
    fn override_store_round_trips_and_replaces_by_domain() {
        let dir = std::env::temp_dir().join(format!("tandem-rules-{}", std::process::id()));
        let store = OverrideStore::new(&dir);
        assert!(store.load().unwrap().is_empty());

        store.set("example.com", Bucket::Warp).unwrap();
        let after_first = store.set("example.com", Bucket::Goida).unwrap();
        assert_eq!(after_first.len(), 1);
        assert_eq!(after_first[0].bucket, Bucket::Goida);

        store.set("other.example", Bucket::Direct).unwrap();
        assert_eq!(store.load().unwrap().len(), 2);

        let after_remove = store.remove("example.com").unwrap();
        assert_eq!(after_remove.len(), 1);
        assert_eq!(after_remove[0].domain, "other.example");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
