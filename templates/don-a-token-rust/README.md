# don-a-token-rust template

E2B sandbox template for Rust donation jobs: official `rust` image
(builder disk scales with the base — plain Ubuntu's ~809M can't fit
the toolchain) + pinned nightly + sccache + git + Codex CLI, running
as non-root `user`. Stable is deleted at build time; the template
disk is 3.2G and ~700M must stay free for repo + registry + target.

## Build

```sh
cd templates/don-a-token-rust
cp .env.example .env   # fill in E2B_API_KEY (and E2B_API_URL for Embed)
npm install
npm run build          # builds alias `don-a-token-rust`
```

Small nodes (11GB WSL2) need a small builder (512MB VMs are all the
hugepage pool fits — see the memfd note in versioning):

```sh
E2B_BUILD_CPU=1 E2B_BUILD_MEM=512 E2B_BUILD_DISK_MB=8192 npm run build -- don-a-token-rust-v2
```

Always boot-verify a new build — the in-builder checks run with the
image ENV, but sandboxes don't inherit it, so only a real boot proves
the toolchain resolves:

```sh
node verify.mjs don-a-token-rust-v2   # needs E2B_API_KEY + E2B_API_URL in env/.env
```

For Embed, point at the node: `E2B_API_URL=http://<embed-host>:3000`
with the install key from the compose seed output.

## Verify

```sh
# Cloud:
node -e "import('e2b').then(async ({Sandbox}) => {
  const sbx = await Sandbox.create('don-a-token-rust');
  console.log((await sbx.commands.run('cargo --version && git --version && codex --version')).stdout);
  await sbx.kill();
})"
```

Jobs expect `codex`, `git`, and `cargo` on PATH for `user`, and a
writable home directory (`/home/user/job` is the job workdir).
(`gh` lives on the host: PRs are published from the exported patch,
so GitHub credentials never enter the sandbox.)

## Versioning

Template aliases are mutable tags. When the toolchain changes, build a
new alias (`don-a-token-rust-v2`), update `projects/registry.json`, and
retire the old alias only after in-flight jobs drain.
