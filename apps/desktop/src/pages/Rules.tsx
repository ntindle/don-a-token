import PlanBadge from "../components/PlanBadge";
import { isoWeek, type Settings } from "../lib/settings";

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
          Don-a-Token only spends capacity inside these bounds — and stops
          immediately on any usage-limit error.
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
          <span>Donate at most this much of remaining usage</span>
          <span className="input-suffix">
            <input
              type="number"
              min={1}
              max={100}
              value={r.maxPctOfRemaining}
              onChange={(e) =>
                set({ maxPctOfRemaining: num(e.target.value, 10) })
              }
            />
            %
          </span>
        </label>

        <label className="row">
          <span>Always leave me at least this much</span>
          <span className="input-suffix">
            <input
              type="number"
              min={0}
              max={100}
              value={r.reserveFloorPct}
              onChange={(e) => set({ reserveFloorPct: num(e.target.value, 5) })}
            />
            %
          </span>
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
