//! Tauri command layer for tandem-vpn.
//!
//! Thin wrappers that adapt [`tandem_core`] to the GUI. Windows-only service
//! operations run through [`tandem_core::sys::RealSys`]; network operations
//! (release downloads, connectivity tests) use `ureq` here so the core crate
//! stays offline-testable.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tandem_core::engine::{self, EngineStatus, RouteInputs, CLASH_API_ADDR};
use tandem_core::goida::{self, GoidaConfig};
use tandem_core::rules::{self, Bucket, Override, OverrideStore};
use tandem_core::sys::{PlannedCommand, RealSys, Sys};
use tandem_core::warp::WarpManager;
use tandem_core::EngineManager;

/// Test URL used for Goida delay-testing (must be `https://`, not `http://`
/// — sing-box's Clash API delay endpoint silently ignores `http://` URLs).
const GOIDA_TEST_URL: &str = "https://www.gstatic.com/generate_204";

/// Configs with a measured delay at or above this are treated as unusable
/// and dropped from test results, per the desired "Test" UX.
const GOIDA_MAX_DELAY_MS: u32 = 200;

/// Shared application state: the active engine install directory, the most
/// recently fetched Goida candidate list (so `goida_test_all`/`goida_select`
/// can reference candidates by tag without re-fetching), and the cached
/// "blocks Russia" community domain list used for the Goida routing bucket.
pub struct AppState {
    install_dir: Mutex<PathBuf>,
    goida_configs: Mutex<Vec<GoidaConfig>>,
    goida_community_domains: Mutex<Vec<String>>,
    /// Tag of the Goida server the user last activated via `goida_select`, so
    /// a subsequent plain `install_engine` ("Запустить движок") rebuilds the
    /// config *with* that server instead of silently dropping it.
    active_goida_tag: Mutex<Option<String>>,
}

impl AppState {
    fn manager(&self) -> EngineManager {
        EngineManager::new(self.install_dir.lock().unwrap().clone())
    }

    fn warp_manager(&self) -> WarpManager {
        WarpManager::new(self.install_dir.lock().unwrap().join("warp"))
    }

    fn override_store(&self) -> OverrideStore {
        OverrideStore::new(self.install_dir.lock().unwrap().clone())
    }

    /// Assemble the shared parts of a config: rule-set definitions, the WARP
    /// endpoint (if a profile has been generated), and `route.rules` built
    /// from user overrides + the community-list buckets. Callers supply the
    /// Goida-specific outbounds/tag (empty/`None` for a plain
    /// `install_engine`, the full selector for `goida_select`).
    fn shared_route_inputs(
        &self,
        goida_outbounds: Vec<serde_json::Value>,
        goida_tag: Option<&str>,
    ) -> CmdResult<RouteInputs> {
        let warp = self.warp_manager();
        let warp_tag = warp.profile_generated().then_some("warp");

        let mut inputs = RouteInputs {
            outbounds: goida_outbounds,
            ..Default::default()
        };
        // The refilter_domains rule-set is referenced by the RKN-blocked rule
        // whenever *either* tunnel is available (see rules::build_rules), so
        // its definition must be present in the same cases — not just for
        // WARP. A rule referencing an undefined rule_set fails `sing-box check`.
        if warp_tag.is_some() || goida_tag.is_some() {
            inputs.rule_set = rules::build_rule_set_defs();
        }

        if let Some(tag) = warp_tag {
            inputs
                .endpoints
                .push(warp.render_endpoint(tag).map_err(err)?);
        }

        let overrides = self.override_store().load().map_err(err)?;
        let community = self.goida_community_domains.lock().unwrap().clone();
        inputs.rules = rules::build_rules(&overrides, warp_tag, goida_tag, &community);

        Ok(inputs)
    }
}

fn default_install_dir() -> PathBuf {
    // Use a stable per-user location so status (WARP account, profile, sing-box binary, configs)
    // persists across restarts, dev builds and packaged installs.
    // %LOCALAPPDATA%\tandem-vpn\engine on Windows.
    if cfg!(windows) {
        std::env::var("LOCALAPPDATA")
            .map(|base| PathBuf::from(base).join("tandem-vpn").join("engine"))
            .unwrap_or_else(|_| PathBuf::from("engine"))
    } else {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("engine")))
            .unwrap_or_else(|| PathBuf::from("engine"))
    }
}

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[derive(Serialize)]
pub struct Settings {
    install_dir: String,
}

#[derive(Serialize)]
pub struct TargetResult {
    url: String,
    ok: bool,
    status: Option<u16>,
    ms: u128,
    error: Option<String>,
}

/// Built-in connectivity-test targets (RU-throttled foreign services).
fn default_test_targets() -> Vec<&'static str> {
    vec![
        "https://www.youtube.com",
        "https://discord.com",
        "https://gateway.discord.gg",
        "https://github.com",
    ]
}

#[tauri::command]
fn get_settings(state: tauri::State<AppState>) -> CmdResult<Settings> {
    let mgr = state.manager();
    Ok(Settings {
        install_dir: mgr.install_dir().to_string_lossy().into_owned(),
    })
}

#[tauri::command]
fn set_install_dir(state: tauri::State<AppState>, dir: String) -> CmdResult<()> {
    *state.install_dir.lock().unwrap() = PathBuf::from(dir);
    Ok(())
}

#[tauri::command]
fn get_engine_status(state: tauri::State<AppState>) -> CmdResult<EngineStatus> {
    state.manager().status(&RealSys).map_err(err)
}

/// Install the engine and start it as a Windows service: WARP wired in if a
/// profile has been generated, the RKN-blocked bucket routed through it,
/// and user overrides applied — all via [`AppState::shared_route_inputs`].
/// Install the engine and start it as a Windows service. If the user has
/// activated a Goida server this session it is preserved (its selector +
/// full candidate bucket are rebuilt); otherwise the config carries just
/// WARP (if a profile exists), the RKN-blocked bucket, and user overrides —
/// all via [`AppState::shared_route_inputs`].
#[tauri::command]
fn install_engine(state: tauri::State<AppState>) -> CmdResult<()> {
    engine::ensure_windows().map_err(err)?;
    let mgr = state.manager();

    if !mgr.binary_present() {
        return Err(
            "sing-box binary not found. First click 'Скачать sing-box' and wait for it to finish."
                .to_string(),
        );
    }

    // Preserve an active Goida selection so "Запустить движок" doesn't quietly
    // revert to a direct-only config after the user picked a server.
    let active = state.active_goida_tag.lock().unwrap().clone();
    let inputs = match active {
        Some(tag) => {
            let configs = state.goida_configs.lock().unwrap().clone();
            if configs.iter().any(|c| c.tag == tag) {
                let goida_outbounds =
                    goida::build_outbounds_with_selector(&configs, "goida", Some(&tag));
                state.shared_route_inputs(goida_outbounds, Some("goida"))?
            } else {
                // Selection no longer valid (list re-fetched, tag gone) — fall
                // back to a plain install rather than referencing a dead tag.
                *state.active_goida_tag.lock().unwrap() = None;
                state.shared_route_inputs(Vec::new(), None)?
            }
        }
        None => state.shared_route_inputs(Vec::new(), None)?,
    };
    let config = mgr.render_config(&inputs);
    mgr.write_config(&config).map_err(err)?;
    mgr.install_service(&RealSys).map_err(err)
}

#[tauri::command]
fn remove_engine(state: tauri::State<AppState>) -> CmdResult<()> {
    engine::ensure_windows().map_err(err)?;
    // Stopping the engine clears the active selection: a subsequent start is a
    // deliberate fresh action, not an implicit re-activation of the old server.
    *state.active_goida_tag.lock().unwrap() = None;
    state.manager().remove_service(&RealSys).map_err(err)
}

#[derive(Serialize)]
pub struct WarpStatus {
    wgcf_present: bool,
    registered: bool,
    profile_generated: bool,
}

#[tauri::command]
fn warp_status(state: tauri::State<AppState>) -> CmdResult<WarpStatus> {
    let warp = state.warp_manager();
    Ok(WarpStatus {
        wgcf_present: warp.wgcf_present(),
        registered: warp.registered(),
        profile_generated: warp.profile_generated(),
    })
}

/// Register a free WARP account and generate its WireGuard profile in one
/// step, natively (no `wgcf.exe`): generate an X25519 keypair, POST it to
/// Cloudflare's device API, parse the response into a `WgProfile`, and write
/// `wgcf-profile.conf`. Re-running the engine install (`install_engine`)
/// afterwards is what actually wires the resulting endpoint into the live
/// config.
///
/// Request shape/version/headers mirror ViRb3/wgcf (see
/// `tandem_core::warp::native`). The HTTP POST lives here so `core` stays
/// offline; the pure keygen/body/parse/render logic lives in `core`.
#[tauri::command]
fn warp_register(state: tauri::State<AppState>) -> CmdResult<()> {
    use tandem_core::warp::native;

    let warp = state.warp_manager();
    let keypair = native::generate_keypair().map_err(err)?;

    let tos = native::rfc3339_utc(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(err)?
            .as_secs(),
    );
    let body = native::build_register_body(&keypair.public_key, "PC", &tos);

    // ponytail: ureq+rustls negotiates TLS 1.3; wgcf defensively pins TLS 1.2
    // max to dodge Cloudflare's `403 error 1020` firewall block. If live
    // registration returns 1020, pin a rustls ClientConfig to TLS 1.2 here.
    let resp = ureq::post(&native::register_url())
        .set("User-Agent", native::USER_AGENT)
        .set("CF-Client-Version", native::CF_CLIENT_VERSION)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(20))
        .send_json(body)
        .map_err(|e| format!("WARP registration request failed: {e}"))?;

    let raw = resp
        .into_string()
        .map_err(|e| format!("could not read WARP registration response: {e}"))?;
    let parsed: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("WARP registration returned non-JSON response: {e}"))?;

    let profile = native::profile_from_response(&parsed, &keypair.private_key).map_err(err)?;
    warp.write_profile(&profile, &raw).map_err(err)
}

/// Dry-run the WARP registration POST and return a full human-readable report
/// — HTTP status and raw body **even on failure** — without writing any files.
///
/// `warp_register` collapses a non-2xx into a terse error; the failure we most
/// need eyes on (Cloudflare's `403 error 1020` firewall block when TLS 1.3 is
/// negotiated instead of wgcf's pinned TLS 1.2) is only legible from the raw
/// status+body. This surfaces exactly that so a live failure is diagnosable
/// rather than silent — the diagnostic backbone carried over from the
/// pinned-wgcf approach, adapted to the native flow.
#[tauri::command]
fn warp_diagnose() -> CmdResult<String> {
    use tandem_core::warp::native;

    let keypair = native::generate_keypair().map_err(err)?;
    let tos = native::rfc3339_utc(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(err)?
            .as_secs(),
    );
    let body = native::build_register_body(&keypair.public_key, "PC", &tos);

    let mut r = String::new();
    r.push_str("=== WARP registration diagnose (dry run, no files written) ===\n");
    r.push_str(&format!("POST {}\n", native::register_url()));
    r.push_str(&format!(
        "CF-Client-Version: {}  User-Agent: {}\n",
        native::CF_CLIENT_VERSION,
        native::USER_AGENT
    ));
    r.push_str(&format!("public_key: {}\n\n", keypair.public_key));

    let outcome = ureq::post(&native::register_url())
        .set("User-Agent", native::USER_AGENT)
        .set("CF-Client-Version", native::CF_CLIENT_VERSION)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(20))
        .send_json(body);

    match outcome {
        Ok(resp) => {
            r.push_str(&format!("HTTP {} OK\n", resp.status()));
            let raw = resp.into_string().unwrap_or_default();
            r.push_str("--- body ---\n");
            r.push_str(&raw.chars().take(1200).collect::<String>());
        }
        // Non-2xx still carries the response body — this is where a 1020
        // firewall block or a stale-client-version rejection shows up.
        Err(ureq::Error::Status(code, resp)) => {
            r.push_str(&format!("HTTP {code} (error status)\n"));
            let raw = resp.into_string().unwrap_or_default();
            r.push_str("--- body ---\n");
            r.push_str(&raw.chars().take(1200).collect::<String>());
            if raw.contains("1020") || code == 403 {
                r.push_str("\n\n>>> Looks like Cloudflare error 1020 (firewall). Fix: pin rustls to TLS 1.2 in warp_register/warp_diagnose (see the ponytail note).");
            }
        }
        Err(e) => {
            r.push_str(&format!("transport error (no HTTP response): {e}\n"));
        }
    }
    Ok(r)
}

/// Download the latest `wgcf` Windows release (a plain, unzipped `.exe`
/// asset — unlike sing-box's zipped releases).
#[tauri::command]
fn download_wgcf_release(state: tauri::State<AppState>) -> CmdResult<()> {
    let resp = ureq::get("https://api.github.com/repos/ViRb3/wgcf/releases/latest")
        .set("User-Agent", "tandem-vpn")
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(err)?
        .into_string()
        .map_err(err)?;

    let release: serde_json::Value = serde_json::from_str(&resp).map_err(err)?;
    let assets = release["assets"]
        .as_array()
        .ok_or_else(|| err("No assets found in release"))?;

    let mut exe_url = None;
    for asset in assets {
        if let Some(name) = asset["name"].as_str() {
            let lower = name.to_lowercase();
            if lower.contains("windows") && lower.contains("amd64") && lower.ends_with(".exe") {
                exe_url = asset["browser_download_url"]
                    .as_str()
                    .map(|s| s.to_string());
                break;
            }
        }
    }
    let exe_url =
        exe_url.ok_or_else(|| err("No windows-amd64 exe asset found in latest release"))?;

    let exe_resp = ureq::get(&exe_url)
        .set("User-Agent", "tandem-vpn")
        .timeout(Duration::from_secs(60))
        .call()
        .map_err(err)?;

    let mut buf = Vec::new();
    let mut reader = exe_resp.into_reader();
    std::io::Read::read_to_end(&mut reader, &mut buf).map_err(err)?;

    let warp = state.warp_manager();
    std::fs::create_dir_all(warp.install_dir()).map_err(err)?;
    std::fs::write(warp.wgcf_path(), buf).map_err(err)?;

    Ok(())
}

#[tauri::command]
fn run_tests() -> CmdResult<Vec<TargetResult>> {
    let targets = default_test_targets();
    let mut results = Vec::with_capacity(targets.len());
    for url in targets {
        let started = Instant::now();
        let res = ureq::get(url).timeout(Duration::from_secs(8)).call();
        let ms = started.elapsed().as_millis();
        match res {
            Ok(resp) => results.push(TargetResult {
                url: url.to_string(),
                ok: resp.status() < 400,
                status: Some(resp.status()),
                ms,
                error: None,
            }),
            Err(e) => results.push(TargetResult {
                url: url.to_string(),
                ok: false,
                status: None,
                ms,
                error: Some(e.to_string()),
            }),
        }
    }
    Ok(results)
}

/// Download the latest `sing-box` Windows release and extract it into the
/// engine install directory (mirrors the previous Flowseal-zip download).
#[tauri::command]
fn download_singbox_release(state: tauri::State<AppState>) -> CmdResult<()> {
    let install_dir = state.manager().install_dir().to_path_buf();
    std::fs::create_dir_all(&install_dir).map_err(err)?;

    // Retry the GitHub API call a few times - GitHub can be flaky or rate-limited
    let mut last_err = String::new();
    let release_json = (0..3)
        .find_map(|attempt| {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(1500 * attempt as u64));
            }
            match ureq::get("https://api.github.com/repos/SagerNet/sing-box/releases/latest")
                .set("User-Agent", "tandem-vpn")
                .timeout(Duration::from_secs(15))
                .call()
            {
                Ok(resp) => match resp.into_string() {
                    Ok(s) => Some(s),
                    Err(e) => {
                        last_err = e.to_string();
                        None
                    }
                },
                Err(e) => {
                    last_err = e.to_string();
                    None
                }
            }
        })
        .ok_or_else(|| format!("Failed to fetch release info after retries: {}", last_err))?;

    let release: serde_json::Value = serde_json::from_str(&release_json).map_err(err)?;
    let assets = release["assets"]
        .as_array()
        .ok_or_else(|| err("No assets found in release"))?;

    let mut zip_url = None;
    for asset in assets {
        if let Some(name) = asset["name"].as_str() {
            let lower = name.to_lowercase();
            if lower.contains("windows")
                && lower.contains("amd64")
                && lower.ends_with(".zip")
                && !lower.contains("legacy")
            {
                zip_url = asset["browser_download_url"]
                    .as_str()
                    .map(|s| s.to_string());
                break;
            }
        }
    }
    let zip_url =
        zip_url.ok_or_else(|| err("No windows-amd64 zip asset found in latest release"))?;

    // Download the zip with retry
    let mut zip_buf = Vec::new();
    let mut download_err = String::new();
    let success = (0..3).any(|attempt| {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(2000 * attempt as u64));
        }
        match ureq::get(&zip_url)
            .set("User-Agent", "tandem-vpn")
            .timeout(Duration::from_secs(180))
            .call()
        {
            Ok(resp) => {
                zip_buf.clear();
                let mut reader = resp.into_reader();
                if std::io::Read::read_to_end(&mut reader, &mut zip_buf).is_ok() {
                    true
                } else {
                    download_err = "Failed to read zip body".to_string();
                    false
                }
            }
            Err(e) => {
                download_err = e.to_string();
                false
            }
        }
    });

    if !success {
        return Err(format!(
            "Failed to download zip after retries: {}",
            download_err
        ));
    }

    let cursor = std::io::Cursor::new(zip_buf);
    zip_extract::extract(cursor, &install_dir, true).map_err(err)?;

    // Verify extraction - sing-box.exe should be present (zips sometimes have a subfolder)
    let mut found = install_dir.join("sing-box.exe");
    if !found.exists() {
        if let Ok(entries) = std::fs::read_dir(&install_dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let candidate = entry.path().join("sing-box.exe");
                    if candidate.exists() {
                        found = candidate;
                        break;
                    }
                }
            }
        }
    }

    if !found.exists() {
        return Err("Download completed but sing-box.exe was not found after extraction. The zip structure may have changed.".to_string());
    }

    // Move to expected root location if needed
    if found != install_dir.join("sing-box.exe") {
        let _ = std::fs::rename(&found, install_dir.join("sing-box.exe"));
    }

    Ok(())
}

/// Fetch a Goida-style public subscription (plaintext, one `vless://`/
/// `trojan://`/`ss://` URI per line — e.g. an AvenCores/goida-vpn-configs
/// raw githubmirror link) and parse it. Stores the parsed list in
/// `AppState` so `goida_test_all`/`goida_select` can reference candidates
/// by tag without re-fetching.
#[tauri::command]
fn goida_fetch_list(state: tauri::State<AppState>, url: String) -> CmdResult<Vec<GoidaConfig>> {
    let body = ureq::get(&url)
        .timeout(Duration::from_secs(20))
        .call()
        .map_err(err)?
        .into_string()
        .map_err(err)?;
    let configs = goida::parse_subscription(&body);
    *state.goida_configs.lock().unwrap() = configs.clone();
    Ok(configs)
}

#[derive(Serialize, Clone)]
pub struct GoidaTestResult {
    tag: String,
    remark: String,
    country_code: Option<String>,
    country_flag: Option<String>,
    delay_ms: u32,
}

/// Bulk delay-test every previously fetched Goida candidate.
///
/// Rebuilds the engine config with *only* the candidate outbounds (no WARP
/// endpoint, no active routing rules — `route.final` stays `direct`), so no
/// live tunnel is in flight while hundreds of test connections run, then
/// reinstalls the service into this "test mode". Delay tests run through
/// sing-box's Clash-compatible API (`GET /proxies/{tag}/delay`) — this
/// bypasses the router entirely, probing each candidate outbound directly,
/// so it can't interfere with (or be interfered with by) routed traffic.
/// Survivors under [`GOIDA_MAX_DELAY_MS`] are returned; the rest are
/// dropped. Call `install_engine` afterwards to leave test mode.
#[tauri::command]
fn goida_test_all(state: tauri::State<AppState>) -> CmdResult<Vec<GoidaTestResult>> {
    engine::ensure_windows().map_err(err)?;
    let configs = state.goida_configs.lock().unwrap().clone();
    if configs.is_empty() {
        return Err(err("No Goida configs loaded — call goida_fetch_list first"));
    }

    let mgr = state.manager();
    let inputs = RouteInputs {
        outbounds: goida::build_outbounds_with_selector(&configs, "goida", None),
        ..Default::default()
    };
    let config = mgr.render_config(&inputs);
    mgr.write_config(&config).map_err(err)?;
    mgr.install_service(&RealSys).map_err(err)?;

    // Give the service + Clash API a moment to come up after (re)install.
    std::thread::sleep(Duration::from_millis(1500));

    // Run delay tests **in parallel**. All candidates are present as outbounds
    // in the temporary test config, so concurrent queries to the Clash API
    // are safe and bring the wall time from minutes down to ~6 seconds.
    let handles: Vec<_> = configs
        .iter()
        .map(|cfg| {
            let tag = cfg.tag.clone();
            let remark = cfg.remark.clone();
            let country_code = cfg.country_code.clone();
            let country_flag = cfg.country_flag.clone();
            let url = format!(
                "http://{CLASH_API_ADDR}/proxies/{}/delay?timeout=5000&url={}",
                urlencode(&tag),
                urlencode(GOIDA_TEST_URL)
            );
            std::thread::spawn(move || {
                let delay_ms = ureq::get(&url)
                    .timeout(Duration::from_secs(6))
                    .call()
                    .ok()
                    .and_then(|resp| resp.into_json::<serde_json::Value>().ok())
                    .and_then(|v| v["delay"].as_u64())
                    .map(|d| d as u32);

                delay_ms.and_then(|d| {
                    if d < GOIDA_MAX_DELAY_MS {
                        Some(GoidaTestResult {
                            tag,
                            remark,
                            country_code,
                            country_flag,
                            delay_ms: d,
                        })
                    } else {
                        None
                    }
                })
            })
        })
        .collect();

    let mut results = Vec::new();
    for handle in handles {
        if let Some(r) = handle.join().ok().flatten() {
            results.push(r);
        }
    }
    Ok(results)
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Activate one previously tested Goida candidate: rebuilds the full config
/// via [`AppState::shared_route_inputs`] (WARP endpoint/rule if present,
/// user overrides, the RKN-blocked and blocks-Russia buckets) plus all
/// Goida candidates grouped under a `selector` defaulted to `tag`, and
/// reinstalls the service — leaving `goida_test_all`'s test mode.
#[tauri::command]
fn goida_select(state: tauri::State<AppState>, tag: String) -> CmdResult<()> {
    engine::ensure_windows().map_err(err)?;
    let configs = state.goida_configs.lock().unwrap().clone();
    if !configs.iter().any(|c| c.tag == tag) {
        return Err(err(format!("unknown goida tag: {tag}")));
    }

    let mgr = state.manager();
    let goida_outbounds = goida::build_outbounds_with_selector(&configs, "goida", Some(&tag));
    let inputs = state.shared_route_inputs(goida_outbounds, Some("goida"))?;
    let config = mgr.render_config(&inputs);
    mgr.write_config(&config).map_err(err)?;
    mgr.install_service(&RealSys).map_err(err)?;
    // Remember the selection so a later "Запустить движок" keeps this server.
    *state.active_goida_tag.lock().unwrap() = Some(tag);
    Ok(())
}

/// Fetch the "blocks Russia" community domain list used for the Goida
/// routing bucket and cache it in `AppState`. Call before `install_engine`/
/// `goida_select` for that bucket's rule to have any effect (it's simply
/// omitted while the cache is empty).
#[tauri::command]
fn rules_refresh_community_list(state: tauri::State<AppState>) -> CmdResult<usize> {
    let body = ureq::get(rules::COMMUNITY_LIST_URL)
        .timeout(Duration::from_secs(20))
        .call()
        .map_err(err)?
        .into_string()
        .map_err(err)?;
    let domains = rules::parse_domain_list(&body);
    let count = domains.len();
    *state.goida_community_domains.lock().unwrap() = domains;
    Ok(count)
}

#[tauri::command]
fn rules_get_overrides(state: tauri::State<AppState>) -> CmdResult<Vec<Override>> {
    state.override_store().load().map_err(err)
}

#[tauri::command]
fn rules_set_override(
    state: tauri::State<AppState>,
    domain: String,
    bucket: Bucket,
) -> CmdResult<Vec<Override>> {
    state.override_store().set(&domain, bucket).map_err(err)
}

#[tauri::command]
fn rules_remove_override(
    state: tauri::State<AppState>,
    domain: String,
) -> CmdResult<Vec<Override>> {
    state.override_store().remove(&domain).map_err(err)
}

/// Relaunch the app requesting Administrator rights via UAC.
/// Call this from the UI when the user sees permission errors.
#[tauri::command]
fn relaunch_as_admin() -> CmdResult<()> {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe().map_err(err)?;
        let workdir = exe.parent().unwrap_or(&exe).to_path_buf();

        let _ = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    r#"Start-Process -FilePath '{}' -Verb runas -WorkingDirectory '{}'"#,
                    exe.display(),
                    workdir.display()
                ),
            ])
            .spawn();

        std::thread::sleep(Duration::from_millis(400));
        std::process::exit(0);
    }

    #[cfg(not(windows))]
    Err(err(
        "Запрос прав администратора требуется только на Windows",
    ))
}

#[tauri::command]
fn check_admin() -> bool {
    #[cfg(windows)]
    {
        // net session succeeds (exit 0) only when running elevated
        std::process::Command::new("net")
            .args(["session"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Returns a big verbose diagnostic report for debugging.
/// This is meant to be dumped to the log so user can copy-paste.
#[tauri::command]
fn get_diagnostics_report(state: tauri::State<AppState>) -> CmdResult<String> {
    let mut r = String::new();
    let mgr = state.manager();
    let warp = state.warp_manager();
    let sys = RealSys;

    r.push_str("\n========== FULL DIAGNOSTICS ==========\n");
    r.push_str(&format!("Is Admin (net session): {}\n", check_admin()));
    r.push_str(&format!(
        "Engine install dir: {}\n",
        mgr.install_dir().display()
    ));
    r.push_str(&format!("Warp dir: {}\n", warp.install_dir().display()));

    // Sing-box binary
    r.push_str("\n--- SING-BOX BINARY ---\n");
    r.push_str(&format!("sing-box.exe exists: {}\n", mgr.binary_present()));
    if mgr.binary_present() {
        if let Ok(meta) = std::fs::metadata(mgr.binary_path()) {
            r.push_str(&format!("Size: {} bytes\n", meta.len()));
        }
        // Try to get version
        if let Ok(out) = sys.run(&PlannedCommand::new(
            mgr.binary_path().to_string_lossy().into_owned(),
            ["version"],
        )) {
            r.push_str(&format!("Version output: {}\n", out.stdout.trim()));
        }
    }

    // Config
    r.push_str("\n--- CONFIG ---\n");
    if mgr.config_path().exists() {
        match std::fs::read_to_string(mgr.config_path()) {
            Ok(content) => {
                r.push_str("config.json present (first 800 chars):\n");
                r.push_str(&content.chars().take(800).collect::<String>());
                r.push('\n');
            }
            Err(e) => r.push_str(&format!("Failed to read config: {}\n", e)),
        }
    } else {
        r.push_str("No config.json found\n");
    }

    // Scheduled task raw output
    r.push_str("\n--- TASK (schtasks /query /tn tandem-singbox) ---\n");
    match sys.run(&PlannedCommand::new(
        "schtasks",
        ["/query", "/tn", "tandem-singbox", "/v", "/fo", "list"],
    )) {
        Ok(out) => {
            r.push_str(&out.stdout);
            if !out.stderr.is_empty() {
                r.push_str("STDERR: ");
                r.push_str(&out.stderr);
            }
            r.push_str(&format!("Exit code: {:?}\n", out.code));
        }
        Err(e) => r.push_str(&format!("Failed to run schtasks /query: {}\n", e)),
    }

    // Processes
    r.push_str("\n--- PROCESSES (sing-box) ---\n");
    if let Ok(out) = sys.run(&PlannedCommand::new(
        "tasklist",
        ["/FI", "IMAGENAME eq sing-box.exe"],
    )) {
        r.push_str(&out.stdout);
    }

    // WARP details
    r.push_str("\n--- WARP FILES ---\n");
    r.push_str(&format!("wgcf.exe present: {}\n", warp.wgcf_present()));
    r.push_str(&format!(
        "wgcf-account.toml present: {}\n",
        warp.registered()
    ));
    r.push_str(&format!(
        "wgcf-profile.conf present: {}\n",
        warp.profile_generated()
    ));

    if warp.registered() {
        if let Ok(s) = std::fs::read_to_string(warp.account_path()) {
            r.push_str("--- account.toml (truncated) ---\n");
            r.push_str(&s.chars().take(400).collect::<String>());
            r.push('\n');
        }
    }
    if warp.profile_generated() {
        if let Ok(s) = std::fs::read_to_string(warp.profile_path()) {
            r.push_str("--- profile.conf (truncated) ---\n");
            r.push_str(&s.chars().take(500).collect::<String>());
            r.push('\n');
        }
    }

    // Goida state (in memory)
    r.push_str("\n--- GOIDA ---\n");
    let goida_count = state.goida_configs.lock().unwrap().len();
    r.push_str(&format!(
        "Loaded Goida configs in memory: {}\n",
        goida_count
    ));
    let community_count = state.goida_community_domains.lock().unwrap().len();
    r.push_str(&format!("Community domains loaded: {}\n", community_count));

    // List files in engine dir (debug)
    r.push_str("\n--- ENGINE DIR CONTENTS ---\n");
    if let Ok(rd) = std::fs::read_dir(mgr.install_dir()) {
        for entry in rd.flatten().take(30) {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            r.push_str(&format!(
                "  {}{} {}\n",
                if is_dir { "[DIR] " } else { "" },
                name,
                entry
                    .metadata()
                    .map(|m| format!("{}B", m.len()))
                    .unwrap_or_default()
            ));
        }
    }

    r.push_str("\n========== END DIAGNOSTICS ==========\n");
    Ok(r)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState {
            install_dir: Mutex::new(default_install_dir()),
            goida_configs: Mutex::new(Vec::new()),
            goida_community_domains: Mutex::new(Vec::new()),
            active_goida_tag: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_install_dir,
            get_engine_status,
            install_engine,
            remove_engine,
            run_tests,
            download_singbox_release,
            warp_status,
            warp_register,
            warp_diagnose,
            download_wgcf_release,
            goida_fetch_list,
            goida_test_all,
            goida_select,
            rules_refresh_community_list,
            rules_get_overrides,
            rules_set_override,
            rules_remove_override,
            relaunch_as_admin,
            check_admin,
            get_diagnostics_report,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tandem-vpn");
}
