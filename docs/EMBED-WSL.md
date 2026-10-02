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

A kernel reboot resets hugepages AND unloads `nbd`, and one-shot
`host-setup` won't re-run, so after any restart (as root unless noted):

```sh
echo 1 > /proc/sys/vm/compact_memory   # defrag; without this only ~20 pages reserve
echo 512 > /proc/sys/vm/nr_hugepages
cd ~/e2b && HUGEPAGES=512 PF_MIN_FREE_GIB=4 \
  docker compose run --rm --no-deps host-setup   # reloads nbd/tun/kvm, sysctls, dirs
docker compose restart orchestrator              # was crash-looping on missing nbd
```

Then normal `up -d` (containers are `unless-stopped` and revive on
their own). The API is reachable from Windows at
`http://<wsl-ip>:3000` (`hostname -I`; localhost forwarding does not
cover it). Key via `docker compose logs ready`.

Without the `host-setup` re-run the orchestrator crash-loops with
`failed to create device pool: ... NBD module not loaded` and every
build fails with `no available build client` (503).

## WSL idle auto-stop (read this before long jobs)

WSL terminates an **idle distro ~15s after its last `wsl.exe`
session exits** (`instanceIdleTimeout`, default 15000ms) — systemd
services do NOT keep it alive — and shuts down the shared VM ~60s
after the last distro stops (`vmIdleTimeout`, default 60000ms). Every
stop kills dockerd, all stack containers, and any running build or
sandbox; the next `wsl` command boots a fresh kernel (nbd/hugepages
reset, see above). Previously this was masked by Docker Desktop's WSL
integration sessions; when Desktop broke, the node started dying
mid-build with `ECONNREFUSED`.

Durable fix in `%USERPROFILE%\.wslconfig` (takes effect at the next
`wsl --shutdown`):

```ini
[general]
instanceIdleTimeout=-1

[wsl2]
vmIdleTimeout=-1
```

(`autoMemoryReclaim` is NOT valid on this box's WSL 2.7.112 —
`wsl` warns `Unknown key` and ignores it. vmmemWSL page-cache bloat
is instead relieved with `sync; echo 3 >
/proc/sys/vm/drop_caches` inside the guest.)

Until a shutdown applies it, hold the distro with a live client for
the duration of any long work (build, verify, E2E):

```powershell
wsl -d Ubuntu -- sleep 3600   # keep alive until the job finishes
```

## Operating discipline (small node)

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
- **Build with 384MB builders on this node**
  (`E2B_BUILD_MEM=384 E2B_BUILD_CPU=1`). Each build step resumes the
  builder from snapshot, transiently needing ~2x builder memory in
  hugetlb. `HugePages_Rsvd` sits at a 63-page baseline from boot
  (the stack's own reservations — verified present on a virgin
  kernel with zero sandboxes ever run), leaving ~449 effective
  pages, so 512MB builders fail resume with `mmap memfd: cannot
  allocate memory`. Template sandboxes inherit build memory
  (v4 runs 384MB).
- **Purge stale sandbox networking after unclean deaths.**
  Crashed sandboxes leak netns (`ns-N`), host `veth-N` links, and
  `10.11.0.0/24` routes; with zero sandboxes alive, delete them all
  as root (`ip netns delete`, `ip link delete veth-N`) and restart
  the orchestrator so its bookkeeping resets. (Stale NAT
  MASQUERADE rules are harmless; leave them.) Docker `veth<hex>`
  links are not E2B's — never touch those.
- **JS SDK needs `E2B_SANDBOX_URL`.** Template scripts run from
  Windows must export all three (`E2B_API_URL`, `E2B_API_KEY`,
  `E2B_SANDBOX_URL=http://<wsl-ip>:3002`). Without the sandbox URL
  the SDK builds cloud-style `https://49983-<id>.<domain>` URLs and
  every `commands.run` fails with `Sandbox is probably not running
  anymore` even though create/kill (API :3000) work. (Our Rust
  client derives the proxy port from `api_base` automatically.)
- Full reset (ghosts + stuck state):
  `docker compose down -v`, `--profile purge run --rm host-teardown`,
  `up` again. Nuclear option only.

## Credential-helper note

Docker Desktop's `desktop.exe` credential helper fails inside WSL
(`logon session does not exist`), breaking all pulls. WSL
`~/.docker/config.json` was replaced with `{}` (backup at
`config.json.bak`) for anonymous pulls. Restore the backup if WSL-side
authenticated pushes are ever needed.
