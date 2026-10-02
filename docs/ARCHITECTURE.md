# Architecture

Don-a-Token is a desktop harness (Tauri 2, Mac/Windows/Linux) that donates
spare ChatGPT plan capacity to curated open-source projects via sandboxed
Codex jobs.

## Components

```text
apps/desktop/          Tauri app: onboarding UI (React) + Rust shell
  src/                 Welcome -> Rules -> Projects onboarding, settings store
  src-tauri/           tray menu, updater, autostart, host id, (spike: SIWC auth)
crates/core/           UI-free Rust: SIWC types, credentials, rules, registry,
                       Codex app-server wiring, runner abstraction. Tested.
projects/              approved donation registry (registry.json + schema)
website/               static site on Cloudflare Pages
```

## Key flows

### Onboarding

1. **Welcome** — provider list (ChatGPT now, more later), `Continue with
   ChatGPT` (SIWC dynamic registration), first-sign-in plan modal.
2. **Rules** — percent caps, reserve floor, per-job caps, cooldown,
   quiet hours, pause, skip-week. Stored in the settings store.
3. **Projects** — curated registry entries with per-job caps; GitHub
   identity linking for PR attribution.

### Donation loop (scheduler lands after the spikes)

```text
rules.should_run(now)? --> pick project --> build job --> runner.submit()
    --> poll status --> on usage-limit error: halt + surface Manage usage
    --> on success: record contribution, cooldown, repeat
```

Percent-of-limit pacing is calibrated from observed per-job burn until a
programmatic usage API exists; usage-limit errors are a hard stop.

### Auth (SIWC token-sharing)

Dynamic registration (`client_id=dynamic_agent_client` + stable
`ext_agent_host_id`) once per ChatGPT account; issued `client_id` reused
after. Loopback callback on `127.0.0.1` with fresh state/nonce/PKCE per
attempt. Credential records live per issued client id under the OS
app-data dir (atomic writes, owner-only on Unix). Refresh rotates
access + refresh tokens together; app-server restarts with the renewed
`ACCESS_TOKEN` and resumes its thread. Full details: [SIWC-NOTES](SIWC-NOTES.md).

### Updates

Tauri updater polls `latest.json` from GitHub releases; the projects
registry ships as `registry.json` on the same release and is pulled on the
same cadence. Release automation: `.github/workflows/release.yml`.

## Data at rest

| What | Where | Protection |
|---|---|---|
| SIWC credential records | app-data dir (`host_id`, `<client>.json`) | atomic write, `0600` Unix |
| Settings (rules, projects, GitHub user) | `settings.json` via store plugin | plaintext, no secrets |
| Updater signing key | maintainer machine + repo secret only | never in repo |

Future hardening: move tokens to the OS keychain (per-platform
credentials API) and derive the host id from a JWK thumbprint.

## Decisions

- **Tauri 2 over Electron:** small binaries, low idle footprint for a
  background donor, Rust shell for OAuth loopback + secure storage.
- **E2B Embed first for isolation:** same API as E2B Cloud, Apache-2.0,
  runs on the donor's own hardware. Linux+KVM runs it on-machine;
  Mac/Windows point at Cloud or a remote Embed node. See [RUNNER](RUNNER.md).
- **Core crate stays UI-free** so the donation logic is unit-tested without
  Tauri, Node, or a display.
- **Registry is curated:** manual approval, shipped as a signed release
  artifact, validated by the core test suite.
