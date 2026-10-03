import { useCallback, useEffect, useState } from "react";
import PlanBadge from "../components/PlanBadge";
import type { Settings } from "../lib/settings";
import {
  isTauri,
  jobCancel,
  schedulerStatus,
  type AccountSummary,
  type SchedulerStatus,
} from "../lib/tauri";
import registry from "../../../../projects/registry.json";

interface Props {
  settings: Settings;
  accounts: AccountSummary[];
  onChange: (s: Settings) => void;
  onNavigate: (view: "rules" | "projects" | "setup" | "welcome") => void;
}

function describeVerdict(v: string): string {
  switch (v) {
    case "unconfigured":
      return "Not configured yet — finish setup to start donating.";
    case "restored":
    case "synced":
    case "submitted":
      return "Starting up…";
    case "job-running":
      return "A job is running.";
    case "no-projects":
      return "No projects selected.";
    case "no-account":
      return "No ChatGPT account with plan usage.";
    case "cooldown":
      return "Cooling down between jobs.";
    case "failure-backoff":
      return "Paused after repeated failures.";
    case "paused":
      return "Paused.";
    case "skip-week":
      return "Skipping this week.";
    case "quiet-hours":
      return "Quiet hours.";
    case "donations-disabled":
      return "Donations are off.";
    default:
      break;
  }
  if (v.startsWith("job-failed:")) return `Last job failed (${v.slice(11).trim()}).`;
  if (v.startsWith("submit-failed:")) return `Couldn't start a job (${v.slice(14).trim()}).`;
  if (v.startsWith("job-")) {
    const s = v.slice(4);
    if (s === "succeeded") return "Last job succeeded.";
    if (s === "no-changes") return "Last job finished with no changes.";
    if (s === "published") return "Last job published a pull request.";
    if (s === "patch-too-large") return "Last job's patch was too large to publish.";
    if (s === "needs-publish") return "Last job is waiting on a contribution token.";
    if (s === "publish-failed") return "Last job couldn't open its pull request.";
    if (s === "no-patch") return "Last job produced no patch.";
    return `Last job: ${s}.`;
  }
  return v;
}

export default function Status({ settings, accounts, onChange, onNavigate }: Props) {
  const [status, setStatus] = useState<SchedulerStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState(false);

  const refresh = useCallback(async () => {
    if (!isTauri()) return;
    try {
      setStatus(await schedulerStatus());
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const t = window.setInterval(() => void refresh(), 10000);
    return () => window.clearInterval(t);
  }, [refresh]);

  const r = settings.rules;
  const paused = r.pausedUntil !== null && new Date(r.pausedUntil) > new Date();
  const setRules = (patch: Partial<typeof r>) =>
    onChange({ ...settings, rules: { ...r, ...patch } });

  async function cancel(handle: string) {
    if (!window.confirm(`Cancel the running job ${handle}?`)) return;
    setCancelling(true);
    try {
      await jobCancel(handle);
      await refresh();
    } catch (e) {
      setError(`Cancel failed: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setCancelling(false);
    }
  }

  const selected = registry.projects.filter((p) =>
    settings.selectedProjectIds.includes(p.id),
  );
  const email = accounts.length > 0 ? accounts[0].email : null;

  let headline: string;
  if (status?.activeJob) headline = `Job ${status.activeJob} is running.`;
  else if (!r.enabled) headline = "Donations are off.";
  else if (paused)
    headline = `Paused until ${new Date(r.pausedUntil!).toLocaleString()}.`;
  else if (status) headline = describeVerdict(status.lastVerdict);
  else headline = "Loading scheduler state…";

  return (
    <section className="page">
      <header className="page-head">
        <h1>Donations</h1>
        <p className="lede">{headline}</p>
      </header>

      <div className="card">
        <h2>Donation status</h2>
        {!isTauri() ? (
          <p className="fineprint">
            Live status is only available in the desktop app.
          </p>
        ) : status ? (
          <>
            <p className="row">
              <span>State</span>
              <span className={`status ${status.activeJob ? "status-running" : "status-idle"}`}>
                {status.activeJob ? "running" : "idle"}
              </span>
            </p>
            {status.activeJob && (
              <p className="row">
                <span>Active job</span>
                <span className="input-suffix">
                  {status.activeJob}{" "}
                  <button
                    className="btn"
                    disabled={cancelling}
                    onClick={() => void cancel(status.activeJob!)}
                  >
                    {cancelling ? "Cancelling…" : "Cancel"}
                  </button>
                </span>
              </p>
            )}
            {!status.activeJob && (
              <p className="row">
                <span>Last update</span>
                <span>{describeVerdict(status.lastVerdict)}</span>
              </p>
            )}
            {status.consecutiveFailures > 0 && (
              <p className="row">
                <span>Consecutive failures</span>
                <span className="error">{status.consecutiveFailures}</span>
              </p>
            )}
          </>
        ) : (
          <p className="fineprint">Loading…</p>
        )}
        {error && <p className="error">{error}</p>}
      </div>

      <div className="card">
        <h2>Controls</h2>
        <label className="row">
          <span>Enable donations</span>
          <input
            type="checkbox"
            checked={r.enabled}
            onChange={(e) => setRules({ enabled: e.target.checked })}
          />
        </label>
        <p className="row">
          <span>{paused ? `Paused until ${new Date(r.pausedUntil!).toLocaleString()}` : "Pause for 24 hours"}</span>
          <button
            className="btn"
            onClick={() =>
              paused
                ? setRules({ pausedUntil: null })
                : setRules({
                    pausedUntil: new Date(Date.now() + 24 * 3600 * 1000).toISOString(),
                  })
            }
          >
            {paused ? "Resume" : "Pause"}
          </button>
        </p>
        <p>
          <button className="btn btn-ghost" onClick={() => onNavigate("rules")}>
            Edit rules
          </button>
        </p>
      </div>

      <div className="card">
        <h2>Projects</h2>
        {selected.length === 0 ? (
          <p className="fineprint">No projects selected — jobs won&apos;t run.</p>
        ) : (
          <ul className="provider-list">
            {selected.map((p) => (
              <li key={p.id}>
                <span className="provider-name">{p.name}</span>
                <span className="provider-note">
                  up to {p.job.max_minutes_per_job} min per job
                </span>
              </li>
            ))}
          </ul>
        )}
        <p>
          <button className="btn btn-ghost" onClick={() => onNavigate("projects")}>
            Edit projects
          </button>
        </p>
      </div>

      <div className="card">
        <h2>Accounts</h2>
        <p className="row">
          <span>ChatGPT</span>
          {email ? (
            <span>{email}</span>
          ) : (
            <button className="btn" onClick={() => onNavigate("welcome")}>
              Sign in
            </button>
          )}
        </p>
        <p className="row">
          <span>GitHub</span>
          <span>{settings.githubUsername ?? "Not linked"}</span>
        </p>
        <p>
          <button className="btn btn-ghost" onClick={() => onNavigate("setup")}>
            Runner &amp; identity
          </button>
        </p>
      </div>

      <PlanBadge />
    </section>
  );
}
