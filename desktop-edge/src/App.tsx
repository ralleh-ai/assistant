import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type CoreStatus = {
  product: string;
  edge: string;
  version: string;
  message: string;
};

function App() {
  const [status, setStatus] = useState<CoreStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function pingCore() {
    setBusy(true);
    setError(null);
    try {
      const result = await invoke<CoreStatus>("core_ping");
      setStatus(result);
    } catch (e) {
      setStatus(null);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="shell">
      <div className="atmosphere" aria-hidden="true" />
      <section className="hero">
        <p className="brand">Ralleh</p>
        <h1 className="headline">Your private operator at the edge.</h1>
        <p className="lede">
          Desktop shell Phase 1 — React UI over a Rust core. Prove the IPC
          path, then wire voice.
        </p>
        <div className="actions">
          <button type="button" className="cta" onClick={pingCore} disabled={busy}>
            {busy ? "Contacting core…" : "Ping Rust core"}
          </button>
        </div>
        {status && (
          <p className="status" role="status">
            {status.product} {status.edge} v{status.version} — {status.message}
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
