import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { presenceApplyReducedMotion } from "./presence";
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

// One calm, consistent crossfade for every view change: fade + blur + settle
// the outgoing view out, then bring the incoming view in the same way. Kept
// in one place so splash → settings → core never snaps or double-animates.
const FADE_MS = 420;

function Splash() {
  return (
    <section className="splash" aria-busy="true" aria-label="Starting">
      <p className="brand splash-brand">Ralleh</p>
      <p className="splash-status">Opening edge…</p>
    </section>
  );
}

function shellClassFor(view: View): string {
  return view === "settings" ? "shell shell-setup" : "shell";
}

function App() {
  const [view, setView] = useState<View>("splash");
  const [renderedView, setRenderedView] = useState<View>("splash");
  const [faded, setFaded] = useState(true);
  const [settings, setSettings] = useState<EdgeSettings | null>(null);
  const [settingsRequired, setSettingsRequired] = useState(true);

  // Bring the very first view in gently instead of popping in at full paint.
  useEffect(() => {
    const raf = requestAnimationFrame(() => setFaded(false));
    return () => cancelAnimationFrame(raf);
  }, []);

  // Phase 4 kickoff — OS accessibility preference drives the presence's
  // reduced-motion state for the session. Uses the non-persisting
  // `presenceApplyReducedMotion` so the OS pref layers over the runtime
  // without silently rewriting a user's explicit dev-panel toggle: their
  // stored value survives restart, while the OS pref reapplies on top of
  // it every boot. Subscribes to media-query changes so an in-session
  // accessibility flip (e.g. turning "Reduce motion" on in system
  // settings) is honored immediately.
  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const apply = (reduce: boolean) => {
      presenceApplyReducedMotion(reduce).catch(() => {
        // Presence may be disabled or not spawned yet — the persisting
        // startup path in Rust's `restore_presence_state` still runs,
        // so this session-only apply failing is not fatal. Swallowed
        // rather than surfaced because the user did not initiate it.
      });
    };
    apply(mq.matches);
    const listener = (e: MediaQueryListEvent) => apply(e.matches);
    mq.addEventListener("change", listener);
    return () => mq.removeEventListener("change", listener);
  }, []);

  // When the target view changes, fade the current one out, swap the
  // rendered content while invisible, then fade the new one in.
  useEffect(() => {
    if (view === renderedView) return;
    setFaded(true);
    const swap = window.setTimeout(() => {
      setRenderedView(view);
      requestAnimationFrame(() => setFaded(false));
    }, FADE_MS);
    return () => window.clearTimeout(swap);
  }, [view, renderedView]);

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

  return (
    <main className={shellClassFor(renderedView)}>
      <div className="atmosphere" aria-hidden="true" />
      <div className={faded ? "view-fade is-hidden" : "view-fade"}>
        {renderedView === "splash" && <Splash />}
        {renderedView === "settings" && (
          <SettingsView
            required={settingsRequired}
            initial={settings}
            onComplete={onSettingsComplete}
            onCancel={settingsRequired ? undefined : onSettingsCancel}
          />
        )}
        {renderedView === "core" && settings && (
          <Core settings={settings} onOpenSettings={openSettings} />
        )}
      </div>
    </main>
  );
}

export default App;
