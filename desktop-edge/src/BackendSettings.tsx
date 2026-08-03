import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ApiKeyUpdate,
  BackendStatus,
  BackendTestResult,
  COMPLETION_KINDS,
  CompletionConfigUpdate,
  CompletionKind,
  SecretStorage,
  assistantBackendStatus,
  assistantSaveBackend,
  assistantTestBackend,
} from "./presence";

/**
 * Enterprise-grade backend configuration surface.
 *
 * Renders a small always-visible status pill (`Currently: openai · gpt-4o`)
 * that expands into a full configuration form when the operator wants to
 * change providers, models, or credentials. The form covers three
 * completion backends -- Echo (local, safe), OpenAI-compatible, and
 * Anthropic -- each with the fields it actually needs.
 *
 * ## Security model
 *
 * The api_key is write-only from the webview's perspective. The Rust
 * side never returns the stored value; the frontend only ever knows
 * whether a key is stored (`hasApiKey: boolean`). Editing the model or
 * base URL doesn't require re-entering the key: the form defaults to
 * `{ op: "keep" }` unless the operator explicitly enters a new value
 * or presses "Clear". This mirrors how enterprise settings UIs handle
 * secrets -- the panel can display the config without ever loading
 * the secret into the DOM.
 *
 * ## Live test → save flow
 *
 * "Test connection" runs the proposed config against a throwaway
 * router with a benign `ping` prompt, surfaces success / latency /
 * response preview on green, and the exact provider error on red.
 * "Save" persists the config and hot-swaps the production router in
 * place -- no restart, no lost in-flight requests. Cleared config
 * falls back to `RALLEH_COMPLETION_*` env vars, or Echo.
 */

type ApiKeyInputMode =
  | { kind: "existing-stored" } // A key is on disk; we don't have it and won't ask for it.
  | { kind: "entering-new"; value: string; reveal: boolean }
  | { kind: "cleared" } // Operator explicitly requested clearing the stored key.
  | { kind: "none-required" }; // Provider doesn't need one (Echo, some OpenAI-compatible).

type PanelState = {
  kind: CompletionKind;
  baseUrl: string;
  model: string;
  apiKey: ApiKeyInputMode;
};

type TestState =
  | { phase: "idle" }
  | { phase: "testing" }
  | { phase: "result"; result: BackendTestResult };

type SaveState =
  | { phase: "idle" }
  | { phase: "saving" }
  | { phase: "error"; message: string };

function initialApiKeyMode(
  kind: CompletionKind,
  hasStoredKey: boolean,
): ApiKeyInputMode {
  // Echo has no key concept at all. Anthropic requires one; OpenAI
  // may or may not (local providers accept none). If a key is already
  // stored we stay in the "don't touch it" state until the operator
  // presses "Change" or "Clear".
  if (kind === "echo") return { kind: "none-required" };
  if (hasStoredKey) return { kind: "existing-stored" };
  return { kind: "entering-new", value: "", reveal: false };
}

function stateFromStatus(status: BackendStatus | null): PanelState {
  const cfg = status?.configured;
  const kind: CompletionKind = cfg?.kind ?? "echo";
  return {
    kind,
    baseUrl: cfg?.baseUrl ?? "",
    model: cfg?.model ?? "",
    apiKey: initialApiKeyMode(kind, cfg?.hasApiKey === true),
  };
}

function apiKeyUpdateFrom(mode: ApiKeyInputMode): ApiKeyUpdate {
  switch (mode.kind) {
    case "existing-stored":
    case "none-required":
      return { op: "keep" };
    case "cleared":
      return { op: "clear" };
    case "entering-new":
      // An empty "entering-new" state is treated as "keep existing"
      // rather than "clear" -- clearing is only reached via the
      // explicit "Clear" button, so an accidentally-blank submit
      // doesn't nuke the stored key.
      return mode.value.length > 0
        ? { op: "set", value: mode.value }
        : { op: "keep" };
  }
}

function isFormValid(state: PanelState): boolean {
  if (state.kind === "echo") return true;
  if (!state.baseUrl.trim()) return false;
  if (!/^https?:\/\//i.test(state.baseUrl.trim())) return false;
  if (!state.model.trim()) return false;
  if (state.kind === "anthropic") {
    // Must have a stored key or be entering a new non-empty one.
    switch (state.apiKey.kind) {
      case "existing-stored":
        return true;
      case "entering-new":
        return state.apiKey.value.length > 0;
      default:
        return false;
    }
  }
  return true;
}

/** Human-readable summary line for the always-visible status pill. */
function pillSummary(status: BackendStatus): string {
  const active = status.activeBackend || "unknown";
  const cfg = status.configured;
  if (!cfg) return `${active} · env-configured`;
  const parts: string[] = [cfg.kind];
  if (cfg.model) parts.push(cfg.model);
  return parts.join(" · ");
}

export function BackendSettings() {
  const [status, setStatus] = useState<BackendStatus | null>(null);
  const [open, setOpen] = useState(false);
  const [form, setForm] = useState<PanelState>(() => stateFromStatus(null));
  const [test, setTest] = useState<TestState>({ phase: "idle" });
  const [save, setSave] = useState<SaveState>({ phase: "idle" });
  const [loadError, setLoadError] = useState<string | null>(null);

  const refreshStatus = useCallback(async () => {
    try {
      const s = await assistantBackendStatus();
      setStatus(s);
      setForm(stateFromStatus(s));
      setLoadError(null);
    } catch (err) {
      setLoadError(String(err));
    }
  }, []);

  useEffect(() => {
    void refreshStatus();
  }, [refreshStatus]);

  // Reset form to persisted state whenever the panel opens, so an
  // aborted edit doesn't leak into the next visit.
  useEffect(() => {
    if (open) setForm(stateFromStatus(status));
    if (open) setTest({ phase: "idle" });
    if (open) setSave({ phase: "idle" });
  }, [open, status]);

  const currentUpdate = useMemo<CompletionConfigUpdate>(
    () => ({
      kind: form.kind,
      baseUrl: form.baseUrl.trim(),
      model: form.model.trim(),
      apiKey: apiKeyUpdateFrom(form.apiKey),
    }),
    [form],
  );

  const runTest = useCallback(async () => {
    setTest({ phase: "testing" });
    try {
      const result = await assistantTestBackend(currentUpdate);
      setTest({ phase: "result", result });
    } catch (err) {
      setTest({
        phase: "result",
        result: {
          ok: false,
          backend: form.kind,
          latencyMs: null,
          sampleResponse: null,
          error: String(err),
        },
      });
    }
  }, [currentUpdate, form.kind]);

  const runSave = useCallback(async () => {
    setSave({ phase: "saving" });
    try {
      const next = await assistantSaveBackend(currentUpdate);
      setStatus(next);
      setForm(stateFromStatus(next));
      setSave({ phase: "idle" });
      setTest({ phase: "idle" });
      setOpen(false);
    } catch (err) {
      setSave({ phase: "error", message: String(err) });
    }
  }, [currentUpdate]);

  const runClear = useCallback(async () => {
    setSave({ phase: "saving" });
    try {
      const next = await assistantSaveBackend(null);
      setStatus(next);
      setForm(stateFromStatus(next));
      setSave({ phase: "idle" });
      setTest({ phase: "idle" });
    } catch (err) {
      setSave({ phase: "error", message: String(err) });
    }
  }, []);

  return (
    <section className="backend-settings" aria-labelledby="backend-heading">
      <BackendPill
        status={status}
        error={loadError}
        open={open}
        onToggle={() => setOpen((v) => !v)}
      />
      {open && (
        <div className="backend-panel" role="region" aria-label="Backend configuration">
          <h3 id="backend-heading" className="backend-panel-title">
            Completion backend
          </h3>
          <p className="backend-panel-lede">
            Choose the model provider Ralleh routes prompts through. Changes apply live — in-flight requests keep using the previous backend.
          </p>

          <KindPicker
            selected={form.kind}
            onSelect={(kind) =>
              setForm((f) => ({
                ...f,
                kind,
                apiKey: initialApiKeyMode(
                  kind,
                  status?.configured?.hasApiKey === true &&
                    status.configured.kind === kind,
                ),
              }))
            }
          />

          {form.kind !== "echo" && (
            <div className="backend-fields">
              <LabeledInput
                id="backend-base-url"
                label="API base URL"
                placeholder={
                  form.kind === "anthropic"
                    ? "https://api.anthropic.com"
                    : "http://localhost:11434/v1"
                }
                value={form.baseUrl}
                onChange={(v) => setForm((f) => ({ ...f, baseUrl: v }))}
                hint={
                  form.kind === "openai"
                    ? "The `/chat/completions` path is appended automatically."
                    : "The `/v1/messages` path is appended automatically."
                }
              />
              <LabeledInput
                id="backend-model"
                label="Model"
                placeholder={
                  form.kind === "anthropic"
                    ? "claude-3-5-sonnet-latest"
                    : "gpt-4o"
                }
                value={form.model}
                onChange={(v) => setForm((f) => ({ ...f, model: v }))}
              />
              <ApiKeyField
                mode={form.apiKey}
                required={form.kind === "anthropic"}
                storage={
                  status?.configured?.kind === form.kind
                    ? status.configured.storage
                    : "none"
                }
                onChange={(mode) => setForm((f) => ({ ...f, apiKey: mode }))}
              />
            </div>
          )}

          <TestResult state={test} />

          <div className="backend-actions">
            <button
              type="button"
              className="backend-btn backend-btn-secondary"
              onClick={runTest}
              disabled={!isFormValid(form) || test.phase === "testing"}
            >
              {test.phase === "testing" ? "Testing…" : "Test connection"}
            </button>
            <button
              type="button"
              className="backend-btn backend-btn-primary"
              onClick={runSave}
              disabled={!isFormValid(form) || save.phase === "saving"}
            >
              {save.phase === "saving" ? "Saving…" : "Save & apply"}
            </button>
            {status?.configured && (
              <button
                type="button"
                className="backend-btn backend-btn-danger"
                onClick={runClear}
                disabled={save.phase === "saving"}
                title="Remove the persisted config and fall back to env vars / Echo."
              >
                Clear
              </button>
            )}
          </div>

          {save.phase === "error" && (
            <p className="backend-error" role="alert">
              {save.message}
            </p>
          )}
        </div>
      )}
    </section>
  );
}

function BackendPill({
  status,
  error,
  open,
  onToggle,
}: {
  status: BackendStatus | null;
  error: string | null;
  open: boolean;
  onToggle: () => void;
}) {
  const summary = status ? pillSummary(status) : error ? "unavailable" : "loading…";
  const active = status?.activeBackend ?? "";
  return (
    <button
      type="button"
      className={`backend-pill ${open ? "backend-pill-open" : ""}`}
      onClick={onToggle}
      aria-expanded={open}
      aria-controls="backend-panel"
      title={active ? `Currently routing through ${active}` : summary}
    >
      <span className="backend-pill-dot" aria-hidden="true" />
      <span className="backend-pill-label">Backend</span>
      <span className="backend-pill-value">{summary}</span>
      <span className="backend-pill-chevron" aria-hidden="true">
        {open ? "▾" : "▸"}
      </span>
    </button>
  );
}

function KindPicker({
  selected,
  onSelect,
}: {
  selected: CompletionKind;
  onSelect: (k: CompletionKind) => void;
}) {
  return (
    <fieldset className="backend-kind-picker">
      <legend className="visually-hidden">Provider</legend>
      {COMPLETION_KINDS.map((k) => (
        <label
          key={k.id}
          className={`backend-kind-option ${
            selected === k.id ? "backend-kind-option-selected" : ""
          }`}
        >
          <input
            type="radio"
            name="backend-kind"
            value={k.id}
            checked={selected === k.id}
            onChange={() => onSelect(k.id)}
          />
          <span className="backend-kind-label">{k.label}</span>
          <span className="backend-kind-hint">{k.hint}</span>
        </label>
      ))}
    </fieldset>
  );
}

function LabeledInput({
  id,
  label,
  placeholder,
  value,
  onChange,
  hint,
}: {
  id: string;
  label: string;
  placeholder: string;
  value: string;
  onChange: (v: string) => void;
  hint?: string;
}) {
  return (
    <div className="backend-field">
      <label htmlFor={id} className="backend-field-label">
        {label}
      </label>
      <input
        id={id}
        type="text"
        className="backend-field-input"
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        autoCorrect="off"
        autoCapitalize="off"
        spellCheck={false}
      />
      {hint && <p className="backend-field-hint">{hint}</p>}
    </div>
  );
}

function ApiKeyField({
  mode,
  required,
  storage,
  onChange,
}: {
  mode: ApiKeyInputMode;
  required: boolean;
  storage: SecretStorage;
  onChange: (m: ApiKeyInputMode) => void;
}) {
  const inputRef = useRef<HTMLInputElement | null>(null);
  // Focus the input the moment the operator switches into edit mode.
  useEffect(() => {
    if (mode.kind === "entering-new") {
      inputRef.current?.focus();
    }
  }, [mode.kind]);

  return (
    <div className="backend-field">
      <label htmlFor="backend-api-key" className="backend-field-label">
        API key{required ? " (required)" : " (optional)"}
      </label>
      {mode.kind === "existing-stored" && (
        <div className="backend-api-key-stored">
          <span className="backend-api-key-mask" aria-label="API key stored">
            ••••••••••••
          </span>
          <StorageBadge storage={storage} />
          <button
            type="button"
            className="backend-btn backend-btn-inline"
            onClick={() => onChange({ kind: "entering-new", value: "", reveal: false })}
          >
            Change
          </button>
          <button
            type="button"
            className="backend-btn backend-btn-inline backend-btn-danger-inline"
            onClick={() => onChange({ kind: "cleared" })}
          >
            Clear
          </button>
        </div>
      )}
      {mode.kind === "cleared" && (
        <div className="backend-api-key-stored">
          <span className="backend-api-key-status backend-api-key-status-warn">
            Will clear stored key on save
          </span>
          <button
            type="button"
            className="backend-btn backend-btn-inline"
            onClick={() => onChange({ kind: "existing-stored" })}
          >
            Keep stored key
          </button>
        </div>
      )}
      {mode.kind === "entering-new" && (
        <div className="backend-api-key-input">
          <input
            id="backend-api-key"
            ref={inputRef}
            type={mode.reveal ? "text" : "password"}
            className="backend-field-input"
            value={mode.value}
            placeholder={required ? "Enter API key" : "Leave blank if not required"}
            onChange={(e) =>
              onChange({ ...mode, value: e.target.value })
            }
            autoComplete="off"
            spellCheck={false}
          />
          <button
            type="button"
            className="backend-btn backend-btn-inline"
            onClick={() => onChange({ ...mode, reveal: !mode.reveal })}
            aria-pressed={mode.reveal}
            title={mode.reveal ? "Hide key" : "Show key"}
          >
            {mode.reveal ? "Hide" : "Show"}
          </button>
        </div>
      )}
      {mode.kind === "none-required" && (
        <p className="backend-field-hint">This provider doesn't use an API key.</p>
      )}
      <p className="backend-field-hint">
        {storage === "keychain"
          ? "Keys are stored in your OS keychain and never sent back to the UI."
          : storage === "cleartext"
          ? "No OS keychain is available on this host — keys are stored in a local settings file."
          : "Keys never leave this device. When stored, they go into your OS keychain."}
      </p>
    </div>
  );
}

/** Small badge under the masked key showing which store the secret
 * actually lives in. Rendering an honest signal here is the whole
 * point of the keychain migration -- it's how the operator learns
 * that their macOS Keychain / Windows Credential Manager is doing
 * its job, or that a headless host has fallen back to cleartext. */
function StorageBadge({ storage }: { storage: SecretStorage }) {
  if (storage === "keychain") {
    return (
      <span
        className="backend-api-key-status backend-storage-badge backend-storage-badge-ok"
        title="This API key is stored in the OS keychain (Windows Credential Manager, macOS Keychain, or Linux Secret Service)."
      >
        <span aria-hidden="true">🔒</span> Stored in OS keychain
      </span>
    );
  }
  if (storage === "cleartext") {
    return (
      <span
        className="backend-api-key-status backend-storage-badge backend-storage-badge-warn"
        title="No OS keychain is available on this host, so the key was written to edge-settings.json in cleartext. Configure a system keychain to store it securely."
      >
        <span aria-hidden="true">⚠</span> Cleartext on disk
      </span>
    );
  }
  return (
    <span className="backend-api-key-status">Not stored yet</span>
  );
}

function TestResult({ state }: { state: TestState }) {
  if (state.phase === "idle") return null;
  if (state.phase === "testing") {
    return (
      <div className="backend-test-result backend-test-result-pending" role="status">
        <span className="backend-test-spinner" aria-hidden="true" />
        Testing backend…
      </div>
    );
  }
  const r = state.result;
  if (r.ok) {
    return (
      <div className="backend-test-result backend-test-result-ok" role="status">
        <span className="backend-test-mark" aria-hidden="true">
          ✓
        </span>
        <div>
          <div className="backend-test-headline">
            {r.backend} responded
            {r.latencyMs != null && ` in ${r.latencyMs} ms`}
          </div>
          {r.sampleResponse && (
            <div className="backend-test-preview">{r.sampleResponse}</div>
          )}
        </div>
      </div>
    );
  }
  return (
    <div className="backend-test-result backend-test-result-error" role="alert">
      <span className="backend-test-mark" aria-hidden="true">
        !
      </span>
      <div>
        <div className="backend-test-headline">Test failed on {r.backend}</div>
        {r.error && <div className="backend-test-preview">{r.error}</div>}
      </div>
    </div>
  );
}
