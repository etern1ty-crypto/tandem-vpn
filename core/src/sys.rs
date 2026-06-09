//! Process-execution abstraction.
//!
//! Windows service management is done by shelling out to `sc`, `net`, `netsh`,
//! `tasklist`, `taskkill` and `reg` — exactly like the upstream `service.bat`.
//! To keep that logic testable on any platform we plan commands as plain data
//! ([`PlannedCommand`]) and run them through a [`Sys`] implementation. Tests use
//! [`MockSys`] to assert the planned commands without touching the OS.

use std::cell::RefCell;
use std::process::Command;

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
