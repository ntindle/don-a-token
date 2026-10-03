//! Job isolation backends. E2B Embed is the default target.
//!
//! E2B Embed ships the whole E2B stack (same SDK/API as E2B Cloud, real
//! Firecracker sandboxes) on one Linux host with KVM — no E2B account or
//! license key needed. Because Embed needs Linux+KVM, it can only run
//! on-machine on Linux; Mac/Windows installs point at E2B Cloud or a remote
//! Embed node, with a local-container fallback where nothing better exists.
//!
//! See `docs/RUNNER.md` for the platform matrix.

use serde::{Deserialize, Serialize};

/// Where sandboxed Codex jobs execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Backend {
    /// Embed stack on this machine (Linux+KVM only).
    EmbedLocal { endpoint: String },
    /// Embed stack on a user-provided node (any desktop OS).
    EmbedRemote { endpoint: String },
    /// Hosted E2B Cloud (needs an API key).
    E2bCloud,
    /// Local Docker/Podman fallback. Weakest containment; last resort.
    DockerLocal,
    /// Direct on-host execution. NO isolation; testing only.
    Local,
}

impl Backend {
    /// Embed API base confirmed against `embed/docs/REFERENCE.md` (:3000)
    /// and the JS SDK debug default (`http://localhost:3000`); sandbox
    /// traffic goes through client-proxy on :3002 (see `e2b` module).
    pub fn default_for_platform() -> Self {
        if cfg!(target_os = "linux") {
            Backend::EmbedLocal {
                endpoint: "http://127.0.0.1:3000".to_string(),
            }
        } else {
            Backend::E2bCloud
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Backend::EmbedLocal { .. } => "embed-local",
            Backend::EmbedRemote { .. } => "embed-remote",
            Backend::E2bCloud => "e2b-cloud",
            Backend::DockerLocal => "docker-local",
            Backend::Local => "local",
        }
    }
}

/// One unit of donated work. Secrets must never appear in `env`; the sandbox
/// receives only the job inputs plus a short-lived contribution token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRequest {
    pub project_id: String,
    pub job_id: String,
    pub template: String,
    pub prompt_pack: String,
    pub env: Vec<(String, String)>,
    pub max_minutes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobHandle {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed { reason: String },
    TimedOut,
}

/// Runner interface. Implementations speak the E2B-compatible API
/// (Embed local/remote, Cloud) or local containers. Async + HTTP arrive
/// with the runner spike; the trait stays object-safe and narrow.
pub trait Runner {
    fn backend(&self) -> &Backend;
    fn submit(&self, job: &JobRequest) -> Result<JobHandle, String>;
    fn status(&self, handle: &JobHandle) -> JobStatus;
    fn cancel(&self, handle: &JobHandle) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    struct FakeRunner {
        backend: Backend,
        jobs: Arc<Mutex<HashMap<String, JobStatus>>>,
    }

    impl Runner for FakeRunner {
        fn backend(&self) -> &Backend {
            &self.backend
        }
        fn submit(&self, job: &JobRequest) -> Result<JobHandle, String> {
            let id = format!("job-{}", job.job_id);
            self.jobs
                .lock()
                .unwrap()
                .insert(id.clone(), JobStatus::Queued);
            Ok(JobHandle { id })
        }
        fn status(&self, handle: &JobHandle) -> JobStatus {
            self.jobs
                .lock()
                .unwrap()
                .get(&handle.id)
                .cloned()
                .unwrap_or(JobStatus::Failed {
                    reason: "unknown handle".to_string(),
                })
        }
        fn cancel(&self, handle: &JobHandle) -> Result<(), String> {
            self.jobs.lock().unwrap().remove(&handle.id);
            Ok(())
        }
    }

    fn sample_job() -> JobRequest {
        JobRequest {
            project_id: "phase".to_string(),
            job_id: "abc".to_string(),
            template: "don-a-token-rust".to_string(),
            prompt_pack: "registry:phase/stable".to_string(),
            env: vec![],
            max_minutes: 30,
        }
    }

    #[test]
    fn platform_default_matches_os() {
        let backend = Backend::default_for_platform();
        if cfg!(target_os = "linux") {
            assert_eq!(backend.name(), "embed-local");
        } else {
            assert_eq!(backend.name(), "e2b-cloud");
        }
    }

    #[test]
    fn fake_runner_lifecycle() {
        let runner = FakeRunner {
            backend: Backend::DockerLocal,
            jobs: Arc::new(Mutex::new(HashMap::new())),
        };
        let handle = runner.submit(&sample_job()).unwrap();
        assert_eq!(runner.status(&handle), JobStatus::Queued);
        runner.cancel(&handle).unwrap();
        assert!(matches!(runner.status(&handle), JobStatus::Failed { .. }));
    }
}
