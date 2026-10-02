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

## Contribution authentication (design)

Requirements: PRs open in the donor's name, attributable, revocable, and
never expose long-lived donor credentials to the sandbox.

- Donors link GitHub in the app (username now; OAuth device flow next,
  other forges later).
- The harness mints **short-lived, per-job contribution credentials**
  (options: GitHub App installation tokens scoped to the target repo, or
  fine-grained PATs via device flow) and injects only those into the
  sandbox. They expire with the job.
- Every PR footer carries: donor identity, project job-spec version, and
  the SIWC `client_id` as harness provenance.
- Projects verify PRs against the contributions tracker + registry history.

## Open questions (artifact sharing)

- Where landed-PR records live for the tracker (D1 vs. generated JSON
  artifact) and who writes them (donor app direct vs. relay).
- Prompt-pack distribution: bundled per release vs. fetched per job.
- Sandbox template registry + versioning across backends.
- Abuse controls: per-project rate limits, donor reputation, kill-switches.
