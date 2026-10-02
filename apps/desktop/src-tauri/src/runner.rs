//! E2B-compatible job runner: sandbox lifecycle + command execution.
//!
//! Speaks the control-plane REST API (Embed `:3000` or Cloud) and envd's
//! Connect-JSON RPC for `process.Process/Start`. Protocol shapes live in
//! `don_a_token_core::e2b`; this module owns HTTP I/O and in-flight jobs.
//!
//! One-shot sandboxes for now: submit creates, the job runs to completion
//! (or timeout/cancel), then the sandbox is killed. Pause/resume and
//! template pre-warming are follow-ups.

use std::collections::HashMap;
use std::time::Duration;

use don_a_token_core::e2b::{self, CommandOutput, E2bConfig, SandboxCreated};
use don_a_token_core::runner::{JobHandle, JobStatus};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tokio::sync::Mutex;
use tokio::time::timeout;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
/// Extra sandbox lifetime beyond the job's own cap, for setup/teardown.
const SANDBOX_MARGIN_SECS: u64 = 180;
const OUTPUT_TAIL_BYTES: usize = 4096;

#[derive(Debug, Clone)]
pub struct E2bRunner {
    client: reqwest::Client,
    cfg: E2bConfig,
}

impl E2bRunner {
    pub fn new(cfg: E2bConfig) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(Self { client, cfg })
    }

    fn api(&self, path: &str) -> String {
        format!("{}{path}", self.cfg.api_base.trim_end_matches('/'))
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.cfg.api_key {
            Some(key) => req.header(e2b::API_KEY_HEADER, key),
            None => req,
        }
    }

    async fn fail_text(res: reqwest::Response) -> String {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(300).collect();
        format!("e2b {status}: {snippet}")
    }

    /// `POST /v2/sandboxes`. Returns the sandbox id + envd access token.
    pub async fn create_sandbox(
        &self,
        template: &str,
        timeout_secs: u64,
        metadata: serde_json::Value,
        env_vars: serde_json::Value,
    ) -> Result<SandboxCreated, String> {
        let body = e2b::create_body(template, timeout_secs, metadata, env_vars, true);
        let res = self
            .authed(self.client.post(self.api("/v2/sandboxes")))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("create sandbox: {e}"))?;
        if !res.status().is_success() {
            return Err(Self::fail_text(res).await);
        }
        let v: serde_json::Value = res.json().await.map_err(|e| format!("create body: {e}"))?;
        e2b::parse_created(&v).map_err(|e| e.to_string())
    }

    /// `GET /sandboxes/{id}`. Lenient: 200 means alive; body kept as JSON.
    pub async fn sandbox_info(&self, sandbox_id: &str) -> Result<serde_json::Value, String> {
        let res = self
            .authed(self.client.get(self.api(&format!("/sandboxes/{sandbox_id}"))))
            .send()
            .await
            .map_err(|e| format!("sandbox info: {e}"))?;
        if !res.status().is_success() {
            return Err(Self::fail_text(res).await);
        }
        res.json().await.map_err(|e| format!("info body: {e}"))
    }

    /// `DELETE /sandboxes/{id}`. Returns false when already gone (404).
    pub async fn kill_sandbox(&self, sandbox_id: &str) -> Result<bool, String> {
        let res = self
            .authed(self.client.delete(self.api(&format!("/sandboxes/{sandbox_id}"))))
            .send()
            .await
            .map_err(|e| format!("kill sandbox: {e}"))?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !res.status().is_success() {
            return Err(Self::fail_text(res).await);
        }
        Ok(true)
    }

    /// `POST /sandboxes/{id}/timeout` — extend lifetime (`timeout` seconds).
    pub async fn set_timeout(&self, sandbox_id: &str, timeout_secs: u64) -> Result<(), String> {
        let res = self
            .authed(self.client.post(self.api(&format!("/sandboxes/{sandbox_id}/timeout"))))
            .json(&serde_json::json!({ "timeout": timeout_secs }))
            .send()
            .await
            .map_err(|e| format!("set timeout: {e}"))?;
        if !res.status().is_success() {
            return Err(Self::fail_text(res).await);
        }
        Ok(())
    }

    /// Run one command to completion via `process.Process/Start`
    /// (Connect JSON, enveloped stream). `overall_timeout` bounds the run.
    pub async fn run_command(
        &self,
        sandbox: &SandboxCreated,
        cmd: &str,
        args: &[String],
        envs: &serde_json::Value,
        cwd: Option<&str>,
        overall_timeout: Duration,
    ) -> Result<CommandOutput, String> {
        let base = e2b::resolve_sandbox_base(&self.cfg, sandbox.domain.as_deref());
        let url = format!("{base}/process.Process/Start");
        let body = e2b::start_body(cmd, args, envs, cwd);
        let mut req = self
            .client
            .post(&url)
            .header(e2b::SANDBOX_ID_HEADER, &sandbox.sandbox_id)
            .header(e2b::SANDBOX_PORT_HEADER, e2b::ENVD_PORT.to_string())
            .header("Content-Type", "application/connect+json");
        if let Some(token) = &sandbox.envd_access_token {
            req = req.header(e2b::ACCESS_TOKEN_HEADER, token);
        }
        let res = req
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("start process: {e}"))?;
        if !res.status().is_success() {
            return Err(Self::fail_text(res).await);
        }
        timeout(overall_timeout, collect_stream(res))
            .await
            .map_err(|_| "command timed out".to_string())?
    }
}

async fn collect_stream(mut res: reqwest::Response) -> Result<CommandOutput, String> {
    let mut buf: Vec<u8> = Vec::new();
    let mut out = CommandOutput::default();
    loop {
        match res.chunk().await.map_err(|e| format!("stream read: {e}"))? {
            Some(chunk) => buf.extend_from_slice(&chunk),
            None => break,
        }
        for env in e2b::drain_envelopes(&mut buf) {
            if env.end {
                if let Some(msg) = e2b::end_stream_error(&env.payload) {
                    return Err(format!("envd error: {msg}"));
                }
                continue;
            }
            let text = String::from_utf8(env.payload)
                .map_err(|_| "envd sent non-utf8 event".to_string())?;
            let event = e2b::parse_event_line(&text).map_err(|e| e.to_string())?;
            out.apply(&event);
        }
    }
    // Drain any envelopes completed by the final chunk boundary.
    for env in e2b::drain_envelopes(&mut buf) {
        if env.end {
            if let Some(msg) = e2b::end_stream_error(&env.payload) {
                return Err(format!("envd error: {msg}"));
            }
            continue;
        }
        let text =
            String::from_utf8(env.payload).map_err(|_| "envd sent non-utf8 event".to_string())?;
        let event = e2b::parse_event_line(&text).map_err(|e| e.to_string())?;
        out.apply(&event);
    }
    if out.exit_code.is_none() {
        return Err("stream ended without an exit event".to_string());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Job state + Tauri commands
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct JobRecord {
    pub(crate) sandbox_id: String,
    pub(crate) sandbox: SandboxCreated,
    pub(crate) cfg: E2bConfig,
    pub(crate) status: JobStatus,
    pub(crate) output: CommandOutput,
    pub(crate) max_minutes: u32,
}

#[derive(Default)]
pub struct JobState(pub Mutex<HashMap<String, JobRecord>>);

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RunnerConfigArgs {
    pub api_base: String,
    pub api_key: Option<String>,
    pub sandbox_base: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SubmitJobArgs {
    pub project_id: String,
    pub job_id: String,
    pub template: String,
    #[allow(dead_code)]
    pub prompt_pack: String,
    pub command: Vec<String>,
    pub env: Vec<(String, String)>,
    pub max_minutes: u32,
}

#[derive(Debug, Serialize)]
pub struct JobHandleView {
    pub id: String,
}

#[derive(Debug, Serialize)]
pub struct JobStatusView {
    pub status: String,
    pub sandbox_id: String,
    pub exit_code: Option<i32>,
    pub stdout_tail: String,
    pub stderr_tail: String,
}

fn status_name(status: &JobStatus) -> String {
    match status {
        JobStatus::Queued => "queued".to_string(),
        JobStatus::Running => "running".to_string(),
        JobStatus::Succeeded => "succeeded".to_string(),
        JobStatus::Failed { reason } => format!("failed: {reason}"),
        JobStatus::TimedOut => "timed-out".to_string(),
    }
}

fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(OUTPUT_TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

/// Submit a job: create a sandbox, run its command in the background,
/// kill the sandbox when done. Returns immediately with a handle.
#[tauri::command]
pub async fn job_submit(
    app: AppHandle,
    jobs: State<'_, JobState>,
    config: RunnerConfigArgs,
    job: SubmitJobArgs,
) -> Result<JobHandleView, String> {
    submit_job_inner(&app, &jobs, config, job).await
}

/// Shared by the `job_submit` command and the background scheduler.
pub async fn submit_job_inner(
    app: &AppHandle,
    jobs: &JobState,
    config: RunnerConfigArgs,
    job: SubmitJobArgs,
) -> Result<JobHandleView, String> {
    if job.command.is_empty() {
        return Err("job has no command".to_string());
    }
    let cfg = E2bConfig {
        api_base: config.api_base,
        api_key: config.api_key,
        sandbox_base: config.sandbox_base,
    };
    let runner = E2bRunner::new(cfg.clone())?;
    let timeout_secs = u64::from(job.max_minutes) * 60 + SANDBOX_MARGIN_SECS;
    let metadata = serde_json::json!({
        "app": "don-a-token",
        "project_id": job.project_id,
        "job_id": job.job_id,
    });
    let env_vars: serde_json::Value = job
        .env
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    let sandbox = runner
        .create_sandbox(&job.template, timeout_secs, metadata, env_vars)
        .await?;

    let handle = JobHandle {
        id: format!("{}-{}", job.project_id, job.job_id),
    };
    jobs.0.lock().await.insert(
        handle.id.clone(),
        JobRecord {
            sandbox_id: sandbox.sandbox_id.clone(),
            sandbox,
            cfg,
            status: JobStatus::Running,
            output: CommandOutput::default(),
            max_minutes: job.max_minutes,
        },
    );

    let app_clone = app.clone();
    let handle_id = handle.id.clone();
    let command = job.command.clone();
    tauri::async_runtime::spawn(async move {
        drive_job(app_clone, handle_id, command).await;
    });

    Ok(JobHandleView { id: handle.id })
}

async fn drive_job(app: AppHandle, handle_id: String, command: Vec<String>) {
    let (runner, sandbox, max_minutes) = {
        let jobs = app.state::<JobState>();
        let guard = jobs.0.lock().await;
        let Some(rec) = guard.get(&handle_id) else {
            return;
        };
        let runner = match E2bRunner::new(rec.cfg.clone()) {
            Ok(r) => r,
            Err(e) => {
                drop(guard);
                finish_job(&app, &handle_id, JobStatus::Failed { reason: e }, None).await;
                return;
            }
        };
        (runner, rec.sandbox.clone(), rec.max_minutes)
    };

    let overall = Duration::from_secs(u64::from(max_minutes) * 60 + SANDBOX_MARGIN_SECS);
    let outcome = runner
        .run_command(
            &sandbox,
            &command[0],
            &command[1..].to_vec(),
            &serde_json::json!({}),
            None,
            overall,
        )
        .await;
    let _ = runner.kill_sandbox(&sandbox.sandbox_id).await;

    match outcome {
        Ok(output) => {
            let status = if output.succeeded() {
                JobStatus::Succeeded
            } else {
                JobStatus::Failed {
                    reason: output
                        .error
                        .clone()
                        .unwrap_or_else(|| format!("exit {}", output.exit_code.unwrap_or(-1))),
                }
            };
            finish_job(&app, &handle_id, status, Some(output)).await;
        }
        Err(e) if e == "command timed out" => {
            finish_job(&app, &handle_id, JobStatus::TimedOut, None).await;
        }
        Err(e) => {
            finish_job(&app, &handle_id, JobStatus::Failed { reason: e }, None).await;
        }
    }
}

/// Record a terminal state, unless the job was already cancelled.
async fn finish_job(app: &AppHandle, handle_id: &str, status: JobStatus, output: Option<CommandOutput>) {
    let jobs = app.state::<JobState>();
    let mut guard = jobs.0.lock().await;
    if let Some(rec) = guard.get_mut(handle_id) {
        if !matches!(rec.status, JobStatus::Running | JobStatus::Queued) {
            return;
        }
        rec.status = status;
        if let Some(out) = output {
            rec.output = out;
        }
    }
}

#[tauri::command]
pub async fn job_status(
    jobs: State<'_, JobState>,
    handle_id: String,
) -> Result<JobStatusView, String> {
    let guard = jobs.0.lock().await;
    let rec = guard
        .get(&handle_id)
        .ok_or_else(|| "unknown job handle".to_string())?;
    Ok(JobStatusView {
        status: status_name(&rec.status),
        sandbox_id: rec.sandbox_id.clone(),
        exit_code: rec.output.exit_code,
        stdout_tail: tail(&rec.output.stdout),
        stderr_tail: tail(&rec.output.stderr),
    })
}

#[tauri::command]
pub async fn job_cancel(
    app: AppHandle,
    jobs: State<'_, JobState>,
    handle_id: String,
) -> Result<(), String> {
    let (cfg, sandbox_id) = {
        let mut guard = jobs.0.lock().await;
        let rec = guard
            .get_mut(&handle_id)
            .ok_or_else(|| "unknown job handle".to_string())?;
        rec.status = JobStatus::Failed {
            reason: "cancelled".to_string(),
        };
        (rec.cfg.clone(), rec.sandbox_id.clone())
    };
    let runner = E2bRunner::new(cfg)?;
    let _ = runner.kill_sandbox(&sandbox_id).await;
    let _ = app;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn envelope(flags: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![flags];
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// Minimal mock E2B: canned control-plane + envd responses, records
    /// request lines + headers for assertions.
    async fn serve_mock(
        listener: TcpListener,
        seen: Arc<StdMutex<Vec<String>>>,
        responders: HashMap<String, Vec<u8>>,
    ) {
        for _ in 0..responders.len() + 2 {
            let accept = timeout(Duration::from_secs(10), listener.accept()).await;
            let Ok(Ok((mut socket, _))) = accept else {
                break;
            };
            let mut buf = vec![0u8; 65536];
            let mut len = 0usize;
            let mut content_len = 0usize;
            let mut header_end = None;
            loop {
                let n = socket.read(&mut buf[len..]).await.unwrap();
                if n == 0 {
                    break;
                }
                len += n;
                if header_end.is_none() {
                    if let Some(pos) = buf[..len]
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                    {
                        header_end = Some(pos + 4);
                        let head = String::from_utf8_lossy(&buf[..pos + 4]).into_owned();
                        for line in head.lines().skip(1) {
                            if let Some(v) = line.strip_prefix("content-length:") {
                                content_len = v.trim().parse().unwrap_or(0);
                            } else if let Some(v) = line.strip_prefix("Content-Length:") {
                                content_len = v.trim().parse().unwrap_or(0);
                            }
                        }
                    }
                }
                if let Some(h) = header_end {
                    if len - h >= content_len {
                        break;
                    }
                }
            }
            let head = String::from_utf8_lossy(&buf[..len]).into_owned();
            let mut lines = head.lines();
            let request_line = lines.next().unwrap_or("").to_string();
            {
                let mut seen = seen.lock().unwrap();
                seen.push(request_line.clone());
                seen.extend(lines.take_while(|l| !l.is_empty()).map(str::to_string));
            }
            let key = request_line.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
            let body = responders.get(&key).cloned().unwrap_or_else(|| b"{}".to_vec());
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        }
    }

    #[tokio::test]
    async fn e2e_against_mock() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");

        // "hi\n" base64; then exit 0; then EndStream.
        let mut stream = envelope(0x00, br#"{"event":{"start":{"pid":7}}}"#);
        stream.extend(envelope(0x00, br#"{"event":{"data":{"stdout":"aGkK"}}}"#));
        stream.extend(envelope(0x00, br#"{"event":{"end":{"exitCode":0,"exited":true,"status":"ok"}}}"#));
        stream.extend(envelope(0x02, br#"{}"#));

        let responders: HashMap<String, Vec<u8>> = [
            (
                "POST /v2/sandboxes".to_string(),
                br#"{"sandboxID":"sbx1","domain":"e2b.app","envdVersion":"0.4.3","envdAccessToken":"tok"}"#.to_vec(),
            ),
            ("POST /process.Process/Start".to_string(), stream),
            ("DELETE /sandboxes/sbx1".to_string(), b"{}".to_vec()),
            (
                "GET /sandboxes/sbx1".to_string(),
                br#"{"sandboxID":"sbx1"}"#.to_vec(),
            ),
        ]
        .into_iter()
        .collect();

        let seen: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
        let server = tokio::spawn(serve_mock(listener, seen.clone(), responders));

        let runner = E2bRunner::new(E2bConfig {
            api_base: base.clone(),
            api_key: Some("test-key".to_string()),
            sandbox_base: Some(base.clone()),
        })
        .unwrap();

        let sandbox = runner
            .create_sandbox("base", 600, serde_json::json!({}), serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(sandbox.sandbox_id, "sbx1");
        assert_eq!(sandbox.envd_access_token.as_deref(), Some("tok"));

        let info = runner.sandbox_info("sbx1").await.unwrap();
        assert_eq!(info["sandboxID"], "sbx1");

        let out = runner
            .run_command(&sandbox, "echo", &["hi".to_string()], &serde_json::json!({}), None, Duration::from_secs(30))
            .await
            .unwrap();
        assert!(out.succeeded());
        assert_eq!(out.stdout, b"hi\n");

        assert!(runner.kill_sandbox("sbx1").await.unwrap());

        // Header assertions (reqwest lowercases nothing; check case-insensitively).
        let seen = seen.lock().unwrap().join("\n").to_lowercase();
        assert!(seen.contains("x-api-key: test-key"), "missing api key header");
        assert!(seen.contains("e2b-sandbox-id: sbx1"), "missing sandbox routing header");
        assert!(seen.contains("e2b-sandbox-port: 49983"), "missing envd port header");
        assert!(seen.contains("x-access-token: tok"), "missing envd token header");
        assert!(seen.contains("content-type: application/connect+json"), "wrong rpc encoding");

        server.abort();
    }

    #[tokio::test]
    async fn kill_is_idempotent_on_404() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = socket.read(&mut buf).await;
            socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await
                .unwrap();
        });
        let runner = E2bRunner::new(E2bConfig {
            api_base: format!("http://127.0.0.1:{port}"),
            api_key: None,
            sandbox_base: None,
        })
        .unwrap();
        assert!(!runner.kill_sandbox("gone").await.unwrap());
    }
}
