import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Core } from "./Core";
import { SettingsView } from "./SettingsView";
import {
  DEFAULT_SETTINGS,
  EdgeSettings,
  EdgeSettingsResponse,
  isSettingsComplete,
  settingsFromResponse,
} from "./settings";
import "./App.css";

type View = "splash" | "settings" | "core";

function Splash() {
  return (
    <section className="splash" aria-busy="true" aria-label="Starting">
      <p className="brand splash-brand">Ralleh</p>
      <p className="splash-status">Opening edge…</p>
    </section>
  );
}

function App() {
  const [view, setView] = useState<View>("splash");
  const [settings, setSettings] = useState<EdgeSettings | null>(null);
  const [settingsRequired, setSettingsRequired] = useState(true);

  useEffect(() => {
    let cancelled = false;
    const started = Date.now();

    (async () => {
      let loaded = DEFAULT_SETTINGS;
      let complete = false;
      try {
        const raw = await invoke<EdgeSettingsResponse>("load_edge_settings");
        loaded = { ...DEFAULT_SETTINGS, ...settingsFromResponse(raw) };
        complete = raw.setupComplete;
      } catch {
        loaded = DEFAULT_SETTINGS;
        complete = isSettingsComplete(loaded);
      }

      const minSplashMs = 700;
      const wait = Math.max(0, minSplashMs - (Date.now() - started));
      await new Promise((r) => setTimeout(r, wait));

      if (cancelled) return;

      setSettings(loaded);
      if (complete) {
        setSettingsRequired(false);
        setView("core");
      } else {
        setSettingsRequired(true);
        setView("settings");
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  function openSettings() {
    setSettingsRequired(false);
    setView("settings");
  }

  function onSettingsComplete(next: EdgeSettings) {
    setSettings(next);
    setSettingsRequired(false);
    setView("core");
  }

  function onSettingsCancel() {
    if (settings && isSettingsComplete(settings)) {
      setView("core");
    }
  }

  const shellClass =
    view === "settings" ? "shell shell-setup" : "shell";

  return (
    <main className={shellClass}>
      <div className="atmosphere" aria-hidden="true" />
      {view === "splash" && <Splash />}
      {view === "settings" && (
        <SettingsView
          required={settingsRequired}
          initial={settings}
          onComplete={onSettingsComplete}
          onCancel={settingsRequired ? undefined : onSettingsCancel}
        />
      )}
      {view === "core" && settings && (
        <Core settings={settings} onOpenSettings={openSettings} />
      )}
    </main>
  );
}

export default App;
