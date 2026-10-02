# Multi-machine test plan

Don-a-Token is cross-platform with per-host identity, so testing spreads
across machines. Roles parallelize: each machine below can be worked
independently once the repo is pushed to `ntindle/don-a-token`.

## Machine roles

1. **Linux Embed node** (KVM capable) — runs E2B Embed via compose:
   `mkdir e2b && cd e2b && curl …/{compose.yaml,.env} && docker compose up -d --wait`.
   Verifies: first sandbox from the SDK, dashboard reachability, template
   builds.
2. **Linux client** — desktop app + `embed-local` backend. Verifies:
   onboarding end-to-end, tray menu, scheduler, first real donated job.
3. **Windows client** — app against `e2b-cloud` (API key) or the remote
   Embed node. Verifies: installer, autostart, updater, tray, loopback
   OAuth on Windows.
4. **macOS client** — same as Windows plus universal binary + LaunchAgent
   autostart + notarization path.
5. **Headless Linux (optional)** — credential transfer flow: OAuth on a
   browser machine, `scp` the credential file to the headless host's
   app-data path, confirm its own host id is preserved and it owns
   refreshes (per the SIWC self-hosted-VM flow).

## Per-machine checklist

- [ ] `cargo test -p don-a-token-core` green.
- [ ] `pnpm install && pnpm build` in `apps/desktop` green.
- [ ] `tauri dev`: Welcome → Rules → Projects → done; settings persist
      across restart.
- [ ] Tray: show, pause, skip-week, quit all take effect.
- [ ] SIWC sign-in completes; credential record on disk; refresh works
      past the 1-hour access-token lifetime.
- [ ] Usage-limit error halts donations and surfaces Manage usage.
- [ ] `tauri build` produces a working installer; updater finds the next
      release; `registry.json` refreshes from the release artifact.

## Credentials safety during testing

- Never commit or paste credential records, tokens, or the updater
  signing key. Test accounts only — no personal ChatGPT accounts with
  payment methods until the flows are trusted.
- Revoke test sessions from ChatGPT settings after each round
  (or exercise the app's sign-out revocation path deliberately).
