//! Host-side publishing: turn an exported sandbox patch into a PR.
//!
//! The sandbox never holds GitHub credentials. After a job succeeds, the
//! scheduler extracts the patch from job stdout and this module clones
//! the target repo to a temp dir, applies the patch, pushes a branch,
//! and opens a PR with `gh`. Secrets travel as process env only — never
//! in argv, never in captured error text.

use don_a_token_core::jobscript;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct PublishSpec<'a> {
    pub repo_url: &'a str,
    pub base_branch: &'a str,
    pub branch: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub result_md: Option<&'a str>,
    pub github_token: &'a str,
    pub job_tag: &'a str,
    /// Push target owner when it differs from upstream (donor fork).
    pub fork_owner: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PatchOutcome {
    NoChanges,
    TooLarge,
    Missing,
    Patch { patch: String, result_md: Option<String> },
}

fn section(text: &str, begin: &str, end: &str) -> Option<String> {
    let start = text.find(begin)? + begin.len();
    let rest = &text[start..];
    let len = rest.find(end)?;
    Some(rest[..len].trim_matches('\n').to_string())
}

/// Extract the exported patch (+ optional agent summary) from sandbox
/// stdout. Framed markers win over sentinels: agent chatter may mention
/// `NO_CHANGES=1` without meaning it, but only the script prints markers.
pub fn extract_patch(stdout: &[u8]) -> PatchOutcome {
    let text = String::from_utf8_lossy(stdout);
    if let Some(patch) = section(&text, jobscript::MARK_PATCH_BEGIN, jobscript::MARK_PATCH_END) {
        let result_md = section(&text, jobscript::MARK_RESULT_BEGIN, jobscript::MARK_RESULT_END);
        return PatchOutcome::Patch { patch, result_md };
    }
    if text.contains("PATCH_TOO_LARGE=1") {
        return PatchOutcome::TooLarge;
    }
    if text.contains("NO_CHANGES=1") {
        return PatchOutcome::NoChanges;
    }
    PatchOutcome::Missing
}

pub fn git_clone_args(repo: &str, base: &str, dir: &str) -> Vec<String> {
    ["clone", "--depth", "1", "--branch", base, repo, dir]
        .map(str::to_string)
        .to_vec()
}

pub fn git_push_args(remote: &str, branch: &str) -> Vec<String> {
    ["push", remote, branch].map(str::to_string).to_vec()
}

/// Fork owner for publishing: the donor, unless they own the upstream
/// repo (then push straight to origin). Owner compare is case-insensitive.
pub fn fork_owner_for<'a>(donor: Option<&'a str>, repo_url: &str) -> Option<&'a str> {
    let donor = donor?;
    let slug = jobscript::repo_slug(repo_url)?;
    let owner = slug.split('/').next()?;
    if donor.eq_ignore_ascii_case(owner) {
        None
    } else {
        Some(donor)
    }
}

/// Make sure `<fork_owner>/<repo>` exists, creating the fork when it
/// doesn't. Re-checks after a failed fork so an already-existing fork
/// (e.g. created by hand mid-job) still proceeds.
fn ensure_fork(
    token: &str,
    upstream_slug: &str,
    fork_owner: &str,
    repo: &str,
    workdir: &Path,
) -> Result<(), String> {
    let view = [
        "repo".to_string(),
        "view".to_string(),
        format!("{fork_owner}/{repo}"),
    ];
    if run("gh", &view, workdir, &gh_env(token)).is_ok() {
        return Ok(());
    }
    let forked = run(
        "gh",
        &[
            "repo".to_string(),
            "fork".to_string(),
            upstream_slug.to_string(),
            "--clone=false".to_string(),
        ],
        workdir,
        &gh_env(token),
    );
    if forked.is_err() && run("gh", &view, workdir, &gh_env(token)).is_err() {
        return Err(forked.unwrap_err());
    }
    Ok(())
}

pub fn gh_pr_create_args(slug: &str, head: &str, base: &str, title: &str, body_file: &str) -> Vec<String> {
    [
        "pr", "create", "--repo", slug, "--head", head, "--base", base, "--title", title,
        "--body-file", body_file,
    ]
    .map(str::to_string)
    .to_vec()
}

/// Auth for `git push` without argv leaks: git reads `GIT_CONFIG_*` env
/// into effective config, so the token never appears in process args.
pub fn git_push_env(token: &str) -> Vec<(String, String)> {
    vec![
        ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
        ("GIT_CONFIG_KEY_0".to_string(), "http.extraHeader".to_string()),
        (
            "GIT_CONFIG_VALUE_0".to_string(),
            format!("AUTHORIZATION: bearer {token}"),
        ),
    ]
}

pub fn gh_env(token: &str) -> Vec<(String, String)> {
    vec![("GH_TOKEN".to_string(), token.to_string())]
}

/// `git` + `gh` must be on PATH for host-side publishing.
pub fn tools_available() -> bool {
    for bin in ["git", "gh"] {
        let ok = Command::new(bin)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            return false;
        }
    }
    true
}

fn run(bin: &str, args: &[String], dir: &Path, env: &[(String, String)]) -> Result<String, String> {
    let out = Command::new(bin)
        .args(args)
        .current_dir(dir)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .output()
        .map_err(|e| format!("spawn {bin}: {e}"))?;
    if !out.status.success() {
        let tail: String = String::from_utf8_lossy(&out.stderr).chars().take(500).collect();
        return Err(format!("{bin} {} failed: {tail}", args.first().map(String::as_str).unwrap_or("")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Clone → apply → push → open PR. Returns the PR URL printed by `gh`.
/// Blocking; call via `spawn_blocking`. The temp dir is removed on
/// success and kept on failure for forensics.
pub fn publish_patch(spec: &PublishSpec, parent: &Path, patch: &str) -> Result<String, String> {
    let slug = jobscript::repo_slug(spec.repo_url)
        .ok_or_else(|| format!("not a github repo url: {}", spec.repo_url))?;
    let upstream_owner = slug.split('/').next().unwrap_or_default().to_string();
    let repo_name = slug.split('/').nth(1).unwrap_or_default().to_string();
    let dir: PathBuf = parent.join(format!("don-a-token-publish-{}", spec.job_tag));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir publish dir: {e}"))?;

    let failed = |e: String| -> String { e };
    let step = |r: Result<String, String>| r.map_err(failed);

    step(run("git", &git_clone_args(spec.repo_url, spec.base_branch, "repo"), &dir, &[]))?;
    let repo = dir.join("repo");
    step(run(
        "git",
        &["checkout".to_string(), "-b".to_string(), spec.branch.to_string()],
        &repo,
        &[],
    ))?;
    let patch_file = dir.join("changes.patch");
    std::fs::write(&patch_file, patch).map_err(|e| format!("write patch: {e}"))?;
    let patch_arg = patch_file.to_string_lossy().to_string();
    step(run("git", &["apply".to_string(), "--check".to_string(), patch_arg.clone()], &repo, &[]))?;
    step(run("git", &["apply".to_string(), patch_arg], &repo, &[]))?;
    step(run("git", &["add".to_string(), "-A".to_string()], &repo, &[]))?;
    step(run(
        "git",
        &[
            "-c".to_string(),
            "user.name=don-a-token".to_string(),
            "-c".to_string(),
            "user.email=donations@don-a-token.local".to_string(),
            // Donors may sign all commits (commit.gpgsign=true); the
            // headless publish path can't answer a passphrase prompt,
            // so donated commits are explicitly unsigned.
            "-c".to_string(),
            "commit.gpgsign=false".to_string(),
            "commit".to_string(),
            "-m".to_string(),
            spec.title.to_string(),
        ],
        &repo,
        &[],
    ))?;
    let (push_remote, head) = match spec.fork_owner {
        Some(fork) if !fork.eq_ignore_ascii_case(&upstream_owner) => {
            ensure_fork(spec.github_token, &slug, fork, &repo_name, &dir)?;
            step(run(
                "git",
                &[
                    "remote".to_string(),
                    "add".to_string(),
                    "fork".to_string(),
                    format!("https://github.com/{fork}/{repo_name}.git"),
                ],
                &repo,
                &[],
            ))?;
            ("fork", format!("{fork}:{}", spec.branch))
        }
        _ => ("origin", spec.branch.to_string()),
    };
    let push_env = git_push_env(spec.github_token);
    if let Err(e) = run("git", &git_push_args(push_remote, spec.branch), &repo, &push_env) {
        return Err(failed(e));
    }
    let mut body = spec.body.to_string();
    if let Some(result) = spec.result_md {
        body.push_str("\n\n## Agent summary\n\n");
        body.push_str(result);
        body.push('\n');
    }
    let body_file = dir.join("pr-body.md");
    std::fs::write(&body_file, body).map_err(|e| format!("write pr body: {e}"))?;
    let url = step(run(
        "gh",
        &gh_pr_create_args(
            &slug,
            &head,
            spec.base_branch,
            spec.title,
            &body_file.to_string_lossy(),
        ),
        &repo,
        &gh_env(spec.github_token),
    ))?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(url.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(patch: &str, result: Option<&str>) -> Vec<u8> {
        let mut s = format!(
            "some agent chatter\n{}\n{}\n{}\n",
            jobscript::MARK_PATCH_BEGIN,
            patch,
            jobscript::MARK_PATCH_END
        );
        if let Some(r) = result {
            s.push_str(&format!(
                "{}\n{}\n{}\n",
                jobscript::MARK_RESULT_BEGIN,
                r,
                jobscript::MARK_RESULT_END
            ));
        }
        s.into_bytes()
    }

    #[test]
    fn extract_finds_patch_and_result() {
        let out = extract_patch(&framed("diff --git a/f", Some("did things")));
        match out {
            PatchOutcome::Patch { patch, result_md } => {
                assert!(patch.contains("diff --git a/f"));
                assert_eq!(result_md.as_deref(), Some("did things"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn extract_markers_win_over_chatter() {
        // Agent mentions the sentinel, but the script's markers are present.
        let mut raw = framed("diff --git a/f", None);
        raw.extend_from_slice(b"note: NO_CHANGES=1 would mean empty\n");
        assert!(matches!(extract_patch(&raw), PatchOutcome::Patch { .. }));
    }

    #[test]
    fn extract_sentinels_without_markers() {
        assert_eq!(extract_patch(b"ok\nNO_CHANGES=1\n"), PatchOutcome::NoChanges);
        assert_eq!(extract_patch(b"ok\nPATCH_TOO_LARGE=1\n"), PatchOutcome::TooLarge);
        assert_eq!(extract_patch(b"codex failed early"), PatchOutcome::Missing);
    }

    #[test]
    fn argv_builders_shape_commands() {
        assert_eq!(
            git_clone_args("https://github.com/o/r", "main", "repo"),
            vec!["clone", "--depth", "1", "--branch", "main", "https://github.com/o/r", "repo"]
        );
        assert_eq!(git_push_args("origin", "b"), vec!["push", "origin", "b"]);
        assert_eq!(git_push_args("fork", "b"), vec!["push", "fork", "b"]);
        let pr = gh_pr_create_args("o/r", "head", "main", "t", "body.md");
        assert_eq!(pr[..3], vec!["pr", "create", "--repo"]);
        assert!(pr.contains(&"o/r".to_string()));
        assert!(!pr.iter().any(|a| a.contains("token") || a.contains("TOKEN")));
    }

    #[test]
    fn push_env_keeps_token_out_of_argv() {
        let env = git_push_env("sekret");
        assert!(env.iter().any(|(k, v)| k == "GIT_CONFIG_KEY_0" && v == "http.extraHeader"));
        assert!(env.iter().any(|(_, v)| v.contains("sekret")));
        assert_eq!(gh_env("sekret"), vec![("GH_TOKEN".to_string(), "sekret".to_string())]);
    }

    #[test]
    fn fork_owner_for_compares_donor_to_upstream() {
        assert_eq!(
            fork_owner_for(Some("donor"), "https://github.com/upstream/repo"),
            Some("donor")
        );
        assert_eq!(
            fork_owner_for(Some("owner"), "https://github.com/owner/repo"),
            None
        );
        assert_eq!(
            fork_owner_for(Some("OWNER"), "https://github.com/owner/repo"),
            None
        );
        assert_eq!(fork_owner_for(None, "https://github.com/u/r"), None);
        assert_eq!(fork_owner_for(Some("d"), "https://example.com/u/r"), None);
    }

    #[test]
    fn publish_rejects_non_github_url() {
        let spec = PublishSpec {
            repo_url: "https://example.com/o/r",
            base_branch: "main",
            branch: "b",
            title: "t",
            body: "b",
            result_md: None,
            github_token: "x",
            job_tag: "test-no-net",
            fork_owner: None,
        };
        let err = publish_patch(&spec, &std::env::temp_dir(), "patch").unwrap_err();
        assert!(err.contains("not a github repo url"));
    }
}
