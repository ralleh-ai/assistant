# Architecture — Current Implementation

This document maps the *current, actually-built* Rust workspace to the plan
in [`DEVELOPMENT.md`](./DEVELOPMENT.md). It answers: "of everything the plan
describes, what exists today, and how does it fit together?"

## Crate map

```text
crates/
  ralleh-policy-core/    Policy evaluation engine (DEVELOPMENT.md §8.3, ADR-004)
  ralleh-tool-gateway/   Tool/capability dispatch broker (§8.5, ADR-004/005)
  ralleh-audit-store/    Audit event persistence (§8.4 "Audit memory", §11 hard invariants)
  ralleh-ai-router/      AI completion routing (§8.7)
  ralleh-audio-core/     Voice pipeline primitives: VAD, wake-word, audio source (§9)
  ralleh-mcp-server/     HTTP surface wiring everything together (maps loosely to §14.1 API surface, though not identical routes yet)
```

This is a subset of the planned `crates/` layout in DEVELOPMENT.md §16
(`ralleh-core`, `policy-core`, `mcp-gateway`, `ai-router`, `memory-core`,
`audit-core`). Naming has diverged slightly (`ralleh-tool-gateway` here vs.
`mcp-gateway` in the plan; `ralleh-audit-store` here vs. `audit-core` in the
plan) — this is worth reconciling later but is not a functional gap, see
[`NEXT_STEPS.md`](./NEXT_STEPS.md).

Not yet started: `memory-core` (Phase 3 in the plan — pgvector-backed
enterprise memory), any TypeScript/NestJS control plane, any Tauri/React
edge shell.

## `ralleh-policy-core`

**Purpose:** every privileged action goes through this before it's allowed
to happen. Implements DEVELOPMENT.md's core policy question set (§8.3):
who, from where, what capability, human confirmation needed?

**Key types:**
- `PolicyRequest` — tenant_id, device_id, actor_id, capability, sensitivity. Validated (rejects empty/whitespace-only fields).
- `PolicyRule` — id, optional tenant/device/actor scoping, optional capability_prefix, optional sensitivity match, `effect` (`Allow` / `Deny` / `RequireApproval`), reason.
- `PolicyEngine` — holds an ordered `Vec<PolicyRule>`. **First-matching-rule-wins** semantics (not "most specific wins" — order matters, more specific rules must be listed first by the caller).
- `PolicyDecision` — the auditable result: which rule matched (if any), the effect, and the full request context, so audit records are self-contained.
- Default-deny: an empty engine, or a request that matches no rule, is **denied**, not allowed. This is deliberate and matches DEVELOPMENT.md §11.2 "least privilege."

**21 tests** cover: rule matching (capability prefix, tenant scoping, sensitivity), first-match-wins in both directions (allow-then-deny and deny-then-allow), tenant isolation (a rule scoped to tenant A must never match tenant B's request), and request validation.

## `ralleh-tool-gateway`

**Purpose:** the capability broker — DEVELOPMENT.md §8.5 "Tool and Integration Layer" / ADR-004/005. No tool executes without going through here.

**Key types:**
- `ToolHandler` trait — `async fn invoke(&self, ToolInvocation) -> ToolResult`. Anything that wants to be callable as a tool implements this.
- `ToolDefinition` — capability id, description, `default_sensitivity`.
- `ToolRegistry` — capability string → `(ToolDefinition, Box<dyn ToolHandler>)` map.
- `ToolGateway` — the actual dispatch chokepoint. `dispatch()`:
  1. Look up capability in registry (unknown → `ToolCallOutcome::UnknownCapability`, policy never consulted).
  2. Ask `PolicyEngine` for a decision.
  3. If `Deny` → `ToolCallOutcome::Denied`, handler never invoked.
  4. If `RequireApproval` → `ToolCallOutcome::ApprovalRequired`, handler never invoked (approval-flow itself is not yet implemented — this just correctly *stops* execution and reports the need for approval; see Next Steps).
  5. If `Allow` and a handler is registered → invoke it; map success/failure to `Succeeded`/`Failed`.
  6. If `Allow` but no handler registered → `NoHandlerRegistered` (a config bug, reported distinctly from a policy denial).
  7. **Every** outcome produces a `GatewayEvent`, which is handed to the configured `AuditSink` before `dispatch()` returns.
- `AuditSink` trait (defined *in this crate*, `pub mod gateway`) — `fn record(&self, event: &GatewayEvent)`, infallible by design (an audit sink outage must never take down tool dispatch). `NoopAuditSink` is the zero-config default; real persistence comes from `ralleh-audit-store`.

**Real (non-mocked) handlers implemented:**
- `FsReadTextHandler` — sandboxed UTF-8 text file reader. Canonicalizes paths, rejects traversal outside a configured root.
- `FsWriteTextHandler` — sandboxed UTF-8 text file writer. Same sandboxing approach (canonicalizes the *parent* directory since the target file may not exist yet). Refuses to overwrite existing files unless `overwrite: true` is explicitly passed. Does not auto-create parent directories.

**24 tests** cover both handlers (traversal rejection, missing args, overwrite semantics, sandbox boundary enforcement) plus gateway-level tests (deny-by-default, approval-required never invokes handler, cross-tenant isolation holds through the full dispatch path, handler failure reported distinctly from policy denial).

## `ralleh-audit-store`

**Purpose:** closes the audit-persistence gap DEVELOPMENT.md flags as a hard invariant (§11.1: audit events for privileged actions) and as an explicit non-negotiable (§22). Before this crate existed, `GatewayEvent`/`PolicyDecision` records were computed and handed back to the caller but never durably written anywhere.

**Key types:**
- `AuditRecordKind` — enum: `ToolDispatch(GatewayEvent)` or `Completion { tenant_id, device_id, actor_id, outcome: CompletionOutcome }`. One enum (not two separate sink methods) so a single log/table can interleave both kinds in true chronological order — this matters for incident reconstruction.
- `AuditRecord` — wraps a `AuditRecordKind` with a `record_id` (UUID) and `recorded_at` (persistence-time timestamp, independent of any timestamp already on the underlying event).
- `AuditSink` trait (defined *in this crate*, distinct from `ralleh-tool-gateway`'s trait of the same name — see [`DECISIONS.md`](./DECISIONS.md) for why) — `fn record(&self, record: &AuditRecord) -> Result<(), AuditSinkError>`, fallible, for actual persistence.
- Three implementations: `NullAuditSink` (discards, for tests), `InMemoryAuditSink` (Vec-backed, for tests), `JsonlFileAuditSink` (the real one — append-only, one JSON object per line, flushed on every write, mutex-serialized so concurrent writers can't interleave partial lines).
- `JsonlFileAuditSink` implements *both* `ralleh-audit-store::AuditSink` (fallible persistence) and `ralleh-tool-gateway::gateway::AuditSink` (infallible, dispatch-facing) — the bridge does `AuditRecord::tool_dispatch(event.clone())` then best-effort `eprintln!` on write failure, so an audit sink outage never propagates up through `dispatch()` and takes down tool execution.

**7 tests** cover: insertion-order preservation, append-across-multiple-opens, one-line-per-record, concurrent-writes-don't-interleave-or-lose-records (8 threads × 25 writes), path getter, null sink always succeeds.

## `ralleh-ai-router`

**Purpose:** DEVELOPMENT.md §8.7 "AI Router" — abstracts model/provider choice behind a stable adapter, policy-gated the same way tool dispatch is.

**Key types:**
- `CompletionBackend` trait — `async fn complete(&self, &CompletionRequest) -> Result<CompletionResponse, String>`. Deliberately mirrors `ToolHandler`'s shape.
- `CompletionRequest`/`CompletionResponse`/`CompletionOutcome` — all `Serialize + Deserialize` (needed for audit round-tripping — this was a real gap fixed mid-session, see [`DECISIONS.md`](./DECISIONS.md)).
- `AiRouter` — policy-gates every completion request through `ralleh-policy-core` before calling the backend; `RoutingError` distinguishes policy denial from backend failure.

**Backends implemented:**
- `EchoBackend` — test/dev double, echoes the prompt back with a fixed prefix. The ai-router equivalent of `ToolHandler`'s `EchoHandler`.
- `HttpCompletionBackend` — real, non-mocked. Speaks the OpenAI-compatible `/chat/completions` wire format (covers OpenAI itself, and self-hosted vLLM/Ollama/llama.cpp — deliberately the lowest common denominator across providers per DEVELOPMENT.md §17's "adapter interfaces over provider lock-in" principle). Supports model-hint override, bearer auth, 30s timeout. Turns network errors, non-2xx HTTP status, malformed JSON, and empty-choices responses all into `Err(String)` rather than panicking.

**13 tests**: 1 for `EchoBackend`, 6 for `HttpCompletionBackend` (using `wiremock` — no live network calls in tests), 6 for `AiRouter` (policy gating, tenant isolation, backend-failure-not-panic).

## `ralleh-audio-core`

**Purpose:** DEVELOPMENT.md §9 "Voice Pipeline" primitives — VAD, wake-word detection, audio source abstraction. This is the *only* crate in the workspace that has not yet been connected to anything real (no microphone capture, no STT/TTS bindings) — it currently operates purely on synthetic/mock audio frames to prove the state-machine logic.

**Key types:**
- `AudioSource` trait + `MockSource` test double.
- VAD state machine: silence → maybe-speech → speech → maybe-silence → silence, with debouncing (a single loud frame doesn't immediately confirm speech; a brief dip during speech doesn't immediately drop back to silence).
- Wake-word detector: acoustic pattern matching against utterance bounds (too-short/too-long rejected), with a cooldown period preventing immediate retrigger.

**17 tests** cover VAD state transitions and wake-word triggering/cooldown/bounds-checking.

This crate is a **pure logic layer** today — real audio I/O (`cpal` or
similar), real STT (`whisper-rs`), and real TTS (Piper/Kokoro bindings) per
DEVELOPMENT.md §2.3/§17/Phase 1 have not been started. See
[`NEXT_STEPS.md`](./NEXT_STEPS.md).

## `ralleh-mcp-server`

**Purpose:** the HTTP surface that wires everything above together into something runnable. Axum-based.

**Routes:**
- `GET /healthz`
- `POST /v1/tools/dispatch` — the HTTP-facing entrypoint to `ToolGateway::dispatch`. Maps `ToolCallOutcome` variants to HTTP status: `Succeeded` → 200, `Denied` → 403, `ApprovalRequired` → 202, `UnknownCapability`/`NoHandlerRegistered` → 404, `Failed` → presumably 5xx (verify against `router.rs` directly — see note below).
- `POST /v1/completions` — HTTP-facing entrypoint to `AiRouter::complete`.

**Wiring in `main.rs`:**
- Loads declarative config via `ServerConfig` (`config/default.toml` by
  default, or `RALLEH_CONFIG`). The shipped default registers
  `FsReadTextHandler` / `FsWriteTextHandler` and the Allow /
  RequireApproval policy split (writes are a materially different risk
  tier than reads — deliberate; see DECISIONS.md).
- Boots a `JsonlFileAuditSink` writing to `RALLEH_AUDIT_LOG_PATH` (env var, defaults to a temp-dir path), passed into `ToolGateway::with_audit_sink`.
- AI backend selection: if `RALLEH_AI_BASE_URL` env var is set, boots a real `HttpCompletionBackend` (model/API key/backend-name configurable via `RALLEH_AI_MODEL`/`RALLEH_AI_API_KEY`/`RALLEH_AI_BACKEND_NAME`); otherwise falls back to `EchoBackend`.

**Config module (`config.rs`):** TOML or JSON; tools name a known
`HandlerKind`; rules deserialize straight into `PolicyRule` (optional
scoping fields use `#[serde(default)]`). Validation rejects duplicate
capabilities and empty rule reasons. See [`DECISIONS.md`](./DECISIONS.md).

**8+ tests** in the HTTP router (including the end-to-end
`write_text_capability_is_gated_dispatched_and_audited_end_to_end`
integration test) plus config-loader unit tests covering TOML/JSON parse,
duplicate-capability rejection, and empty-reason rejection.

## Design principle used throughout

Every "hot path" crate (`ralleh-tool-gateway`, `ralleh-ai-router`) follows
the same shape deliberately: a `Result`-returning or infallible trait
abstracting over "the thing that actually does work" (`ToolHandler` /
`CompletionBackend`), a test-double implementation (`EchoHandler` /
`EchoBackend`), at least one real implementation, and every call going
through a single chokepoint (`ToolGateway::dispatch` /
`AiRouter::complete`) that enforces policy first and emits an auditable
record regardless of outcome. This consistency was a deliberate choice so
a new engineer or agent only needs to learn the pattern once.
