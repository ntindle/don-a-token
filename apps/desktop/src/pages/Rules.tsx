import PlanBadge from "../components/PlanBadge";
import { isoWeek, type Settings } from "../lib/settings";
import { MANAGE_USAGE_URL, openExternal } from "../lib/tauri";

interface Props {
  settings: Settings;
  onChange: (s: Settings) => void;
  onBack: () => void;
  onNext: () => void;
}

function num(v: string, fallback: number): number {
  const n = Number.parseInt(v, 10);
  return Number.isFinite(n) ? n : fallback;
}

export default function Rules({ settings, onChange, onBack, onNext }: Props) {
  const r = settings.rules;
  const set = (patch: Partial<typeof r>) =>
    onChange({ ...settings, rules: { ...r, ...patch } });

  const paused = r.pausedUntil !== null && new Date(r.pausedUntil) > new Date();

  return (
    <section className="page">
      <header className="page-head">
        <h1>Configure your rules</h1>
        <p className="lede">
          Don-a-Token only runs jobs inside these bounds. Cap plan
          spending with a per-app percent max in ChatGPT settings.
        </p>
      </header>

      <div className="card">
        <label className="row">
          <span>Enable donations</span>
          <input
            type="checkbox"
            checked={r.enabled}
            onChange={(e) => set({ enabled: e.target.checked })}
          />
        </label>

        <label className="row">
          <span>Max minutes per job</span>
          <input
            type="number"
            min={5}
            max={240}
            value={r.maxMinutesPerJob}
            onChange={(e) => set({ maxMinutesPerJob: num(e.target.value, 30) })}
          />
        </label>

        <label className="row">
          <span>Cooldown between jobs (minutes)</span>
          <input
            type="number"
            min={0}
            max={1440}
            value={r.cooldownMinutesBetweenJobs}
            onChange={(e) =>
              set({ cooldownMinutesBetweenJobs: num(e.target.value, 15) })
            }
          />
        </label>
      </div>

      <div className="card">
        <h2>ChatGPT plan cap</h2>
        <p className="fineprint">
          Set a per-app percent max in ChatGPT settings — that&apos;s the
          enforced limit on plan spending. This app can&apos;t see your
          usage, so it can&apos;t cap a percent itself.
        </p>
        <p>
          <a
            href={MANAGE_USAGE_URL}
            onClick={(e) => {
              e.preventDefault();
              void openExternal(MANAGE_USAGE_URL);
            }}
          >
            Manage usage
          </a>
        </p>
      </div>

      <div className="card">
        <h2>Pause &amp; quiet time</h2>
        <div className="button-row">
          {paused ? (
            <button className="btn" onClick={() => set({ pausedUntil: null })}>
              Resume (paused until{" "}
              {new Date(r.pausedUntil!).toLocaleString()})
            </button>
          ) : (
            <button
              className="btn"
              onClick={() =>
                set({
                  pausedUntil: new Date(
                    Date.now() + 24 * 3600 * 1000,
                  ).toISOString(),
                })
              }
            >
              Pause for 24 hours
            </button>
          )}
          <button
            className="btn"
            onClick={() =>
              set(
                r.skipThisWeek
                  ? { skipThisWeek: false, skipWeekOf: null }
                  : { skipThisWeek: true, skipWeekOf: isoWeek() },
              )
            }
          >
            {r.skipThisWeek ? "Un-skip this week" : "Skip this week"}
          </button>
        </div>
        <p className="fineprint">
          Pause and skip are also in the tray menu, so you never have to open
          the app to stop donations.
        </p>
      </div>

      <PlanBadge />

      <nav className="nav-row">
        <button className="btn" onClick={onBack}>
          Back
        </button>
        <button className="btn btn-primary" onClick={onNext}>
          Pick your projects
        </button>
      </nav>
    </section>
  );
}
