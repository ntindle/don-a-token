import { useCallback, useEffect, useRef, useState } from "react";
import Welcome from "./pages/Welcome";
import Rules from "./pages/Rules";
import Projects from "./pages/Projects";
import FirstRunModal from "./components/FirstRunModal";
import {
  isoWeek,
  loadSettings,
  saveSettings,
  type OnboardingStep,
  type Settings,
} from "./lib/settings";
import { onTrayAction } from "./lib/tauri";

function stepFromHash(): OnboardingStep | null {
  const h = window.location.hash.replace(/^#\/?/, "");
  return h === "welcome" || h === "rules" || h === "projects" ? h : null;
}

export default function App() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [signedIn, setSignedIn] = useState(false);

  useEffect(() => {
    loadSettings().then(setSettings);
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

  if (!settings) {
    return (
      <main className="shell">
        <p>Loading…</p>
      </main>
    );
  }

  const go = (step: OnboardingStep) => {
    window.location.hash = `#/${step}`;
    void update({ ...settings, onboardingStep: step });
  };

  const active: OnboardingStep =
    stepFromHash() ?? (settings.onboardingStep === "done" ? "done" : settings.onboardingStep);

  // First-sign-in plan confirmation, shown exactly once.
  const showPlanModal = signedIn && !settings.seenPlanModal;

  return (
    <main className="shell">
      {showPlanModal && (
        <FirstRunModal
          onDismiss={() => void update({ ...settings, seenPlanModal: true })}
        />
      )}
      {active === "welcome" && (
        <Welcome
          onSignedIn={() => {
            setSignedIn(true);
            go("rules");
          }}
          onExplore={() => go("rules")}
        />
      )}
      {active === "rules" && (
        <Rules
          settings={settings}
          onChange={update}
          onBack={() => go("welcome")}
          onNext={() => go("projects")}
        />
      )}
      {(active === "projects" || active === "done") && (
        <Projects
          settings={settings}
          onChange={update}
          onBack={() => go("rules")}
          finished={active === "done"}
          onFinish={() => void update({ ...settings, onboardingStep: "done" })}
        />
      )}
    </main>
  );
}
