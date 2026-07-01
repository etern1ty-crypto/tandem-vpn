//! `tandem-core` — cross-platform core logic for the tandem-vpn GUI.
//!
//! A rule-based routing core built around `sing-box`: a TUN inbound captures
//! all system traffic, and `route.rules` send it out one of three ways —
//! `direct` (RU-domestic sites, games, anything unmatched), `warp`
//! (Cloudflare WARP, for RU-throttled-but-not-blocked foreign services), or
//! a Goida-sourced outbound (for services that geo-block Russia outright).
//!
//! The module is split into:
//! * [`sys`] — a thin abstraction over process execution so command planning
//!   can be unit-tested without a Windows host.
//! * [`engine`] — sing-box process/service lifecycle and config generation.
//! * [`warp`] — free Cloudflare WARP account registration (via `wgcf`) and
//!   rendering the resulting WireGuard profile into a sing-box endpoint.
//! * [`hosts`] — idempotent hosts-file merging.

pub mod engine;
pub mod hosts;
pub mod sys;
pub mod warp;

pub use engine::{EngineManager, EngineStatus, RouteInputs};
pub use sys::ServiceState;

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
    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
