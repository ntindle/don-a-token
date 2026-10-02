//! E2B protocol shapes: control-plane REST + envd Connect-JSON RPC.
//!
//! Sources: the E2B runtime repo (`packages/envd/spec/`, `docs/ARCHITECTURE.md`)
//! and the published JS SDK (v2.52.0), which speaks Connect **JSON**
//! (`useBinaryFormat: false`). Everything here is pure and unit-tested;
//! the async HTTP client lives in the Tauri shell (`runner.rs`).
//!
//! Control plane (auth: `X-API-Key`):
//! - `POST /v2/sandboxes` → `201 {sandboxID, domain?, envdVersion,
//!   envdAccessToken?, trafficAccessToken?}`
//! - `GET /sandboxes/{id}` → sandbox info (200 = alive)
//! - `DELETE /sandboxes/{id}` → kill (404 = already gone)
//! - `POST /sandboxes/{id}/timeout` → extend lifetime
//!
//! Envd (per-sandbox agent, port 49983):
//! - Connect JSON: `POST {sandbox_base}/process.Process/Start`
//! - Headers: `E2b-Sandbox-Id`, `E2b-Sandbox-Port: 49983`,
//!   `X-Access-Token: <envdAccessToken>`
//! - Streaming NDJSON responses; `stdout`/`stderr` bytes are base64.

use base64::Engine as _;
use serde_json::{json, Value};

use crate::CoreError;

pub const API_KEY_HEADER: &str = "X-API-Key";
pub const ACCESS_TOKEN_HEADER: &str = "X-Access-Token";
pub const SANDBOX_ID_HEADER: &str = "E2b-Sandbox-Id";
pub const SANDBOX_PORT_HEADER: &str = "E2b-Sandbox-Port";

pub const ENVD_PORT: u16 = 49983;
pub const CLOUD_API_BASE: &str = "https://api.e2b.dev";
pub const CLOUD_DOMAIN: &str = "e2b.app";
pub const EMBED_API_BASE: &str = "http://127.0.0.1:3000";
pub const EMBED_PROXY_PORT: u16 = 3002;

/// Connection settings for one E2B-compatible backend.
#[derive(Debug, Clone)]
pub struct E2bConfig {
    /// Control-plane base, e.g. `http://127.0.0.1:3000` (Embed) or
    /// `https://api.e2b.dev` (Cloud).
    pub api_base: String,
    /// Team API key. Embed mints one per install; Cloud uses `E2B_API_KEY`.
    pub api_key: Option<String>,
    /// Sandbox-traffic base override. When `None`, derived: Embed →
    /// same host on port 3002; Cloud → `https://sandbox.{domain}`.
    pub sandbox_base: Option<String>,
}

impl E2bConfig {
    pub fn embed_local(api_key: Option<String>) -> Self {
        Self {
            api_base: EMBED_API_BASE.to_string(),
            api_key,
            sandbox_base: None,
        }
    }

    pub fn cloud(api_key: String) -> Self {
        Self {
            api_base: CLOUD_API_BASE.to_string(),
            api_key: Some(api_key),
            sandbox_base: None,
        }
    }
}

/// Resolve where sandbox traffic (envd RPC) goes.
pub fn resolve_sandbox_base(cfg: &E2bConfig, created_domain: Option<&str>) -> String {
    if let Some(base) = &cfg.sandbox_base {
        return base.trim_end_matches('/').to_string();
    }
    if let Ok(mut url) = url::Url::parse(&cfg.api_base) {
        let host = url.host_str().unwrap_or_default().to_string();
        if host == "127.0.0.1" || host == "localhost" || url.port() == Some(3000) {
            let _ = url.set_port(Some(EMBED_PROXY_PORT));
            return url.as_str().trim_end_matches('/').to_string();
        }
    }
    let domain = created_domain.unwrap_or(CLOUD_DOMAIN);
    format!("https://sandbox.{domain}")
}

/// `POST /v2/sandboxes` body. Timeout is in seconds; omission of
/// `autoPause` keeps the default kill-on-timeout, which is what
/// unattended donation jobs want.
pub fn create_body(
    template_id: &str,
    timeout_secs: u64,
    metadata: Value,
    env_vars: Value,
    allow_internet_access: bool,
) -> Value {
    json!({
        "templateID": template_id,
        "timeout": timeout_secs,
        "metadata": metadata,
        "envVars": env_vars,
        "allow_internet_access": allow_internet_access,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxCreated {
    pub sandbox_id: String,
    pub domain: Option<String>,
    pub envd_version: String,
    pub envd_access_token: Option<String>,
}

fn string_field(v: &Value, name: &str) -> Option<String> {
    v.get(name)?.as_str().map(str::to_string)
}

/// Parse `POST /v2/sandboxes` success response. Unknown fields ignored.
pub fn parse_created(v: &Value) -> Result<SandboxCreated, CoreError> {
    let sandbox_id = string_field(v, "sandboxID")
        .ok_or_else(|| CoreError::InvalidRegistry("create response has no sandboxID".into()))?;
    Ok(SandboxCreated {
        sandbox_id,
        domain: string_field(v, "domain"),
        envd_version: string_field(v, "envdVersion").unwrap_or_default(),
        envd_access_token: string_field(v, "envdAccessToken"),
    })
}

/// `POST /process.Process/Start` body (Connect JSON).
pub fn start_body(cmd: &str, args: &[String], envs: &Value, cwd: Option<&str>) -> Value {
    let mut process = json!({
        "cmd": cmd,
        "args": args,
        "envs": envs,
    });
    if let Some(dir) = cwd {
        process["cwd"] = json!(dir);
    }
    json!({ "process": process })
}

/// One decoded process event from the Start response stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcEvent {
    Started { pid: u32 },
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Ended { exit_code: i32, error: Option<String> },
    KeepAlive,
}

/// Parse one NDJSON event line. `stdout`/`stderr` arrive base64-encoded
/// per protobuf JSON mapping and are decoded here.
pub fn parse_event_line(line: &str) -> Result<ProcEvent, CoreError> {
    let v: Value = serde_json::from_str(line)?;
    let event = v
        .get("event")
        .ok_or_else(|| CoreError::InvalidRegistry("event line has no event".into()))?;
    if let Some(start) = event.get("start") {
        let pid = start.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
        return Ok(ProcEvent::Started { pid });
    }
    if let Some(data) = event.get("data") {
        if let Some(b64) = data.get("stdout").and_then(Value::as_str) {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|_| CoreError::InvalidRegistry("bad base64 stdout".into()))?;
            return Ok(ProcEvent::Stdout(bytes));
        }
        if let Some(b64) = data.get("stderr").and_then(Value::as_str) {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|_| CoreError::InvalidRegistry("bad base64 stderr".into()))?;
            return Ok(ProcEvent::Stderr(bytes));
        }
        if let Some(b64) = data.get("pty").and_then(Value::as_str) {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|_| CoreError::InvalidRegistry("bad base64 pty".into()))?;
            return Ok(ProcEvent::Stdout(bytes));
        }
        return Err(CoreError::InvalidRegistry("data event has no output".into()));
    }
    if let Some(end) = event.get("end") {
        let exit_code = end.get("exitCode").and_then(Value::as_i64).unwrap_or(-1) as i32;
        let error = end.get("error").and_then(Value::as_str).map(str::to_string);
        return Ok(ProcEvent::Ended { exit_code, error });
    }
    if event.get("keepalive").is_some() {
        return Ok(ProcEvent::KeepAlive);
    }
    Err(CoreError::InvalidRegistry("unknown event shape".into()))
}

/// Aggregate a full Start stream into output + exit code.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

impl CommandOutput {
    pub fn apply(&mut self, event: &ProcEvent) {
        match event {
            ProcEvent::Stdout(b) => self.stdout.extend_from_slice(b),
            ProcEvent::Stderr(b) => self.stderr.extend_from_slice(b),
            ProcEvent::Ended { exit_code, error } => {
                self.exit_code = Some(*exit_code);
                self.error = error.clone();
            }
            ProcEvent::Started { .. } | ProcEvent::KeepAlive => {}
        }
    }

    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// One Connect-protocol streaming envelope. `Start` is server-streaming,
/// so responses arrive enveloped even with JSON encoding
/// (`Content-Type: application/connect+json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    /// True for the terminal EndStream envelope (flags bit 0x02).
    pub end: bool,
    pub payload: Vec<u8>,
}

/// Drain complete envelopes from the front of `buf`, leaving any partial
/// envelope buffered. Framing: 1 flag byte + 4-byte big-endian length.
pub fn drain_envelopes(buf: &mut Vec<u8>) -> Vec<Envelope> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while buf.len() - at >= 5 {
        let flags = buf[at];
        let len = u32::from_be_bytes([buf[at + 1], buf[at + 2], buf[at + 3], buf[at + 4]]) as usize;
        if buf.len() - at - 5 < len {
            break;
        }
        out.push(Envelope {
            end: flags & 0x02 != 0,
            payload: buf[at + 5..at + 5 + len].to_vec(),
        });
        at += 5 + len;
    }
    buf.drain(..at);
    out
}

/// Extract a Connect `EndStreamResponse` error message, if present.
pub fn end_stream_error(payload: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(payload).ok()?;
    let msg = v.get("error")?.get("message")?.as_str()?;
    if msg.is_empty() {
        None
    } else {
        Some(msg.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_base_derives_embed_proxy() {
        let cfg = E2bConfig::embed_local(None);
        assert_eq!(resolve_sandbox_base(&cfg, None), "http://127.0.0.1:3002");
        let remote = E2bConfig {
            api_base: "http://node.lan:3000".to_string(),
            api_key: None,
            sandbox_base: None,
        };
        assert_eq!(resolve_sandbox_base(&remote, None), "http://node.lan:3002");
    }

    #[test]
    fn sandbox_base_cloud_uses_sandbox_subdomain() {
        let cfg = E2bConfig::cloud("key".to_string());
        assert_eq!(
            resolve_sandbox_base(&cfg, Some("e2b.app")),
            "https://sandbox.e2b.app"
        );
        assert_eq!(resolve_sandbox_base(&cfg, None), "https://sandbox.e2b.app");
    }

    #[test]
    fn sandbox_base_override_wins() {
        let cfg = E2bConfig {
            api_base: EMBED_API_BASE.to_string(),
            api_key: None,
            sandbox_base: Some("http://proxy:4000/".to_string()),
        };
        assert_eq!(resolve_sandbox_base(&cfg, None), "http://proxy:4000");
    }

    #[test]
    fn create_body_shape() {
        let body = create_body("base", 3600, json!({"job": "1"}), json!({}), true);
        assert_eq!(body["templateID"], "base");
        assert_eq!(body["timeout"], 3600);
        assert_eq!(body["allow_internet_access"], true);
        assert!(body.get("autoPause").is_none());
    }

    #[test]
    fn parse_created_response() {
        let v = json!({
            "sandboxID": "abc123",
            "domain": "e2b.app",
            "envdVersion": "0.4.3",
            "envdAccessToken": "tok",
            "trafficAccessToken": "t2",
        });
        assert_eq!(
            parse_created(&v).unwrap(),
            SandboxCreated {
                sandbox_id: "abc123".to_string(),
                domain: Some("e2b.app".to_string()),
                envd_version: "0.4.3".to_string(),
                envd_access_token: Some("tok".to_string()),
            }
        );
        assert!(parse_created(&json!({})).is_err());
    }

    #[test]
    fn start_body_shape() {
        let body = start_body("git", &["clone".to_string()], &json!({}), Some("/work"));
        assert_eq!(body["process"]["cmd"], "git");
        assert_eq!(body["process"]["args"][0], "clone");
        assert_eq!(body["process"]["cwd"], "/work");
    }

    #[test]
    fn event_stream_parses_and_aggregates() {
        // "hi\n" and "oops\n" base64-encoded.
        let lines = [
            r#"{"event":{"start":{"pid":42}}}"#,
            r#"{"event":{"data":{"stdout":"aGkK"}}}"#,
            r#"{"event":{"data":{"stderr":"b29wcwo="}}}"#,
            r#"{"event":{"keepalive":{}}}"#,
            r#"{"event":{"end":{"exitCode":0,"exited":true,"status":"done"}}}"#,
        ];
        let mut out = CommandOutput::default();
        for line in lines {
            out.apply(&parse_event_line(line).unwrap());
        }
        assert_eq!(out.stdout, b"hi\n");
        assert_eq!(out.stderr, b"oops\n");
        assert!(out.succeeded());
    }

    #[test]
    fn failed_exit_reports_code_and_error() {
        let mut out = CommandOutput::default();
        out.apply(
            &parse_event_line(r#"{"event":{"end":{"exitCode":3,"exited":true,"status":"x","error":"boom"}}}"#)
                .unwrap(),
        );
        assert!(!out.succeeded());
        assert_eq!(out.exit_code, Some(3));
        assert_eq!(out.error.as_deref(), Some("boom"));
    }

    #[test]
    fn bad_lines_rejected() {
        assert!(parse_event_line("not json").is_err());
        assert!(parse_event_line(r#"{"event":{}}"#).is_err());
        assert!(parse_event_line(r#"{"event":{"data":{"stdout":"!!!"}}}"#).is_err());
    }

    fn envelope(flags: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![flags];
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn envelopes_drain_and_leave_partial() {
        let a = envelope(0x00, br#"{"event":{"start":{"pid":1}}}"#);
        let end = envelope(0x02, br#"{}"#);
        let mut buf = [a.clone(), end.clone()].concat();
        // Append a truncated envelope: header claims 10 bytes, only 3 present.
        buf.extend_from_slice(&[0x00, 0, 0, 0, 10, 1, 2, 3]);

        let drained = drain_envelopes(&mut buf);
        assert_eq!(drained.len(), 2);
        assert!(!drained[0].end);
        assert_eq!(
            parse_event_line(std::str::from_utf8(&drained[0].payload).unwrap()).unwrap(),
            ProcEvent::Started { pid: 1 }
        );
        assert!(drained[1].end);
        assert_eq!(end_stream_error(&drained[1].payload), None);
        // Partial envelope stays buffered.
        assert_eq!(buf, vec![0x00, 0, 0, 0, 10, 1, 2, 3]);
        assert!(drain_envelopes(&mut buf).is_empty());
    }

    #[test]
    fn end_stream_error_extracted() {
        let payload = br#"{"error":{"code":"internal","message":"boom"}}"#;
        assert_eq!(end_stream_error(payload).as_deref(), Some("boom"));
        assert_eq!(end_stream_error(br#"{}"#), None);
        assert_eq!(end_stream_error(b"nope"), None);
    }
}
