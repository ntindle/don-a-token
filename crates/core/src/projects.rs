//! Donation-opportunity registry: the manually approved project list.
//!
//! Source of truth is `projects/registry.json` in this repo, shipped as a
//! release artifact. The app refreshes from the latest release. Entries are
//! curated by maintainers; see `projects/README.md`.

use serde::{Deserialize, Serialize};

use crate::CoreError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProjectStatus {
    Active,
    Paused,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contribution {
    pub method: String,
    pub base_branch: String,
    pub attribution_footer: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobSpec {
    pub kind: String,
    pub template: String,
    pub max_tokens_per_job: u64,
    pub max_minutes_per_job: u32,
    pub contribution: Contribution,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub description: String,
    pub homepage: String,
    pub status: ProjectStatus,
    pub job: JobSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Registry {
    pub version: u32,
    pub updated_at: String,
    pub projects: Vec<Project>,
}

impl Registry {
    pub fn load_from_str(text: &str) -> Result<Self, CoreError> {
        let registry: Registry = serde_json::from_str(text)?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        if self.version == 0 {
            return Err(CoreError::InvalidRegistry("version must be >= 1".into()));
        }
        let mut seen = std::collections::HashSet::new();
        for p in &self.projects {
            if p.id.trim().is_empty() {
                return Err(CoreError::InvalidRegistry("project id must be set".into()));
            }
            if !seen.insert(p.id.clone()) {
                return Err(CoreError::InvalidRegistry(format!(
                    "duplicate project id: {}",
                    p.id
                )));
            }
            if !(p.repo.starts_with("https://") && p.repo.contains("github.com")) {
                return Err(CoreError::InvalidRegistry(format!(
                    "project {} repo must be an https github URL",
                    p.id
                )));
            }
            if p.job.max_tokens_per_job == 0 || p.job.max_minutes_per_job == 0 {
                return Err(CoreError::InvalidRegistry(format!(
                    "project {} needs positive job caps",
                    p.id
                )));
            }
        }
        Ok(())
    }

    pub fn active_projects(&self) -> Vec<&Project> {
        self.projects
            .iter()
            .filter(|p| p.status == ProjectStatus::Active)
            .collect()
    }

    pub fn find(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "version": 1,
        "updated_at": "2026-10-01",
        "projects": [
            {
                "id": "phase",
                "name": "Phase",
                "repo": "https://github.com/phase-rs/phase",
                "description": "x",
                "homepage": "https://github.com/phase-rs/phase",
                "status": "active",
                "job": {
                    "kind": "codex-task",
                    "template": "don-a-token-rust",
                    "max_tokens_per_job": 50000,
                    "max_minutes_per_job": 30,
                    "contribution": {
                        "method": "pull-request",
                        "base_branch": "main",
                        "attribution_footer": true
                    }
                }
            }
        ]
    }"#;

    #[test]
    fn fixture_loads_and_selects_active() {
        let reg = Registry::load_from_str(FIXTURE).unwrap();
        assert_eq!(reg.version, 1);
        assert_eq!(reg.active_projects().len(), 1);
        assert_eq!(reg.find("phase").unwrap().name, "Phase");
    }

    #[test]
    fn duplicate_ids_rejected() {
        let mut reg = Registry::load_from_str(FIXTURE).unwrap();
        reg.projects.push(reg.projects[0].clone());
        assert!(reg.validate().is_err());
    }

    #[test]
    fn non_github_repo_rejected() {
        let mut reg = Registry::load_from_str(FIXTURE).unwrap();
        reg.projects[0].repo = "http://example.com/x".to_string();
        assert!(reg.validate().is_err());
    }

    #[test]
    fn repo_registry_file_is_valid() {
        let path = format!(
            "{}/../../projects/registry.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).unwrap();
        let reg = Registry::load_from_str(&text).unwrap();
        assert!(!reg.active_projects().is_empty());
    }
}
