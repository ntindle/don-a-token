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

## Local backend (testing only)

`local` runs the job bootstrap directly on-host (Git Bash + host
`codex`/`cargo`/`git`) with **no isolation**. It exists to prove the
end-to-end loop (run → patch → PR) before trusting a sandbox with it.
Never the default; the Setup UI labels it testing-only. Timeouts kill
the whole process tree so no orphaned agent burns plan after the cap.
The temp workdir (clone + `target/`) is removed when the job reaches a
terminal state — retained workdirs once filled a donor disk. Cleanup
is donor-configurable (Setup → Runner backend): an optional custom
cleanup command (run via `sh -c` with the job dir in `JOB_WORKDIR`,
60s cap, failures logged and never fatal), else the built-in delete
under a retention choice — always remove (default), keep failed jobs
for debugging, or keep everything. Each selected project's
`maintainer_notes` from the registry are shown next to the field so
the donor knows the stack, what jobs leave behind, and what to
install when running many jobs.

Host toolchain notes (Windows): the scheduler pins
`RUSTUP_TOOLCHAIN` from the registry's `job.local_toolchain` so the
agent's and the harness's `cargo` resolve to the project's toolchain
instead of the host default triple (a bare `rust-toolchain.toml`
channel resolves MSVC, which the donor may not have installed). The
driver also prepends `~/bin/w64devkit`'s gcc to `PATH` when no gcc is
found, since Git Bash ships none and GNU-target linking needs one.

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

Codex runs as `codex exec --approve-for-me` with the plan provider
overrides (codex-cli ≥ 0.157; `--full-auto` was removed and an explicit
`--sandbox` conflicts with `--approve-for-me`, which already scopes to
the workspace-write sandbox) and no dangerous bypass flags.

## Contribution authentication

Requirements: PRs open in the donor's name, attributable, revocable, and
never expose long-lived donor credentials to the sandbox.

- Donors link GitHub in the app (username + OAuth device flow,
  `gh auth login`-style; pasting a classic PAT is still accepted).
- The linked token is used per publish, host-side only (process env for
  `git push` / `gh pr create`, never argv, never the sandbox).
  Follow-up: short-lived, per-job contribution credentials scoped to
  the target repo.
- PRs to repos the donor doesn't own go via their fork (created on
  demand with `gh repo fork`); the PR head is `donor:branch`.
- Every PR footer carries: donor identity, project job-spec version, and
  the SIWC `client_id` as harness provenance.
- Projects verify PRs against the contributions tracker + registry history.

## Open questions (artifact sharing)

- Where landed-PR records live for the tracker (D1 vs. generated JSON
  artifact) and who writes them (donor app direct vs. relay).
- Prompt-pack distribution: bundled per release vs. fetched per job.
- Sandbox template registry + versioning across backends.
- Abuse controls: per-project rate limits, donor reputation, kill-switches.
