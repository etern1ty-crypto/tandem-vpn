//! Owned-service lifecycle with checked exit codes, bounded transitions and
//! a durable rollback journal. Shared WinDivert services are never deleted.
use crate::sys::{checked, PlannedCommand, Sys};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SERVICE_NAME: &str = "tandem-zapret";
const JOURNAL: &str = "service-recovery.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Stopped,
    StartPending,
    StopPending,
    Running,
    ContinuePending,
    PausePending,
    Paused,
    Unknown,
}
impl ServiceState {
    pub fn from_code(code: u32) -> Self {
        match code {
            1 => Self::Stopped,
            2 => Self::StartPending,
            3 => Self::StopPending,
            4 => Self::Running,
            5 => Self::ContinuePending,
            6 => Self::PausePending,
            7 => Self::Paused,
            _ => Self::Unknown,
        }
    }
    fn stable(self) -> bool {
        matches!(self, Self::Stopped | Self::Running)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceSnapshot {
    pub state: ServiceState,
    pub binary_path: String,
    pub start_type: u32,
    pub account: String,
    pub process_id: u32,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    before: Option<ServiceSnapshot>,
}

pub fn ensure_owned(root: &Path, snapshot: &ServiceSnapshot) -> Result<()> {
    let expected = crate::zapret::strategy::quote_arg(
        &root
            .join("engine")
            .join("bin")
            .join("winws.exe")
            .to_string_lossy(),
    );
    let binary = snapshot.binary_path.to_ascii_lowercase();
    if !binary.starts_with(&(expected.to_ascii_lowercase() + " "))
        || !(snapshot.account.eq_ignore_ascii_case("LocalSystem")
            || snapshot
                .account
                .eq_ignore_ascii_case("NT AUTHORITY\\SYSTEM"))
        || !matches!(snapshot.start_type, 2 | 3)
    {
        return Err(Error::Security(
            "The tandem-zapret service is not owned by this installation; no changes were made"
                .into(),
        ));
    }
    Ok(())
}

pub fn require_absent(sys: &impl Sys) -> Result<()> {
    if sys.query(SERVICE_NAME)?.is_some() {
        return Err(Error::Operation(
            "Remove the Tandem service before replacing or rolling back engine files".into(),
        ));
    }
    Ok(())
}

pub fn require_stopped(sys: &impl Sys, root: &Path) -> Result<()> {
    if let Some(snapshot) = sys.query(SERVICE_NAME)? {
        ensure_owned(root, &snapshot)?;
        if snapshot.state != ServiceState::Stopped {
            return Err(Error::Operation(
                "Stop the Tandem service before changing settings or lists".into(),
            ));
        }
    }
    Ok(())
}

fn wait_for(sys: &impl Sys, target: Option<ServiceState>) -> Result<()> {
    for _ in 0..200 {
        let state = sys.query(SERVICE_NAME)?.map(|snapshot| snapshot.state);
        if state == target {
            return Ok(());
        }
        sys.pause();
    }
    Err(Error::Timeout(format!(
        "Service did not reach {target:?} within 20 seconds"
    )))
}
fn stop_owned(sys: &impl Sys, root: &Path) -> Result<()> {
    if let Some(snapshot) = sys.query(SERVICE_NAME)? {
        ensure_owned(root, &snapshot)?;
        if snapshot.state != ServiceState::Stopped {
            checked(sys, PlannedCommand::sc(["stop", SERVICE_NAME]), &[1062])?;
            wait_for(sys, Some(ServiceState::Stopped))?;
        }
    }
    Ok(())
}
fn configure(sys: &impl Sys, binary: &str, start_type: u32) -> Result<()> {
    let operation = if sys.query(SERVICE_NAME)?.is_some() {
        "config"
    } else {
        "create"
    };
    checked(
        sys,
        PlannedCommand::sc([
            operation,
            SERVICE_NAME,
            "binPath=",
            binary,
            "DisplayName=",
            "Tandem Workbench — Zapret",
            "start=",
            if start_type == 3 { "demand" } else { "auto" },
            "obj=",
            "LocalSystem",
            "depend=",
            "BFE",
        ]),
        &[],
    )?;
    checked(
        sys,
        PlannedCommand::sc([
            "description",
            SERVICE_NAME,
            "Managed exclusively by Tandem Workbench; no VPN tunnel or encryption",
        ]),
        &[],
    )
}
fn start_owned(sys: &impl Sys, root: &Path) -> Result<()> {
    let snapshot = sys
        .query(SERVICE_NAME)?
        .ok_or_else(|| Error::Operation("Tandem service is not installed".into()))?;
    ensure_owned(root, &snapshot)?;
    if snapshot.state != ServiceState::Running {
        checked(sys, PlannedCommand::sc(["start", SERVICE_NAME]), &[1056])?;
        wait_for(sys, Some(ServiceState::Running))?;
    }
    Ok(())
}
fn delete_owned(sys: &impl Sys, root: &Path) -> Result<()> {
    stop_owned(sys, root)?;
    if let Some(snapshot) = sys.query(SERVICE_NAME)? {
        ensure_owned(root, &snapshot)?;
        checked(sys, PlannedCommand::sc(["delete", SERVICE_NAME]), &[1060])?;
        wait_for(sys, None)?;
    }
    Ok(())
}
fn restore(sys: &impl Sys, root: &Path, journal: &Journal) -> Result<()> {
    if journal.schema_version != 1 {
        return Err(Error::Invalid("Unknown service journal version".into()));
    }
    if let Some(snapshot) = &journal.before {
        ensure_owned(root, snapshot)?;
    }
    stop_owned(sys, root)?;
    if let Some(snapshot) = &journal.before {
        configure(sys, &snapshot.binary_path, snapshot.start_type)?;
        if snapshot.state == ServiceState::Running {
            start_owned(sys, root)?;
        }
    } else {
        delete_owned(sys, root)?;
    }
    Ok(())
}

fn transaction(sys: &impl Sys, root: &Path, apply: impl FnOnce() -> Result<()>) -> Result<()> {
    let path = root.join(JOURNAL);
    if path.exists() {
        return Err(Error::Operation(
            "A service recovery journal exists. Run Recover before changing the service".into(),
        ));
    }
    let before = sys.query(SERVICE_NAME)?;
    if let Some(snapshot) = &before {
        ensure_owned(root, snapshot)?;
        if !snapshot.state.stable() {
            return Err(Error::Operation(
                "Service is transitioning, paused or unknown; wait or inspect it in Services"
                    .into(),
            ));
        }
    }
    let journal = Journal {
        schema_version: 1,
        before,
    };
    crate::files::atomic_write(&path, &serde_json::to_vec_pretty(&journal)?)?;
    if let Err(error) = apply() {
        return match restore(sys, root, &journal) {
            Ok(()) => { std::fs::remove_file(path)?; Err(Error::Operation(format!("{error}. Previous service state restored"))) }
            Err(rollback) => Err(Error::Operation(format!("{error}. Recovery also failed: {rollback}. Journal retained; run Recover after resolving the Windows error"))),
        };
    }
    std::fs::remove_file(path)?;
    Ok(())
}

pub fn install(
    sys: &impl Sys,
    root: &Path,
    plan: &crate::zapret::strategy::StrategyPlan,
) -> Result<()> {
    if sys.query("zapret")?.is_some() {
        return Err(Error::Operation("An unmanaged legacy zapret service exists. Remove it using its owner tool before installing Tandem".into()));
    }
    let bfe = sys
        .query("BFE")?
        .ok_or_else(|| Error::Operation("Base Filtering Engine is missing".into()))?;
    if bfe.state != ServiceState::Running {
        return Err(Error::Operation(
            "Base Filtering Engine is not running".into(),
        ));
    }
    // The plan is already validated; the executable is always in protected root.
    let expected = root
        .join("engine")
        .join("bin")
        .join("winws.exe")
        .to_string_lossy()
        .into_owned();
    if !plan.executable.eq_ignore_ascii_case(&expected) {
        return Err(Error::Security("Unexpected service executable".into()));
    }
    transaction(sys, root, || {
        stop_owned(sys, root)?;
        configure(sys, &plan.binary_path, 2)?;
        start_owned(sys, root)
    })
}
pub fn start(sys: &impl Sys, root: &Path) -> Result<()> {
    transaction(sys, root, || start_owned(sys, root))
}
pub fn stop(sys: &impl Sys, root: &Path) -> Result<()> {
    transaction(sys, root, || stop_owned(sys, root))
}
pub fn remove(sys: &impl Sys, root: &Path) -> Result<()> {
    transaction(sys, root, || delete_owned(sys, root))
}
pub fn recover(sys: &impl Sys, root: &Path) -> Result<bool> {
    let path = root.join(JOURNAL);
    if !path.exists() {
        return Ok(false);
    }
    let journal = serde_json::from_slice(&crate::files::read_bounded(&path, 128 * 1024)?)?;
    restore(sys, root, &journal)?;
    std::fs::remove_file(path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::{CmdOutput, MockSys};
    struct TestSys(MockSys);
    impl Sys for TestSys {
        fn pause(&self) {}
        fn run(&self, cmd: &PlannedCommand) -> Result<CmdOutput> {
            self.0.run(cmd)
        }
        fn query(&self, name: &str) -> Result<Option<ServiceSnapshot>> {
            if name == "zapret" {
                return Ok(None);
            }
            if name == "BFE" {
                return Ok(Some(ServiceSnapshot {
                    state: ServiceState::Running,
                    binary_path: String::new(),
                    start_type: 2,
                    account: "LocalSystem".into(),
                    process_id: 0,
                }));
            }
            self.0.query(name)
        }
    }
    fn setup() -> (std::path::PathBuf, crate::zapret::strategy::StrategyPlan) {
        let root = crate::files::test_dir("service");
        let plan = crate::zapret::strategy::parse_and_render(
            "start x \"%BIN%winws.exe\" --wf-tcp=443",
            &root.join("engine"),
            crate::config::GameFilter::Disabled,
            false,
        )
        .unwrap();
        (root, plan)
    }
    #[test]
    fn service_install_and_remove_are_scoped() {
        let (root, plan) = setup();
        let sys = TestSys(MockSys::default());
        install(&sys, &root, &plan).unwrap();
        assert_eq!(
            sys.query(SERVICE_NAME).unwrap().unwrap().state,
            ServiceState::Running
        );
        remove(&sys, &root).unwrap();
        assert!(sys.query(SERVICE_NAME).unwrap().is_none());
        assert!(sys
            .0
            .recorded
            .borrow()
            .iter()
            .all(|cmd| cmd.args.get(1).map(String::as_str) == Some(SERVICE_NAME)));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn failed_install_rolls_back_new_service() {
        let (root, plan) = setup();
        let sys = TestSys(MockSys::default());
        *sys.0.fail_on.borrow_mut() = Some("start".into());
        assert!(install(&sys, &root, &plan).is_err());
        assert!(sys.query(SERVICE_NAME).unwrap().is_none());
        assert!(!root.join(JOURNAL).exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn foreign_service_is_not_touched() {
        let (root, _) = setup();
        let sys = TestSys(MockSys::default());
        *sys.0.snapshot.borrow_mut() = Some(ServiceSnapshot {
            state: ServiceState::Running,
            binary_path: "\"C:\\foreign\\winws.exe\" --wf-tcp=443".into(),
            start_type: 2,
            account: "LocalSystem".into(),
            process_id: 0,
        });
        assert!(remove(&sys, &root).is_err());
        assert!(sys.0.recorded.borrow().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn windows_state_mapping_is_numeric() {
        assert_eq!(ServiceState::from_code(4), ServiceState::Running);
        assert_eq!(ServiceState::from_code(999), ServiceState::Unknown);
    }
}
