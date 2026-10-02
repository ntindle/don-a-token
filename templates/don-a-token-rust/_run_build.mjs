import "dotenv/config";
import { Template, defaultBuildLogger } from "e2b";
import { template } from "./_run_template.mjs";

const alias = process.argv[2] ?? "don-a-token-rust";
// Small nodes (11GB WSL2, 512 hugepages) build with E2B_BUILD_MEM=384
// E2B_BUILD_CPU=1: step resume transiently needs ~2x builder memory,
// and leaked Rsvd leaves 512MB short (see README).
const cpuCount = parseInt(process.env.E2B_BUILD_CPU ?? "2", 10);
const memoryMB = parseInt(process.env.E2B_BUILD_MEM ?? "4096", 10);
const diskMB = process.env.E2B_BUILD_DISK_MB
  ? parseInt(process.env.E2B_BUILD_DISK_MB, 10)
  : undefined;

async function main() {
  await Template.build(template, alias, {
    cpuCount,
    memoryMB,
    ...(diskMB !== undefined ? { minFreeDiskMb: diskMB } : {}),
    onBuildLogs: defaultBuildLogger(),
  });
  console.log(`Template built: ${alias}`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
