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
use tandem_core::engine::{self, EngineStatus, RouteInputs};
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

/// Shared application state: the active engine install directory.
pub struct AppState {
    install_dir: Mutex<PathBuf>,
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState {
            install_dir: Mutex::new(default_install_dir()),
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tandem-vpn");
}
