//! Job payload builders: sandbox bootstrap script, PR body, attribution.
//!
//! Pure and unit-tested. The plan token travels as a process env var,
//! never embedded in the script or logs. The sandbox never sees GitHub
//! credentials: it exports its work as a patch on stdout, and the host
//! publishes the PR (see the desktop shell's `publish` module).

use crate::codex;

/// Env var carrying the base64-encoded job prompt inside the sandbox.
pub const ENV_JOB_PROMPT_B64: &str = "JOB_PROMPT_B64";

/// Max patch bytes exported on stdout; larger diffs are reported as
/// `PATCH_TOO_LARGE` and left unpublished.
pub const PATCH_MAX_BYTES: usize = 262144;

/// Stdout markers framing the exported patch and the agent's summary.
pub const MARK_PATCH_BEGIN: &str = "---DON-A-TOKEN-PATCH-BEGIN---";
pub const MARK_PATCH_END: &str = "---DON-A-TOKEN-PATCH-END---";
pub const MARK_RESULT_BEGIN: &str = "---DON-A-TOKEN-RESULT-BEGIN---";
pub const MARK_RESULT_END: &str = "---DON-A-TOKEN-RESULT-END---";

pub struct BootstrapSpec<'a> {
    pub repo_url: &'a str,
    pub base_branch: &'a str,
    pub workdir: &'a str,
    pub checks: &'a [String],
    pub codex_args: &'a [String],
}

/// Shell-quote one argv (single-quote style, safe for curated inputs).
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `owner/repo` slug from an `https://github.com/owner/repo[.git]` URL.
pub fn repo_slug(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let rest = rest.strip_suffix(".git").unwrap_or(rest).trim_matches('/');
    let mut parts = rest.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// Assemble the one-shot bootstrap script run as `sh -c <script>`.
/// The prompt arrives base64-encoded via env; the plan token arrives as
/// env. Nothing secret is embedded in the returned script. After checks
/// pass, the worktree is exported as a patch framed by stdout markers;
/// the host publishes the PR.
pub fn build_bootstrap(spec: &BootstrapSpec) -> String {
    let repo = sh_quote(spec.repo_url);
    let base = sh_quote(spec.base_branch);
    let work = sh_quote(spec.workdir);
    let codex = spec
        .codex_args
        .iter()
        .map(|a| sh_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    let mut checks = String::new();
    for check in spec.checks {
        checks.push_str(&format!(
            "(cd \"$REPO\" && sh -c {}) || exit 21\n",
            sh_quote(check)
        ));
    }
    format!(
        r#"set -euo pipefail
WORK={work}
REPO="$WORK/repo"
mkdir -p "$WORK"
echo "$JOB_PROMPT_B64" | base64 -d > "$WORK/prompt.md"
git clone --depth 1 --branch {base} {repo} "$REPO"
cd "$REPO"
BASE="$(git rev-parse HEAD)"
ACCESS_TOKEN="$ACCESS_TOKEN" codex {codex} "$(cat "$WORK/prompt.md")"
{checks}git add -N . >/dev/null 2>&1 || true
# Diff against the pre-agent base: the agent commits its work, so a
# bare `git diff HEAD` would come back empty and drop good patches.
git diff "$BASE" > "$WORK/changes.patch"
if [ ! -s "$WORK/changes.patch" ]; then echo NO_CHANGES=1; exit 0; fi
if [ "$(wc -c < "$WORK/changes.patch")" -gt {patch_max} ]; then echo PATCH_TOO_LARGE=1; exit 0; fi
echo '{mark_patch_begin}'
cat "$WORK/changes.patch"
echo '{mark_patch_end}'
if [ -f "$WORK/RESULT.md" ]; then echo '{mark_result_begin}'; cat "$WORK/RESULT.md"; echo '{mark_result_end}'; fi
"#,
        work = work,
        base = base,
        repo = repo,
        codex = codex,
        checks = checks,
        patch_max = PATCH_MAX_BYTES,
        mark_patch_begin = MARK_PATCH_BEGIN,
        mark_patch_end = MARK_PATCH_END,
        mark_result_begin = MARK_RESULT_BEGIN,
        mark_result_end = MARK_RESULT_END,
    )
}

/// Attribution footer appended to every donated PR body.
pub fn attribution_footer(
    donor: Option<&str>,
    project_id: &str,
    job_id: &str,
    template: &str,
    client_id: Option<&str>,
) -> String {
    let donor = donor
        .map(|d| format!("@{d}"))
        .unwrap_or_else(|| "anonymous donor".to_string());
    let provenance = client_id.unwrap_or("unlinked");
    format!(
        "\n---\n*Donated via [don-a-token](https://github.com/ntindle/don-a-token) by {donor} · project `{project_id}` · job `{job_id}` · template `{template}` · harness `{provenance}`*"
    )
}

/// Full PR body: static intro + footer. The host appends the agent's
/// `RESULT.md` (exported on sandbox stdout) before opening the PR.
pub fn pr_body(project_name: &str, footer: &str) -> String {
    format!(
        "Automated contribution to {project_name}, produced by an unattended\
         \ndonated-compute job. A human should review carefully before merging.\
         \n{footer}\n"
    )
}

/// Redact secret values from captured output before storing/displaying.
pub fn redact(mut text: Vec<u8>, secrets: &[&str]) -> Vec<u8> {
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        let needle = secret.as_bytes();
        let mut at = 0;
        while let Some(pos) = text[at..].windows(needle.len()).position(|w| w == needle) {
            let start = at + pos;
            text.splice(start..start + needle.len(), b"[redacted]".iter().copied());
            at = start + "[redacted]".len();
        }
    }
    text
}

/// Convenience: exec argv for the standard job layout under `workdir`.
pub fn standard_exec_args(workdir: &str) -> Vec<String> {
    codex::exec_args(&format!("{workdir}/repo"), &format!("{workdir}/result.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_wires_clone_codex_checks_patch_export() {
        let checks = ["cargo test".to_string()];
        let codex_args = ["exec".to_string(), "-C".to_string(), ".".to_string()];
        let spec = BootstrapSpec {
            repo_url: "https://github.com/phase-rs/phase",
            base_branch: "main",
            workdir: "/home/user/job",
            checks: &checks,
            codex_args: &codex_args,
        };
        let script = build_bootstrap(&spec);
        assert!(script
            .contains("git clone --depth 1 --branch 'main' 'https://github.com/phase-rs/phase'"));
        assert!(script.contains("codex 'exec'"));
        assert!(script.contains("echo \"$JOB_PROMPT_B64\" | base64 -d"));
        assert!(script.contains("sh -c 'cargo test'"));
        assert!(script.contains("git add -N .")); // new files join the diff
        assert!(script.contains("BASE=\"$(git rev-parse HEAD)\""));
        assert!(script.contains("git diff \"$BASE\"")); // committed work included
        assert!(!script.contains("git diff HEAD")); // ...never against moving HEAD
        assert!(script.contains(MARK_PATCH_BEGIN));
        assert!(script.contains(MARK_PATCH_END));
        assert!(script.contains(MARK_RESULT_BEGIN));
        assert!(script.contains(&PATCH_MAX_BYTES.to_string()));
        assert!(script.contains("NO_CHANGES=1"));
        assert!(script.contains("PATCH_TOO_LARGE=1"));
        assert!(!script.contains("gh pr create")); // host publishes
        assert!(!script.contains("push origin")); // ...so no push here
        assert!(!script.contains("DONATION_GITHUB_TOKEN")); // no GH creds in sandbox
    }

    #[test]
    fn repo_slug_parses_github_urls() {
        assert_eq!(
            repo_slug("https://github.com/phase-rs/phase").as_deref(),
            Some("phase-rs/phase")
        );
        assert_eq!(
            repo_slug("https://github.com/o/r.git").as_deref(),
            Some("o/r")
        );
        assert_eq!(repo_slug("https://example.com/o/r"), None);
        assert_eq!(repo_slug("https://github.com/only"), None);
    }

    #[test]
    fn sh_quote_escapes_single_quotes() {
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
        assert_eq!(sh_quote("plain"), "'plain'");
    }

    #[test]
    fn footer_and_body_carry_attribution() {
        let footer = attribution_footer(Some("octocat"), "phase", "j1", "t", Some("oaiapp_1"));
        assert!(footer.contains("@octocat"));
        assert!(footer.contains("oaiapp_1"));
        let body = pr_body("Phase", &footer);
        assert!(body.contains("Automated contribution to Phase"));
        assert!(body.contains("don-a-token"));
    }

    #[test]
    fn redact_replaces_secrets() {
        let out = redact(b"token abc123 here abc123".to_vec(), &["abc123", ""]);
        assert_eq!(out, b"token [redacted] here [redacted]");
    }
}
