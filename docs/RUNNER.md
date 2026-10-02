# Runner / isolation design

Unattended model runs must never execute on the donor's bare desktop.
E2B Embed is the default isolation target.

## E2B Embed

Embed ships the whole E2B stack — same SDK/API as E2B Cloud, real
Firecracker sandboxes, dashboard — on one machine the donor controls,
Apache-2.0, no E2B account or license key. The app speaks the
E2B-compatible API, so Embed local, Embed remote, and E2B Cloud differ
only by endpoint + credentials. Reference:
[e2b-dev/runtime/embed](https://github.com/e2b-dev/runtime/tree/main/embed).

Constraint: Embed needs a **Linux host with KVM**. That shapes the
platform matrix, not the API.

## Platform matrix

| Donor OS | Default backend | Why |
|---|---|---|
| Linux (KVM) | `embed-local` (`http://127.0.0.1:3000`, confirm vs REFERENCE) | Sandboxes stay on-machine, zero account needed |
| Linux (no KVM) | `e2b-cloud` or `embed-remote` | No local Firecracker available |
| macOS / Windows | `e2b-cloud` (API key) or user `embed-remote` node | Embed can't run on-machine |
| Any, last resort | `docker-local` | Weakest containment; clearly labeled in UI |

Implemented as `Backend` in `crates/core/src/runner.rs`; the narrow
`Runner` trait (`submit`/`status`/`cancel`) keeps backends swappable.

## Job lifecycle

```text
JobRequest { project_id, job_id, template, prompt_pack, env (no secrets), max_minutes }
  -> submit -> Queued -> Running -> Succeeded | Failed(reason) | TimedOut
  -> contribution attempt (PR) -> record outcome -> cooldown
```

- One job at a time per donor (concurrency comes later, carefully).
- Timeouts enforced by both the sandbox and the scheduler.
- Sandbox templates per project stack (`don-a-token-rust` first);
  templates build through the same E2B API on every backend.

## Trust model: what enters the sandbox

Each job receives, as per-command process env (never on the sandbox
record, never in logs — captured output is redacted):

- `ACCESS_TOKEN`: the donor's SIWC access token (1-hour life, refreshed
  at submit). The donor's plan pays for inference, so the token must be
  present where Codex runs. On Embed-local the sandbox is the donor's own
  hardware; on Cloud/remote the token leaves the machine — donors opt
  into that backend explicitly, and short expiry bounds the exposure.
- `JOB_PROMPT_B64`: the job prompt.
- Nothing else. GitHub credentials never enter the sandbox: the job
  exports its work as a patch on stdout (framed markers), and the host
  clones, applies, pushes, and opens the PR with `gh`.

Codex runs as `codex exec` with the plan provider overrides, its own
`workspace-write` sandbox on (defense-in-depth inside E2B), and no
dangerous bypass flags.

## Contribution authentication

Requirements: PRs open in the donor's name, attributable, revocable, and
never expose long-lived donor credentials to the sandbox.

- Donors link GitHub in the app (username + interim PAT now; OAuth
  device flow and other forges later).
- Interim: the donor's PAT is used per publish, host-side only (process
  env for `git push` / `gh pr create`, never argv). Follow-up: the
  harness mints **short-lived, per-job contribution credentials**
  (GitHub App installation tokens scoped to the target repo, or
  fine-grained PATs via device flow) that expire with the job.
- Every PR footer carries: donor identity, project job-spec version, and
  the SIWC `client_id` as harness provenance.
- Projects verify PRs against the contributions tracker + registry history.

## Open questions (artifact sharing)

- Where landed-PR records live for the tracker (D1 vs. generated JSON
  artifact) and who writes them (donor app direct vs. relay).
- Prompt-pack distribution: bundled per release vs. fetched per job.
- Sandbox template registry + versioning across backends.
- Abuse controls: per-project rate limits, donor reputation, kill-switches.
