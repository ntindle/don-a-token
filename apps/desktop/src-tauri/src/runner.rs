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
use std::path::PathBuf;
use std::time::Duration;

use don_a_token_core::e2b::{self, CommandOutput, E2bConfig, SandboxCreated};
use don_a_token_core::jobscript;
use don_a_token_core::runner::{Backend, JobHandle, JobStatus};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use tokio::time::{timeout, timeout_at};

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
        let raw = serde_json::to_vec(&body).map_err(|e| format!("start body: {e}"))?;
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
            .body(e2b::envelope_message(&raw))
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
            if std::env::var("E2B_DEBUG").is_ok() {
                eprintln!("envd event: {text}");
            }
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
    pub(crate) sandbox: Option<SandboxCreated>,
    pub(crate) cfg: E2bConfig,
    pub(crate) status: JobStatus,
    pub(crate) output: CommandOutput,
    pub(crate) max_minutes: u32,
    pub(crate) env_vars: Vec<(String, String)>,
    /// Host pid of a local-backend job, for cancel/timeout tree-kill.
    pub(crate) local_pid: Option<u32>,
}

#[derive(Default)]
pub struct JobState(pub Mutex<HashMap<String, JobRecord>>);

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RunnerConfigArgs {
    /// Backend name (`Backend::name`); `"local"` runs on-host, no isolation.
    pub backend: String,
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
    let handle = JobHandle {
        id: format!("{}-{}", job.project_id, job.job_id),
    };
    if config.backend == Backend::Local.name() {
        return submit_local(&app, jobs, cfg, job, handle).await;
    }
    let runner = E2bRunner::new(cfg.clone())?;
    let timeout_secs = u64::from(job.max_minutes) * 60 + SANDBOX_MARGIN_SECS;
    let metadata = serde_json::json!({
        "app": "don-a-token",
        "project_id": job.project_id,
        "job_id": job.job_id,
    });
    // Secrets travel as per-command Start env, never on the sandbox record.
    let sandbox = runner
        .create_sandbox(&job.template, timeout_secs, metadata, serde_json::json!({}))
        .await?;

    jobs.0.lock().await.insert(
        handle.id.clone(),
        JobRecord {
            sandbox_id: sandbox.sandbox_id.clone(),
            sandbox: Some(sandbox),
            cfg,
            status: JobStatus::Running,
            output: CommandOutput::default(),
            max_minutes: job.max_minutes,
            env_vars: job.env,
            local_pid: None,
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
    let (runner, sandbox, max_minutes, envs, secrets) = {
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
        let Some(sandbox) = rec.sandbox.clone() else {
            drop(guard);
            finish_job(
                &app,
                &handle_id,
                JobStatus::Failed {
                    reason: "no sandbox".to_string(),
                },
                None,
            )
            .await;
            return;
        };
        let envs: serde_json::Value = rec
            .env_vars
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        let secrets: Vec<String> = rec.env_vars.iter().map(|(_, v)| v.clone()).collect();
        (
            runner,
            sandbox,
            rec.max_minutes,
            envs,
            secrets,
        )
    };

    let overall = Duration::from_secs(u64::from(max_minutes) * 60 + SANDBOX_MARGIN_SECS);
    let outcome = runner
        .run_command(&sandbox, &command[0], &command[1..].to_vec(), &envs, None, overall)
        .await;
    let _ = runner.kill_sandbox(&sandbox.sandbox_id).await;

    match outcome {
        Ok(output) => finish_output(&app, &handle_id, output, &secrets).await,
        Err(e) if e == "command timed out" => {
            finish_job(&app, &handle_id, JobStatus::TimedOut, None).await;
        }
        Err(e) => {
            finish_job(&app, &handle_id, JobStatus::Failed { reason: e }, None).await;
        }
    }
}

/// Native temp workdir for a local job. The scheduler removes it when
/// the job reaches a terminal state so clones + target dirs can't fill
/// the donor's disk; the patch/result are already captured in output.
pub fn local_workdir_native(job_id: &str) -> PathBuf {
    std::env::temp_dir().join(format!("don-a-token-job-{job_id}"))
}

/// Temp workdir for a local job, in posix form for Git Bash
/// (`C:\...` -> `/c/...`; unchanged on unix). The bootstrap script
/// creates it (`mkdir -p`).
pub fn local_workdir_posix(job_id: &str) -> String {
    let native = std::env::temp_dir().join(format!("don-a-token-job-{job_id}"));
    let s = native.to_string_lossy().replace('\\', "/");
    let b = s.as_bytes();
    if b.len() > 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        format!("/{}{}", b[0].to_ascii_lowercase() as char, &s[2..])
    } else {
        s
    }
}

/// `sh` for local jobs. Prefers Git Bash on Windows (the `bash.exe` on
/// PATH is usually the WSL launcher, which is the wrong machine); plain
/// PATH lookup elsewhere.
fn resolve_sh() -> Result<PathBuf, String> {
    #[cfg(windows)]
    for fixed in [
        "C:\\Program Files\\Git\\bin\\bash.exe",
        "C:\\Program Files (x86)\\Git\\bin\\bash.exe",
    ] {
        let p = PathBuf::from(fixed);
        if p.is_file() {
            return Ok(p);
        }
    }
    #[cfg(windows)]
    let want: &[&str] = &["bash.exe", "sh.exe"];
    #[cfg(not(windows))]
    let want: &[&str] = &["sh"];
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            #[cfg(windows)]
            if dir.to_string_lossy().to_lowercase().contains("system32") {
                continue; // WSL launcher, not a local shell
            }
            for name in want {
                let p = dir.join(name);
                if p.is_file() {
                    return Ok(p);
                }
            }
        }
    }
    Err("local backend needs a POSIX shell (Git Bash on Windows)".to_string())
}

/// Windows local jobs: GNU-target cargo needs a mingw gcc for
/// linking, and Git Bash ships none. When PATH lacks gcc, prepend the
/// w64devkit dev install (`~/bin/w64devkit`) if present; otherwise log
/// that link steps will fail. Unix donors use the system gcc.
#[cfg(windows)]
fn ensure_linker_on_path(cmd: &mut tokio::process::Command) {
    let has_gcc = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("gcc.exe").is_file()))
        .unwrap_or(false);
    if has_gcc {
        return;
    }
    let bin = std::env::var_os("USERPROFILE").map(|h| {
        PathBuf::from(h)
            .join("bin")
            .join("w64devkit")
            .join("w64devkit")
            .join("bin")
    });
    match bin {
        Some(bin) if bin.join("gcc.exe").is_file() => {
            let mut paths = vec![bin];
            if let Some(p) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&p));
            }
            if let Ok(joined) = std::env::join_paths(paths) {
                cmd.env("PATH", joined);
                eprintln!("[runner] local: using ~/bin/w64devkit gcc for linking");
            }
        }
        _ => eprintln!("[runner] local: no gcc on PATH; GNU-target link steps will fail"),
    }
}

/// Env var carrying the native workdir path to a donor cleanup command.
pub const ENV_CLEANUP_WORKDIR: &str = "JOB_WORKDIR";
/// Hard cap for a donor cleanup command; expiry is logged, not an error.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(60);

/// Run a donor-defined cleanup command after a local job. Best effort:
/// timeouts and failures are logged and never fail the job.
pub async fn run_custom_cleanup(workdir: &std::path::Path, command: &str) {
    let sh = match resolve_sh() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[runner] cleanup: no shell: {e}");
            return;
        }
    };
    let mut cmd = tokio::process::Command::new(&sh);
    cmd.args(["-c", command]);
    cmd.env(ENV_CLEANUP_WORKDIR, workdir);
    cmd.current_dir(std::env::temp_dir());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[runner] cleanup: spawn: {e}");
            return;
        }
    };
    match timeout(CLEANUP_TIMEOUT, child.wait()).await {
        Ok(Ok(exit)) => eprintln!("[runner] cleanup: exit {}", exit.code().unwrap_or(-1)),
        Ok(Err(e)) => eprintln!("[runner] cleanup: wait: {e}"),
        Err(_) => {
            eprintln!("[runner] cleanup: timed out after 60s, killing");
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

/// Fail fast when the host lacks the local-execution toolchain.
async fn preflight_local() -> Result<(), String> {
    let _ = resolve_sh()?;
    let ok = tokio::process::Command::new("codex")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        return Err("local backend needs the codex CLI on PATH".to_string());
    }
    Ok(())
}

/// Kill a local job's whole process tree. Best effort; the caller
/// ignores errors (the pid may already be gone). Runs twice with a
/// pause: tree enumeration can miss grandchildren born mid-kill, and a
/// surviving agent keeps burning plan (plus holds the output pipes
/// open, which would wedge job completion).
async fn kill_tree(pid: u32) {
    for _ in 0..2 {
        #[cfg(windows)]
        {
            let _ = tokio::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output()
                .await;
        }
        #[cfg(not(windows))]
        {
            let _ = tokio::process::Command::new("pkill")
                .args(["-9", "-P", &pid.to_string()])
                .output()
                .await;
            let _ = tokio::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .output()
                .await;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Seconds to wait for captured output after the child is gone before
/// recording the outcome anyway. Orphaned grandchildren can hold the
/// pipes open indefinitely; the job must not wedge behind them.
const OUTPUT_DRAIN_SECS: u64 = 15;

/// Join the output-drain task, giving up after `OUTPUT_DRAIN_SECS`
/// (the task is dropped; orphans keep whatever they hold).
async fn drain_output(
    out_task: tokio::task::JoinHandle<(Vec<u8>, Vec<u8>)>,
    handle_id: &str,
) -> (Vec<u8>, Vec<u8>) {
    match timeout(Duration::from_secs(OUTPUT_DRAIN_SECS), out_task).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            eprintln!("[runner] {handle_id}: output drain failed: {e}");
            (Vec::new(), Vec::new())
        }
        Err(_) => {
            eprintln!(
                "[runner] {handle_id}: output pipes still open after\
                 {OUTPUT_DRAIN_SECS}s; an orphaned grandchild survived the kill"
            );
            (Vec::new(), Vec::new())
        }
    }
}

/// Redact secrets, map exit to status, record terminal state. Shared by
/// the E2B and local drivers.
async fn finish_output(
    app: &AppHandle,
    handle_id: &str,
    mut output: CommandOutput,
    secrets: &[String],
) {
    let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
    output.stdout = jobscript::redact(std::mem::take(&mut output.stdout), &refs);
    output.stderr = jobscript::redact(std::mem::take(&mut output.stderr), &refs);
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
    finish_job(app, handle_id, status, Some(output)).await;
}

/// Submit a local job: no sandbox; the command runs on-host.
/// `submit_job_inner` routes here for `backend == "local"`.
async fn submit_local(
    app: &AppHandle,
    jobs: &JobState,
    cfg: E2bConfig,
    job: SubmitJobArgs,
    handle: JobHandle,
) -> Result<JobHandleView, String> {
    preflight_local().await?;
    jobs.0.lock().await.insert(
        handle.id.clone(),
        JobRecord {
            sandbox_id: "local".to_string(),
            sandbox: None,
            cfg,
            status: JobStatus::Running,
            output: CommandOutput::default(),
            max_minutes: job.max_minutes,
            env_vars: job.env,
            local_pid: None,
        },
    );
    let app_clone = app.clone();
    let handle_id = handle.id.clone();
    let command = job.command.clone();
    tauri::async_runtime::spawn(async move {
        drive_local(app_clone, handle_id, command).await;
    });
    Ok(JobHandleView { id: handle.id })
}

/// Run the job command directly on the host, capturing output. On
/// timeout the whole tree is killed while the parent is still alive, so
/// no orphaned agent keeps burning plan after the cap.
async fn drive_local(app: AppHandle, handle_id: String, command: Vec<String>) {
    if command.len() < 2 {
        finish_job(
            &app,
            &handle_id,
            JobStatus::Failed {
                reason: "job command has no argv".to_string(),
            },
            None,
        )
        .await;
        return;
    }
    let (max_minutes, envs, secrets) = {
        let jobs = app.state::<JobState>();
        let guard = jobs.0.lock().await;
        let Some(rec) = guard.get(&handle_id) else {
            return;
        };
        (
            rec.max_minutes,
            rec.env_vars.clone(),
            rec.env_vars
                .iter()
                .map(|(_, v)| v.clone())
                .collect::<Vec<_>>(),
        )
    };

    let prog = if command.first().map(String::as_str) == Some("sh") {
        match resolve_sh() {
            Ok(p) => p,
            Err(e) => {
                finish_job(&app, &handle_id, JobStatus::Failed { reason: e }, None).await;
                return;
            }
        }
    } else {
        PathBuf::from(&command[0])
    };
    let mut cmd = tokio::process::Command::new(&prog);
    cmd.args(&command[1..]);
    cmd.envs(envs);
    #[cfg(windows)]
    ensure_linker_on_path(&mut cmd);
    cmd.current_dir(std::env::temp_dir());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            finish_job(
                &app,
                &handle_id,
                JobStatus::Failed {
                    reason: format!("spawn {}: {e}", prog.display()),
                },
                None,
            )
            .await;
            return;
        }
    };
    let pid = child.id();
    if let Some(pid) = pid {
        let jobs = app.state::<JobState>();
        let mut guard = jobs.0.lock().await;
        if let Some(rec) = guard.get_mut(&handle_id) {
            rec.local_pid = Some(pid);
        }
    }

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let out_task = tokio::spawn(async move {
        let mut so = Vec::new();
        let mut se = Vec::new();
        if let Some(mut p) = stdout_pipe {
            let _ = p.read_to_end(&mut so).await;
        }
        if let Some(mut p) = stderr_pipe {
            let _ = p.read_to_end(&mut se).await;
        }
        (so, se)
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(u64::from(max_minutes) * 60 + 60);
    match timeout_at(deadline, child.wait()).await {
        Ok(Ok(exit)) => {
            let (so, se) = drain_output(out_task, &handle_id).await;
            finish_output(
                &app,
                &handle_id,
                CommandOutput {
                    stdout: so,
                    stderr: se,
                    exit_code: exit.code(),
                    error: None,
                },
                &secrets,
            )
            .await;
        }
        Ok(Err(e)) => {
            finish_job(
                &app,
                &handle_id,
                JobStatus::Failed {
                    reason: format!("wait: {e}"),
                },
                None,
            )
            .await;
        }
        Err(_) => {
            if let Some(pid) = pid {
                kill_tree(pid).await;
            }
            if timeout(Duration::from_secs(15), child.wait()).await.is_err() {
                eprintln!(
                    "[runner] {handle_id}: child {pid:?} survived the timeout kill and keeps running (plan may keep burning)"
                );
            }
            let (so, se) = drain_output(out_task, &handle_id).await;
            let mut output = CommandOutput {
                stdout: so,
                stderr: se,
                exit_code: None,
                error: Some("command timed out".to_string()),
            };
            let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
            output.stdout = jobscript::redact(std::mem::take(&mut output.stdout), &refs);
            output.stderr = jobscript::redact(std::mem::take(&mut output.stderr), &refs);
            finish_job(&app, &handle_id, JobStatus::TimedOut, Some(output)).await;
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
    let (cfg, sandbox_id, local_pid, is_local) = {
        let mut guard = jobs.0.lock().await;
        let rec = guard
            .get_mut(&handle_id)
            .ok_or_else(|| "unknown job handle".to_string())?;
        rec.status = JobStatus::Failed {
            reason: "cancelled".to_string(),
        };
        (
            rec.cfg.clone(),
            rec.sandbox_id.clone(),
            rec.local_pid,
            rec.sandbox.is_none(),
        )
    };
    if is_local {
        if let Some(pid) = local_pid {
            kill_tree(pid).await;
        }
    } else {
        let runner = E2bRunner::new(cfg)?;
        let _ = runner.kill_sandbox(&sandbox_id).await;
    }
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

    #[test]
    fn local_workdir_is_posix_shaped() {
        let w = local_workdir_posix("12345");
        assert!(!w.contains('\\'));
        assert!(w.contains("don-a-token-job-12345"));
        #[cfg(windows)]
        {
            assert!(w.starts_with('/'));
            assert!(w.chars().nth(1).unwrap().is_ascii_alphabetic());
        }
    }

    #[test]
    fn resolve_sh_finds_a_shell() {
        let sh = resolve_sh().unwrap();
        assert!(sh.is_file());
    }

    #[tokio::test]
    async fn custom_cleanup_receives_workdir_and_never_fails() {
        let dir = std::env::temp_dir()
            .join(format!("dat-cleanup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        run_custom_cleanup(&dir, "touch \"$JOB_WORKDIR/proof\"").await;
        assert!(dir.join("proof").is_file());
        // Failing commands are logged, not propagated (no panic = pass).
        run_custom_cleanup(&dir, "exit 3").await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Live smoke against a real backend (Embed or Cloud). Ignored by
    /// default; run explicitly with the backend's key:
    /// `E2B_API_KEY=e2b_... E2B_API_URL=http://127.0.0.1:3000 cargo test
    /// -- --ignored live_`. Creates one sandbox, runs `echo`, kills it.
    #[tokio::test]
    #[ignore]
    async fn live_embed_smoke() {
        let base =
            std::env::var("E2B_API_URL").unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());
        let key = std::env::var("E2B_API_KEY")
            .expect("live test needs E2B_API_KEY (Embed: `docker compose logs ready`)");
        let runner = E2bRunner::new(E2bConfig {
            api_base: base,
            api_key: Some(key),
            sandbox_base: None,
        })
        .unwrap();
        let sandbox = runner
            .create_sandbox("base", 300, serde_json::json!({}), serde_json::json!({}))
            .await
            .expect("create sandbox");
        // Always release the sandbox (small nodes fit exactly one).
        let run = runner
            .run_command(
                &sandbox,
                "echo",
                &["hello-live".to_string()],
                &serde_json::json!({}),
                None,
                Duration::from_secs(60),
            )
            .await;
        let killed = runner.kill_sandbox(&sandbox.sandbox_id).await;
        let out = run.expect("run echo");
        assert!(
            out.succeeded(),
            "exit={:?} error={:?} stdout={:?} stderr={:?}",
            out.exit_code,
            out.error,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("hello-live"),
            "unexpected stdout: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(killed.unwrap(), "kill sandbox");
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
