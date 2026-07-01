//! WARP integration: registers a free Cloudflare WARP account via the
//! `wgcf` CLI ([ViRb3/wgcf](https://github.com/ViRb3/wgcf)) and renders the
//! resulting WireGuard profile into a sing-box `wireguard` endpoint.
//!
//! We shell out to `wgcf` rather than reimplementing Cloudflare's
//! registration API ourselves — same pattern as driving `sing-box` (and
//! previously `winws.exe`) as a proven external binary instead of
//! reimplementing protocol-level networking in Rust. Exact command shape
//! (`register --accept-tos --config <path>`, `generate --config <path>
//! --profile <path>`) and the generated profile format (`[Interface]`
//! PrivateKey/Address/DNS/MTU, `[Peer]` PublicKey/AllowedIPs/Endpoint, no
//! reserved-bytes field) are taken directly from wgcf's own source
//! (`cmd/register`, `cmd/generate`, `wireguard/profile.go`), not guessed.

use crate::sys::{PlannedCommand, Sys};
use crate::{Error, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Manages a single WARP (`wgcf`) installation directory.
pub struct WarpManager {
    install_dir: PathBuf,
}

impl WarpManager {
    pub fn new(install_dir: impl Into<PathBuf>) -> Self {
        Self {
            install_dir: install_dir.into(),
        }
    }

    pub fn install_dir(&self) -> &Path {
        &self.install_dir
    }
    pub fn wgcf_path(&self) -> PathBuf {
        self.install_dir.join("wgcf.exe")
    }
    pub fn account_path(&self) -> PathBuf {
        self.install_dir.join("wgcf-account.toml")
    }
    pub fn profile_path(&self) -> PathBuf {
        self.install_dir.join("wgcf-profile.conf")
    }

    pub fn wgcf_present(&self) -> bool {
        self.wgcf_path().exists()
    }
    pub fn registered(&self) -> bool {
        self.account_path().exists()
    }
    pub fn profile_generated(&self) -> bool {
        self.profile_path().exists()
    }

    /// Register a new free WARP account, accepting Cloudflare's ToS
    /// non-interactively via `--accept-tos` (mirrors ticking the box in the
    /// official 1.1.1.1 app). Writes `wgcf-account.toml`.
    pub fn register<S: Sys>(&self, sys: &S) -> Result<()> {
        std::fs::create_dir_all(&self.install_dir)?;
        sys.run(&PlannedCommand::new(
            self.wgcf_path().to_string_lossy().into_owned(),
            [
                "register".to_string(),
                "--accept-tos".to_string(),
                "--config".to_string(),
                self.account_path().to_string_lossy().into_owned(),
            ],
        ))?;
        Ok(())
    }

    /// Generate the WireGuard profile from the registered account. Writes
    /// `wgcf-profile.conf`.
    pub fn generate_profile<S: Sys>(&self, sys: &S) -> Result<()> {
        sys.run(&PlannedCommand::new(
            self.wgcf_path().to_string_lossy().into_owned(),
            [
                "generate".to_string(),
                "--config".to_string(),
                self.account_path().to_string_lossy().into_owned(),
                "--profile".to_string(),
                self.profile_path().to_string_lossy().into_owned(),
            ],
        ))?;
        Ok(())
    }

    /// Read the generated `wgcf-profile.conf` and render it as a sing-box
    /// `wireguard` endpoint stanza (see [`WgProfile::to_singbox_endpoint`]).
    pub fn render_endpoint(&self, tag: &str) -> Result<Value> {
        let contents = std::fs::read_to_string(self.profile_path())?;
        let profile = parse_wg_profile(&contents)
            .ok_or_else(|| Error::Other("could not parse wgcf-profile.conf".into()))?;
        Ok(profile.to_singbox_endpoint(tag))
    }
}

/// A parsed standard WireGuard profile, as written by wgcf's `generate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgProfile {
    pub private_key: String,
    pub address: Vec<String>,
    pub mtu: u32,
    pub peer_public_key: String,
    pub allowed_ips: Vec<String>,
    pub endpoint: String,
}

impl WgProfile {
    /// Render as a sing-box `wireguard` endpoint (the current, non-deprecated
    /// config shape — the old `wireguard` *outbound* type was deprecated in
    /// sing-box 1.11 and is being removed).
    pub fn to_singbox_endpoint(&self, tag: &str) -> Value {
        // wgcf's Endpoint is a plain `host:port` (Cloudflare never returns a
        // bracketed IPv6 literal here), so a simple rsplit is safe.
        let (host, port) = self
            .endpoint
            .rsplit_once(':')
            .unwrap_or((self.endpoint.as_str(), "2408"));
        let port: u32 = port.parse().unwrap_or(2408);
        json!({
            "type": "wireguard",
            "tag": tag,
            "address": self.address,
            "private_key": self.private_key,
            "mtu": self.mtu,
            "peers": [{
                "address": host,
                "port": port,
                "public_key": self.peer_public_key,
                "allowed_ips": self.allowed_ips,
            }]
        })
    }
}

/// Parse a standard WireGuard `.conf` in exactly the shape wgcf's `generate`
/// writes it: `[Interface]` PrivateKey/Address/DNS/MTU, `[Peer]`
/// PublicKey/AllowedIPs/Endpoint. Returns `None` if a required field
/// (PrivateKey, PublicKey, Endpoint) is missing.
pub fn parse_wg_profile(contents: &str) -> Option<WgProfile> {
    let mut private_key = None;
    let mut address = Vec::new();
    let mut mtu = 1280u32;
    let mut peer_public_key = None;
    let mut allowed_ips = Vec::new();
    let mut endpoint = None;

    for line in contents.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "PrivateKey" => private_key = Some(value.to_string()),
            "Address" => address = value.split(',').map(|s| s.trim().to_string()).collect(),
            "MTU" => mtu = value.parse().unwrap_or(1280),
            "PublicKey" => peer_public_key = Some(value.to_string()),
            "AllowedIPs" => allowed_ips = value.split(',').map(|s| s.trim().to_string()).collect(),
            "Endpoint" => endpoint = Some(value.to_string()),
            _ => {}
        }
    }

    Some(WgProfile {
        private_key: private_key?,
        address,
        mtu,
        peer_public_key: peer_public_key?,
        allowed_ips,
        endpoint: endpoint?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::MockSys;

    // Exact shape wgcf's `wireguard/profile.go` template produces.
    const SAMPLE_PROFILE: &str = "[Interface]\nPrivateKey = aGVsbG8td29ybGQtcHJpdmF0ZS1rZXk=\nAddress = 172.16.0.2/32, 2606:4700:110:8a36:df01:4433:4c1b:1b/128\nDNS = 1.1.1.1, 1.0.0.1, 2606:4700:4700::1111, 2606:4700:4700::1001\nMTU = 1280\n[Peer]\nPublicKey = YnFjTFF1dGxpbmVQdWJsaWNLZXlIZXJl=\nAllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = engage.cloudflareclient.com:2408\n";

    fn temp_manager(label: &str) -> WarpManager {
        let dir = std::env::temp_dir().join(format!("tandem-warp-{label}-{}", std::process::id()));
        WarpManager::new(dir)
    }

    #[test]
    fn parses_wgcf_profile_format() {
        let profile = parse_wg_profile(SAMPLE_PROFILE).unwrap();
        assert_eq!(profile.private_key, "aGVsbG8td29ybGQtcHJpdmF0ZS1rZXk=");
        assert_eq!(
            profile.address,
            vec![
                "172.16.0.2/32".to_string(),
                "2606:4700:110:8a36:df01:4433:4c1b:1b/128".to_string(),
            ]
        );
        assert_eq!(profile.mtu, 1280);
        assert_eq!(profile.allowed_ips, vec!["0.0.0.0/0", "::/0"]);
        assert_eq!(profile.endpoint, "engage.cloudflareclient.com:2408");
    }

    #[test]
    fn renders_singbox_wireguard_endpoint() {
        let profile = parse_wg_profile(SAMPLE_PROFILE).unwrap();
        let ep = profile.to_singbox_endpoint("warp");
        assert_eq!(ep["type"], "wireguard");
        assert_eq!(ep["tag"], "warp");
        assert_eq!(ep["peers"][0]["address"], "engage.cloudflareclient.com");
        assert_eq!(ep["peers"][0]["port"], 2408);
        assert_eq!(
            ep["peers"][0]["public_key"],
            "YnFjTFF1dGxpbmVQdWJsaWNLZXlIZXJl="
        );
    }

    #[test]
    fn missing_required_field_returns_none() {
        assert!(parse_wg_profile("[Interface]\nAddress = 1.2.3.4/32\n").is_none());
    }

    #[test]
    fn register_plans_expected_command() {
        let mgr = temp_manager("register");
        let sys = MockSys::ok();
        mgr.register(&sys).unwrap();
        let log = sys.log();
        assert!(log
            .iter()
            .any(|c| c.contains("register") && c.contains("--accept-tos")));
        let _ = std::fs::remove_dir_all(mgr.install_dir());
    }

    #[test]
    fn generate_profile_plans_expected_command() {
        let mgr = temp_manager("generate");
        let sys = MockSys::ok();
        mgr.generate_profile(&sys).unwrap();
        let log = sys.log();
        assert!(log
            .iter()
            .any(|c| c.contains("generate") && c.contains("wgcf-profile.conf")));
    }

    #[test]
    fn render_endpoint_reads_generated_profile() {
        let mgr = temp_manager("render");
        std::fs::create_dir_all(mgr.install_dir()).unwrap();
        std::fs::write(mgr.profile_path(), SAMPLE_PROFILE).unwrap();
        let ep = mgr.render_endpoint("warp").unwrap();
        assert_eq!(ep["tag"], "warp");
        let _ = std::fs::remove_dir_all(mgr.install_dir());
    }
}
