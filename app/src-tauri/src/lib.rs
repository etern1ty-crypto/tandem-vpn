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
use tandem_core::sys::RealSys;
use tandem_core::warp::WarpManager;
use tandem_core::EngineManager;

/// Seed list of RU-throttled-but-not-blocked foreign services routed
/// through WARP. This is a starting point, not the final routing-rule
/// design (Phase 4 replaces it with the full domestic/throttled/blocked
/// bucket system).
const WARP_DOMAIN_SUFFIXES: &[&str] = &[
    ".youtube.com",
    ".googlevideo.com",
    ".ytimg.com",
    ".discord.com",
    ".discordapp.com",
    ".discord.gg",
    ".github.com",
    ".githubusercontent.com",
];

/// Placeholder seed for the "services that geo-block Russia" bucket routed
/// through Goida. Phase 4 replaces this with the full curated/user-editable
/// bucket system — this only exists so `goida_select` has something to wire
/// a rule to and prove the plumbing end-to-end.
const GOIDA_DOMAIN_SUFFIXES: &[&str] = &[".gemini.google.com"];

/// Test URL used for Goida delay-testing (must be `https://`, not `http://`
/// — sing-box's Clash API delay endpoint silently ignores `http://` URLs).
const GOIDA_TEST_URL: &str = "https://www.gstatic.com/generate_204";

/// Configs with a measured delay at or above this are treated as unusable
/// and dropped from test results, per the desired "Test" UX.
const GOIDA_MAX_DELAY_MS: u32 = 200;

/// Shared application state: the active engine install directory, plus the
/// most recently fetched Goida candidate list (kept so `goida_test_all`/
/// `goida_select` can reference candidates by tag without re-fetching).
pub struct AppState {
    install_dir: Mutex<PathBuf>,
    goida_configs: Mutex<Vec<GoidaConfig>>,
}

impl AppState {
    fn manager(&self) -> EngineManager {
        EngineManager::new(self.install_dir.lock().unwrap().clone())
    }

    fn warp_manager(&self) -> WarpManager {
        WarpManager::new(self.install_dir.lock().unwrap().join("warp"))
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

/// Install the engine and start it as a Windows service. If a WARP profile
/// has already been generated (via [`warp_register`]), its endpoint is
/// wired in with a rule sending the seed RU-throttled-domain list through
/// it; otherwise the config is `direct`-only. Goida outbounds/rules and the
/// full routing-bucket system are added in later phases.
#[tauri::command]
fn install_engine(state: tauri::State<AppState>) -> CmdResult<()> {
    engine::ensure_windows().map_err(err)?;
    let mgr = state.manager();
    let warp = state.warp_manager();

    let mut inputs = RouteInputs::default();
    if warp.profile_generated() {
        let endpoint = warp.render_endpoint("warp").map_err(err)?;
        inputs.endpoints.push(endpoint);
        inputs.rules.push(serde_json::json!({
            "domain_suffix": WARP_DOMAIN_SUFFIXES,
            "outbound": "warp"
        }));
    }

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

    let mut results = Vec::new();
    for cfg in &configs {
        let url = format!(
            "http://{CLASH_API_ADDR}/proxies/{}/delay?timeout=5000&url={}",
            urlencode(&cfg.tag),
            urlencode(GOIDA_TEST_URL)
        );
        let delay_ms = ureq::get(&url)
            .timeout(Duration::from_secs(6))
            .call()
            .ok()
            .and_then(|resp| resp.into_json::<serde_json::Value>().ok())
            .and_then(|v| v["delay"].as_u64())
            .map(|d| d as u32);

        if let Some(delay_ms) = delay_ms {
            if delay_ms < GOIDA_MAX_DELAY_MS {
                results.push(GoidaTestResult {
                    tag: cfg.tag.clone(),
                    remark: cfg.remark.clone(),
                    country_code: cfg.country_code.clone(),
                    country_flag: cfg.country_flag.clone(),
                    delay_ms,
                });
            }
        }
    }
    Ok(results)
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Activate one previously tested Goida candidate: rebuilds the full config
/// (WARP endpoint/rule if present, all Goida candidates grouped under a
/// `selector` defaulted to `tag`, plus the seed foreign-blocks-Russia rule)
/// and reinstalls the service — leaving `goida_test_all`'s test mode.
#[tauri::command]
fn goida_select(state: tauri::State<AppState>, tag: String) -> CmdResult<()> {
    engine::ensure_windows().map_err(err)?;
    let configs = state.goida_configs.lock().unwrap().clone();
    if !configs.iter().any(|c| c.tag == tag) {
        return Err(err(format!("unknown goida tag: {tag}")));
    }

    let mgr = state.manager();
    let warp = state.warp_manager();
    let mut inputs = RouteInputs::default();

    if warp.profile_generated() {
        let endpoint = warp.render_endpoint("warp").map_err(err)?;
        inputs.endpoints.push(endpoint);
        inputs.rules.push(serde_json::json!({
            "domain_suffix": WARP_DOMAIN_SUFFIXES,
            "outbound": "warp"
        }));
    }

    inputs.outbounds = goida::build_outbounds_with_selector(&configs, "goida", Some(&tag));
    inputs.rules.push(serde_json::json!({
        "domain_suffix": GOIDA_DOMAIN_SUFFIXES,
        "outbound": "goida"
    }));

    let config = mgr.render_config(&inputs);
    mgr.write_config(&config).map_err(err)?;
    mgr.install_service(&RealSys).map_err(err)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState {
            install_dir: Mutex::new(default_install_dir()),
            goida_configs: Mutex::new(Vec::new()),
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tandem-vpn");
}
