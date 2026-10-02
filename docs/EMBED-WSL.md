# E2B Embed on Windows (WSL2) runbook

How the project's Embed node runs on Nick's Windows box, and every
gotcha hit standing it up. All paths/ports below are the live values.

## Layout

- Distro: Ubuntu (WSL2), `~/e2b` holds `compose.yaml` + `.env`.
- **Native Docker Engine in WSL**, not Docker Desktop: the compose
  stack's `host-setup` must write `/etc/modules-load.d` on its host,
  and from a container the "host" behind Docker Desktop is its
  immutable VM. Native `dockerd` (systemd) sees the mutable WSL root.
  - The user's other workloads stay on Docker Desktop untouched;
    WSL-side `docker` now talks to the native daemon.
- No E2B account, no external token: `seed` mints a team API key on
  first run; `docker compose logs ready` prints the four exports.

## Required deviations from upstream compose

Upstream `compose.yaml` is used with a backup at `compose.yaml.orig`.

1. **Ports** (user's AutoGPT stack owns 5432 + 3001):
   - postgres host binding `127.0.0.1:5432` → `:5442`, plus the two
     host-networked `POSTGRES_CONNECTION_STRING`s using
     `127.0.0.1:5432` → `:5442`. (Service-DNS users of
     `postgres:5432` stay on the container port.)
   - dashboard `PORT: "3001"` → `"3101"` + its healthcheck URL +
     the `sdk.env` export line.
2. **Orchestrator shm**: add `shm_size: 2gb` to the `orchestrator`
   service. Default 64M cannot back guest memfds (`mmap memfd:
   cannot allocate memory`).
3. **Env overrides on every `up`** (11GB WSL VM):
   `HUGEPAGES=512 PF_MIN_FREE_GIB=4`. Defaults (2048 / 20GiB) exceed
   the box. 512 pages = 1GB reservation: this node fits exactly one
   512MB sandbox at a time.

## Bring-up after a WSL restart

Hugepages don't survive reboot and one-shot `host-setup` won't re-run,
so before the first sandbox after any restart, as root:

```sh
echo 1 > /proc/sys/vm/compact_memory   # defrag; without this only ~20 pages reserve
echo 512 > /proc/sys/vm/nr_hugepages
```

Then normal `up -d` (containers are `unless-stopped` and revive on
their own). The API is reachable from Windows at
`http://<wsl-ip>:3000` (`hostname -I`; localhost forwarding does not
cover it). Key via `docker compose logs ready`.

## Operating discipline (small node)

- **One sandbox at a time.** A second concurrent create fails fast
  with 500 `sandbox_create_failed`.
- **Always kill via the API.** `kill -9` on a Firecracker process
  orphans its API record (ghost `running` rows) and leaks hugetlb
  reservations (`HugePages_Rsvd` stuck high); later creates fail even
  with pages "free". Audit with `GET /v2/sandboxes` and
  `grep HugePages_ /proc/meminfo`. The live test kills in a cleanup
  path for exactly this reason.
- **Don't run the phase compile next to the stack.** A full
  `cargo test --no-run` on phase plus the stack OOM'd the 31GB host
  (vmmemWSL never released; WSL went unresponsive). One heavy job at
  a time, and cap cargo jobs (`CARGO_BUILD_JOBS=4`).
- Full reset (ghosts + stuck state):
  `docker compose down -v`, `--profile purge run --rm host-teardown`,
  `up` again. Nuclear option only.

## Credential-helper note

Docker Desktop's `desktop.exe` credential helper fails inside WSL
(`logon session does not exist`), breaking all pulls. WSL
`~/.docker/config.json` was replaced with `{}` (backup at
`config.json.bak`) for anonymous pulls. Restore the backup if WSL-side
authenticated pushes are ever needed.
