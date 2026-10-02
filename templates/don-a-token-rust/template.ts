import { Template } from "e2b";

/**
 * don-a-token-rust: Rust toolchain + GitHub-less job deps + Codex CLI.
 *
 * Base is the official `rust` image (not plain Ubuntu): the builder
 * disk scales with the base image (~809M for ubuntu:24.04 vs ~3.2G
 * for rust:1-bookworm), and only the Rust base fits the toolchain.
 * No GitHub CLI: PRs are published from the host, so GitHub
 * credentials never enter the sandbox. Build steps run as `user` by
 * default; privileged installs switch to root explicitly and switch
 * back. All runCmd bodies must stay POSIX-sh compatible.
 */
export const template = Template()
  .fromImage("rust:1-bookworm")
  // Sandboxes don't inherit the image's ENV, so re-export the toolchain
  // locations (without these, rustup finds no default toolchain).
  .setEnvs({
    RUSTUP_HOME: "/usr/local/rustup",
    CARGO_HOME: "/usr/local/cargo",
  })
  .aptInstall(["git", "ca-certificates", "gnupg"])
  .setUser("root")
  .runCmd(
    "curl -fsSL https://deb.nodesource.com/setup_22.x | bash -" +
      " && apt-get install -y --no-install-recommends nodejs" +
      " && apt-get clean && rm -rf /var/lib/apt/lists/*" +
      " && npm install -g @openai/codex && npm cache clean --force",
  )
  .runCmd(
    "TC=$(echo /usr/local/rustup/toolchains/*/bin/cargo)" +
      " && test -x \"$TC\"" +
      " && ln -sf \"$TC\" /usr/local/bin/cargo" +
      " && ln -sf \"$(dirname \"$TC\")/rustc\" /usr/local/bin/rustc",
  )
  .setUser("user")
  .runCmd(
    "env -u RUSTUP_HOME -u CARGO_HOME cargo --version" +
      " && env -u RUSTUP_HOME -u CARGO_HOME rustc --version" +
      " && git --version && codex --version",
  );
