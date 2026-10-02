import "dotenv/config";
import { Template, defaultBuildLogger } from "e2b";
import { template } from "./template";

const alias = process.argv[2] ?? "don-a-token-rust";

async function main() {
  await Template.build(template, alias, {
    cpuCount: 2,
    memoryMB: 4096,
    onBuildLogs: defaultBuildLogger(),
  });
  console.log(`Template built: ${alias}`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
