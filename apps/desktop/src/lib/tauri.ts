import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";

/** True when running inside the Tauri webview (vs plain browser dev). */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Stable per-host id, persisted by the Rust shell. Dev fallback included. */
export async function getHostId(): Promise<string> {
  if (!isTauri()) return "dev-host";
  return invoke<string>("get_host_id");
}

/** Tray menu actions arrive as these events; no-op in plain browser. */
export async function onTrayAction(
  event: "tray:pause" | "tray:skip-week",
  handler: () => void,
): Promise<() => void> {
  if (!isTauri()) return () => {};
  const unlisten = await listen(event, handler);
  return unlisten;
}

export interface SignInResult {
  email: string;
  client_id: string;
  has_plan_usage: boolean;
}

/**
 * SIWC sign-in. Omit `clientId` to register a new ChatGPT account;
 * pass a saved id to reauthorize it.
 */
export async function startSignIn(clientId?: string): Promise<SignInResult> {
  if (!isTauri()) throw new Error("browser-dev");
  return invoke<SignInResult>("start_sign_in", {
    clientId: clientId ?? null,
  });
}

export async function openExternal(url: string): Promise<void> {
  if (!isTauri()) {
    window.open(url, "_blank", "noopener");
    return;
  }
  await openUrl(url);
}

export interface DeviceStart {
  device_code: string;
  user_code: string;
  verification_uri: string;
  verification_uri_complete: string | null;
  expires_in: number;
  interval_secs: number;
}

export type DevicePoll =
  | { status: "pending" }
  | { status: "slow_down" }
  | { status: "done"; access_token: string }
  | { status: "expired" }
  | { status: "denied" };

/** Start a GitHub device flow; returns the user code to display. */
export async function githubDeviceStart(): Promise<DeviceStart> {
  if (!isTauri()) throw new Error("browser-dev");
  return invoke<DeviceStart>("github_device_start");
}

/** One device-flow poll; the caller paces calls by the start interval. */
export async function githubDevicePoll(
  deviceCode: string,
): Promise<DevicePoll> {
  if (!isTauri()) throw new Error("browser-dev");
  return invoke<DevicePoll>("github_device_poll", { deviceCode });
}

export const MANAGE_USAGE_URL = "https://chatgpt.com/settings/usage";

export interface RunnerConfig {
  backend: string;
  api_base: string;
  api_key: string | null;
  sandbox_base: string | null;
}

export interface SubmitJob {
  project_id: string;
  job_id: string;
  template: string;
  prompt_pack: string;
  command: string[];
  env: Array<[string, string]>;
  max_minutes: number;
}

export interface JobStatus {
  status: string;
  sandbox_id: string;
  exit_code: number | null;
  stdout_tail: string;
  stderr_tail: string;
}

export async function jobSubmit(
  config: RunnerConfig,
  job: SubmitJob,
): Promise<{ id: string }> {
  if (!isTauri()) throw new Error("browser-dev");
  return invoke("job_submit", { config, job });
}

export async function jobStatus(handleId: string): Promise<JobStatus> {
  if (!isTauri()) throw new Error("browser-dev");
  return invoke("job_status", { handleId });
}

export async function jobCancel(handleId: string): Promise<void> {
  if (!isTauri()) throw new Error("browser-dev");
  await invoke("job_cancel", { handleId });
}
