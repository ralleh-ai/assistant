import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
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

function App() {
  const [status, setStatus] = useState<CoreStatus | null>(null);
  const [voice, setVoice] = useState<VoiceSmoke | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<"ping" | "voice" | null>(null);

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

  return (
    <main className="shell">
      <div className="atmosphere" aria-hidden="true" />
      <section className="hero">
        <p className="brand">Ralleh</p>
        <h1 className="headline">Your private operator at the edge.</h1>
        <p className="lede">
          Desktop shell Phase 1 — React over Rust. Ping the core, then run a
          mock voice pipeline (no microphone).
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
        </div>
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
        {error && (
          <p className="status error" role="alert">
            {error}
          </p>
        )}
      </section>
    </main>
  );
}

export default App;
