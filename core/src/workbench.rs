//! Shared use-case layer. CLI and GUI have identical validation and policy.
use crate::config::Config;
use crate::files::{atomic_write, read_bounded, read_text, OperationLock};
use crate::service::{self, ServiceState};
use crate::sys::{RealSys, Sys};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    GetDashboard {},
    Initialize {},
    Diagnostics {},
    ExportSupport {},
    CheckUpdates {},
    TestTargets {},
    SaveConfig {
        config: Config,
    },
    PreviewStrategy {
        strategy: String,
    },
    InstallService {
        strategy: String,
    },
    StartService {},
    StopService {},
    RemoveService {},
    ImportBundle {
        path: String,
        sha256: String,
        tag: String,
    },
    DownloadBundle {
        sha256: String,
        tag: String,
    },
    RollbackBundle {},
    Recover {},
    ImportIpset {
        path: String,
    },
    ApplyHosts {
        path: String,
    },
    RestoreHosts {},
}

#[derive(Clone)]
pub struct Workbench {
    root: PathBuf,
}
impl Workbench {
    pub fn new() -> Result<Self> {
        Ok(Self {
            root: crate::platform::root_dir()?,
        })
    }
    fn config(&self) -> Result<Config> {
        Config::load(&self.root.join("config.json"))
    }
    fn manager(&self) -> crate::zapret::ZapretManager {
        crate::zapret::ZapretManager::new(self.root.join("engine"))
    }
    fn pending(&self) -> Vec<&'static str> {
        [
            "service-recovery.json",
            "deploy-recovery.json",
            "settings-recovery.json",
        ]
        .into_iter()
        .filter(|name| self.root.join(name).exists())
        .collect()
    }
    fn mutation(
        &self,
        action: &str,
        recovering: bool,
        apply: impl FnOnce() -> Result<Value>,
    ) -> Result<Value> {
        crate::platform::prepare_root(&self.root)?;
        let _lock = OperationLock::acquire(&self.root)?;
        if !recovering && !self.pending().is_empty() {
            return Err(Error::Operation(
                "An interrupted operation needs recovery; select Recover before continuing".into(),
            ));
        }
        self.event(action, "begin", None)?;
        let outcome = apply();
        let log = match &outcome {
            Ok(_) => self.event(action, "success", None),
            Err(error) => self.event(action, "error", Some(&error.to_string())),
        };
        match (outcome, log) {
            (Ok(value), Ok(())) => Ok(value),
            (Ok(_), Err(log)) => Err(Error::Operation(format!("Action completed, but recording its result failed: {log}. Refresh status before retrying"))),
            (Err(error), _) => Err(error),
        }
    }
    fn event(&self, action: &str, status: &str, error: Option<&str>) -> Result<()> {
        let path = self.root.join("events.jsonl");
        if path.exists() && fs::metadata(&path)?.len() > 1024 * 1024 {
            let previous = self.root.join("events.previous.jsonl");
            if previous.exists() {
                fs::remove_file(&previous)?;
            }
            fs::rename(&path, previous)?;
        }
        let event = json!({
            "unix_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis(),
            "action": action, "status": status, "error": error.map(|message| message.chars().take(2048).collect::<String>()),
        });
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(file, "{event}")?;
        file.sync_data()?;
        if std::env::var_os("TANDEM_QUIET").as_deref() != Some(std::ffi::OsStr::new("1")) {
            eprintln!("{event}");
        }
        Ok(())
    }
    fn dashboard(&self) -> Result<Value> {
        let config = self.config()?;
        let own_service = RealSys.query(service::SERVICE_NAME)?;
        let ownership_ok = own_service
            .as_ref()
            .map(|snapshot| service::ensure_owned(&self.root, snapshot).is_ok());
        Ok(json!({
            "schema_version": 1, "app_version": env!("CARGO_PKG_VERSION"), "platform": std::env::consts::OS,
            "root": self.root, "initialized": self.root.join("config.json").is_file(), "administrator": crate::platform::is_admin(),
            "config": config, "bundle": crate::bundle::manifest(&self.root.join("engine"))?,
            "strategies": self.manager().list_strategies()?, "service": own_service, "service_owned": ownership_ok,
            "legacy_service_present": RealSys.query("zapret")?.is_some(),
            "driver_present": self.manager().driver_present(), "pending_recovery": self.pending(),
            "hosts_restore_available": self.root.join("hosts-recovery.json").exists(),
            "bundle_rollback_available": self.root.join("engine.previous").is_dir(),
        }))
    }
    fn diagnostics(&self) -> Result<Value> {
        let dashboard = self.dashboard()?;
        let bfe = RealSys.query("BFE")?;
        let bfe_running = bfe
            .as_ref()
            .is_some_and(|snapshot| snapshot.state == ServiceState::Running);
        let mut issues = Vec::new();
        if !bfe_running {
            issues.push("bfe_not_running");
        }
        if dashboard["legacy_service_present"] == true {
            issues.push("legacy_service_conflict");
        }
        if dashboard["service_owned"] == false {
            issues.push("foreign_service_name_collision");
        }
        if !self.manager().driver_present() {
            issues.push("driver_file_missing");
        }
        if !self.pending().is_empty() {
            issues.push("recovery_required");
        }
        let service_state = dashboard["service"]["state"].clone();
        if service_state != "running" {
            issues.push("service_not_running");
        }
        Ok(json!({
            "schema_version": 1, "bfe_running": bfe_running, "service_state": service_state, "issues": issues,
            "windivert": RealSys.query("WinDivert")?.map(|snapshot| snapshot.state),
            "windivert14": RealSys.query("WinDivert14")?.map(|snapshot| snapshot.state),
            "scope": "Local service and file checks only; does not prove bypass effectiveness, TLS privacy, voice, QUIC or game compatibility",
        }))
    }
    fn support(&self) -> Result<Value> {
        let config = self.config()?;
        let bundle = crate::bundle::manifest(&self.root.join("engine"))?;
        // Deliberately omit local paths, hosts contents, raw logs, hostnames,
        // IP addresses, environment variables and the raw service command line.
        Ok(json!({
            "report_schema": 1, "app_version": env!("CARGO_PKG_VERSION"), "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
            "bundle_tag": bundle.as_ref().map(|receipt| &receipt.tag), "bundle_sha256": bundle.as_ref().map(|receipt| &receipt.sha256),
            "game_filter": config.game_filter, "ipset_filter": config.ipset_filter, "target_count": config.targets.len(),
            "diagnostics": self.diagnostics()?, "redacted": true,
        }))
    }
    pub fn execute(&self, action: Action) -> Result<Value> {
        match action {
            Action::GetDashboard {} => self.dashboard(),
            Action::Diagnostics {} => self.diagnostics(),
            Action::ExportSupport {} => self.support(),
            Action::PreviewStrategy { strategy } => Ok(serde_json::to_value(self.manager().plan(&strategy, self.config()?.game_filter)?)?),
            Action::TestTargets {} => Ok(serde_json::to_value(crate::network::test_targets(&self.config()?)?)?),
            Action::CheckUpdates {} => {
                let release = crate::network::release(None)?;
                let local = crate::bundle::manifest(&self.root.join("engine"))?;
                // Do not invent semantic ordering for upstream's custom tags.
                // A different tag is a candidate to review, not an auto-upgrade.
                Ok(json!({ "local_tag": local.as_ref().map(|receipt| &receipt.tag), "remote_tag": release.tag_name,
                    "different_release": local.as_ref().is_none_or(|receipt| receipt.tag != release.tag_name),
                    "release_url": format!("https://github.com/{}/releases/tag/{}", crate::network::REPOSITORY, release.tag_name),
                    "assets": release.assets, "automatic_install": false }))
            }
            Action::Initialize {} => self.mutation("initialize", false, || {
                if !self.root.join("config.json").exists() { Config::default().save(&self.root.join("config.json"))?; }
                self.dashboard()
            }),
            Action::SaveConfig { config } => self.mutation("save_config", false, || {
                config.validate()?; service::require_stopped(&RealSys, &self.root)?;
                self.settings_transaction(|| {
                    if self.root.join("engine").is_dir() { self.manager().apply_ipset(config.ipset_filter)?; }
                    config.save(&self.root.join("config.json"))
                })?; Ok(json!({"ok":true,"message":"Settings saved; reinstall the stopped service to apply strategy arguments"}))
            }),
            Action::InstallService { strategy } => self.mutation("install_service", false, || {
                service::require_stopped(&RealSys, &self.root)?;
                let config = self.config()?;
                self.settings_transaction(|| self.manager().apply_ipset(config.ipset_filter))?;
                let plan = self.manager().plan(&strategy, config.game_filter)?;
                service::install(&RealSys, &self.root, &plan)?;
                Ok(json!({"ok":true,"message":"Service installed and confirmed running"}))
            }),
            Action::StartService {} => self.mutation("start_service", false, || { service::start(&RealSys, &self.root)?; Ok(json!({"ok":true})) }),
            Action::StopService {} => self.mutation("stop_service", false, || { service::stop(&RealSys, &self.root)?; Ok(json!({"ok":true})) }),
            Action::RemoveService {} => self.mutation("remove_service", false, || { service::remove(&RealSys, &self.root)?; Ok(json!({"ok":true,"message":"Only tandem-zapret was removed; shared drivers were not modified"})) }),
            Action::ImportBundle { path, sha256, tag } => self.mutation("import_bundle", false, || {
                service::require_absent(&RealSys)?; let path = input_path(&path)?;
                let bytes = read_bounded(&path, crate::network::MAX_ARCHIVE)?;
                Ok(serde_json::to_value(crate::bundle::install(&self.root, &bytes, &sha256, &tag)?)?)
            }),
            Action::DownloadBundle { sha256, tag } => self.mutation("download_bundle", false, || {
                service::require_absent(&RealSys)?;
                let bytes = crate::network::download(&tag, &sha256)?;
                Ok(serde_json::to_value(crate::bundle::install(&self.root, &bytes, &sha256, &tag)?)?)
            }),
            Action::RollbackBundle {} => self.mutation("rollback_bundle", false, || {
                service::require_absent(&RealSys)?; crate::bundle::rollback(&self.root)?; Ok(json!({"ok":true}))
            }),
            Action::ImportIpset { path } => self.mutation("import_ipset", false, || {
                service::require_stopped(&RealSys, &self.root)?;
                let text = read_text(&input_path(&path)?, 8 * 1024 * 1024)?; let mode = self.config()?.ipset_filter;
                let mut count = 0;
                self.settings_transaction(|| { count = self.manager().import_ipset(&text, mode)?; Ok(()) })?;
                Ok(json!({"entries":count,"active_mode":mode}))
            }),
            Action::ApplyHosts { path } => self.mutation("apply_hosts", false, || {
                let text = read_text(&input_path(&path)?, 2 * 1024 * 1024)?;
                let count = crate::hosts::apply(&crate::platform::hosts_path()?, &self.root, &text, &self.config()?.hosts_allowed_suffixes)?;
                Ok(json!({"entries":count,"restore_available":true}))
            }),
            Action::RestoreHosts {} => self.mutation("restore_hosts", false, || {
                Ok(json!({"restored":crate::hosts::restore(&crate::platform::hosts_path()?, &self.root)?}))
            }),
            Action::Recover {} => self.mutation("recover", true, || {
                let service_restored = service::recover(&RealSys, &self.root)?;
                let deployment_restored = if self.root.join("deploy-recovery.json").exists() { service::require_absent(&RealSys)?; crate::bundle::recover(&self.root)? } else { false };
                let settings_restored = if self.root.join("settings-recovery.json").exists() { service::require_stopped(&RealSys, &self.root)?; self.restore_settings()? } else { false };
                Ok(json!({"service_restored":service_restored,"deployment_restored":deployment_restored,"settings_restored":settings_restored,
                    "hosts_restore_separate":self.root.join("hosts-recovery.json").exists()}))
            }),
        }
    }

    fn settings_transaction(&self, mut apply: impl FnMut() -> Result<()>) -> Result<()> {
        let journal = self.root.join("settings-recovery.json");
        if journal.exists() {
            return Err(Error::Operation("Settings recovery is required".into()));
        }
        let before: Vec<_> = SETTINGS_FILES
            .iter()
            .map(|name| -> Result<FileBackup> {
                let path = self.root.join(name);
                Ok(FileBackup {
                    relative: (*name).into(),
                    contents: if path.exists() {
                        Some(read_text(&path, 8 * 1024 * 1024)?)
                    } else {
                        None
                    },
                })
            })
            .collect::<Result<_>>()?;
        atomic_write(&journal, &serde_json::to_vec(&before)?)?;
        if let Err(error) = apply() {
            return match self.restore_settings() {
                Ok(_) => Err(Error::Operation(format!(
                    "{error}; previous settings restored"
                ))),
                Err(recovery) => Err(Error::Operation(format!(
                    "{error}; settings recovery failed: {recovery}"
                ))),
            };
        }
        fs::remove_file(journal)?;
        Ok(())
    }
    fn restore_settings(&self) -> Result<bool> {
        let journal = self.root.join("settings-recovery.json");
        if !journal.exists() {
            return Ok(false);
        }
        let before: Vec<FileBackup> =
            serde_json::from_slice(&read_bounded(&journal, 128 * 1024 * 1024)?)?;
        if before.len() != SETTINGS_FILES.len()
            || !SETTINGS_FILES.iter().all(|name| {
                before
                    .iter()
                    .filter(|entry| entry.relative == *name)
                    .count()
                    == 1
            })
        {
            return Err(Error::Security("Invalid settings recovery paths".into()));
        }
        for backup in before {
            let path = self.root.join(&backup.relative);
            if let Some(contents) = backup.contents {
                atomic_write(&path, contents.as_bytes())?;
            } else if path.exists() {
                fs::remove_file(path)?;
            }
        }
        fs::remove_file(journal)?;
        Ok(true)
    }
}
const SETTINGS_FILES: &[&str] = &[
    "config.json",
    "engine/lists/ipset-all.txt",
    "engine/lists/ipset-source.txt",
];
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileBackup {
    relative: String,
    contents: Option<String>,
}

pub fn input_path(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if value.len() > 1024
        || value.contains('\0')
        || value.starts_with("\\\\")
        || !path.is_absolute()
    {
        return Err(Error::Invalid(
            "Use an absolute local file path; UNC/device paths are not allowed".into(),
        ));
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if !matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)))
        {
            return Err(Error::Security(
                "Only local drive-letter paths are accepted".into(),
            ));
        }
        for component in path.components().skip(1) {
            if let Component::Normal(name) = component {
                crate::files::safe_component(&name.to_string_lossy())?;
            }
        }
    }
    crate::files::reject_links(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_ipc_fields_fail() {
        assert!(
            serde_json::from_str::<Action>(r#"{"type":"remove_service","all_drivers":true}"#)
                .is_err()
        );
    }
    #[test]
    fn ipc_no_arbitrary_shell() {
        assert!(
            serde_json::from_str::<Action>(r#"{"type":"run_shell","command":"calc.exe"}"#).is_err()
        );
    }
    #[test]
    fn relative_input_paths_fail() {
        assert!(input_path("../bundle.zip").is_err());
    }
    #[test]
    fn settings_failure_restores_original() {
        let root = crate::files::test_dir("settings");
        let workbench = Workbench { root: root.clone() };
        Config::default().save(&root.join("config.json")).unwrap();
        let before = fs::read(root.join("config.json")).unwrap();
        assert!(workbench
            .settings_transaction(|| {
                atomic_write(&root.join("config.json"), b"broken")?;
                Err(Error::Operation("Simulated write error".into()))
            })
            .is_err());
        assert_eq!(fs::read(root.join("config.json")).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }
}
