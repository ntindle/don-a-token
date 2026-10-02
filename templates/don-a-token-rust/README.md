# don-a-token-rust template

E2B sandbox template for Rust donation jobs: Ubuntu 24.04 + Rust stable
+ GitHub CLI + Codex CLI, running as non-root `user`.

## Build

```sh
cd templates/don-a-token-rust
cp .env.example .env   # fill in E2B_API_KEY (and E2B_API_URL for Embed)
npm install
npm run build          # builds alias `don-a-token-rust`
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
