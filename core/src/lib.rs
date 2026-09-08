//! Local-first, auditable Windows connectivity operations.
//!
//! The core is shared by the desktop application and the `tandem` CLI.
//! Parsing, validation, bundle inspection and recovery planning work offline.
//! Privileged changes are Windows-only and confined to one protected root.

pub mod bundle;
pub mod config;
pub mod files;
pub mod hosts;
pub mod network;
pub mod platform;
pub mod service;
pub mod sys;
pub mod workbench;
pub mod zapret;

pub use workbench::{Action, Workbench};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Invalid input: {0}")]
    Invalid(String),
    #[error("Security check failed: {0}")]
    Security(String),
    #[error("Another operation is active; wait and retry")]
    Busy,
    #[error("Operation timed out: {0}")]
    Timeout(String),
    #[error("This operation requires Windows x64")]
    UnsupportedPlatform,
    #[error("Run the desktop app or terminal as Administrator for this operation")]
    AdministratorRequired,
    #[error("Windows operation {operation} failed with code {code}")]
    Windows { operation: String, code: u32 },
    #[error("{program} failed (exit {code:?}): {detail}")]
    Command {
        program: String,
        code: Option<i32>,
        detail: String,
    },
    #[error("{0}")]
    Operation(String),
}

pub type Result<T> = std::result::Result<T, Error>;
