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
use tandem_core::sys::RealSys;
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
            rule_set: rules::build_rule_set_defs(),
            ..Default::default()
        };

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
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("engine")))
        .unwrap_or_else(|| PathBuf::from("engine"))
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
/// No Goida server is selected by a plain install; use `goida_select` for
/// that (it reinstalls with the full Goida bucket wired in too).
#[tauri::command]
fn install_engine(state: tauri::State<AppState>) -> CmdResult<()> {
    engine::ensure_windows().map_err(err)?;
    let mgr = state.manager();
    let inputs = state.shared_route_inputs(Vec::new(), None)?;
    let config = mgr.render_config(&inputs);
    mgr.write_config(&config).map_err(err)?;
    mgr.install_service(&RealSys).map_err(err)
}

#[tauri::command]
fn remove_engine(state: tauri::State<AppState>) -> CmdResult<()> {
    engine::ensure_windows().map_err(err)?;
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
/// step. Re-running the engine install (`install_engine`) afterwards is
/// what actually wires the resulting endpoint into the live config.
#[tauri::command]
fn warp_register(state: tauri::State<AppState>) -> CmdResult<()> {
    engine::ensure_windows().map_err(err)?;
    let warp = state.warp_manager();
    warp.register(&RealSys).map_err(err)?;
    warp.generate_profile(&RealSys).map_err(err)
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
    let resp = ureq::get("https://api.github.com/repos/SagerNet/sing-box/releases/latest")
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

    let mut zip_url = None;
    for asset in assets {
        if let Some(name) = asset["name"].as_str() {
            let lower = name.to_lowercase();
            // Releases also ship a `-legacy-windows-7` amd64 zip; skip it so
            // this doesn't nondeterministically pick whichever comes first.
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

    let zip_resp = ureq::get(&zip_url)
        .set("User-Agent", "tandem-vpn")
        .timeout(Duration::from_secs(120))
        .call()
        .map_err(err)?;

    let mut buf = Vec::new();
    let mut reader = zip_resp.into_reader();
    std::io::Read::read_to_end(&mut reader, &mut buf).map_err(err)?;

    let install_dir = state.manager().install_dir().to_path_buf();
    std::fs::create_dir_all(&install_dir).map_err(err)?;

    let cursor = std::io::Cursor::new(buf);
    zip_extract::extract(cursor, &install_dir, true).map_err(err)?;

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
    mgr.install_service(&RealSys).map_err(err)
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState {
            install_dir: Mutex::new(default_install_dir()),
            goida_configs: Mutex::new(Vec::new()),
            goida_community_domains: Mutex::new(Vec::new()),
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
            download_wgcf_release,
            goida_fetch_list,
            goida_test_all,
            goida_select,
            rules_refresh_community_list,
            rules_get_overrides,
            rules_set_override,
            rules_remove_override,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tandem-vpn");
}
