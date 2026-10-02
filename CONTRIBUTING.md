# Contributing

## Setup

- Rust stable (`cargo test -p don-a-token-core` must pass)
- Node 20+ with pnpm (`pnpm install`, then
  `pnpm --filter @don-a-token/desktop tauri dev`)

## Rules of the road

- The Rust core (`crates/core/`) stays UI-free and unit-tested. New behavior
  there ships with tests.
- Never commit credentials, tokens, updater signing keys, or `.env` files.
  SIWC credential records live under the OS app-data dir, never in the repo.
- The projects registry (`projects/registry.json`) is manually approved:
  additions need maintainer review. Validate against `projects/schema.json`
  and keep `updated_at` current.

## Commits

For non-release commits, commit unsigned:

```sh
git commit --no-gpg-sign -m "..."
```

(GPG signing is reserved for releases; the key passphrase cannot be answered
from an agent session.)
