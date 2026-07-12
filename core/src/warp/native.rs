//! Native Cloudflare WARP device registration — the pure, offline half.
//!
//! This reimplements the `wgcf register` + `wgcf generate` flow directly so
//! the app no longer needs to ship/drive `wgcf.exe`. Everything here is pure
//! computation (keygen, request-body construction, response parsing, profile
//! rendering); the single network call (the registration POST) lives in the
//! Tauri layer (`app/src-tauri/src/lib.rs`) to keep `core` network-free.
//!
//! The request shape, endpoint, API version and headers are mirrored exactly
//! from ViRb3/wgcf so Cloudflare's API accepts us (it rejects stale client
//! versions with `403 error 1020`). Sources verified against, master branch:
//!
//! - `cloudflare/api.go` — ApiUrl, ApiVersion, DefaultHeaders, Register():
//!   <https://github.com/ViRb3/wgcf/blob/master/cloudflare/api.go>
//! - `openapi/model_register_200_response.go` + `model_config*.go`,
//!   `model_peer.go`, `model_endpoint.go`, `model_network_address.go` —
//!   response struct / JSON field names.
//! - `wireguard/keys.go` — Curve25519 keygen + base64-std encoding:
//!   <https://github.com/ViRb3/wgcf/blob/master/wireguard/keys.go>
//! - `wireguard/profile.go` — profile template (DNS/MTU/AllowedIPs):
//!   <https://github.com/ViRb3/wgcf/blob/master/wireguard/profile.go>
//!
//! As of verification: ApiVersion `v0a1922`, CF-Client-Version `a-6.3-1922`.

use crate::warp::WgProfile;
use crate::{Error, Result};
use base64::Engine;
use serde_json::{json, Value};

/// Base URL of Cloudflare's WARP device API (`cloudflare/api.go` `ApiUrl`).
pub const API_URL: &str = "https://api.cloudflareclient.com";
/// Pinned API version path segment (`cloudflare/api.go` `ApiVersion`). The
/// WARP API rejects requests carrying a stale version — keep this in lockstep
/// with [`CF_CLIENT_VERSION`] when bumping.
pub const API_VERSION: &str = "v0a1922";
/// `User-Agent` wgcf sends to mimic the official Android OkHttp client
/// (`cloudflare/api.go` `DefaultHeaders`).
pub const USER_AGENT: &str = "okhttp/3.12.1";
/// `CF-Client-Version` header wgcf sends (`cloudflare/api.go`
/// `DefaultHeaders`). Cloudflare validates this against [`API_VERSION`].
pub const CF_CLIENT_VERSION: &str = "a-6.3-1922";

/// Full registration endpoint: `POST {API_URL}/{API_VERSION}/reg`
/// (`openapi/api_default.go` `RegisterExecute`, path
/// `/{apiVersion}/reg`).
pub fn register_url() -> String {
    format!("{API_URL}/{API_VERSION}/reg")
}

/// A generated WireGuard keypair, both halves base64-standard encoded exactly
/// as wgcf writes them (`wireguard/keys.go` `Key::String`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPair {
    /// Base64 (std) of the clamped 32-byte Curve25519 secret.
    pub private_key: String,
    /// Base64 (std) of the derived Curve25519 public key — this is the value
    /// sent to Cloudflare in the registration body's `key` field.
    pub public_key: String,
}

/// Derive a [`KeyPair`] from a fixed 32-byte secret. Mirrors
/// `wireguard/keys.go`: the secret is clamped (`&= 248`, high bits fixed) and
/// the public key is `Curve25519(secret) · basepoint`. Split out from
/// [`generate_keypair`] so a unit test can assert the derived public key from
/// a known secret.
pub fn keypair_from_secret(mut secret: [u8; 32]) -> KeyPair {
    // WireGuard/wgcf clamping (wireguard/keys.go `NewPrivateKey`).
    secret[0] &= 248;
    secret[31] = (secret[31] & 127) | 64;

    let sk = x25519_dalek::StaticSecret::from(secret);
    let pk = x25519_dalek::PublicKey::from(&sk);

    let engine = base64::engine::general_purpose::STANDARD;
    KeyPair {
        private_key: engine.encode(secret),
        public_key: engine.encode(pk.as_bytes()),
    }
}

/// Generate a fresh random [`KeyPair`] using the OS CSPRNG.
pub fn generate_keypair() -> Result<KeyPair> {
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret)
        .map_err(|e| Error::Other(format!("failed to gather randomness for WARP keypair: {e}")))?;
    Ok(keypair_from_secret(secret))
}

/// Build the JSON body for the registration POST. Field names/values mirror
/// `cloudflare/api.go` `Register()`: empty `fcm_token`/`install_id` (the real
/// 1.1.1.1 app populates these, but the API accepts empties), `locale`
/// `en_US`, `type` `Android`. `public_key` is the base64 public key;
/// `tos_timestamp` is an RFC3339 timestamp (see [`rfc3339_utc`]) — passed in
/// so this stays pure/testable.
pub fn build_register_body(public_key: &str, model: &str, tos_timestamp: &str) -> Value {
    json!({
        "fcm_token": "",
        "install_id": "",
        "key": public_key,
        "locale": "en_US",
        "model": model,
        "tos": tos_timestamp,
        "type": "Android",
    })
}

/// Parse a Cloudflare registration response into a [`WgProfile`], pairing the
/// server-returned peer/address config with the locally held `private_key`.
///
/// Field path mirrors the wgcf response models:
/// `config.interface.addresses.{v4,v6}`, `config.peers[0].public_key`,
/// `config.peers[0].endpoint.host` (a plain `host:port`). Addresses are
/// suffixed `/32` and `/128` to match wgcf's profile template. Returns
/// [`Error::Other`] if a required field is missing.
pub fn profile_from_response(response: &Value, private_key: &str) -> Result<WgProfile> {
    let config = &response["config"];
    let v4 = config["interface"]["addresses"]["v4"]
        .as_str()
        .ok_or_else(|| {
            Error::Other("WARP response missing config.interface.addresses.v4".into())
        })?;
    let v6 = config["interface"]["addresses"]["v6"]
        .as_str()
        .ok_or_else(|| {
            Error::Other("WARP response missing config.interface.addresses.v6".into())
        })?;

    let peer = config["peers"]
        .as_array()
        .and_then(|p| p.first())
        .ok_or_else(|| Error::Other("WARP response missing config.peers[0]".into()))?;
    let peer_public_key = peer["public_key"]
        .as_str()
        .ok_or_else(|| Error::Other("WARP response missing config.peers[0].public_key".into()))?;
    let endpoint = peer["endpoint"]["host"].as_str().ok_or_else(|| {
        Error::Other("WARP response missing config.peers[0].endpoint.host".into())
    })?;

    Ok(WgProfile {
        private_key: private_key.to_string(),
        address: vec![format!("{v4}/32"), format!("{v6}/128")],
        mtu: 1280,
        peer_public_key: peer_public_key.to_string(),
        allowed_ips: vec!["0.0.0.0/0".to_string(), "::/0".to_string()],
        endpoint: endpoint.to_string(),
    })
}

/// Render a [`WgProfile`] as a standard WireGuard `.conf`, byte-for-byte in
/// the shape wgcf's `generate` writes `wgcf-profile.conf` (`wireguard/
/// profile.go` template). Round-trips through [`crate::warp::parse_wg_profile`]
/// so the existing diagnostics/`render_endpoint` path is unchanged.
pub fn render_wg_conf(profile: &WgProfile) -> String {
    format!(
        "[Interface]\n\
         PrivateKey = {}\n\
         Address = {}\n\
         DNS = 1.1.1.1, 1.0.0.1, 2606:4700:4700::1111, 2606:4700:4700::1001\n\
         MTU = {}\n\
         [Peer]\n\
         PublicKey = {}\n\
         AllowedIPs = {}\n\
         Endpoint = {}\n",
        profile.private_key,
        profile.address.join(", "),
        profile.mtu,
        profile.peer_public_key,
        profile.allowed_ips.join(", "),
        profile.endpoint,
    )
}

/// Format a Unix-seconds timestamp as an RFC3339 UTC string (`...Z`), matching
/// the `tos` value wgcf sends (`util.GetTimestamp` → `time.RFC3339`). Pure so
/// the caller in the Tauri layer supplies `SystemTime::now()` and this stays
/// testable. Implemented directly (civil-from-days) to avoid pulling in a date
/// crate for a single field.
pub fn rfc3339_utc(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let (hour, min, sec) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Convert days-since-Unix-epoch to a `(year, month, day)` civil date.
/// Howard Hinnant's public-domain `civil_from_days` algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fixed all-0x01 secret. After clamping (byte0 &= 248 -> 0x00... wait
    // 0x01 & 248 = 0x00; byte31 = (0x01 & 127)|64 = 0x41) the derived public
    // key is deterministic; we assert the exact base64 so a dalek/version bump
    // that changes the curve math is caught.
    #[test]
    fn keypair_from_fixed_secret_is_deterministic() {
        let kp = keypair_from_secret([1u8; 32]);
        // Private key is the clamped secret, base64-std.
        assert_eq!(
            kp.private_key,
            "AAEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAUE="
        );
        // Public key derived from that clamped secret (X25519 basepoint mult).
        assert_eq!(
            kp.public_key,
            "pOCSkrZRwni5dyxWn1+puxPZBrRqtoyd+dwrRAn4ogk="
        );
        // Both halves are valid 32-byte base64.
        let eng = base64::engine::general_purpose::STANDARD;
        assert_eq!(eng.decode(&kp.private_key).unwrap().len(), 32);
        assert_eq!(eng.decode(&kp.public_key).unwrap().len(), 32);
    }

    #[test]
    fn clamping_is_applied_to_private_key() {
        let kp = keypair_from_secret([0xffu8; 32]);
        let sk = base64::engine::general_purpose::STANDARD
            .decode(&kp.private_key)
            .unwrap();
        assert_eq!(sk[0] & 0b0000_0111, 0, "low 3 bits must be cleared");
        assert_eq!(
            sk[31] & 0b1100_0000,
            0b0100_0000,
            "high bits must be set to 01"
        );
    }

    #[test]
    fn generate_keypair_produces_valid_base64() {
        let kp = generate_keypair().unwrap();
        let eng = base64::engine::general_purpose::STANDARD;
        assert_eq!(eng.decode(&kp.private_key).unwrap().len(), 32);
        assert_eq!(eng.decode(&kp.public_key).unwrap().len(), 32);
        // Two calls must not collide (randomness actually happening).
        assert_ne!(kp.private_key, generate_keypair().unwrap().private_key);
    }

    #[test]
    fn register_body_has_expected_fields() {
        let kp = keypair_from_secret([2u8; 32]);
        let body = build_register_body(&kp.public_key, "PC", "2024-01-01T00:00:00Z");
        assert_eq!(body["key"], kp.public_key);
        assert_eq!(body["model"], "PC");
        assert_eq!(body["type"], "Android");
        assert_eq!(body["locale"], "en_US");
        assert_eq!(body["tos"], "2024-01-01T00:00:00Z");
        assert_eq!(body["fcm_token"], "");
        assert_eq!(body["install_id"], "");
        // The pubkey we put in the body base64-decodes back to 32 bytes.
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(body["key"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded.len(), 32);
    }

    // Trimmed but structurally faithful sample of a real registration response.
    const SAMPLE_RESPONSE: &str = r#"{
      "id": "t.abc",
      "token": "sekrit-token",
      "account": { "id": "acc", "license": "LICENSE-KEY" },
      "config": {
        "client_id": "aBc1",
        "interface": {
          "addresses": {
            "v4": "172.16.0.2",
            "v6": "2606:4700:110:8a36:df01:4433:4c1b:1b"
          }
        },
        "peers": [
          {
            "public_key": "bmF0aXZlLXBlZXItcHVibGljLWtleS1oZXJlLXh4eHg=",
            "endpoint": {
              "host": "engage.cloudflareclient.com:2408",
              "v4": "162.159.192.1:0",
              "v6": "[2606:4700:d0::a29f:c001]:0"
            }
          }
        ]
      }
    }"#;

    #[test]
    fn parses_registration_response_into_profile() {
        let resp: Value = serde_json::from_str(SAMPLE_RESPONSE).unwrap();
        let profile = profile_from_response(&resp, "my-private-key-b64").unwrap();
        assert_eq!(profile.private_key, "my-private-key-b64");
        assert_eq!(
            profile.address,
            vec![
                "172.16.0.2/32".to_string(),
                "2606:4700:110:8a36:df01:4433:4c1b:1b/128".to_string(),
            ]
        );
        assert_eq!(profile.mtu, 1280);
        assert_eq!(
            profile.peer_public_key,
            "bmF0aXZlLXBlZXItcHVibGljLWtleS1oZXJlLXh4eHg="
        );
        assert_eq!(profile.allowed_ips, vec!["0.0.0.0/0", "::/0"]);
        assert_eq!(profile.endpoint, "engage.cloudflareclient.com:2408");
    }

    #[test]
    fn parse_response_missing_peer_errors() {
        let resp: Value = serde_json::from_str(
            r#"{"config":{"interface":{"addresses":{"v4":"1.2.3.4","v6":"::1"}},"peers":[]}}"#,
        )
        .unwrap();
        assert!(profile_from_response(&resp, "pk").is_err());
    }

    #[test]
    fn rendered_conf_round_trips_through_parser() {
        let resp: Value = serde_json::from_str(SAMPLE_RESPONSE).unwrap();
        let profile = profile_from_response(&resp, "AAEBAQEBAQEBAQEBAQEBAQ==").unwrap();
        let conf = render_wg_conf(&profile);
        // The rendered .conf must parse back to an identical WgProfile so the
        // existing render_endpoint()/diagnostics path is unaffected.
        let reparsed = crate::warp::parse_wg_profile(&conf).unwrap();
        assert_eq!(reparsed, profile);
        // And it produces a valid sing-box endpoint.
        let ep = reparsed.to_singbox_endpoint("warp");
        assert_eq!(ep["peers"][0]["address"], "engage.cloudflareclient.com");
        assert_eq!(ep["peers"][0]["port"], 2408);
    }

    #[test]
    fn rfc3339_formats_known_epochs() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        // 2021-01-01T00:00:00Z
        assert_eq!(rfc3339_utc(1_609_459_200), "2021-01-01T00:00:00Z");
        // 2024-02-29T12:24:56Z (leap day)
        assert_eq!(rfc3339_utc(1_709_209_496), "2024-02-29T12:24:56Z");
    }
}
