import { useCallback, useEffect, useRef, useState } from "react";
import Welcome from "./pages/Welcome";
import Rules from "./pages/Rules";
import Projects from "./pages/Projects";
import Setup from "./pages/Setup";
import Status from "./pages/Status";
import FirstRunModal from "./components/FirstRunModal";
import {
  isoWeek,
  loadSettings,
  saveSettings,
  type OnboardingStep,
  type Settings,
} from "./lib/settings";
import { listen } from "@tauri-apps/api/event";
import {
  MANAGE_USAGE_URL,
  isTauri,
  listAccounts,
  onTrayAction,
  openExternal,
  type AccountSummary,
} from "./lib/tauri";
import { pushScheduler } from "./lib/scheduler";

/** Routable views: onboarding steps plus the post-onboarding dashboard. */
type View = OnboardingStep | "status";

function stepFromHash(): View | null {
  const h = window.location.hash.replace(/^#\/?/, "");
  return h === "welcome" ||
    h === "rules" ||
    h === "projects" ||
    h === "setup" ||
    h === "status"
    ? h
    : null;
}

export default function App() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [accounts, setAccounts] = useState<AccountSummary[] | null>(null);
  const [halted, setHalted] = useState(false);

  useEffect(() => {
    loadSettings().then(setSettings);
  }, []);

  const refreshAccounts = useCallback(async () => {
    try {
      setAccounts(await listAccounts());
    } catch {
      setAccounts([]);
    }
  }, []);

  useEffect(() => {
    void refreshAccounts();
  }, [refreshAccounts]);

  // Push every settings change to the background scheduler.
  useEffect(() => {
    if (settings) void pushScheduler(settings);
  }, [settings]);

  // Scheduler halts after repeated failures; surface it once seen.
  useEffect(() => {
    if (!isTauri()) return;
    let off = () => {};
    void listen("scheduler:halted", () => setHalted(true)).then((u) => {
      off = u;
    });
    return () => off();
  }, []);

  const update = useCallback(async (next: Settings) => {
    setSettings(next);
    await saveSettings(next);
  }, []);

  // Ref mirror so tray handlers always see current settings.
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const updateRef = useRef(update);
  updateRef.current = update;

  // Tray menu actions (Pause donations, Skip this week).
  useEffect(() => {
    let unsubs: Array<() => void> = [];
    let cancelled = false;
    (async () => {
      const offPause = await onTrayAction("tray:pause", () => {
        const s = settingsRef.current;
        if (!s) return;
        void updateRef.current({
          ...s,
          rules: {
            ...s.rules,
            pausedUntil: new Date(Date.now() + 24 * 3600 * 1000).toISOString(),
          },
        });
      });
      const offSkip = await onTrayAction("tray:skip-week", () => {
        const s = settingsRef.current;
        if (!s) return;
        void updateRef.current({
          ...s,
          rules: { ...s.rules, skipThisWeek: true, skipWeekOf: isoWeek() },
        });
      });
      if (cancelled) {
        offPause();
        offSkip();
      } else {
        unsubs = [offPause, offSkip];
      }
    })();
    return () => {
      cancelled = true;
      unsubs.forEach((u) => u());
    };
  }, []);

  if (!settings || !accounts) {
    return (
      <main className="shell">
        <p>Loading…</p>
      </main>
    );
  }

  const done = settings.onboardingStep === "done";
  const signedIn = accounts.length > 0;

  const go = (step: View) => {
    window.location.hash = `#/${step}`;
    // During onboarding the step persists as progress; post-onboarding
    // navigation is ephemeral and must not regress the saved step.
    if (!done && step !== "status") {
      void update({ ...settings, onboardingStep: step });
    }
  };

  // Status-aware routing: signed-in users never land on the sign-in
  // page, and finished users land on the dashboard by default.
  const active: View = (() => {
    const hash = stepFromHash();
    if (done) {
      if (hash === "rules" || hash === "projects" || hash === "setup") return hash;
      if (hash === "welcome" && !signedIn) return "welcome";
      return "status";
    }
    if (hash === "status") return settings.onboardingStep;
    if (hash) {
      if (hash === "welcome" && signedIn) return "rules";
      return hash;
    }
    if (settings.onboardingStep === "welcome" && signedIn) return "rules";
    return settings.onboardingStep;
  })();

  // First-sign-in plan confirmation, shown exactly once.
  const showPlanModal = signedIn && !settings.seenPlanModal;

  const sections: Array<{ view: View; label: string }> = [
    { view: "status", label: "Status" },
    { view: "rules", label: "Rules" },
    { view: "projects", label: "Projects" },
    { view: "setup", label: "Setup" },
  ];

  return (
    <main className="shell">
      {halted && (
        <p className="halt-banner" role="alert">
          Donations paused after repeated job failures. Check the runner
          backend and API key, then change any setting to retry.{" "}
          <a
            href={MANAGE_USAGE_URL}
            onClick={(e) => {
              e.preventDefault();
              void openExternal(MANAGE_USAGE_URL);
            }}
          >
            Manage usage
          </a>{" "}
          <button className="btn btn-ghost" onClick={() => setHalted(false)}>
            Dismiss
          </button>
        </p>
      )}
      {showPlanModal && (
        <FirstRunModal
          onDismiss={() => void update({ ...settings, seenPlanModal: true })}
        />
      )}
      {done && (
        <nav className="top-nav" aria-label="Sections">
          {sections.map((s) => (
            <button
              key={s.view}
              className={`btn btn-ghost${active === s.view ? " active" : ""}`}
              onClick={() => go(s.view)}
            >
              {s.label}
            </button>
          ))}
        </nav>
      )}
      {active === "welcome" && (
        <Welcome
          accounts={accounts}
          onSignedIn={() => {
            void refreshAccounts();
            go(done ? "status" : "rules");
          }}
          onExplore={() => go(done ? "status" : "rules")}
        />
      )}
      {active === "rules" && (
        <Rules
          settings={settings}
          onChange={update}
          settingsMode={done}
          hideBack={!done && signedIn}
          onBack={() => go(done ? "status" : "welcome")}
          onNext={() => go("projects")}
        />
      )}
      {active === "projects" && (
        <Projects
          settings={settings}
          onChange={update}
          onBack={() => go(done ? "status" : "rules")}
          finished={done}
          onNext={() => go("setup")}
        />
      )}
      {active === "setup" && (
        <Setup
          settings={settings}
          onChange={update}
          onBack={() => go(done ? "status" : "projects")}
          onFinish={() => void update({ ...settings, onboardingStep: "done" })}
          settingsMode={done}
        />
      )}
      {active === "status" && (
        <Status
          settings={settings}
          accounts={accounts}
          onChange={update}
          onNavigate={(view) => go(view)}
        />
      )}
    </main>
  );
}
