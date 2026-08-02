import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type EdgeSettings = {
  tenantId: string;
  deviceId: string;
  actorId: string;
  mcpBaseUrl: string;
  micAcknowledged: boolean;
};

type Plate = {
  id: "station" | "identity" | "conduit" | "voice";
  numeral: string;
  title: string;
  lede: string;
  detail: string;
};

const PLATES: Plate[] = [
  {
    id: "station",
    numeral: "I",
    title: "Station",
    lede: "Every capability on this edge is scoped to a tenant.",
    detail:
      "Pick a short, stable name for the organization or workspace this machine serves. Policy, audit trails, and later mcp-server calls will all key off it — changing it later means re-aligning those records.",
  },
  {
    id: "identity",
    numeral: "II",
    title: "Identity",
    lede: "Label the machine and the person who runs it.",
    detail:
      "Device and actor IDs travel with every privileged action. Keep them unique within the tenant (for example a hostname and a login), not marketing names. They show up in approvals and logs so operators can tell who did what.",
  },
  {
    id: "conduit",
    numeral: "III",
    title: "Conduit",
    lede: "Point this shell at the local mcp-server.",
    detail:
      "The desktop edge talks to tools and policy through mcp-server on your machine. Loopback (127.0.0.1) is the safe default until you intentionally expose a LAN endpoint. Use http or https only — no filesystem paths.",
  },
  {
    id: "voice",
    numeral: "IV",
    title: "Voice",
    lede: "Microphone capture is an OS grant, not a Ralleh default.",
    detail:
      "Acknowledge that you understand how to allow the mic in system settings. Capture stays behind an explicit Rust feature; this shell will not open the microphone until you opt in later — this step only records that the guidance was read.",
  },
];

type Props = {
  onDone: () => void;
};

export function Setup({ onDone }: Props) {
  const [plateIdx, setPlateIdx] = useState(0);
  const [draft, setDraft] = useState<EdgeSettings | null>(null);
  const [path, setPath] = useState<string>("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const plate = PLATES[plateIdx];

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [settings, settingsPath] = await Promise.all([
          invoke<EdgeSettings>("load_edge_settings"),
          invoke<string>("edge_settings_path"),
        ]);
        if (!cancelled) {
          setDraft(settings);
          setPath(settingsPath);
        }
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
          setDraft({
            tenantId: "local",
            deviceId: "desktop-1",
            actorId: "operator",
            mcpBaseUrl: "http://127.0.0.1:8787",
            micAcknowledged: false,
          });
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  function patch(partial: Partial<EdgeSettings>) {
    setDraft((prev) => (prev ? { ...prev, ...partial } : prev));
  }

  async function saveAnd(next: () => void) {
    if (!draft) return;
    setBusy(true);
    setError(null);
    try {
      const saved = await invoke<EdgeSettings>("save_edge_settings", {
        settings: draft,
      });
      setDraft(saved);
      next();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  if (!draft) {
    return (
      <section className="setup">
        <p className="setup-loading">Opening station log…</p>
      </section>
    );
  }

  return (
    <section className="setup" aria-label="Edge setup">
      <aside className="setup-rail" aria-hidden="true">
        <p className="setup-rail-brand">Ralleh</p>
        <ol className="setup-index">
          {PLATES.map((p, i) => (
            <li key={p.id}>
              <button
                type="button"
                className={
                  i === plateIdx
                    ? "setup-index-item is-active"
                    : i < plateIdx
                      ? "setup-index-item is-past"
                      : "setup-index-item"
                }
                onClick={() => setPlateIdx(i)}
              >
                <span className="setup-numeral">{p.numeral}</span>
                <span className="setup-index-title">{p.title}</span>
              </button>
            </li>
          ))}
        </ol>
        <p className="setup-path" title={path}>
          {path || "…"}
        </p>
      </aside>

      <div className="setup-stage" key={plate.id}>
        <header className="setup-stage-head">
          <h1 className="setup-plate-title">{plate.title}</h1>
          <p className="setup-lede">{plate.lede}</p>
          <p className="setup-detail">{plate.detail}</p>
        </header>

        <div className="setup-fields">
          {plate.id === "station" && (
            <label className="field">
              <span className="field-label">Tenant</span>
              <span className="field-hint">
                Isolation boundary for policy and audit on this edge
              </span>
              <input
                className="field-input"
                value={draft.tenantId}
                onChange={(e) => patch({ tenantId: e.target.value })}
                autoComplete="off"
                spellCheck={false}
                placeholder="local"
              />
            </label>
          )}

          {plate.id === "identity" && (
            <>
              <label className="field">
                <span className="field-label">Device</span>
                <span className="field-hint">
                  Stable id for this machine within the tenant
                </span>
                <input
                  className="field-input"
                  value={draft.deviceId}
                  onChange={(e) => patch({ deviceId: e.target.value })}
                  autoComplete="off"
                  spellCheck={false}
                  placeholder="desktop-1"
                />
              </label>
              <label className="field">
                <span className="field-label">Actor</span>
                <span className="field-hint">
                  Operator or service principal who uses this shell
                </span>
                <input
                  className="field-input"
                  value={draft.actorId}
                  onChange={(e) => patch({ actorId: e.target.value })}
                  autoComplete="off"
                  spellCheck={false}
                  placeholder="operator"
                />
              </label>
            </>
          )}

          {plate.id === "conduit" && (
            <label className="field">
              <span className="field-label">mcp-server base URL</span>
              <span className="field-hint">
                Origin only — no trailing path required
              </span>
              <input
                className="field-input"
                value={draft.mcpBaseUrl}
                onChange={(e) => patch({ mcpBaseUrl: e.target.value })}
                autoComplete="off"
                spellCheck={false}
                placeholder="http://127.0.0.1:8787"
              />
            </label>
          )}

          {plate.id === "voice" && (
            <div className="voice-clearance">
              <p>
                On Windows, allow microphone access for this app under{" "}
                <em>Settings → Privacy → Microphone</em>. Capture stays behind
                an explicit Rust feature; this shell will not open the mic
                until you opt in later.
              </p>
              <button
                type="button"
                className={
                  draft.micAcknowledged
                    ? "clearance-stamp is-set"
                    : "clearance-stamp"
                }
                onClick={() =>
                  patch({ micAcknowledged: !draft.micAcknowledged })
                }
                aria-pressed={draft.micAcknowledged}
              >
                {draft.micAcknowledged
                  ? "Clearance noted"
                  : "Stamp clearance"}
              </button>
            </div>
          )}
        </div>

        <footer className="setup-footer">
          <button
            type="button"
            className="text-nav"
            disabled={plateIdx === 0}
            onClick={() => setPlateIdx((i) => Math.max(0, i - 1))}
          >
            ← Prior
          </button>
          <div className="setup-footer-actions">
            {plateIdx < PLATES.length - 1 ? (
              <button
                type="button"
                className="cta"
                disabled={busy}
                onClick={() =>
                  saveAnd(() => setPlateIdx((i) => i + 1))
                }
              >
                {busy ? "Saving…" : "Continue"}
              </button>
            ) : (
              <button
                type="button"
                className="cta"
                disabled={busy}
                onClick={() => saveAnd(onDone)}
              >
                {busy ? "Saving…" : "Enter shell"}
              </button>
            )}
          </div>
        </footer>

        {error && (
          <p className="status error" role="alert">
            {error}
          </p>
        )}
      </div>
    </section>
  );
}
