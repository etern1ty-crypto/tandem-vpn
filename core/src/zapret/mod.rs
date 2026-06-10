//! Zapret (Flowseal) engine: a GUI-friendly reimplementation of `service.bat`.
//!
//! Command planning (the `sc`/`net`/`netsh`/`reg` invocations) is testable on
//! any platform via the [`crate::sys::Sys`] abstraction; filesystem-backed
//! settings (toggles, lists) use the real filesystem. Network access (update
//! downloads, connectivity tests) is intentionally kept out of this crate — the
//! GUI layer performs HTTP and feeds bytes back in — so `core` stays dependency
//! light and unit-testable offline.

pub mod strategy;

pub use strategy::GameFilter;

use crate::sys::{PlannedCommand, Sys};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Windows service name created for the bypass.
pub const SERVICE_NAME: &str = "zapret";
/// WinDivert driver service names that may be left behind.
pub const WINDIVERT_SERVICES: [&str; 2] = ["WinDivert", "WinDivert14"];

/// Upstream URLs used for update checks (the GUI performs the actual HTTP).
pub const VERSION_URL: &str =
    "https://raw.githubusercontent.com/Flowseal/zapret-discord-youtube/main/.service/version.txt";
pub const RELEASE_TAG_URL: &str =
    "https://github.com/Flowseal/zapret-discord-youtube/releases/tag/";
pub const LATEST_RELEASE_URL: &str =
    "https://github.com/Flowseal/zapret-discord-youtube/releases/latest";
pub const IPSET_LIST_URL: &str =
    "https://raw.githubusercontent.com/Flowseal/zapret-discord-youtube/main/lists/ipset-all.txt";

/// State of a Windows service as reported by `sc query`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Running,
    Stopped,
    StopPending,
    StartPending,
    NotInstalled,
    Unknown,
}

/// IPSet filter mode, mirroring the upstream `none` / `loaded` / `any` options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpsetFilter {
    /// No IPs are subjected to the filter (empty ipset).
    None,
    /// IPs are checked against the loaded `ipset-all.txt` list.
    Loaded,
    /// Every IP is subjected to the filter.
    Any,
}

/// Aggregate status shown on the dashboard (mirrors `service.bat` "Check Status").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZapretStatus {
    pub service: ServiceState,
    pub windivert: ServiceState,
    pub winws_running: bool,
    pub windivert_sys_present: bool,
    pub installed_strategy: Option<String>,
}

/// Diagnostics report (mirrors `service.bat` "Run Diagnostics").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsReport {
    pub bfe_running: bool,
    pub windivert_sys_present: bool,
    pub winws_running: bool,
    pub service: ServiceState,
    pub notes: Vec<String>,
}

/// Parse a service state out of `sc query` stdout.
pub fn parse_sc_state(stdout: &str, exit_code: Option<i32>) -> ServiceState {
    // 1060 == "service does not exist"; sc also prints FAILED 1060.
    if exit_code == Some(1060) || stdout.contains("1060") {
        return ServiceState::NotInstalled;
    }
    for line in stdout.lines() {
        let l = line.trim();
        if l.starts_with("STATE") {
            let upper = l.to_uppercase();
            if upper.contains("STOP_PENDING") {
                return ServiceState::StopPending;
            }
            if upper.contains("START_PENDING") {
                return ServiceState::StartPending;
            }
            if upper.contains("RUNNING") {
                return ServiceState::Running;
            }
            if upper.contains("STOPPED") {
                return ServiceState::Stopped;
            }
        }
    }
    if exit_code == Some(0) {
        ServiceState::Unknown
    } else {
        ServiceState::NotInstalled
    }
}

/// Manages a single Zapret installation directory.
pub struct ZapretManager {
    install_dir: PathBuf,
}

impl ZapretManager {
    pub fn new(install_dir: impl Into<PathBuf>) -> Self {
        Self {
            install_dir: install_dir.into(),
        }
    }

    pub fn install_dir(&self) -> &Path {
        &self.install_dir
    }
    pub fn bin_dir(&self) -> PathBuf {
        self.install_dir.join("bin")
    }
    pub fn lists_dir(&self) -> PathBuf {
        self.install_dir.join("lists")
    }
    pub fn utils_dir(&self) -> PathBuf {
        self.install_dir.join("utils")
    }
    fn winws_path(&self) -> PathBuf {
        self.bin_dir().join("winws.exe")
    }

    /// List available strategy `.bat` files (everything except `service*`),
    /// sorted case-insensitively. Mirrors the install picker in `service.bat`.
    pub fn list_strategies(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let rd = match std::fs::read_dir(&self.install_dir) {
            Ok(rd) => rd,
            Err(_) => return Ok(out),
        };
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let lower = name.to_lowercase();
            if lower.ends_with(".bat") && !lower.starts_with("service") {
                out.push(name);
            }
        }
        out.sort_by_key(|a| a.to_lowercase());
        Ok(out)
    }

    /// Build the `binPath=` value for `sc create`: the quoted winws path
    /// followed by the rendered strategy arguments.
    pub fn build_bin_path(&self, strategy_contents: &str, game: &GameFilter) -> Result<String> {
        let args = strategy::parse_and_render(
            strategy_contents,
            &self.bin_dir(),
            &self.lists_dir(),
            game,
        )?;
        Ok(format!("\"{}\" {}", self.winws_path().display(), args))
    }

    /// Enable TCP timestamps (`netsh`) — required by some strategies.
    pub fn tcp_enable<S: Sys>(&self, sys: &S) -> Result<()> {
        let cmd = PlannedCommand::new(
            "netsh",
            ["interface", "tcp", "set", "global", "timestamps=enabled"],
        );
        sys.run(&cmd)?;
        Ok(())
    }

    /// Install a strategy as an auto-start Windows service (menu option 1).
    pub fn install_service<S: Sys>(
        &self,
        sys: &S,
        strategy_filename: &str,
        strategy_contents: &str,
        game: &GameFilter,
    ) -> Result<()> {
        let bin_path = self.build_bin_path(strategy_contents, game)?;

        // Clean any previous instance first.
        sys.run(&PlannedCommand::new("net", ["stop", SERVICE_NAME]))?;
        sys.run(&PlannedCommand::new("sc", ["delete", SERVICE_NAME]))?;

        sys.run(&PlannedCommand::new(
            "sc",
            [
                "create",
                SERVICE_NAME,
                "binPath=",
                &bin_path,
                "DisplayName=",
                "zapret",
                "start=",
                "auto",
            ],
        ))?;
        sys.run(&PlannedCommand::new(
            "sc",
            ["description", SERVICE_NAME, "Zapret DPI bypass software"],
        ))?;
        self.tcp_enable(sys)?;
        sys.run(&PlannedCommand::new("sc", ["start", SERVICE_NAME]))?;

        // Record the installed strategy name in the registry (like upstream).
        let stem = Path::new(strategy_filename)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| strategy_filename.to_string());
        sys.run(&PlannedCommand::new(
            "reg",
            [
                "add",
                r"HKLM\System\CurrentControlSet\Services\zapret",
                "/v",
                "zapret-discord-youtube",
                "/t",
                "REG_SZ",
                "/d",
                &stem,
                "/f",
            ],
        ))?;
        Ok(())
    }

    /// Remove the zapret service, kill winws and remove WinDivert (menu option 2).
    pub fn remove_service<S: Sys>(&self, sys: &S) -> Result<()> {
        sys.run(&PlannedCommand::new("net", ["stop", SERVICE_NAME]))?;
        sys.run(&PlannedCommand::new("sc", ["delete", SERVICE_NAME]))?;
        sys.run(&PlannedCommand::new("taskkill", ["/IM", "winws.exe", "/F"]))?;
        for svc in WINDIVERT_SERVICES {
            sys.run(&PlannedCommand::new("net", ["stop", svc]))?;
            sys.run(&PlannedCommand::new("sc", ["delete", svc]))?;
        }
        Ok(())
    }

    fn query_state<S: Sys>(&self, sys: &S, service: &str) -> Result<ServiceState> {
        let out = sys.run(&PlannedCommand::new("sc", ["query", service]))?;
        Ok(parse_sc_state(&out.stdout, out.code))
    }

    fn winws_running<S: Sys>(&self, sys: &S) -> Result<bool> {
        let out = sys.run(&PlannedCommand::new(
            "tasklist",
            ["/FI", "IMAGENAME eq winws.exe"],
        ))?;
        Ok(out.stdout.to_lowercase().contains("winws.exe"))
    }

    fn windivert_sys_present(&self) -> bool {
        let bin = self.bin_dir();
        std::fs::read_dir(&bin)
            .map(|rd| {
                rd.flatten().any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .to_lowercase()
                        .ends_with(".sys")
                })
            })
            .unwrap_or(false)
    }

    /// Read the installed strategy name from the registry (menu option 3).
    fn installed_strategy<S: Sys>(&self, sys: &S) -> Option<String> {
        let out = sys
            .run(&PlannedCommand::new(
                "reg",
                [
                    "query",
                    r"HKLM\System\CurrentControlSet\Services\zapret",
                    "/v",
                    "zapret-discord-youtube",
                ],
            ))
            .ok()?;
        for line in out.stdout.lines() {
            if let Some(idx) = line.find("REG_SZ") {
                let val = line[idx + "REG_SZ".len()..].trim();
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
        None
    }

    /// Aggregate status (menu option 3).
    pub fn status<S: Sys>(&self, sys: &S) -> Result<ZapretStatus> {
        let service = self.query_state(sys, SERVICE_NAME)?;
        let windivert = self.query_state(sys, "WinDivert")?;
        Ok(ZapretStatus {
            service,
            windivert,
            winws_running: self.winws_running(sys)?,
            windivert_sys_present: self.windivert_sys_present(),
            installed_strategy: self.installed_strategy(sys),
        })
    }

    /// Diagnostics (menu option 10).
    pub fn diagnostics<S: Sys>(&self, sys: &S) -> Result<DiagnosticsReport> {
        let bfe = self.query_state(sys, "BFE")?;
        let bfe_running = bfe == ServiceState::Running;
        let windivert_sys_present = self.windivert_sys_present();
        let winws_running = self.winws_running(sys)?;
        let service = self.query_state(sys, SERVICE_NAME)?;

        let mut notes = Vec::new();
        if !bfe_running {
            notes.push("Base Filtering Engine (BFE) is not running — zapret cannot work.".into());
        }
        if !windivert_sys_present {
            notes.push("WinDivert64.sys not found in bin/.".into());
        }
        if service == ServiceState::StopPending {
            notes.push(
                "zapret is STOP_PENDING — likely a conflict with another bypass tool.".into(),
            );
        }
        if !winws_running && service != ServiceState::Running {
            notes.push("Bypass (winws.exe) is not running.".into());
        }
        Ok(DiagnosticsReport {
            bfe_running,
            windivert_sys_present,
            winws_running,
            service,
            notes,
        })
    }

    // ---- Filesystem-backed settings -------------------------------------

    fn marker(&self, name: &str) -> PathBuf {
        self.utils_dir().join(name)
    }

    fn read_flag(&self, name: &str) -> bool {
        self.marker(name).exists()
    }

    fn write_flag(&self, name: &str, enabled: bool) -> Result<()> {
        std::fs::create_dir_all(self.utils_dir())?;
        let path = self.marker(name);
        if enabled {
            std::fs::write(&path, b"")?;
        } else if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// Auto-update toggle (menu option 6) — mirrors `utils/check_updates.enabled`.
    pub fn auto_update_enabled(&self) -> bool {
        self.read_flag("check_updates.enabled")
    }
    pub fn set_auto_update(&self, enabled: bool) -> Result<()> {
        self.write_flag("check_updates.enabled", enabled)
    }

    /// Game filter toggle (menu option 4) — persisted for the next install.
    pub fn game_filter_enabled(&self) -> bool {
        self.read_flag("game_filter.enabled")
    }
    pub fn set_game_filter(&self, enabled: bool) -> Result<()> {
        self.write_flag("game_filter.enabled", enabled)
    }
    pub fn current_game_filter(&self) -> GameFilter {
        if self.game_filter_enabled() {
            GameFilter::enabled()
        } else {
            GameFilter::disabled()
        }
    }

    /// IPSet filter selection (menu option 5), persisted to `utils/ipset_filter`.
    pub fn ipset_filter(&self) -> IpsetFilter {
        match std::fs::read_to_string(self.marker("ipset_filter")) {
            Ok(s) => match s.trim() {
                "none" => IpsetFilter::None,
                "any" => IpsetFilter::Any,
                _ => IpsetFilter::Loaded,
            },
            Err(_) => IpsetFilter::Loaded,
        }
    }
    pub fn set_ipset_filter(&self, filter: IpsetFilter) -> Result<()> {
        std::fs::create_dir_all(self.utils_dir())?;
        let value = match filter {
            IpsetFilter::None => "none",
            IpsetFilter::Loaded => "loaded",
            IpsetFilter::Any => "any",
        };
        std::fs::write(self.marker("ipset_filter"), value)?;
        // Apply the selection to the ipset file used by strategies.
        match filter {
            IpsetFilter::None => self.write_ipset_list(b"")?,
            IpsetFilter::Any => self.write_ipset_list(b"0.0.0.0/0\n::/0\n")?,
            IpsetFilter::Loaded => {} // keep whatever list is on disk
        }
        Ok(())
    }

    /// Overwrite `lists/ipset-all.txt` (menu option 7 applies downloaded bytes here).
    pub fn write_ipset_list(&self, bytes: &[u8]) -> Result<()> {
        std::fs::create_dir_all(self.lists_dir())?;
        std::fs::write(self.lists_dir().join("ipset-all.txt"), bytes)?;
        Ok(())
    }

    /// Default connectivity-test targets (menu option 11, Standard tests) read
    /// from `utils/targets.txt`, falling back to a built-in list.
    pub fn test_targets(&self) -> Vec<String> {
        if let Ok(s) = std::fs::read_to_string(self.utils_dir().join("targets.txt")) {
            let v: Vec<String> = s
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(|l| l.to_string())
                .collect();
            if !v.is_empty() {
                return v;
            }
        }
        default_test_targets()
    }
}

/// Built-in fallback list of domains for connectivity testing.
pub fn default_test_targets() -> Vec<String> {
    [
        "https://www.youtube.com",
        "https://rr1---sn-axq7sn7s.googlevideo.com",
        "https://discord.com",
        "https://gateway.discord.gg",
        "https://web.telegram.org",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Compare a local version to a remote one; `true` when an update is available.
pub fn update_available(local: &str, remote: &str) -> bool {
    local.trim() != remote.trim()
}

/// Best-effort guard used by the GUI layer before running Windows-only actions.
pub fn ensure_windows() -> Result<()> {
    if cfg!(windows) {
        Ok(())
    } else {
        Err(Error::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::{CmdOutput, MockSys};

    #[test]
    fn parses_running_state() {
        let out = "SERVICE_NAME: zapret\n        STATE              : 4  RUNNING";
        assert_eq!(parse_sc_state(out, Some(0)), ServiceState::Running);
    }

    #[test]
    fn parses_stop_pending_and_missing() {
        let out = "        STATE              : 3  STOP_PENDING";
        assert_eq!(parse_sc_state(out, Some(0)), ServiceState::StopPending);
        let missing = "[SC] EnumQueryServicesStatus:OpenService FAILED 1060:";
        assert_eq!(
            parse_sc_state(missing, Some(1060)),
            ServiceState::NotInstalled
        );
    }

    #[test]
    fn install_plans_expected_commands() {
        let mgr = ZapretManager::new("/opt/zapret");
        let sys = MockSys::ok();
        let strategy =
            "start \"z\" /min \"%BIN%winws.exe\" --wf-tcp=80,443 --hostlist=\"%LISTS%a.txt\"";
        mgr.install_service(&sys, "general (ALT).bat", strategy, &GameFilter::disabled())
            .unwrap();
        let log = sys.log();
        assert!(log.iter().any(|c| c.starts_with("sc create zapret")));
        assert!(log.iter().any(|c| c.contains("winws.exe")));
        assert!(log.iter().any(|c| c.starts_with("sc start zapret")));
        assert!(log.iter().any(|c| c.contains("zapret-discord-youtube")));
        // strategy stem (without extension) recorded
        assert!(log.iter().any(|c| c.contains("general (ALT)")));
    }

    #[test]
    fn remove_plans_windivert_cleanup() {
        let mgr = ZapretManager::new("/opt/zapret");
        let sys = MockSys::ok();
        mgr.remove_service(&sys).unwrap();
        let log = sys.log();
        assert!(log.iter().any(|c| c == "sc delete zapret"));
        assert!(log.iter().any(|c| c.contains("WinDivert14")));
        assert!(log.iter().any(|c| c.contains("taskkill")));
    }

    #[test]
    fn status_reports_running_when_sc_says_so() {
        let mgr = ZapretManager::new("/opt/zapret");
        let sys = MockSys::with(|cmd| {
            if cmd.program == "sc" && cmd.args.first().map(String::as_str) == Some("query") {
                CmdOutput {
                    code: Some(0),
                    stdout: "STATE : 4 RUNNING".into(),
                    stderr: String::new(),
                }
            } else if cmd.program == "tasklist" {
                CmdOutput {
                    code: Some(0),
                    stdout: "winws.exe  1234 Console".into(),
                    stderr: String::new(),
                }
            } else {
                CmdOutput {
                    code: Some(0),
                    ..Default::default()
                }
            }
        });
        let status = mgr.status(&sys).unwrap();
        assert_eq!(status.service, ServiceState::Running);
        assert!(status.winws_running);
    }

    #[test]
    fn toggles_persist_to_filesystem() {
        let dir = std::env::temp_dir().join(format!("tandem-test-{}", std::process::id()));
        let mgr = ZapretManager::new(&dir);
        assert!(!mgr.auto_update_enabled());
        mgr.set_auto_update(true).unwrap();
        assert!(mgr.auto_update_enabled());
        mgr.set_auto_update(false).unwrap();
        assert!(!mgr.auto_update_enabled());

        mgr.set_ipset_filter(IpsetFilter::Any).unwrap();
        assert_eq!(mgr.ipset_filter(), IpsetFilter::Any);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_compare() {
        assert!(update_available("1.9.9a", "1.9.9b"));
        assert!(!update_available("1.9.9a", " 1.9.9a "));
    }
}
