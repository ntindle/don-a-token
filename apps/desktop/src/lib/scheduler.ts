import { invoke } from "@tauri-apps/api/core";
import { isTauri, type RunnerConfig } from "./tauri";
import type { Settings } from "./settings";
import registry from "../../../../projects/registry.json";
import phasePrompt from "../../../../prompt-packs/phase/prompt.md?raw";

const PROMPT_PACKS: Record<string, string> = { phase: phasePrompt };

interface SchedulerProject {
  id: string;
  template: string;
  repo: string;
  baseBranch: string;
  prompt: string;
  checks: string[];
  promptPack: string;
  maxMinutes: number;
  localToolchain: string | null;
}

interface SchedulerConfig {
  rules: Settings["rules"];
  projects: SchedulerProject[];
  runner: RunnerConfig;
  donor: string | null;
  githubToken: string | null;
  workdirRetention: string;
}

export function buildSchedulerConfig(settings: Settings): SchedulerConfig {
  const byId = new Map(registry.projects.map((p) => [p.id, p]));
  const projects: SchedulerProject[] = settings.selectedProjectIds.flatMap(
    (id) => {
      const p = byId.get(id);
      if (!p || p.status !== "active") return [];
      const prompt = PROMPT_PACKS[p.job.prompt_pack];
      if (!prompt) return [];
      return [
        {
          id: p.id,
          template: p.job.template,
          repo: p.repo,
          baseBranch: p.job.contribution.base_branch,
          prompt,
          checks: p.job.checks,
          promptPack: p.job.prompt_pack,
          localToolchain: p.job.local_toolchain ?? null,
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
        backend: "embed-local",
        api_base: "http://127.0.0.1:3000",
        api_key: settings.e2bApiKey,
        sandbox_base: null,
      };
      break;
    case "embed-remote":
      runner = {
        backend: "embed-remote",
        api_base: settings.embedEndpoint,
        api_key: settings.e2bApiKey,
        sandbox_base: null,
      };
      break;
    case "e2b-cloud":
      runner = {
        backend: "e2b-cloud",
        api_base: "https://api.e2b.dev",
        api_key: settings.e2bApiKey,
        sandbox_base: null,
      };
      break;
    case "local":
      runner = {
        backend: "local",
        api_base: "",
        api_key: null,
        sandbox_base: null,
      };
      break;
    case "docker-local":
      // No local-container backend yet: sync with no projects so the
      // scheduler idles instead of running against a stale config.
      return {
        rules: settings.rules,
        projects: [],
        runner: {
          backend: "docker-local",
          api_base: "http://127.0.0.1:3000",
          api_key: null,
          sandbox_base: null,
        },
        donor: settings.githubUsername,
        githubToken: settings.githubToken,
        workdirRetention: settings.workdirRetention,
      };
  }
  return {
    rules: settings.rules,
    projects,
    runner,
    donor: settings.githubUsername,
    githubToken: settings.githubToken,
    workdirRetention: settings.workdirRetention,
  };
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
