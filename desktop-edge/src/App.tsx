import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Setup } from "./Setup";
import "./App.css";

type CoreStatus = {
  product: string;
  edge: string;
  version: string;
  message: string;
};

type VoiceSmoke = {
  transcript: string;
  ttsSamples: number;
  sampleRateHz: number;
};

type ClipboardSmoke = {
  backend: string;
  written: string;
  readBack: string;
  tenantId: string;
  deviceId: string;
  actorId: string;
  policyOutcome: string;
  policyRuleId: string | null;
  policyReason: string;
};

type View = "home" | "setup";

function Home({ onOpenSetup }: { onOpenSetup: () => void }) {
  const [status, setStatus] = useState<CoreStatus | null>(null);
  const [voice, setVoice] = useState<VoiceSmoke | null>(null);
  const [clipboard, setClipboard] = useState<ClipboardSmoke | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<"ping" | "voice" | "clipboard" | null>(
    null,
  );

  async function pingCore() {
    setBusy("ping");
    setError(null);
    try {
      setStatus(await invoke<CoreStatus>("core_ping"));
    } catch (e) {
      setStatus(null);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  }

  async function runVoiceSmoke() {
    setBusy("voice");
    setError(null);
    try {
      setVoice(await invoke<VoiceSmoke>("voice_smoke"));
    } catch (e) {
      setVoice(null);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  }

  async function runClipboardSmoke() {
    setBusy("clipboard");
    setError(null);
    try {
      setClipboard(await invoke<ClipboardSmoke>("clipboard_smoke"));
    } catch (e) {
      setClipboard(null);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="hero">
      <p className="brand">Ralleh</p>
      <h1 className="headline">Your private operator at the edge.</h1>
      <p className="lede">
        Desktop shell Phase 1 — prove the Rust core, run a mock voice pass,
        then file your station log.
      </p>
      <div className="actions">
        <button
          type="button"
          className="cta"
          onClick={pingCore}
          disabled={busy !== null}
        >
          {busy === "ping" ? "Contacting core…" : "Ping Rust core"}
        </button>
        <button
          type="button"
          className="cta secondary"
          onClick={runVoiceSmoke}
          disabled={busy !== null}
        >
          {busy === "voice" ? "Running pipeline…" : "Voice smoke (mock)"}
        </button>
        <button
          type="button"
          className="cta secondary"
          onClick={runClipboardSmoke}
          disabled={busy !== null}
        >
          {busy === "clipboard"
            ? "Checking clipboard…"
            : "Clipboard smoke (mock)"}
        </button>
      </div>
      <button type="button" className="text-nav home-setup" onClick={onOpenSetup}>
        Open station log →
      </button>
      {status && (
        <p className="status" role="status">
          {status.product} {status.edge} v{status.version} — {status.message}
        </p>
      )}
      {voice && (
        <p className="status" role="status">
          Transcript “{voice.transcript}” · TTS {voice.ttsSamples} samples @{" "}
          {voice.sampleRateHz} Hz
        </p>
      )}
      {clipboard && (
        <p className="status" role="status">
          Clipboard {clipboard.backend} · {clipboard.policyOutcome} via{" "}
          {clipboard.policyRuleId ?? "—"} · round-trip “{clipboard.readBack}”
        </p>
      )}
      {error && (
        <p className="status error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

function App() {
  const [view, setView] = useState<View>("home");

  return (
    <main className={view === "setup" ? "shell shell-setup" : "shell"}>
      <div className="atmosphere" aria-hidden="true" />
      {view === "home" ? (
        <Home onOpenSetup={() => setView("setup")} />
      ) : (
        <Setup onDone={() => setView("home")} />
      )}
    </main>
  );
}

export default App;
