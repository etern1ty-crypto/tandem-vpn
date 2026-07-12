//! WARP integration: registers a free Cloudflare WARP account and renders the
//! resulting WireGuard profile into a sing-box `wireguard` endpoint.
//!
//! Registration is done **natively in Rust** ([`native`]) against Cloudflare's
//! device API, mirroring ViRb3/wgcf's request shape/version/headers — we no
//! longer shell out to `wgcf.exe` (which failed silently: the app logged
//! "account created" but no profile appeared). The pure parts (keygen,
//! request-body construction, response parsing, profile rendering) live in
//! [`native`]; the single registration POST lives in the Tauri layer so `core`
//! stays offline. The profile is still written to `wgcf-profile.conf` in the
//! exact `[Interface]`/`[Peer]` shape wgcf's `generate` produced, so the
//! downstream `render_endpoint`/diagnostics paths are unchanged.

pub mod native;

use crate::{Error, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Manages a single WARP installation directory.
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

    /// Persist a natively-registered profile: writes the WireGuard `.conf`
    /// (`wgcf-profile.conf`, so `render_endpoint`/diagnostics are unchanged)
    /// plus a small account marker (`wgcf-account.toml`) so [`registered`]
    /// reports true. `account_marker` is the raw registration response the
    /// caller obtained from Cloudflare (stored for diagnostics/debugging).
    ///
    /// [`registered`]: WarpManager::registered
    pub fn write_profile(&self, profile: &WgProfile, account_marker: &str) -> Result<()> {
        std::fs::create_dir_all(&self.install_dir)?;
        std::fs::write(self.profile_path(), native::render_wg_conf(profile))?;
        std::fs::write(self.account_path(), account_marker)?;
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
    fn render_endpoint_reads_generated_profile() {
        let mgr = temp_manager("render");
        std::fs::create_dir_all(mgr.install_dir()).unwrap();
        std::fs::write(mgr.profile_path(), SAMPLE_PROFILE).unwrap();
        let ep = mgr.render_endpoint("warp").unwrap();
        assert_eq!(ep["tag"], "warp");
        let _ = std::fs::remove_dir_all(mgr.install_dir());
    }
}
