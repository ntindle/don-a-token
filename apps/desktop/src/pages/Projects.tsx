import PlanBadge from "../components/PlanBadge";
import { openExternal } from "../lib/tauri";
import type { Settings } from "../lib/settings";
// Bundled copy of the approved registry. The app refreshes from the latest
// release artifact; see projects/README.md.
import registry from "../../../../projects/registry.json";

interface Props {
  settings: Settings;
  onChange: (s: Settings) => void;
  onBack: () => void;
  finished: boolean;
  onFinish: () => void;
}

export default function Projects({
  settings,
  onChange,
  onBack,
  finished,
  onFinish,
}: Props) {
  const selected = new Set(settings.selectedProjectIds);
  const toggle = (id: string) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    void onChange({ ...settings, selectedProjectIds: [...next] });
  };

  return (
    <section className="page">
      <header className="page-head">
        <h1>{finished ? "Donations" : "Pick your projects"}</h1>
        <p className="lede">
          {finished
            ? "You're set. Jobs run inside isolated sandboxes within your rules."
            : "Every listing is manually approved. Pick any number — jobs run one at a time inside your rules."}
        </p>
      </header>

      <ul className="project-list">
        {registry.projects.map((p) => (
          <li key={p.id} className="card project">
            <label className="project-head">
              <input
                type="checkbox"
                checked={selected.has(p.id)}
                onChange={() => toggle(p.id)}
              />
              <span className="project-name">{p.name}</span>
              <span className={`status status-${p.status}`}>{p.status}</span>
            </label>
            <p>{p.description}</p>
            <p className="fineprint">
              <a
                href={p.repo}
                onClick={(e) => {
                  e.preventDefault();
                  void openExternal(p.repo);
                }}
              >
                {p.repo}
              </a>
              {" · "}up to {p.job.max_tokens_per_job.toLocaleString()}{" "}
              tokens / {p.job.max_minutes_per_job} min per job
            </p>
          </li>
        ))}
      </ul>

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
                runnerBackend: e.target.value as Settings["runnerBackend"],
              })
            }
          >
            <option value="embed-local">E2B Embed, this machine (Linux)</option>
            <option value="embed-remote">E2B Embed, remote node</option>
            <option value="e2b-cloud">E2B Cloud</option>
            <option value="docker-local">Local containers (coming soon)</option>
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
      </div>

      <div className="card">
        <h2>Contribution identity</h2>
        <p>
          Pull requests are attributed to you. Link your GitHub username so
          jobs can open PRs in your name; the SIWC client id is attached as
          provenance.
        </p>
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
            placeholder="ghp_… (interim)"
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
          Interim: a classic PAT with repo scope so jobs can push branches
          and open PRs. Short-lived per-job tokens via a GitHub App replace
          this next. Without a token, jobs stop after committing locally.
        </p>
      </div>

      <PlanBadge />

      <nav className="nav-row">
        {!finished && (
          <button className="btn" onClick={onBack}>
            Back
          </button>
        )}
        {!finished ? (
          <button
            className="btn btn-primary"
            onClick={onFinish}
            disabled={selected.size === 0}
          >
            Start donating
          </button>
        ) : (
          <button className="btn" onClick={onBack}>
            Edit rules
          </button>
        )}
      </nav>
    </section>
  );
}
