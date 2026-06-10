//! `tandem-core` — cross-platform core logic for the tandem-vpn GUI.
//!
//! Phase 1 covers the Zapret (Flowseal) engine: everything that the upstream
//! `service.bat` does, reimplemented as testable Rust so it can be driven from
//! a Tauri GUI instead of an interactive batch-file menu.
//!
//! The module is split into:
//! * [`sys`] — a thin abstraction over process execution so command planning
//!   can be unit-tested without a Windows host.
//! * [`zapret`] — service install/remove/status, toggles and updates.
//! * [`zapret::strategy`] — parsing a Flowseal strategy `.bat` into the
//!   `winws.exe` argument string used to create the Windows service.

pub mod hosts;
pub mod sys;
pub mod zapret;

pub use zapret::{GameFilter, IpsetFilter, ServiceState, ZapretManager, ZapretStatus};

/// Crate-wide error type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(String),
    #[error("command `{program}` failed (code {code:?}): {stderr}")]
    Command {
        program: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error("unsupported platform: this operation requires Windows")]
    UnsupportedPlatform,
    #[error("could not parse strategy file: {0}")]
    Strategy(String),
    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
