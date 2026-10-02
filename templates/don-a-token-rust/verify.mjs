import "dotenv/config";
import { Sandbox } from "e2b";

// Boot-verify a built template alias: toolchain resolves for `user`
// in a real sandbox (in-builder checks run with the image ENV, which
// sandboxes don't inherit, so only this proves the final state).
const alias = process.argv[2] ?? "don-a-token-rust-v2";

async function main() {
  const sbx = await Sandbox.create(alias, { timeoutMs: 120_000 });
  try {
    const r = await sbx.commands.run(
      "whoami && pwd && cargo --version && rustc --version && git --version && codex --version && df -h / | tail -1"
    );
    console.log("EXIT=" + r.exitCode);
    console.log(r.stdout);
  } finally {
    await sbx.kill();
  }
  console.log("TEMPLATE_VERIFY_OK");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
