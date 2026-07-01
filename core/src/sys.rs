//! Process-execution abstraction.
//!
//! Windows service management is done by shelling out to `sc`, `net`, `netsh`,
//! `tasklist`, `taskkill` and `reg` — exactly like the upstream `service.bat`.
//! To keep that logic testable on any platform we plan commands as plain data
//! ([`PlannedCommand`]) and run them through a [`Sys`] implementation. Tests use
//! [`MockSys`] to assert the planned commands without touching the OS.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::process::Command;

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

/// Parse a service state out of `sc query` stdout. Generic to any service
/// name — used for both the routing-engine service and ancillary services
/// (e.g. BFE) queried during diagnostics.
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

/// A command to be executed: a program plus its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl PlannedCommand {
    pub fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    /// Render the command roughly as a shell would display it (for logs/UI).
    pub fn display(&self) -> String {
        let mut out = self.program.clone();
        for a in &self.args {
            out.push(' ');
            if a.contains(' ') || a.is_empty() {
                out.push('"');
                out.push_str(a);
                out.push('"');
            } else {
                out.push_str(a);
            }
        }
        out
    }
}

/// Result of running a command.
#[derive(Debug, Clone, Default)]
pub struct CmdOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// Abstraction over command execution.
pub trait Sys {
    fn run(&self, cmd: &PlannedCommand) -> crate::Result<CmdOutput>;
}

/// Executes commands for real via [`std::process::Command`].
#[derive(Debug, Default, Clone, Copy)]
pub struct RealSys;

impl Sys for RealSys {
    fn run(&self, cmd: &PlannedCommand) -> crate::Result<CmdOutput> {
        let out = Command::new(&cmd.program).args(&cmd.args).output()?;
        Ok(CmdOutput {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// A mock `Sys` for tests: records planned commands and returns scripted output.
pub struct MockSys {
    pub recorded: RefCell<Vec<PlannedCommand>>,
    responder: Box<dyn Fn(&PlannedCommand) -> CmdOutput>,
}

impl MockSys {
    /// Build a mock that returns success (exit code 0, empty output) for every command.
    pub fn ok() -> Self {
        Self::with(|_| CmdOutput {
            code: Some(0),
            ..Default::default()
        })
    }

    /// Build a mock with a custom responder closure.
    pub fn with(responder: impl Fn(&PlannedCommand) -> CmdOutput + 'static) -> Self {
        Self {
            recorded: RefCell::new(Vec::new()),
            responder: Box::new(responder),
        }
    }

    /// The list of commands seen so far, rendered via [`PlannedCommand::display`].
    pub fn log(&self) -> Vec<String> {
        self.recorded.borrow().iter().map(|c| c.display()).collect()
    }
}

impl Sys for MockSys {
    fn run(&self, cmd: &PlannedCommand) -> crate::Result<CmdOutput> {
        self.recorded.borrow_mut().push(cmd.clone());
        Ok((self.responder)(cmd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_running_state() {
        let out = "SERVICE_NAME: tandem-singbox\n        STATE              : 4  RUNNING";
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
}
