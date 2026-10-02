import { Template } from "e2b";

/**
 * don-a-token-rust: Ubuntu + Rust toolchain + Codex CLI.
 *
 * No GitHub CLI: PRs are published from the host, so GitHub
 * credentials never enter the sandbox. Build steps run as `user` by
 * default; privileged installs switch to root explicitly and switch
 * back. Rust installs system-wide under /opt/rust with symlinks on
 * PATH so `user` jobs just work. All runCmd bodies must stay
 * POSIX-sh compatible.
 */
export const template = Template()
  .fromUbuntuImage("24.04")
  .aptInstall([
    "curl",
    "git",
    "build-essential",
    "pkg-config",
    "libssl-dev",
    "ca-certificates",
    "gnupg",
  ])
  .setUser("root")
  .runCmd(
    "curl -fsSL https://deb.nodesource.com/setup_22.x | bash -" +
      " && apt-get install -y nodejs" +
      " && npm install -g @openai/codex",
  )
  .runCmd(
    "curl -sS https://sh.rustup.rs | RUSTUP_HOME=/opt/rust/rustup CARGO_HOME=/opt/rust/cargo" +
      " sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path" +
      " && ln -sf /opt/rust/cargo/bin/cargo /usr/local/bin/cargo" +
      " && ln -sf /opt/rust/cargo/bin/rustc /usr/local/bin/rustc" +
      " && ln -sf /opt/rust/cargo/bin/rustup /usr/local/bin/rustup" +
      " && chmod -R a+rX /opt/rust",
  )
  .setUser("user")
  .runCmd("cargo --version && rustc --version && git --version && codex --version");
