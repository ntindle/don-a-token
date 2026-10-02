//! Portable core for don-a-token: no UI, no async runtime, pure logic + types.
//!
//! Modules mirror the moving parts of the donation harness:
//! - [`siwc`] — Sign in with ChatGPT OAuth constants, authorize-URL builder,
//!   PKCE helpers, callback parsing.
//! - [`credentials`] — protected local credential record (save/load/refresh).
//! - [`host`] — stable per-host `ext_agent_host_id` management.
//! - [`rules`] — user donation rules and the run/skip verdict.
//! - [`projects`] — donation-opportunity registry loader + validation.
//! - [`codex`] — Codex app-server launch args and thread message builders.
//! - [`runner`] — job isolation backends (E2B Embed first).

pub mod codex;
pub mod credentials;
pub mod e2b;
pub mod host;
pub mod jobscript;
pub mod projects;
pub mod rules;
pub mod runner;
pub mod siwc;

use thiserror::Error;

/// Errors returned by core helpers. No secrets are ever included in messages.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid OAuth callback: {0}")]
    InvalidCallback(String),

    #[error("new registration callback did not include an issued client id")]
    MissingClientId,

    #[error("callback client id does not match the pending registration")]
    ClientIdMismatch,

    #[error("refusing to save the dynamic registration entrypoint as an issued client id")]
    DynamicClientIdNotIssuable,

    #[error("invalid host id: {0}")]
    InvalidHostId(String),

    #[error("invalid projects registry: {0}")]
    InvalidRegistry(String),

    #[error("url error: {0}")]
    Url(#[from] url::ParseError),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
