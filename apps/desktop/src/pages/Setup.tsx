import { useEffect, useRef, useState } from "react";
import PlanBadge from "../components/PlanBadge";
import {
  githubDevicePoll,
  githubDeviceStart,
  openExternal,
  type DeviceStart,
} from "../lib/tauri";
import type { Settings as AppSettings } from "../lib/settings";
import registry from "../../../../projects/registry.json";

interface Props {
  settings: AppSettings;
  onChange: (s: AppSettings) => void;
  onBack: () => void;
  onFinish: () => void;
}

export default function Setup({ settings, onChange, onBack, onFinish }: Props) {
  const [link, setLink] = useState<DeviceStart | null>(null);
  const [linkBusy, setLinkBusy] = useState(false);
  const [linkError, setLinkError] = useState<string | null>(null);
  const [linked, setLinked] = useState(false);
  const timer = useRef<number | null>(null);

  // Ref mirror so the poll loop always saves against current settings.
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  function stopPoll() {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
  }
  useEffect(() => () => stopPoll(), []);

  async function poll(deviceCode: string, intervalSecs: number) {
    try {
      const r = await githubDevicePoll(deviceCode);
      if (r.status === "done") {
        stopPoll();
        setLink(null);
        setLinked(true);
        onChange({ ...settingsRef.current, githubToken: r.access_token });
      } else if (r.status === "pending") {
        timer.current = window.setTimeout(
          () => void poll(deviceCode, intervalSecs),
          intervalSecs * 1000,
        );
      } else if (r.status === "slow_down") {
        const next = intervalSecs + 5;
        timer.current = window.setTimeout(
          () => void poll(deviceCode, next),
          next * 1000,
        );
      } else if (r.status === "expired") {
        stopPoll();
        setLink(null);
        setLinkError("Code expired — try again.");
      } else {
        stopPoll();
        setLink(null);
        setLinkError("Authorization denied in the browser.");
      }
    } catch (e) {
      stopPoll();
      setLink(null);
      setLinkError(e instanceof Error ? e.message : String(e));
    }
  }

  async function startLink() {
    stopPoll();
    setLinkError(null);
    setLinked(false);
    setLinkBusy(true);
    try {
      const s = await githubDeviceStart();
      setLink(s);
      timer.current = window.setTimeout(
        () => void poll(s.device_code, s.interval_secs),
        s.interval_secs * 1000,
      );
    } catch (e) {
      setLinkError(e instanceof Error ? e.message : String(e));
    } finally {
      setLinkBusy(false);
    }
  }

  const verifyUrl = "https://github.com/login/device";

  return (
    <section className="page">
      <header className="page-head">
        <h1>Runner &amp; identity</h1>
        <p className="lede">
          Where jobs execute, and whose name goes on the pull requests.
        </p>
      </header>

      <div className="card">
        <h2>Runner backend</h2>
        <p>
          Where sandboxed jobs execute. Embed runs the E2B stack on hardware
          you control; Cloud needs an API key.
        </p>
        <label className="row">
          <span>Backend</span>
          <select
            value={settings.runnerBackend}
            onChange={(e) =>
              onChange({
                ...settings,
                runnerBackend: e.target.value as AppSettings["runnerBackend"],
              })
            }
          >
            <option value="embed-local">E2B Embed, this machine (Linux)</option>
            <option value="embed-remote">E2B Embed, remote node</option>
            <option value="e2b-cloud">E2B Cloud</option>
            <option value="docker-local">Local containers (coming soon)</option>
            <option value="local">Local machine (no isolation — testing only)</option>
          </select>
        </label>
        {settings.runnerBackend === "embed-remote" && (
          <label className="row">
            <span>Embed API base</span>
            <input
              type="text"
              value={settings.embedEndpoint}
              onChange={(e) =>
                onChange({ ...settings, embedEndpoint: e.target.value.trim() })
              }
            />
          </label>
        )}
        <label className="row">
          <span>E2B API key</span>
          <input
            type="password"
            placeholder="e2b_… (Cloud) or install key (Embed)"
            value={settings.e2bApiKey ?? ""}
            onChange={(e) =>
              onChange({
                ...settings,
                e2bApiKey: e.target.value.trim() || null,
              })
            }
          />
        </label>
        <p className="fineprint">
          Embed mints its install key on first start (see the compose
          output). Keys move to the OS keychain in a follow-up.
        </p>
        <label className="row">
          <span>Job workdir cleanup</span>
          <select
            value={settings.workdirRetention}
            onChange={(e) =>
              onChange({
                ...settings,
                workdirRetention: e.target.value as AppSettings["workdirRetention"],
              })
            }
          >
            <option value="none">Always remove (saves disk)</option>
            <option value="on-failure">Keep failed jobs for debugging</option>
            <option value="always">Keep everything (fills disk fast)</option>
          </select>
        </label>
        <p className="fineprint">
          Local backend only: each job leaves a repo clone plus build
          artifacts (several GB). E2B sandboxes are always destroyed
          after the run.
        </p>
        <label className="row">
          <span>Custom cleanup command</span>
          <input
            type="text"
            placeholder='e.g. rm -rf "$JOB_WORKDIR"'
            value={settings.cleanupCommand ?? ""}
            onChange={(e) =>
              onChange({
                ...settings,
                cleanupCommand: e.target.value.trim() || null,
              })
            }
          />
        </label>
        <p className="fineprint">
          Optional shell command run after each local job with the job
          dir in $JOB_WORKDIR. When empty, the retention choice above
          applies. Failures are logged, never fail the job.
        </p>
        {settings.selectedProjectIds.map((id) => {
          const p = registry.projects.find((q) => q.id === id);
          const notes = (p?.job as { maintainer_notes?: string } | undefined)
            ?.maintainer_notes;
          if (!p || !notes) return null;
          return (
            <p key={id} className="fineprint">
              <strong>{p.name} maintainer notes:</strong> {notes}
            </p>
          );
        })}
      </div>

      <div className="card">
        <h2>Contribution identity</h2>
        <p>
          Pull requests are attributed to you. Link your GitHub username so
          jobs can open PRs in your name; the SIWC client id is attached as
          provenance.
        </p>
        {!link ? (
          <p>
            <button
              className="btn"
              onClick={() => void startLink()}
              disabled={linkBusy}
            >
              {linkBusy
                ? "Requesting code…"
                : settings.githubToken
                  ? "Re-link GitHub"
                  : "Link GitHub"}
            </button>
          </p>
        ) : (
          <div>
            <p>
              Enter code <strong>{link.user_code}</strong> at{" "}
              <a
                href={link.verification_uri_complete ?? link.verification_uri}
                onClick={(e) => {
                  e.preventDefault();
                  void openExternal(
                    link.verification_uri_complete ?? link.verification_uri,
                  );
                }}
              >
                {verifyUrl}
              </a>
            </p>
            <p className="fineprint">Waiting for browser approval…</p>
            <p>
              <button
                className="btn btn-ghost"
                onClick={() => {
                  stopPoll();
                  setLink(null);
                }}
              >
                Cancel
              </button>
            </p>
          </div>
        )}
        {linked && <p className="fineprint">GitHub linked.</p>}
        {linkError && <p className="error">{linkError}</p>}
        <label className="row">
          <span>GitHub username</span>
          <input
            type="text"
            placeholder="octocat"
            value={settings.githubUsername ?? ""}
            onChange={(e) =>
              onChange({
                ...settings,
                githubUsername: e.target.value.trim() || null,
              })
            }
          />
        </label>
        <label className="row">
          <span>Contribution token</span>
          <input
            type="password"
            placeholder="ghp_… (or link above)"
            value={settings.githubToken ?? ""}
            onChange={(e) =>
              onChange({
                ...settings,
                githubToken: e.target.value.trim() || null,
              })
            }
          />
        </label>
        <p className="fineprint">
          Link GitHub above to mint a token via device code, like gh auth
          login — or paste a classic PAT with repo scope manually. Without
          a token, jobs stop after committing locally.
        </p>
      </div>

      <PlanBadge />

      <nav className="nav-row">
        <button className="btn" onClick={onBack}>
          Back
        </button>
        <button
          className="btn btn-primary"
          onClick={onFinish}
          disabled={settings.selectedProjectIds.length === 0}
        >
          Start donating
        </button>
      </nav>
    </section>
  );
}
