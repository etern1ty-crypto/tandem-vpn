//! Routing engine: process/service lifecycle + config generation for
//! `sing-box`, which replaces the old Zapret/WinDivert engine.
//!
//! sing-box runs as a Windows service (the same `sc create`/`sc start`
//! pattern used for the previous `winws.exe` service) with a TUN inbound
//! that transparently captures all system traffic. `route.rules` decide,
//! per connection, whether traffic goes out `direct`, through `warp`, or
//! through a Goida-sourced outbound — see [`crate::warp`] and
//! [`crate::goida`] (added in later phases) for those outbound kinds.
//!
//! This module only owns the engine's own lifecycle and the *shape* of the
//! config; it stays agnostic of what outbounds/rules callers pass in so it
//! doesn't need to change as WARP/Goida are wired up.

use crate::sys::{parse_sc_state, PlannedCommand, ServiceState, Sys};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Windows service name created for the routing engine.
pub const SERVICE_NAME: &str = "tandem-singbox";

/// Address of sing-box's Clash-compatible control API (`experimental.clash_api`),
/// used by the Tauri layer to drive delay-tests and outbound selection.
pub const CLASH_API_ADDR: &str = "127.0.0.1:9191";

/// Aggregate status shown on the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineStatus {
    pub service: ServiceState,
    pub sing_box_running: bool,
    pub binary_present: bool,
}

/// Outbounds/endpoints/rules assembled by callers (the `warp` and `goida`
/// modules) and handed to [`EngineManager::render_config`]. Kept as one
/// struct so adding a new source (e.g. Goida's many per-config outbounds)
/// doesn't grow `render_config`'s parameter list.
#[derive(Debug, Clone, Default)]
pub struct RouteInputs {
    /// Entries for the top-level `outbounds` array (`direct` is added
    /// automatically and must not be included here).
    pub outbounds: Vec<Value>,
    /// Entries for the top-level `endpoints` array (e.g. a WARP `wireguard`
    /// endpoint). Endpoint tags are valid `route.rules[].outbound` targets.
    pub endpoints: Vec<Value>,
    /// Entries for `route.rule_set` (remote/local rule-set definitions
    /// referenced by tag from `rules`, e.g. [`crate::rules::build_rule_set_defs`]).
    pub rule_set: Vec<Value>,
    /// Entries for `route.rules`.
    pub rules: Vec<Value>,
}

/// Manages a single sing-box installation directory (binary + config).
pub struct EngineManager {
    install_dir: PathBuf,
}

impl EngineManager {
    pub fn new(install_dir: impl Into<PathBuf>) -> Self {
        Self {
            install_dir: install_dir.into(),
        }
    }

    pub fn install_dir(&self) -> &Path {
        &self.install_dir
    }

    pub fn binary_path(&self) -> PathBuf {
        self.install_dir.join("sing-box.exe")
    }

    pub fn config_path(&self) -> PathBuf {
        self.install_dir.join("config.json")
    }

    pub fn binary_present(&self) -> bool {
        self.binary_path().exists()
    }

    /// Build the sing-box config: a TUN inbound capturing all system
    /// traffic, `direct` plus any caller-supplied outbounds/endpoints, and
    /// caller-supplied routing rules.
    ///
    /// `route.final = "direct"` is load-bearing: it's what keeps games and
    /// any traffic not matched by a rule on the zero-extra-hop direct path
    /// by default, rather than accidentally tunneling everything.
    ///
    /// Endpoints (e.g. a WARP `wireguard` endpoint from [`crate::warp`]) are
    /// a separate top-level array from `outbounds`, but their tags are valid
    /// `route.rules[].outbound` targets just like a regular outbound —
    /// verified against the real `sing-box check` (1.13.14), since the docs
    /// alone don't state this explicitly.
    pub fn render_config(&self, inputs: &RouteInputs) -> Value {
        let mut outbounds = vec![json!({ "type": "direct", "tag": "direct" })];
        outbounds.extend(inputs.outbounds.iter().cloned());

        json!({
            "log": { "level": "info", "timestamp": true },
            "experimental": {
                "clash_api": { "external_controller": CLASH_API_ADDR }
            },
            "inbounds": [{
                "type": "tun",
                "tag": "tun-in",
                "interface_name": "tandem-tun",
                "address": ["172.19.0.1/30"],
                "mtu": 9000,
                "auto_route": true,
                "strict_route": true,
                "stack": "mixed"
            }],
            "endpoints": inputs.endpoints,
            "outbounds": outbounds,
            "route": {
                "auto_detect_interface": true,
                "final": "direct",
                "rule_set": inputs.rule_set,
                "rules": inputs.rules
            }
        })
    }

    pub fn write_config(&self, config: &Value) -> Result<()> {
        std::fs::create_dir_all(&self.install_dir)?;
        let bytes = serde_json::to_vec_pretty(config).map_err(|e| Error::Other(e.to_string()))?;
        std::fs::write(self.config_path(), bytes)?;
        Ok(())
    }

    /// Build the `binPath=` value for `sc create`: the quoted sing-box path
    /// invoking `run -c <config>`.
    fn bin_path(&self) -> String {
        format!(
            "\"{}\" run -c \"{}\" --disable-color",
            self.binary_path().display(),
            self.config_path().display()
        )
    }

    /// Install (or reinstall) the engine as an auto-start Windows service.
    /// Callers must have already written the config via [`Self::write_config`].
    pub fn install_service<S: Sys>(&self, sys: &S) -> Result<()> {
        // Clean any previous instance first.
        sys.run(&PlannedCommand::new("net", ["stop", SERVICE_NAME]))?;
        sys.run(&PlannedCommand::new("sc", ["delete", SERVICE_NAME]))?;

        sys.run(&PlannedCommand::new(
            "sc",
            [
                "create",
                SERVICE_NAME,
                "binPath=",
                &self.bin_path(),
                "DisplayName=",
                "tandem-vpn engine",
                "start=",
                "auto",
            ],
        ))?;
        sys.run(&PlannedCommand::new(
            "sc",
            [
                "description",
                SERVICE_NAME,
                "tandem-vpn routing engine (sing-box)",
            ],
        ))?;
        sys.run(&PlannedCommand::new("sc", ["start", SERVICE_NAME]))?;
        Ok(())
    }

    /// Stop and remove the engine service.
    pub fn remove_service<S: Sys>(&self, sys: &S) -> Result<()> {
        sys.run(&PlannedCommand::new("net", ["stop", SERVICE_NAME]))?;
        sys.run(&PlannedCommand::new("sc", ["delete", SERVICE_NAME]))?;
        sys.run(&PlannedCommand::new(
            "taskkill",
            ["/IM", "sing-box.exe", "/F"],
        ))?;
        Ok(())
    }

    fn sing_box_running<S: Sys>(&self, sys: &S) -> Result<bool> {
        let out = sys.run(&PlannedCommand::new(
            "tasklist",
            ["/FI", "IMAGENAME eq sing-box.exe"],
        ))?;
        Ok(out.stdout.to_lowercase().contains("sing-box.exe"))
    }

    /// Aggregate status.
    pub fn status<S: Sys>(&self, sys: &S) -> Result<EngineStatus> {
        let out = sys.run(&PlannedCommand::new("sc", ["query", SERVICE_NAME]))?;
        let service = parse_sc_state(&out.stdout, out.code);
        Ok(EngineStatus {
            service,
            sing_box_running: self.sing_box_running(sys)?,
            binary_present: self.binary_present(),
        })
    }
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

    fn temp_manager(label: &str) -> EngineManager {
        let dir =
            std::env::temp_dir().join(format!("tandem-engine-{label}-{}", std::process::id()));
        EngineManager::new(dir)
    }

    #[test]
    fn render_config_defaults_to_direct_final() {
        let mgr = temp_manager("render");
        let cfg = mgr.render_config(&RouteInputs::default());
        assert_eq!(cfg["route"]["final"], "direct");
        let outbounds = cfg["outbounds"].as_array().unwrap();
        assert_eq!(outbounds.len(), 1);
        assert_eq!(outbounds[0]["type"], "direct");
        assert_eq!(cfg["inbounds"][0]["type"], "tun");
        assert!(cfg["endpoints"].as_array().unwrap().is_empty());
    }

    #[test]
    fn render_config_appends_endpoints_outbounds_and_rules() {
        let mgr = temp_manager("extra");
        let warp_endpoint = json!({ "type": "wireguard", "tag": "warp" });
        let goida_outbound = json!({ "type": "vless", "tag": "goida-1" });
        let rule = json!({ "domain_suffix": [".youtube.com"], "outbound": "warp" });
        let inputs = RouteInputs {
            outbounds: vec![goida_outbound],
            endpoints: vec![warp_endpoint],
            rules: vec![rule],
            ..Default::default()
        };
        let cfg = mgr.render_config(&inputs);
        let outbounds = cfg["outbounds"].as_array().unwrap();
        assert_eq!(outbounds.len(), 2);
        assert_eq!(outbounds[1]["tag"], "goida-1");
        assert_eq!(cfg["endpoints"][0]["tag"], "warp");
        assert_eq!(cfg["route"]["rules"][0]["outbound"], "warp");
        // direct must still be present and final, even with extra outbounds.
        assert_eq!(cfg["route"]["final"], "direct");
    }

    #[test]
    fn write_config_round_trips() {
        let mgr = temp_manager("write");
        let cfg = mgr.render_config(&RouteInputs::default());
        mgr.write_config(&cfg).unwrap();
        let read_back: Value =
            serde_json::from_str(&std::fs::read_to_string(mgr.config_path()).unwrap()).unwrap();
        assert_eq!(read_back["route"]["final"], "direct");
        let _ = std::fs::remove_dir_all(mgr.install_dir());
    }

    #[test]
    fn install_plans_expected_commands() {
        let mgr = temp_manager("install");
        let sys = MockSys::ok();
        mgr.install_service(&sys).unwrap();
        let log = sys.log();
        assert!(log
            .iter()
            .any(|c| c.starts_with("sc create tandem-singbox")));
        assert!(log.iter().any(|c| c.contains("sing-box.exe")));
        assert!(log.iter().any(|c| c.contains("run -c")));
        assert!(log.iter().any(|c| c.starts_with("sc start tandem-singbox")));
    }

    #[test]
    fn remove_plans_expected_commands() {
        let mgr = temp_manager("remove");
        let sys = MockSys::ok();
        mgr.remove_service(&sys).unwrap();
        let log = sys.log();
        assert!(log.iter().any(|c| c == "sc delete tandem-singbox"));
        assert!(log.iter().any(|c| c.contains("sing-box.exe")));
    }

    #[test]
    fn status_reports_running_when_sc_and_tasklist_say_so() {
        let mgr = temp_manager("status");
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
                    stdout: "sing-box.exe  4321 Console".into(),
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
        assert!(status.sing_box_running);
    }
}
