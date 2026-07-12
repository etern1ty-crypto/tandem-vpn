//! Process-execution abstraction.
//!
//! Windows persistence is done by shelling out to `schtasks`, `net`,
//! `tasklist` and `taskkill`. To keep that logic testable on any platform we
//! plan commands as plain data ([`PlannedCommand`]) and run them through a
//! [`Sys`] implementation. Tests use [`MockSys`] to assert the planned
//! commands without touching the OS.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// State of the engine's scheduled task, as reported by `schtasks /query`.
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

/// Parse a task state out of `schtasks /query /fo list` stdout. Used for the
/// scheduled task that persists `sing-box.exe` (a plain console binary, not
/// an SCM service — see [`crate::engine`]).
pub fn parse_schtasks_state(stdout: &str, exit_code: Option<i32>) -> ServiceState {
    if exit_code != Some(0) {
        return ServiceState::NotInstalled;
    }
    for line in stdout.lines() {
        if let Some(status) = line.trim().strip_prefix("Status:") {
            return match status.trim().to_uppercase().as_str() {
                "RUNNING" => ServiceState::Running,
                "READY" | "DISABLED" | "QUEUED" => ServiceState::Stopped,
                _ => ServiceState::Unknown,
            };
        }
    }
    ServiceState::Unknown
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
        let mut command = Command::new(&cmd.program);
        command.args(&cmd.args);

        // Hide any console window on Windows (prevents "sc opens empty console" and hanging UI artifacts)
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let out = command.output()?;
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
    fn parses_schtasks_states() {
        let running = "Folder: \\\nTaskName: \\tandem-singbox\nStatus:   Running\n";
        assert_eq!(
            parse_schtasks_state(running, Some(0)),
            ServiceState::Running
        );

        let ready = "Folder: \\\nTaskName: \\tandem-singbox\nStatus:   Ready\n";
        assert_eq!(parse_schtasks_state(ready, Some(0)), ServiceState::Stopped);

        let missing = "ERROR: The system cannot find the file specified.";
        assert_eq!(
            parse_schtasks_state(missing, Some(1)),
            ServiceState::NotInstalled
        );
    }
}
