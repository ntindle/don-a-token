//! Job payload builders: sandbox bootstrap script, PR body, attribution.
//!
//! Pure and unit-tested. Secrets (plan token, contribution token) travel
//! as process env vars, never embedded in the script or logs.

use crate::codex;

/// Environment variable names consumed inside the sandbox.
pub const ENV_JOB_PROMPT_B64: &str = "JOB_PROMPT_B64";
pub const ENV_PR_BODY_B64: &str = "PR_BODY_B64";
pub const ENV_CONTRIB_TOKEN: &str = "DONATION_GITHUB_TOKEN";

pub struct BootstrapSpec<'a> {
    pub repo_url: &'a str,
    pub base_branch: &'a str,
    pub branch: &'a str,
    pub workdir: &'a str,
    pub checks: &'a [String],
    pub codex_args: &'a [String],
    pub pr_title: &'a str,
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
/// Prompt and PR body arrive base64-encoded via env; tokens arrive as
/// env vars. Nothing secret is embedded in the returned script.
pub fn build_bootstrap(spec: &BootstrapSpec) -> String {
    let repo = sh_quote(spec.repo_url);
    let base = sh_quote(spec.base_branch);
    let branch = sh_quote(spec.branch);
    let work = sh_quote(spec.workdir);
    let title = sh_quote(spec.pr_title);
    let codex = spec.codex_args.iter().map(|a| sh_quote(a)).collect::<Vec<_>>().join(" ");
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
echo "${ENV_JOB_PROMPT_B64}" | base64 -d > "$WORK/prompt.md"
echo "${ENV_PR_BODY_B64}" | base64 -d > "$WORK/pr-body.md"
git clone --depth 1 --branch {base} {repo} "$REPO"
cd "$REPO"
git checkout -b {branch}
ACCESS_TOKEN="$ACCESS_TOKEN" codex {codex} "$(cat "$WORK/prompt.md")"
{checks}git -c user.name='don-a-token' -c user.email='donations@don-a-token.local' add -A
if git diff --cached --quiet; then echo NO_CHANGES=1; exit 0; fi
git commit -m {title}
[ -f "$WORK/RESULT.md" ] && cat "$WORK/RESULT.md" >> "$WORK/pr-body.md" || true
if [ -z "${{{ENV_CONTRIB_TOKEN}:-}}" ]; then echo NO_PR_TOKEN=1; exit 0; fi
git -c http.extraHeader="AUTHORIZATION: bearer ${ENV_CONTRIB_TOKEN}" push origin {branch}
GH_TOKEN="${ENV_CONTRIB_TOKEN}" gh pr create --repo $REPO_SLUG --head {branch} --base {base} --title {title} --body-file "$WORK/pr-body.md"
"#,
        work = work,
        base = base,
        repo = repo,
        branch = branch,
        codex = codex,
        checks = checks,
        title = title,
    )
    // REPO_SLUG is derived at build time (not in-sandbox) for clarity.
    .replace("$REPO_SLUG", &sh_quote(&repo_slug(spec.repo_url).unwrap_or_default()))
}

/// Attribution footer appended to every donated PR body.
pub fn attribution_footer(
    donor: Option<&str>,
    project_id: &str,
    job_id: &str,
    template: &str,
    client_id: Option<&str>,
) -> String {
    let donor = donor.map(|d| format!("@{d}")).unwrap_or_else(|| "anonymous donor".to_string());
    let provenance = client_id.unwrap_or("unlinked");
    format!(
        "\n---\n*Donated via [don-a-token](https://github.com/ntindle/don-a-token) by {donor} · project `{project_id}` · job `{job_id}` · template `{template}` · harness `{provenance}`*"
    )
}

/// Full PR body: static intro + footer. The agent's `RESULT.md` is
/// appended in-sandbox before `gh pr create`.
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
        while let Some(pos) = text[at..]
            .windows(needle.len())
            .position(|w| w == needle)
        {
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
    fn bootstrap_wires_clone_codex_checks_push_pr() {
        let checks = ["cargo test".to_string()];
        let codex_args = ["exec".to_string(), "-C".to_string(), ".".to_string()];
        let spec = BootstrapSpec {
            repo_url: "https://github.com/phase-rs/phase",
            base_branch: "main",
            branch: "don-a-token/job-1",
            workdir: "/work/job",
            checks: &checks,
            codex_args: &codex_args,
            pr_title: "don-a-token test",
        };
        let script = build_bootstrap(&spec);
        assert!(script.contains("git clone --depth 1 --branch 'main' 'https://github.com/phase-rs/phase'"));
        assert!(script.contains("git checkout -b 'don-a-token/job-1'"));
        assert!(script.contains("codex 'exec'"));
        assert!(script.contains("sh -c 'cargo test'"));
        assert!(script.contains("gh pr create --repo 'phase-rs/phase'"));
        assert!(script.contains("extraHeader")); // no token in push URL
        assert!(!script.contains("DONATION_GITHUB_TOKEN=")); // env-only
        assert!(script.contains("NO_PR_TOKEN=1")); // graceful without token
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
