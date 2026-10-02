import { load } from "@tauri-apps/plugin-store";
import { isTauri } from "./tauri";

export interface QuietHours {
  startHour: number;
  endHour: number;
}

export interface DonationRules {
  enabled: boolean;
  maxPctOfRemaining: number;
  reserveFloorPct: number;
  maxMinutesPerJob: number;
  cooldownMinutesBetweenJobs: number;
  quietHours: QuietHours | null;
  pausedUntil: string | null;
  skipThisWeek: boolean;
  skipWeekOf: string | null;
}

export type OnboardingStep = "welcome" | "rules" | "projects" | "done";
export type RunnerBackend =
  | "embed-local"
  | "embed-remote"
  | "e2b-cloud"
  | "docker-local";

export interface Settings {
  rules: DonationRules;
  selectedProjectIds: string[];
  githubUsername: string | null;
  runnerBackend: RunnerBackend;
  embedEndpoint: string;
  /** E2B team API key (Cloud) or Embed install key. TODO: move to OS keychain. */
  e2bApiKey: string | null;
  /** Interim contribution credential for opening PRs. GitHub App tokens are the follow-up. */
  githubToken: string | null;
  seenPlanModal: boolean;
  onboardingStep: OnboardingStep;
}

export const DEFAULT_SETTINGS: Settings = {
  rules: {
    enabled: true,
    maxPctOfRemaining: 10,
    reserveFloorPct: 5,
    maxMinutesPerJob: 30,
    cooldownMinutesBetweenJobs: 15,
    quietHours: null,
    pausedUntil: null,
    skipThisWeek: false,
    skipWeekOf: null,
  },
  selectedProjectIds: [],
  githubUsername: null,
  runnerBackend: "e2b-cloud",
  embedEndpoint: "http://127.0.0.1:3000",
  e2bApiKey: null,
  githubToken: null,
  seenPlanModal: false,
  onboardingStep: "welcome",
};

const STORE_PATH = "settings.json";
const BROWSER_KEY = "don-a-token:settings";

function mergeDefaults(raw: unknown): Settings {
  if (!raw || typeof raw !== "object") return structuredClone(DEFAULT_SETTINGS);
  const s = raw as Partial<Settings>;
  return {
    ...structuredClone(DEFAULT_SETTINGS),
    ...s,
    rules: {
      ...structuredClone(DEFAULT_SETTINGS.rules),
      ...((s.rules ?? {}) as Partial<DonationRules>),
    },
  };
}

export async function loadSettings(): Promise<Settings> {
  if (!isTauri()) {
    try {
      return mergeDefaults(JSON.parse(localStorage.getItem(BROWSER_KEY) ?? "null"));
    } catch {
      return structuredClone(DEFAULT_SETTINGS);
    }
  }
  const store = await load(STORE_PATH);
  return mergeDefaults(await store.get<Settings>("settings"));
}

export async function saveSettings(settings: Settings): Promise<void> {
  if (!isTauri()) {
    localStorage.setItem(BROWSER_KEY, JSON.stringify(settings));
    return;
  }
  const store = await load(STORE_PATH);
  await store.set("settings", settings);
  await store.save();
}

/** ISO week label like "2026-W40" for skip-week matching. */
export function isoWeek(d = new Date()): string {
  const date = new Date(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()));
  const day = date.getUTCDay() || 7;
  date.setUTCDate(date.getUTCDate() + 4 - day);
  const yearStart = new Date(Date.UTC(date.getUTCFullYear(), 0, 1));
  const week = Math.ceil(((+date - +yearStart) / 86400000 + 1) / 7);
  return `${date.getUTCFullYear()}-W${String(week).padStart(2, "0")}`;
}
