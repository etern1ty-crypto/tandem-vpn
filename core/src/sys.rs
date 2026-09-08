//! A bounded process adapter. Only the trusted system sc.exe is executable.
use crate::{Error, Result};
use std::cell::RefCell;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCommand {
    pub args: Vec<String>,
}
impl PlannedCommand {
    pub fn sc(args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}
#[derive(Debug, Default, Clone)]
pub struct CmdOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}
pub trait Sys {
    fn run(&self, command: &PlannedCommand) -> Result<CmdOutput>;
    fn query(&self, service: &str) -> Result<Option<crate::service::ServiceSnapshot>>;
    fn pause(&self) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
#[derive(Debug, Default, Clone, Copy)]
pub struct RealSys;
impl Sys for RealSys {
    fn query(&self, service: &str) -> Result<Option<crate::service::ServiceSnapshot>> {
        crate::platform::query_service(service)
    }
    fn run(&self, command: &PlannedCommand) -> Result<CmdOutput> {
        crate::platform::ensure_windows()?;
        run_process(command)
    }
}

#[cfg(not(windows))]
fn run_process(_: &PlannedCommand) -> Result<CmdOutput> {
    Err(Error::UnsupportedPlatform)
}

#[cfg(windows)]
fn run_process(command: &PlannedCommand) -> Result<CmdOutput> {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    fn drain(mut pipe: impl Read) -> std::io::Result<String> {
        let mut captured = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = pipe.read(&mut buf)?;
            if n == 0 {
                break;
            }
            let remaining = (128 * 1024usize).saturating_sub(captured.len());
            captured.extend_from_slice(&buf[..n.min(remaining)]);
        }
        Ok(String::from_utf8_lossy(&captured).into_owned())
    }
    let mut child = Command::new(crate::platform::system_dir()?.join("sc.exe"))
        .args(&command.args)
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Operation("Missing child stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Operation("Missing child stderr".into()))?;
    let out = std::thread::spawn(move || drain(stdout));
    let err = std::thread::spawn(move || drain(stderr));
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                timed_out = true;
                let _ = child.kill();
                break child.wait();
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(error);
            }
        }
    };
    let stdout = out
        .join()
        .map_err(|_| Error::Operation("stdout reader failed".into()))??;
    let stderr = err
        .join()
        .map_err(|_| Error::Operation("stderr reader failed".into()))??;
    if timed_out {
        return Err(Error::Timeout(
            "sc.exe was terminated after 15 seconds".into(),
        ));
    }
    Ok(CmdOutput {
        code: status?.code(),
        stdout,
        stderr,
    })
}

pub fn checked(sys: &impl Sys, command: PlannedCommand, allowed: &[i32]) -> Result<()> {
    let out = sys.run(&command)?;
    if out.code == Some(0) || out.code.is_some_and(|code| allowed.contains(&code)) {
        return Ok(());
    }
    Err(Error::Command {
        program: "sc.exe".into(),
        code: out.code,
        detail: format!("{} {}", out.stdout.trim(), out.stderr.trim())
            .trim()
            .into(),
    })
}

/// Explicit simulation used only by unit tests, never by the desktop backend.
pub struct MockSys {
    pub recorded: RefCell<Vec<PlannedCommand>>,
    pub snapshot: RefCell<Option<crate::service::ServiceSnapshot>>,
    pub fail_on: RefCell<Option<String>>,
}
impl Default for MockSys {
    fn default() -> Self {
        Self {
            recorded: RefCell::new(Vec::new()),
            snapshot: RefCell::new(None),
            fail_on: RefCell::new(None),
        }
    }
}
impl Sys for MockSys {
    fn pause(&self) {}
    fn query(&self, _: &str) -> Result<Option<crate::service::ServiceSnapshot>> {
        Ok(self.snapshot.borrow().clone())
    }
    fn run(&self, command: &PlannedCommand) -> Result<CmdOutput> {
        use crate::service::{ServiceSnapshot, ServiceState};
        self.recorded.borrow_mut().push(command.clone());
        let operation = command.args.first().map(String::as_str).unwrap_or_default();
        if self.fail_on.borrow().as_deref() == Some(operation) {
            self.fail_on.borrow_mut().take();
            return Ok(CmdOutput {
                code: Some(5),
                stdout: "Access denied".into(),
                stderr: String::new(),
            });
        }
        let mut snapshot = self.snapshot.borrow_mut();
        match operation {
            "create" | "config" => {
                let path = command
                    .args
                    .windows(2)
                    .find(|pair| pair[0] == "binPath=")
                    .map(|pair| pair[1].clone())
                    .unwrap_or_default();
                let start_type = if command.args.iter().any(|s| s == "demand") {
                    3
                } else {
                    2
                };
                *snapshot = Some(ServiceSnapshot {
                    state: ServiceState::Stopped,
                    binary_path: path,
                    start_type,
                    account: "LocalSystem".into(),
                    process_id: 0,
                });
            }
            "start" => {
                if let Some(value) = snapshot.as_mut() {
                    value.state = ServiceState::Running;
                }
            }
            "stop" => {
                if let Some(value) = snapshot.as_mut() {
                    value.state = ServiceState::Stopped;
                }
            }
            "delete" => {
                *snapshot = None;
            }
            _ => {}
        }
        Ok(CmdOutput {
            code: Some(0),
            ..CmdOutput::default()
        })
    }
}
