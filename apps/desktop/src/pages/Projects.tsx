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
  onNext: () => void;
}

export default function Projects({
  settings,
  onChange,
  onBack,
  finished,
  onNext,
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
        <h1>{finished ? "Projects" : "Pick your projects"}</h1>
        <p className="lede">
          {finished
            ? "Jobs run one at a time inside your rules."
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
              Requires: {p.required_providers.map((r) => r.kind).join(", ")}
              {" · "}Work:{" "}
              {p.work_categories.map((c) => `${c.label} (${c.tier})`).join(", ")}
            </p>
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
            onClick={onNext}
            disabled={selected.size === 0}
          >
            Continue
          </button>
        ) : (
          <button className="btn" onClick={onBack}>
            Back to status
          </button>
        )}
      </nav>
    </section>
  );
}
