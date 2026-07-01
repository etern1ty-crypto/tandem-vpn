//! Goida integration: parse a public VLESS/Trojan/Shadowsocks subscription
//! list (the de-facto V2Ray/Xray convention — one `vless://`/`trojan://`/
//! `ss://` URI per line, e.g. the raw githubmirror lists published by
//! `AvenCores/goida-vpn-configs`) into sing-box outbounds.
//!
//! sing-box outbound field mappings for `vless`, `trojan` and `shadowsocks`
//! (including the shared `tls`/`reality` and `ws`/`grpc` transport blocks)
//! were verified against sing-box's own docs, not guessed. `vmess://`
//! entries use a base64-encoded JSON body — a different parsing path — and
//! are recognized but skipped for now rather than risk an unverified
//! mapping.
//!
//! Actually fetching the subscription text and driving delay-tests through
//! sing-box's Clash-compatible HTTP API is network I/O and lives in the
//! Tauri layer, matching how the rest of this crate keeps network access
//! out of `core` so parsing/rendering stays offline-testable.

use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use url::Url;

/// A single parsed candidate config, ready to be dropped into a sing-box
/// `outbounds` array and addressed by `tag` through the Clash API.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GoidaConfig {
    pub tag: String,
    pub protocol: &'static str,
    pub server: String,
    pub server_port: u16,
    pub remark: String,
    /// Two-letter country code parsed from a flag emoji in the remark, if any.
    pub country_code: Option<String>,
    /// The flag emoji itself, reconstructed from `country_code`.
    pub country_flag: Option<String>,
    pub outbound: Value,
}

/// Parse the raw subscription text (one URI per line). Lines that fail to
/// parse (bad URI, unsupported protocol/transport) are skipped rather than
/// failing the whole batch — the list is large and heterogeneous by nature.
pub fn parse_subscription(raw: &str) -> Vec<GoidaConfig> {
    raw.lines()
        .enumerate()
        .filter_map(|(i, line)| parse_uri(line.trim(), i))
        .collect()
}

/// Build the sing-box `outbounds` entries for a set of candidates: each
/// config's own outbound stanza, plus a `selector` grouping them all under
/// `selector_tag` — the single routable name other rules/the UI refer to
/// ("goida"). Selecting among candidates later is a `PUT
/// /proxies/{selector_tag}` Clash API call, not a config rewrite.
pub fn build_outbounds_with_selector(
    configs: &[GoidaConfig],
    selector_tag: &str,
    default_tag: Option<&str>,
) -> Vec<Value> {
    let mut outbounds: Vec<Value> = configs.iter().map(|c| c.outbound.clone()).collect();
    let tags: Vec<&str> = configs.iter().map(|c| c.tag.as_str()).collect();

    let mut selector = serde_json::Map::new();
    selector.insert("type".into(), json!("selector"));
    selector.insert("tag".into(), json!(selector_tag));
    selector.insert("outbounds".into(), json!(tags));
    if let Some(default) = default_tag.or_else(|| tags.first().copied()) {
        selector.insert("default".into(), json!(default));
    }
    outbounds.push(Value::Object(selector));
    outbounds
}

fn parse_uri(line: &str, index: usize) -> Option<GoidaConfig> {
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let url = Url::parse(line).ok()?;
    match url.scheme() {
        "vless" => parse_vless(&url, index),
        "trojan" => parse_trojan(&url, index),
        "ss" => parse_shadowsocks(&url, index),
        // vmess:// bodies are base64-encoded JSON, not a query-string URI —
        // needs a separate parser. Skipped for now.
        _ => None,
    }
}

fn query_map(url: &Url) -> HashMap<String, String> {
    url.query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// `Url::fragment()` returns the fragment still percent-encoded (per the
/// WHATWG URL spec the `url` crate follows); decode it for display/parsing.
fn remark(url: &Url) -> String {
    url.fragment().map(percent_decode).unwrap_or_default()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Extract a two-letter country code from a pair of Unicode "regional
/// indicator symbol" characters (how flag emoji are encoded, e.g. 🇨🇦 is
/// U+1F1E8 U+1F1E6, "letters" C and A) anywhere in the remark. Returns the
/// code and the flag emoji reconstructed from it.
fn extract_country(remark: &str) -> (Option<String>, Option<String>) {
    const BASE: u32 = 0x1F1E6; // regional indicator symbol letter A
    let indicators: Vec<char> = remark
        .chars()
        .filter(|&c| ('\u{1F1E6}'..='\u{1F1FF}').contains(&c))
        .collect();
    if indicators.len() < 2 {
        return (None, None);
    }
    let a = indicators[0] as u32 - BASE;
    let b = indicators[1] as u32 - BASE;
    if a > 25 || b > 25 {
        return (None, None);
    }
    let code = format!(
        "{}{}",
        char::from_u32(b'A' as u32 + a).unwrap(),
        char::from_u32(b'A' as u32 + b).unwrap()
    );
    let flag = format!("{}{}", indicators[0], indicators[1]);
    (Some(code), Some(flag))
}

/// Shared `tls` object for vless/trojan, built from URI query params
/// (`security`, `sni`, `fp`, `pbk`, `sid`, `alpn`). Returns `None` when
/// `security=none` is explicit, or when absent and `force` is false (vless
/// with no `security` param means plain TCP). `force` is true for trojan,
/// which implies TLS by protocol convention even without a `security` param
/// — so an absent `security` there must still honor `sni`/`fp`/`alpn`.
fn build_tls(q: &HashMap<String, String>, force: bool) -> Option<Value> {
    let security = q.get("security").map(String::as_str).unwrap_or("");
    if security == "none" || (security.is_empty() && !force) {
        return None;
    }
    let mut tls = serde_json::Map::new();
    tls.insert("enabled".into(), json!(true));
    if let Some(sni) = q.get("sni") {
        tls.insert("server_name".into(), json!(sni));
    }
    if let Some(alpn) = q.get("alpn") {
        let list: Vec<&str> = alpn.split(',').collect();
        tls.insert("alpn".into(), json!(list));
    }
    if let Some(fp) = q.get("fp") {
        tls.insert("utls".into(), json!({ "enabled": true, "fingerprint": fp }));
    }
    if security == "reality" {
        let mut reality = serde_json::Map::new();
        reality.insert("enabled".into(), json!(true));
        if let Some(pbk) = q.get("pbk") {
            reality.insert("public_key".into(), json!(pbk));
        }
        if let Some(sid) = q.get("sid") {
            reality.insert("short_id".into(), json!(sid));
        }
        tls.insert("reality".into(), Value::Object(reality));
    }
    Some(Value::Object(tls))
}

/// Shared V2Ray `transport` object for vless/trojan, built from `type`
/// (`ws`/`grpc`), `path`/`host` or `serviceName`. Returns `None` for plain
/// `tcp`/unspecified transport.
fn build_transport(q: &HashMap<String, String>) -> Option<Value> {
    match q.get("type").map(String::as_str) {
        Some("ws") => {
            let mut t = serde_json::Map::new();
            t.insert("type".into(), json!("ws"));
            t.insert(
                "path".into(),
                json!(q.get("path").cloned().unwrap_or_default()),
            );
            if let Some(host) = q.get("host") {
                t.insert("headers".into(), json!({ "Host": host }));
            }
            Some(Value::Object(t))
        }
        Some("grpc") => {
            let service = q
                .get("serviceName")
                .or_else(|| q.get("service_name"))
                .cloned()
                .unwrap_or_default();
            Some(json!({ "type": "grpc", "service_name": service }))
        }
        _ => None,
    }
}

fn parse_vless(url: &Url, index: usize) -> Option<GoidaConfig> {
    let uuid = url.username();
    if uuid.is_empty() {
        return None;
    }
    let server = url.host_str()?.to_string();
    let port = url.port()?;
    let q = query_map(url);
    let remark = remark(url);
    let (country_code, country_flag) = extract_country(&remark);
    let tag = format!("goida-{index}");

    let mut outbound = serde_json::Map::new();
    outbound.insert("type".into(), json!("vless"));
    outbound.insert("tag".into(), json!(tag));
    outbound.insert("server".into(), json!(server));
    outbound.insert("server_port".into(), json!(port));
    outbound.insert("uuid".into(), json!(uuid));
    if let Some(flow) = q.get("flow") {
        outbound.insert("flow".into(), json!(flow));
    }
    if let Some(tls) = build_tls(&q, false) {
        outbound.insert("tls".into(), tls);
    }
    if let Some(transport) = build_transport(&q) {
        outbound.insert("transport".into(), transport);
    }

    Some(GoidaConfig {
        tag,
        protocol: "vless",
        server,
        server_port: port,
        remark,
        country_code,
        country_flag,
        outbound: Value::Object(outbound),
    })
}

fn parse_trojan(url: &Url, index: usize) -> Option<GoidaConfig> {
    let password = url.username();
    if password.is_empty() {
        return None;
    }
    let server = url.host_str()?.to_string();
    let port = url.port()?;
    let q = query_map(url);
    let remark = remark(url);
    let (country_code, country_flag) = extract_country(&remark);
    let tag = format!("goida-{index}");

    let mut outbound = serde_json::Map::new();
    outbound.insert("type".into(), json!("trojan"));
    outbound.insert("tag".into(), json!(tag));
    outbound.insert("server".into(), json!(server));
    outbound.insert("server_port".into(), json!(port));
    outbound.insert("password".into(), json!(password));
    // force=true: trojan implies TLS by convention even without an explicit
    // `security` param, so sni/fp/alpn must still be honored.
    outbound.insert(
        "tls".into(),
        build_tls(&q, true).unwrap_or_else(|| json!({ "enabled": true })),
    );
    if let Some(transport) = build_transport(&q) {
        outbound.insert("transport".into(), transport);
    }

    Some(GoidaConfig {
        tag,
        protocol: "trojan",
        server,
        server_port: port,
        remark,
        country_code,
        country_flag,
        outbound: Value::Object(outbound),
    })
}

/// SIP002 form: `ss://base64(method:password)@host:port#remark`.
fn parse_shadowsocks(url: &Url, index: usize) -> Option<GoidaConfig> {
    let server = url.host_str()?.to_string();
    let port = url.port()?;
    let userinfo = url.username();
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(userinfo.trim_end_matches('='))
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(userinfo))
        .ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (method, password) = decoded.split_once(':')?;
    let remark = remark(url);
    let (country_code, country_flag) = extract_country(&remark);
    let tag = format!("goida-{index}");

    let outbound = json!({
        "type": "shadowsocks",
        "tag": tag,
        "server": server,
        "server_port": port,
        "method": method,
        "password": password,
    });

    Some(GoidaConfig {
        tag,
        protocol: "ss",
        server,
        server_port: port,
        remark,
        country_code,
        country_flag,
        outbound,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_country_from_flag_emoji() {
        let (code, flag) = extract_country("[OpenRay] \u{1F1E8}\u{1F1E6} CA-12345");
        assert_eq!(code.as_deref(), Some("CA"));
        assert_eq!(flag.as_deref(), Some("\u{1F1E8}\u{1F1E6}"));
    }

    #[test]
    fn no_flag_returns_none() {
        let (code, flag) = extract_country("Dynamic-99999");
        assert_eq!(code, None);
        assert_eq!(flag, None);
    }

    #[test]
    fn parses_vless_reality_uri() {
        let uri = "vless://0b08b1c4-342a-4992-ac5f-5b9eff2327a0@20.151.233.69:443?encryption=none&flow=xtls-rprx-vision&security=reality&sni=example.com&fp=chrome&pbk=abc123&sid=ef#%5BOpenRay%5D%20%F0%9F%87%A8%F0%9F%87%A6%20CA-00001";
        let cfg = parse_uri(uri, 0).unwrap();
        assert_eq!(cfg.protocol, "vless");
        assert_eq!(cfg.server, "20.151.233.69");
        assert_eq!(cfg.server_port, 443);
        assert_eq!(cfg.country_code.as_deref(), Some("CA"));
        assert_eq!(cfg.outbound["uuid"], "0b08b1c4-342a-4992-ac5f-5b9eff2327a0");
        assert_eq!(cfg.outbound["flow"], "xtls-rprx-vision");
        assert_eq!(cfg.outbound["tls"]["reality"]["public_key"], "abc123");
        assert_eq!(cfg.outbound["tls"]["reality"]["short_id"], "ef");
        assert_eq!(cfg.outbound["tls"]["utls"]["fingerprint"], "chrome");
    }

    #[test]
    fn parses_vless_ws_tls_uri() {
        let uri = "vless://uuid-here@biskoit.example.com:9010?encryption=none&security=tls&type=ws&path=%2Fpath&host=cdn.example.com&sni=cdn.example.com#test";
        let cfg = parse_uri(uri, 1).unwrap();
        assert_eq!(cfg.outbound["transport"]["type"], "ws");
        assert_eq!(cfg.outbound["transport"]["path"], "/path");
        assert_eq!(
            cfg.outbound["transport"]["headers"]["Host"],
            "cdn.example.com"
        );
        assert_eq!(cfg.outbound["tls"]["server_name"], "cdn.example.com");
    }

    #[test]
    fn parses_trojan_uri() {
        let uri = "trojan://mypassword@trojan.example.com:443?sni=trojan.example.com#Trojan-Node";
        let cfg = parse_uri(uri, 2).unwrap();
        assert_eq!(cfg.protocol, "trojan");
        assert_eq!(cfg.outbound["password"], "mypassword");
        assert_eq!(cfg.outbound["tls"]["enabled"], true);
        assert_eq!(cfg.outbound["tls"]["server_name"], "trojan.example.com");
    }

    #[test]
    fn parses_shadowsocks_sip002_uri() {
        // method:password = "aes-256-gcm:hunter2", base64url-encoded.
        let encoded =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("aes-256-gcm:hunter2");
        let uri = format!("ss://{encoded}@ss.example.com:8388#SS-Node");
        let cfg = parse_uri(&uri, 3).unwrap();
        assert_eq!(cfg.protocol, "ss");
        assert_eq!(cfg.outbound["method"], "aes-256-gcm");
        assert_eq!(cfg.outbound["password"], "hunter2");
        assert_eq!(cfg.server, "ss.example.com");
        assert_eq!(cfg.server_port, 8388);
    }

    #[test]
    fn skips_vmess_and_malformed_lines() {
        let sub = "vmess://eyJhbGciOiJ==\nnot a uri at all\n\n# comment line\n";
        let configs = parse_subscription(sub);
        assert!(configs.is_empty());
    }

    #[test]
    fn parses_full_subscription_skipping_bad_lines() {
        let sub = "trojan://pw@a.example.com:443#A\n\nnot a uri\nvless://uuid@b.example.com:443?encryption=none#B\n";
        let configs = parse_subscription(sub);
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].protocol, "trojan");
        assert_eq!(configs[1].protocol, "vless");
        // tags stay unique via line index even though two lines were skipped.
        assert_ne!(configs[0].tag, configs[1].tag);
    }

    #[test]
    fn builds_selector_grouping_all_candidates() {
        let sub =
            "trojan://pw@a.example.com:443#A\nvless://uuid@b.example.com:443?encryption=none#B\n";
        let configs = parse_subscription(sub);
        let outbounds = build_outbounds_with_selector(&configs, "goida", None);
        // 2 candidate outbounds + 1 selector.
        assert_eq!(outbounds.len(), 3);
        let selector = outbounds.last().unwrap();
        assert_eq!(selector["type"], "selector");
        assert_eq!(selector["tag"], "goida");
        assert_eq!(
            selector["outbounds"],
            json!([configs[0].tag, configs[1].tag])
        );
        assert_eq!(selector["default"], configs[0].tag);
    }
}
