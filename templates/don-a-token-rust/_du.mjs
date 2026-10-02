import "dotenv/config";
import { Sandbox } from "e2b";

async function main() {
  const sbx = await Sandbox.create("don-a-token-rust-v3", { timeoutMs: 120_000 });
  try {
    const r = await sbx.commands.run(
      "du -sh /usr/local/rustup /usr/local/cargo /usr/lib/node_modules /usr/share/doc /usr/share/man /opt 2>/dev/null; ls /usr/local/rustup/toolchains/"
    );
    console.log(r.stdout);
  } finally {
    await sbx.kill();
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
