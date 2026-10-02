import { Template } from "e2b";

/**
 * don-a-token-rust: pinned nightly Rust + sccache + git + Codex CLI.
 *
 * Base is the official `rust` image (not plain Ubuntu): the builder
 * disk scales with the base image (~809M for ubuntu:24.04 vs ~3.2G
 * for rust:1-bookworm), and only the Rust base fits the toolchain.
 * No GitHub CLI: PRs are published from the host, so GitHub
 * credentials never enter the sandbox. Build steps run as `user` by
 * default; privileged installs switch to root explicitly and switch
 * back. All runCmd bodies must stay POSIX-sh compatible.
 *
 * Toolchain: nightly-2026-04-19 (phase's pin), minimal profile +
 * rustfmt + clippy. Cranelift/wasm are skipped: phase builds the llvm
 * backend for the host target, and the 3.2G template disk is the
 * binding constraint (~640M must cover repo + registry + target/).
 * Stable is deleted outright to reclaim disk. Real toolchain binaries
 * are linked onto PATH because sandboxes don't inherit the image ENV,
 * so rustup shims resolve nothing there.
 */
const NIGHTLY = "nightly-2026-04-19";
const SCCACHE = "0.18.0";

export const template = Template()
  .fromImage("rust:1-bookworm")
  // Sandboxes don't inherit the image's ENV, so re-export the toolchain
  // locations for anything that still consults them.
  .setEnvs({
    RUSTUP_HOME: "/usr/local/rustup",
    CARGO_HOME: "/usr/local/cargo",
  })
  .aptInstall(["git", "ca-certificates", "gnupg"])
  .setUser("root")
  .runCmd(
    `export RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo` +
      ` && /usr/local/cargo/bin/rustup toolchain install ${NIGHTLY} --profile minimal` +
      " -c rustfmt -c clippy" +
      ` && /usr/local/cargo/bin/rustup default ${NIGHTLY}` +
      " && rm -rf /usr/local/rustup/toolchains/1.*" +
      " /usr/local/rustup/downloads /usr/local/rustup/tmp",
  )
  .runCmd(
    "curl -fsSL https://deb.nodesource.com/setup_22.x | bash -" +
      " && apt-get install -y --no-install-recommends nodejs" +
      " && apt-get clean && rm -rf /var/lib/apt/lists/*" +
      " && npm install -g @openai/codex && npm cache clean --force" +
      " && rm -rf /usr/lib/node_modules/npm /usr/lib/node_modules/corepack" +
      " /usr/share/doc /usr/share/man",
  )
  .runCmd(
    `curl -fsSL -o /tmp/sccache.tar.gz https://github.com/mozilla/sccache/releases/download/v${SCCACHE}/sccache-v${SCCACHE}-x86_64-unknown-linux-musl.tar.gz` +
      ` && curl -fsSL -o /tmp/sccache.sha256 https://github.com/mozilla/sccache/releases/download/v${SCCACHE}/sccache-v${SCCACHE}-x86_64-unknown-linux-musl.tar.gz.sha256` +
      " && echo \"$(cut -d' ' -f1 /tmp/sccache.sha256)  /tmp/sccache.tar.gz\" | sha256sum -c -" +
      " && tar -xzf /tmp/sccache.tar.gz -C /tmp" +
      " && install -m755 /tmp/sccache-*/sccache /usr/local/bin/sccache" +
      " && rm -rf /tmp/sccache*",
  )
  .runCmd(
    "TC=$(echo /usr/local/rustup/toolchains/nightly-*/bin/cargo)" +
      ' && test -x "$TC"' +
      ' && ln -sf "$TC" /usr/local/bin/cargo' +
      ' && ln -sf "$(dirname "$TC")/rustc" /usr/local/bin/rustc',
  )
  .setUser("user")
  .runCmd(
    "env -u RUSTUP_HOME -u CARGO_HOME cargo --version" +
      " && env -u RUSTUP_HOME -u CARGO_HOME rustc --version" +
      " && git --version && codex --version && sccache --version" +
      " && test $(df -m / --output=avail | tail -1) -ge 700",
  );
