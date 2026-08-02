import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  DEFAULT_SETTINGS,
  EdgeSettings,
  EdgeSettingsResponse,
  VOICE_STYLES,
  isSettingsComplete,
  settingsFromResponse,
} from "./settings";

type Plate = {
  id: "station" | "identity" | "conduit" | "voice" | "style";
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
      "Choose a short, stable name for the organization or workspace this machine serves. Policy and audit trails key off it — changing it later means re-aligning those records.",
  },
  {
    id: "identity",
    numeral: "II",
    title: "Identity",
    lede: "Label the machine and the person who runs it.",
    detail:
      "Device and actor IDs travel with every privileged action. Keep them unique within the tenant (for example a hostname and a login). They show up in approvals and logs so operators can tell who did what.",
  },
  {
    id: "conduit",
    numeral: "III",
    title: "Conduit",
    lede: "Point this shell at the local mcp-server.",
    detail:
      "The desktop edge talks to tools and policy through mcp-server on your machine. Loopback (127.0.0.1) is the safe default. Use http or https only.",
  },
  {
    id: "voice",
    numeral: "IV",
    title: "Voice",
    lede: "Microphone access is an OS grant, not a Ralleh default.",
    detail:
      "Allow the microphone for this app in system settings, then acknowledge below. You can verify capture with a short listen once clearance is stamped.",
  },
  {
    id: "style",
    numeral: "V",
    title: "Style",
    lede: "How should Ralleh sound when it speaks?",
    detail:
      "Pick a speaking style for this edge. It is stored with your settings and will shape future voice replies — nothing is synthesized on this screen.",
  },
];

type MicCheck = {
  frames: number;
  samples: number;
  sampleRateHz: number;
  peakRms: number;
  maxAbs: number;
};

type Props = {
  /** When true, user cannot leave until settings are complete. */
  required: boolean;
  initial?: EdgeSettings | null;
  onComplete: (settings: EdgeSettings) => void;
  onCancel?: () => void;
};

export function SettingsView({ required, initial, onComplete, onCancel }: Props) {
  const [plateIdx, setPlateIdx] = useState(0);
  const [draft, setDraft] = useState<EdgeSettings | null>(initial ?? null);
  const [path, setPath] = useState<string>("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [micBusy, setMicBusy] = useState(false);
  const [micCheck, setMicCheck] = useState<MicCheck | null>(null);

  const plate = PLATES[plateIdx];
  const complete = draft ? isSettingsComplete(draft) : false;

  useEffect(() => {
    if (initial) {
      setDraft(initial);
    }
  }, [initial]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [settings, settingsPath] = await Promise.all([
          initial
            ? Promise.resolve(initial)
            : invoke<EdgeSettingsResponse>("load_edge_settings").then(
                settingsFromResponse,
              ),
          invoke<string>("edge_settings_path"),
        ]);
        if (!cancelled) {
          setDraft({
            ...DEFAULT_SETTINGS,
            ...settings,
            voiceStyle: settings.voiceStyle ?? "",
          });
          setPath(settingsPath);
        }
      } catch (e) {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : String(e));
          setDraft(DEFAULT_SETTINGS);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [initial]);

  function patch(partial: Partial<EdgeSettings>) {
    setDraft((prev) => (prev ? { ...prev, ...partial } : prev));
    setMicCheck(null);
  }

  async function saveAnd(next: () => void) {
    if (!draft) return;
    setBusy(true);
    setError(null);
    try {
      const saved = await invoke<EdgeSettingsResponse>("save_edge_settings", {
        settings: draft,
      });
      setDraft(settingsFromResponse(saved));
      next();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function listenOnce() {
    if (!draft?.micAcknowledged) {
      setError("Stamp microphone clearance before listening.");
      return;
    }
    setMicBusy(true);
    setError(null);
    setMicCheck(null);
    try {
      // Persist clearance first so the Rust gate sees it.
      await invoke<EdgeSettingsResponse>("save_edge_settings", {
        settings: draft,
      });
      const result = await invoke<MicCheck>("mic_smoke");
      setMicCheck(result);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setMicBusy(false);
    }
  }

  function finish() {
    if (!draft || !isSettingsComplete(draft)) {
      setError("Finish every required step before entering the shell.");
      return;
    }
    setBusy(true);
    setError(null);
    (async () => {
      try {
        const saved = await invoke<EdgeSettingsResponse>("save_edge_settings", {
          settings: draft,
        });
        const next = settingsFromResponse(saved);
        setDraft(next);
        onComplete(next);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(false);
      }
    })();
  }

  if (!draft) {
    return (
      <section className="setup">
        <p className="setup-loading">Loading settings…</p>
      </section>
    );
  }

  return (
    <section className="setup" aria-label="Settings">
      <aside className="setup-rail">
        <p className="setup-rail-brand">Ralleh</p>
        <p className="setup-rail-label">Settings</p>
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
                <em>Settings → Privacy → Microphone</em>. Ralleh will not open
                the mic until you are ready.
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
              <button
                type="button"
                className="cta secondary listen-once"
                disabled={!draft.micAcknowledged || micBusy || busy}
                onClick={listenOnce}
              >
                {micBusy ? "Listening…" : "Listen once"}
              </button>
              {micCheck && (
                <p className="mic-check-result" role="status">
                  Heard {micCheck.frames} frames @ {micCheck.sampleRateHz} Hz ·
                  peak {micCheck.peakRms.toFixed(4)}
                </p>
              )}
            </div>
          )}

          {plate.id === "style" && (
            <div className="style-list" role="radiogroup" aria-label="Voice style">
              {VOICE_STYLES.map((style) => (
                <button
                  key={style.id}
                  type="button"
                  role="radio"
                  aria-checked={draft.voiceStyle === style.id}
                  className={
                    draft.voiceStyle === style.id
                      ? "style-option is-selected"
                      : "style-option"
                  }
                  onClick={() => patch({ voiceStyle: style.id })}
                >
                  <span className="style-option-label">{style.label}</span>
                  <span className="style-option-desc">{style.description}</span>
                </button>
              ))}
            </div>
          )}
        </div>

        <footer className="setup-footer">
          <div className="setup-footer-start">
            {!required && onCancel && (
              <button type="button" className="text-nav" onClick={onCancel}>
                ← Back
              </button>
            )}
            {plateIdx > 0 && (
              <button
                type="button"
                className="text-nav"
                onClick={() => setPlateIdx((i) => Math.max(0, i - 1))}
              >
                ← Prior
              </button>
            )}
          </div>
          <div className="setup-footer-actions">
            {plateIdx < PLATES.length - 1 ? (
              <button
                type="button"
                className="cta"
                disabled={busy}
                onClick={() => saveAnd(() => setPlateIdx((i) => i + 1))}
              >
                {busy ? "Saving…" : "Continue"}
              </button>
            ) : (
              <button
                type="button"
                className="cta"
                disabled={busy || !complete}
                onClick={finish}
              >
                {busy ? "Saving…" : required ? "Enter shell" : "Save & return"}
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
