//! Background donation scheduler: rules verdict → project pick → submit
//! → poll → cooldown/backoff → contribution log.
//!
//! The frontend owns settings UI and pushes a synced copy here on every
//! change (`scheduler_sync`), persisted so the loop works headless after
//! restart. One job at a time; three consecutive failures halt with an
//! event the frontend surfaces.

use base64::Engine as _;
use chrono::{DateTime, Utc};
use don_a_token_core::projects::pick_next;
use don_a_token_core::rules::DonationRules;
use don_a_token_core::{codex, credentials, jobscript};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;

use crate::publish;
use crate::runner::{self, JobState, RunnerConfigArgs, SubmitJobArgs};

const TICK_SECS: u64 = 60;
const FAILURE_HALT_COUNT: u32 = 3;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedProject {
    pub id: String,
    pub template: String,
    pub repo: String,
    pub base_branch: String,
    pub prompt: String,
    pub checks: Vec<String>,
    pub prompt_pack: String,
    pub max_minutes: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerConfig {
    pub rules: DonationRules,
    pub projects: Vec<SchedProject>,
    pub runner: RunnerConfigArgs,
    pub donor: Option<String>,
    /// Contribution credential (device-flow token or donor PAT).
    /// Host-side only; see docs/RUNNER.md.
    pub github_token: Option<String>,
}

/// Host-side publish context for the active job. The sandbox only
/// exports a patch; the branch/PR/title below are applied on the host.
#[derive(Debug, Clone)]
struct ActiveJobMeta {
    project_id: String,
    job_id: String,
    repo: String,
    base_branch: String,
    branch: String,
    pr_title: String,
    pr_body: String,
    github_token: Option<String>,
}

struct SchedulerData {
    config: Option<SchedulerConfig>,
    active_handle: Option<String>,
    active_meta: Option<ActiveJobMeta>,
    last_project_id: Option<String>,
    last_finished: Option<DateTime<Utc>>,
    consecutive_failures: u32,
    last_verdict: String,
    halted_emitted: bool,
}

impl Default for SchedulerData {
    fn default() -> Self {
        Self {
            config: None,
            active_handle: None,
            active_meta: None,
            last_project_id: None,
            last_finished: None,
            consecutive_failures: 0,
            last_verdict: "unconfigured".to_string(),
            halted_emitted: false,
        }
    }
}

pub struct SchedulerState(pub Mutex<SchedulerData>);

impl Default for SchedulerState {
    fn default() -> Self {
        Self(Mutex::new(SchedulerData::default()))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TickDecision {
    Submit,
    Wait(&'static str),
}

/// Pure per-tick decision. The caller handles Submit (project pick +
/// submit) and records Wait reasons for status display.
pub fn decide_tick(
    verdict_run: bool,
    verdict_reason: &'static str,
    cooldown_ok: bool,
    failures: u32,
    active: bool,
) -> TickDecision {
    if active {
        return TickDecision::Wait("job-running");
    }
    if failures >= FAILURE_HALT_COUNT {
        return TickDecision::Wait("failure-backoff");
    }
    if !verdict_run {
        return TickDecision::Wait(verdict_reason);
    }
    if !cooldown_ok {
        return TickDecision::Wait("cooldown");
    }
    TickDecision::Submit
}

/// One JSON line for the local contribution log.
pub fn log_line(
    ts: &str,
    project_id: &str,
    handle_id: &str,
    status: &str,
    exit_code: Option<i32>,
) -> String {
    log_line_detail(ts, project_id, handle_id, status, exit_code, None)
}

/// Same, with an optional detail (PR URL, pending-patch path, error).
pub fn log_line_detail(
    ts: &str,
    project_id: &str,
    handle_id: &str,
    status: &str,
    exit_code: Option<i32>,
    detail: Option<&str>,
) -> String {
    serde_json::json!({
        "ts": ts,
        "project_id": project_id,
        "handle_id": handle_id,
        "status": status,
        "exit_code": exit_code,
        "detail": detail,
    })
    .to_string()
}

fn app_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Restore the last synced config so the loop works after restart.
pub fn restore(app: &AppHandle) {
    let path = match app_dir(app) {
        Ok(d) => d.join("scheduler.json"),
        Err(_) => return,
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    if let Ok(config) = serde_json::from_slice::<SchedulerConfig>(&bytes) {
        let state = app.state::<SchedulerState>();
        if let Ok(mut data) = state.0.try_lock() {
            data.config = Some(config);
            data.last_verdict = "restored".to_string();
        };
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerStatus {
    pub configured: bool,
    pub active_job: Option<String>,
    pub last_verdict: String,
    pub consecutive_failures: u32,
}

/// Push settings from the frontend (called on every change + at startup).
#[tauri::command]
pub async fn scheduler_sync(
    app: AppHandle,
    state: State<'_, SchedulerState>,
    config: SchedulerConfig,
) -> Result<(), String> {
    let dir = app_dir(&app)?;
    let bytes =
        serde_json::to_vec_pretty(&config).map_err(|e| format!("encode config: {e}"))?;
    std::fs::write(dir.join("scheduler.json"), bytes).map_err(|e| format!("save config: {e}"))?;
    let mut data = state.0.lock().await;
    data.config = Some(config);
    // A settings change is an explicit retry: clear any backoff.
    data.consecutive_failures = 0;
    data.halted_emitted = false;
    if data.last_verdict == "unconfigured" {
        data.last_verdict = "synced".to_string();
    }
    Ok(())
}

#[tauri::command]
pub async fn scheduler_status(state: State<'_, SchedulerState>) -> Result<SchedulerStatus, String> {
    let data = state.0.lock().await;
    Ok(SchedulerStatus {
        configured: data.config.is_some(),
        active_job: data.active_handle.clone(),
        last_verdict: data.last_verdict.clone(),
        consecutive_failures: data.consecutive_failures,
    })
}

pub fn spawn_loop(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
        loop {
            interval.tick().await;
            tick(&app).await;
        }
    });
}

async fn tick(app: &AppHandle) {
    let jobs = app.state::<JobState>();
    let sched = app.state::<SchedulerState>();
    let now = Utc::now();

    // Poll the active job, if any.
    let active_handle = sched.0.lock().await.active_handle.clone();
    if let Some(handle_id) = active_handle {
        let terminal = {
            let guard = jobs.0.lock().await;
            guard.get(&handle_id).map(|rec| {
                let terminal = !matches!(
                    rec.status,
                    don_a_token_core::runner::JobStatus::Running
                        | don_a_token_core::runner::JobStatus::Queued
                );
                (terminal, format!("{:?}", rec.status), rec.output.exit_code)
            })
        };
        match terminal {
            None => {
                // Handle vanished (e.g. jobs map reset); drop it.
                sched.0.lock().await.active_handle = None;
            }
            Some((false, _, _)) => {
                sched.0.lock().await.last_verdict = "job-running".to_string();
                return;
            }
            Some((true, status, exit_code)) => {
                finish_active(app, &handle_id, &status, exit_code).await;
                return;
            }
        }
    }

    // Nothing running: decide whether to submit.
    let (config, last_finished, failures) = {
        let data = sched.0.lock().await;
        match &data.config {
            None => {
                drop(data);
                sched.0.lock().await.last_verdict = "unconfigured".to_string();
                return;
            }
            Some(cfg) => (cfg.clone(), data.last_finished, data.consecutive_failures),
        }
    };
    if config.projects.is_empty() {
        sched.0.lock().await.last_verdict = "no-projects".to_string();
        return;
    }
    let verdict = config.rules.should_run(now);
    let cooldown_ok = last_finished
        .map(|t| now - t >= chrono::Duration::minutes(config.rules.cooldown_minutes_between_jobs as i64))
        .unwrap_or(true);
    match decide_tick(verdict.run, verdict.reason, cooldown_ok, failures, false) {
        TickDecision::Wait(reason) => {
            let mut data = sched.0.lock().await;
            data.last_verdict = reason.to_string();
            if reason == "failure-backoff" && !data.halted_emitted {
                data.halted_emitted = true;
                let _ = app.emit("scheduler:halted", reason);
            }
        }
        TickDecision::Submit => {
            submit_next(app, &config).await;
        }
    }
}

async fn finish_active(app: &AppHandle, handle_id: &str, status: &str, exit_code: Option<i32>) {
    let (project_id, failed, stdout) = {
        let jobs = app.state::<JobState>();
        let guard = jobs.0.lock().await;
        match guard.get(handle_id) {
            Some(rec) => {
                let failed = !matches!(rec.status, don_a_token_core::runner::JobStatus::Succeeded);
                // project id is the handle prefix ("{project}-{job}").
                let project = handle_id.split('-').next().unwrap_or("?").to_string();
                (project, failed, rec.output.stdout.clone())
            }
            None => ("?".to_string(), true, Vec::new()),
        }
    };
    let meta = app.state::<SchedulerState>().0.lock().await.active_meta.take();
    let (final_status, detail, failed) = match (failed, meta) {
        (true, _) => (status.to_string(), None, true),
        (false, None) => ("succeeded".to_string(), None, false),
        (false, Some(meta)) => publish_succeeded(app, handle_id, &stdout, &meta).await,
    };
    append_log(app, handle_id, &project_id, &final_status, exit_code, detail.as_deref());
    let sched = app.state::<SchedulerState>();
    let mut data = sched.0.lock().await;
    data.active_handle = None;
    data.last_finished = Some(Utc::now());
    if failed {
        data.consecutive_failures += 1;
        data.last_verdict = format!("job-failed: {final_status}");
    } else {
        data.consecutive_failures = 0;
        data.halted_emitted = false;
        data.last_verdict = format!("job-{final_status}");
    }
}

/// Host-side publish for a succeeded sandbox run. Returns
/// (log status, optional detail, counts-as-failure).
async fn publish_succeeded(
    app: &AppHandle,
    handle_id: &str,
    stdout: &[u8],
    meta: &ActiveJobMeta,
) -> (String, Option<String>, bool) {
    match publish::extract_patch(stdout) {
        publish::PatchOutcome::NoChanges => ("no-changes".to_string(), None, false),
        publish::PatchOutcome::TooLarge => ("patch-too-large".to_string(), None, false),
        publish::PatchOutcome::Missing => ("no-patch".to_string(), None, true),
        publish::PatchOutcome::Patch { patch, result_md } => {
            let Some(token) = meta.github_token.clone() else {
                let path = save_pending_patch(app, handle_id, meta, &patch);
                return ("needs-publish".to_string(), path, false);
            };
            if !publish::tools_available() {
                let path = save_pending_patch(app, handle_id, meta, &patch);
                return ("needs-publish".to_string(), path.or(Some("git/gh not on PATH".to_string())), false);
            }
            let owned = (
                meta.repo.clone(),
                meta.base_branch.clone(),
                meta.branch.clone(),
                meta.pr_title.clone(),
                meta.pr_body.clone(),
                result_md.clone(),
                meta.job_id.clone(),
                patch.clone(),
            );
            let res = tokio::task::spawn_blocking(move || {
                let spec = publish::PublishSpec {
                    repo_url: &owned.0,
                    base_branch: &owned.1,
                    branch: &owned.2,
                    title: &owned.3,
                    body: &owned.4,
                    result_md: owned.5.as_deref(),
                    github_token: &token,
                    job_tag: &owned.6,
                };
                publish::publish_patch(&spec, &std::env::temp_dir(), &owned.7)
            })
            .await;
            match res {
                Ok(Ok(url)) => ("published".to_string(), Some(url), false),
                Ok(Err(e)) => {
                    let path = save_pending_patch(app, handle_id, meta, &patch);
                    let detail = path.unwrap_or_else(|| truncate(&e, 200));
                    ("publish-failed".to_string(), Some(detail), true)
                }
                Err(e) => ("publish-failed".to_string(), Some(format!("join: {e}")), true),
            }
        }
    }
}

/// Stash an unpublished patch + its publish context under the app dir
/// for later manual publishing. Returns the patch path when written.
fn save_pending_patch(
    app: &AppHandle,
    handle_id: &str,
    meta: &ActiveJobMeta,
    patch: &str,
) -> Option<String> {
    let dir = app_dir(app).ok()?.join("pending-patches");
    std::fs::create_dir_all(&dir).ok()?;
    let safe: String = handle_id.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let patch_path = dir.join(format!("{safe}.patch"));
    std::fs::write(&patch_path, patch).ok()?;
    let ctx = serde_json::json!({
        "project_id": meta.project_id,
        "job_id": meta.job_id,
        "repo": meta.repo,
        "base_branch": meta.base_branch,
        "branch": meta.branch,
        "pr_title": meta.pr_title,
        "pr_body": meta.pr_body,
    });
    let _ = std::fs::write(dir.join(format!("{safe}.json")), ctx.to_string());
    Some(patch_path.to_string_lossy().to_string())
}

fn append_log(
    app: &AppHandle,
    handle_id: &str,
    project_id: &str,
    status: &str,
    exit_code: Option<i32>,
    detail: Option<&str>,
) {
    let Ok(dir) = app_dir(app) else {
        return;
    };
    let line = log_line_detail(&Utc::now().to_rfc3339(), project_id, handle_id, status, exit_code, detail);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("donations.jsonl")) {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

async fn submit_next(app: &AppHandle, config: &SchedulerConfig) {
    let ids: Vec<&str> = config.projects.iter().map(|p| p.id.as_str()).collect();
    let sched = app.state::<SchedulerState>();
    let last = sched.0.lock().await.last_project_id.clone();
    let Some(next_id) = pick_next(&ids, last.as_deref()) else {
        return;
    };
    let Some(spec) = config.projects.iter().find(|p| p.id == next_id) else {
        return;
    };
    let (client_id, access_token) = match plan_account_token(app).await {
        Ok(t) => t,
        Err(e) => {
            sched.0.lock().await.last_verdict = e;
            return;
        }
    };
    let job_id = Utc::now().timestamp().to_string();
    let branch = format!("don-a-token/{}-{job_id}", spec.id);
    let footer = jobscript::attribution_footer(
        config.donor.as_deref(),
        &spec.id,
        &job_id,
        &spec.template,
        Some(&client_id),
    );
    let pr_title = format!("don-a-token({}): automated contribution {job_id}", spec.id);
    let pr_body = jobscript::pr_body(&spec.id, &footer);
    // Home-relative: sandboxes run as non-root `user`, so /work is not writable.
    let exec = jobscript::standard_exec_args("/home/user/job");
    let bspec = jobscript::BootstrapSpec {
        repo_url: &spec.repo,
        base_branch: &spec.base_branch,
        workdir: "/home/user/job",
        checks: &spec.checks,
        codex_args: &exec,
    };
    let script = jobscript::build_bootstrap(&bspec);
    let b64 = base64::engine::general_purpose::STANDARD;
    // Only the plan token + prompt enter the sandbox. GitHub credentials
    // stay on the host; the PR is published from the exported patch.
    let env = vec![
        (
            codex::ENV_ACCESS_TOKEN.to_string(),
            access_token,
        ),
        (
            jobscript::ENV_JOB_PROMPT_B64.to_string(),
            b64.encode(spec.prompt.as_bytes()),
        ),
    ];
    let max_minutes = spec.max_minutes.min(config.rules.max_minutes_per_job);
    let args = SubmitJobArgs {
        project_id: spec.id.clone(),
        job_id: job_id.clone(),
        template: spec.template.clone(),
        prompt_pack: spec.prompt_pack.clone(),
        command: vec!["sh".to_string(), "-c".to_string(), script],
        env,
        max_minutes,
    };
    let jobs = app.state::<JobState>();
    match runner::submit_job_inner(app, &jobs, config.runner.clone(), args).await {
        Ok(handle) => {
            let mut data = sched.0.lock().await;
            data.active_handle = Some(handle.id);
            data.active_meta = Some(ActiveJobMeta {
                project_id: spec.id.clone(),
                job_id,
                repo: spec.repo.clone(),
                base_branch: spec.base_branch.clone(),
                branch,
                pr_title,
                pr_body,
                github_token: config.github_token.clone(),
            });
            data.last_project_id = Some(spec.id.clone());
            data.last_verdict = "submitted".to_string();
        }
        Err(e) => {
            let mut data = sched.0.lock().await;
            data.consecutive_failures += 1;
            data.last_verdict = format!("submit-failed: {}", truncate(&e, 120));
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// First saved account with plan usage, refreshed if due.
/// Returns (client_id, access_token).
async fn plan_account_token(app: &AppHandle) -> Result<(String, String), String> {
    let dir = app_dir(app)?;
    let ids = credentials::list_client_ids(&dir).map_err(|e| format!("list accounts: {e}"))?;
    for id in ids {
        let rec = match credentials::load_record(&dir, &id) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !rec.has_plan_usage() {
            continue;
        }
        if rec.should_refresh(Utc::now()) {
            if crate::auth::refresh_account(app.clone(), id.clone()).await.is_err() {
                continue;
            }
            let Ok(fresh) = credentials::load_record(&dir, &id) else {
                continue;
            };
            return Ok((id, fresh.access_token));
        }
        return Ok((id, rec.access_token));
    }
    Err("no-account".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions_cover_all_gates() {
        use TickDecision::*;
        assert_eq!(decide_tick(true, "ok", true, 0, true), Wait("job-running"));
        assert_eq!(decide_tick(true, "ok", true, 3, false), Wait("failure-backoff"));
        assert_eq!(decide_tick(true, "ok", true, 2, false), Submit);
        assert_eq!(decide_tick(false, "paused", true, 0, false), Wait("paused"));
        assert_eq!(decide_tick(true, "ok", false, 0, false), Wait("cooldown"));
        assert_eq!(decide_tick(true, "ok", true, 0, false), Submit);
    }

    #[test]
    fn log_line_is_json() {
        let line = log_line("2026-10-02T00:00:00Z", "phase", "phase-1", "Succeeded", Some(0));
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["project_id"], "phase");
        assert_eq!(v["exit_code"], 0);
    }

    #[test]
    fn config_roundtrips_camel_case() {
        let json = serde_json::json!({
            "rules": {
                "enabled": true,
                "maxMinutesPerJob": 30,
                "cooldownMinutesBetweenJobs": 15,
                "quietHours": null,
                "pausedUntil": null,
                "skipThisWeek": false,
                "skipWeekOf": null
            },
            "projects": [{
                "id": "phase",
                "template": "don-a-token-rust",
                "repo": "https://github.com/phase-rs/phase",
                "baseBranch": "main",
                "prompt": "do good work",
                "checks": ["cargo test"],
                "promptPack": "phase",
                "maxMinutes": 30
            }],
            "runner": { "api_base": "http://127.0.0.1:3000", "api_key": null, "sandbox_base": null },
            "donor": "octocat",
            "githubToken": null
        });
        let cfg: SchedulerConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.projects[0].id, "phase");
        assert_eq!(cfg.projects[0].prompt, "do good work");
        assert_eq!(cfg.donor.as_deref(), Some("octocat"));
    }
}
