import { invoke } from "@tauri-apps/api/core";
import { isTauri, type RunnerConfig } from "./tauri";
import type { Settings } from "./settings";
import registry from "../../../../projects/registry.json";

interface SchedulerProject {
  id: string;
  template: string;
  command: string[];
  maxMinutes: number;
}

interface SchedulerConfig {
  rules: Settings["rules"];
  projects: SchedulerProject[];
  runner: RunnerConfig;
}

/**
 * Probe command until prompt packs land: proves the full loop
 * (verdict → submit → sandbox → output → log) without needing a
 * real job template.
 */
function probeCommand(projectId: string): string[] {
  return ["sh", "-c", `echo don-a-token probe for ${projectId}`];
}

export function buildSchedulerConfig(settings: Settings): SchedulerConfig {
  const byId = new Map(registry.projects.map((p) => [p.id, p]));
  const projects: SchedulerProject[] = settings.selectedProjectIds.flatMap(
    (id) => {
      const p = byId.get(id);
      if (!p || p.status !== "active") return [];
      return [
        {
          id: p.id,
          template: p.job.template,
          command: probeCommand(p.id),
          maxMinutes: Math.min(
            p.job.max_minutes_per_job,
            settings.rules.maxMinutesPerJob,
          ),
        },
      ];
    },
  );

  let runner: RunnerConfig;
  switch (settings.runnerBackend) {
    case "embed-local":
      runner = {
        api_base: "http://127.0.0.1:3000",
        api_key: settings.e2bApiKey,
        sandbox_base: null,
      };
      break;
    case "embed-remote":
      runner = {
        api_base: settings.embedEndpoint,
        api_key: settings.e2bApiKey,
        sandbox_base: null,
      };
      break;
    case "e2b-cloud":
      runner = {
        api_base: "https://api.e2b.dev",
        api_key: settings.e2bApiKey,
        sandbox_base: null,
      };
      break;
    case "docker-local":
      // No local-container backend yet: sync with no projects so the
      // scheduler idles instead of running against a stale config.
      return { rules: settings.rules, projects: [], runner: {
        api_base: "http://127.0.0.1:3000",
        api_key: null,
        sandbox_base: null,
      } };
  }
  return { rules: settings.rules, projects, runner };
}

/** Push settings to the background scheduler. No-op outside Tauri. */
export async function pushScheduler(settings: Settings): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("scheduler_sync", { config: buildSchedulerConfig(settings) });
  } catch (e) {
    console.warn("scheduler sync failed", e);
  }
}
