# don-a-token

Donate spare ChatGPT plan capacity to open-source projects.
Think Folding@home / SETI@home, but for tokens: your machine runs small,
sandboxed Codex jobs for projects you pick, on rules you set.

> Status: skeleton. Onboarding UI, SIWC auth core types, rules engine,
> projects registry, and release automation are scaffolded. No donations
> run yet.

## How it works

1. **Sign in with ChatGPT** — the desktop app registers a user-defined agent
   via Sign in with ChatGPT (SIWC) with ChatGPT plan usage. Your keystrokes
   never leave your machine; only eligible inference requests use your plan.
2. **Set your rules** — cap donations (e.g. at most 10% of remaining usage),
   keep a reserve floor (e.g. always leave 5%), quiet hours, pause, skip a week.
3. **Pick your projects** — choose from a manually approved registry of
   donation opportunities. Jobs run unattended inside isolated sandboxes
   (E2B Embed on Linux, E2B Cloud or containers elsewhere) and contribute
   back as pull requests attributed to you.

## Repo map

- `apps/desktop/` — Tauri 2 desktop app (Mac, Windows, Linux): onboarding,
  tray menu, scheduler, auto-update.
- `crates/core/` — portable Rust core: SIWC auth types, credential store,
  rules engine, projects registry loader, Codex app-server wiring, runner
  abstraction. Fully unit-tested, no UI dependencies.
- `projects/` — the approved donation-opportunity registry (`registry.json`
  + JSON Schema). Shipped as a release artifact; the app refreshes from it.
- `website/` — static landing page + docs.
- `docs/` — architecture, SIWC notes, runner design, multi-machine test plan.

## Quickstart

Prereqs: Rust stable, Node 20+, pnpm 9+.

```sh
# Run the Rust core tests (no Node needed)
cargo test -p don-a-token-core

# Install JS deps and run the desktop app in dev
pnpm install
pnpm --filter @don-a-token/desktop tauri dev
```

## Publishing

This repo is intended to live at `ntindle/don-a-token` as a **public**
repository. Public matters: ChatGPT plan usage via SIWC token-sharing is
only available to open-source, locally hosted apps.

```sh
gh repo create ntindle/don-a-token --public --source=. --push
```

Releases ship the desktop installers, the updater manifest (`latest.json`),
and the projects registry (`registry.json`) as artifacts. See
`.github/workflows/release.yml`.

## Docs

- [Architecture](docs/ARCHITECTURE.md)
- [SIWC implementation notes](docs/SIWC-NOTES.md)
- [Runner / isolation design](docs/RUNNER.md)
- [Multi-machine test plan](docs/TESTING.md)

Upstream references:

- [SIWC token-sharing docs](https://developers.openai.com/siwc/token-sharing-open-source)
- [SIWC docs index](https://developers.openai.com/siwc/llms.txt)
- [E2B Embed](https://github.com/e2b-dev/runtime/tree/main/embed) (Apache-2.0)

## License

Apache-2.0. See [LICENSE](LICENSE).
