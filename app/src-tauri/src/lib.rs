//! Tauri command layer for tandem-vpn.
//!
//! Thin wrappers that adapt [`tandem_core`] to the GUI. Windows-only service
//! operations run through [`tandem_core::sys::RealSys`]; network operations
//! (update checks, list downloads, connectivity tests) use `ureq` here so the
//! core crate stays offline-testable.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tandem_core::sys::RealSys;
use tandem_core::zapret::{self, DiagnosticsReport, IpsetFilter, ZapretStatus};
use tandem_core::ZapretManager;

/// Shared application state: the active Zapret install directory.
pub struct AppState {
    install_dir: Mutex<PathBuf>,
}

impl AppState {
    fn manager(&self) -> ZapretManager {
        ZapretManager::new(self.install_dir.lock().unwrap().clone())
    }
}

fn default_install_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("zapret")))
        .unwrap_or_else(|| PathBuf::from("zapret"))
}

type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[derive(Serialize)]
pub struct Settings {
    install_dir: String,
    game_filter: bool,
    auto_update: bool,
    ipset_filter: IpsetFilter,
}

#[derive(Serialize)]
pub struct UpdateCheck {
    local: String,
    remote: String,
    update_available: bool,
    release_url: String,
}

#[derive(Serialize)]
pub struct TargetResult {
    url: String,
    ok: bool,
    status: Option<u16>,
    ms: u128,
    error: Option<String>,
}

#[tauri::command]
fn get_settings(state: tauri::State<AppState>) -> CmdResult<Settings> {
    let mgr = state.manager();
    Ok(Settings {
        install_dir: mgr.install_dir().to_string_lossy().into_owned(),
        game_filter: mgr.game_filter_enabled(),
        auto_update: mgr.auto_update_enabled(),
        ipset_filter: mgr.ipset_filter(),
    })
}

#[tauri::command]
fn set_install_dir(state: tauri::State<AppState>, dir: String) -> CmdResult<()> {
    *state.install_dir.lock().unwrap() = PathBuf::from(dir);
    Ok(())
}

#[tauri::command]
fn list_strategies(state: tauri::State<AppState>) -> CmdResult<Vec<String>> {
    state.manager().list_strategies().map_err(err)
}

#[tauri::command]
fn get_status(state: tauri::State<AppState>) -> CmdResult<ZapretStatus> {
    state.manager().status(&RealSys).map_err(err)
}

#[tauri::command]
fn run_diagnostics(state: tauri::State<AppState>) -> CmdResult<DiagnosticsReport> {
    state.manager().diagnostics(&RealSys).map_err(err)
}

#[tauri::command]
fn install_service(
    state: tauri::State<AppState>,
    strategy: String,
    game_filter: bool,
) -> CmdResult<()> {
    zapret::ensure_windows().map_err(err)?;
    let mgr = state.manager();
    let contents = std::fs::read_to_string(mgr.install_dir().join(&strategy)).map_err(err)?;
    let game = if game_filter {
        zapret::GameFilter::enabled()
    } else {
        zapret::GameFilter::disabled()
    };
    mgr.set_game_filter(game_filter).map_err(err)?;
    mgr.install_service(&RealSys, &strategy, &contents, &game)
        .map_err(err)
}

#[tauri::command]
fn remove_service(state: tauri::State<AppState>) -> CmdResult<()> {
    zapret::ensure_windows().map_err(err)?;
    state.manager().remove_service(&RealSys).map_err(err)
}

#[tauri::command]
fn set_game_filter(state: tauri::State<AppState>, enabled: bool) -> CmdResult<()> {
    state.manager().set_game_filter(enabled).map_err(err)
}

#[tauri::command]
fn set_auto_update(state: tauri::State<AppState>, enabled: bool) -> CmdResult<()> {
    state.manager().set_auto_update(enabled).map_err(err)
}

#[tauri::command]
fn set_ipset_filter(state: tauri::State<AppState>, mode: IpsetFilter) -> CmdResult<()> {
    state.manager().set_ipset_filter(mode).map_err(err)
}

fn http_get_text(url: &str) -> CmdResult<String> {
    ureq::get(url)
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(err)?
        .into_string()
        .map_err(err)
}

#[tauri::command]
fn check_updates(local_version: String) -> CmdResult<UpdateCheck> {
    let remote = http_get_text(zapret::VERSION_URL)?.trim().to_string();
    let update = zapret::update_available(&local_version, &remote);
    let release_url = if update {
        format!("{}{}", zapret::RELEASE_TAG_URL, remote)
    } else {
        zapret::LATEST_RELEASE_URL.to_string()
    };
    Ok(UpdateCheck {
        local: local_version,
        remote,
        update_available: update,
        release_url,
    })
}

#[tauri::command]
fn update_ipset_list(state: tauri::State<AppState>) -> CmdResult<usize> {
    let body = ureq::get(zapret::IPSET_LIST_URL)
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(err)?
        .into_string()
        .map_err(err)?;
    state
        .manager()
        .write_ipset_list(body.as_bytes())
        .map_err(err)?;
    Ok(body.lines().filter(|l| !l.trim().is_empty()).count())
}

#[tauri::command]
fn run_tests(state: tauri::State<AppState>) -> CmdResult<Vec<TargetResult>> {
    let targets = state.manager().test_targets();
    let mut results = Vec::with_capacity(targets.len());
    for url in targets {
        let started = Instant::now();
        let res = ureq::get(&url).timeout(Duration::from_secs(8)).call();
        let ms = started.elapsed().as_millis();
        match res {
            Ok(resp) => results.push(TargetResult {
                url,
                ok: resp.status() < 400,
                status: Some(resp.status()),
                ms,
                error: None,
            }),
            Err(e) => results.push(TargetResult {
                url,
                ok: false,
                status: None,
                ms,
                error: Some(e.to_string()),
            }),
        }
    }
    Ok(results)
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
            list_strategies,
            get_status,
            run_diagnostics,
            install_service,
            remove_service,
            set_game_filter,
            set_auto_update,
            set_ipset_filter,
            check_updates,
            update_ipset_list,
            run_tests,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tandem-vpn");
}
