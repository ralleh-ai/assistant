# Ralleh — Enterprise Voice Assistant Development Document

**Version:** 0.3  
**Status:** Product/engineering planning reference (crystallized in Vault)  
**Audience:** Human engineers, coding agents, product/architecture reviewers  
**Product / wake-word name:** Ralleh (working name — trademark/domain clearance still pending)  
**Source inspirations reviewed:**
- <https://github.com/isair/jarvis>
- <https://github.com/atharva-shinde7/JARVIS-AI>

**Changelog:**
- v0.1: Initial dev doc from repo research (TypeScript/NestJS/Tauri+React baseline).
- v0.2: Renamed product/wake-word to "Ralleh".
- v0.3: Full review pass — Rust-first architecture propagated consistently through diagram/roadmap/repo-structure/ADRs; MVP scope decision locked instead of left open; redundancy trimmed between Edge Agent and Voice Pipeline sections; added streamlining notes.

---

## 1. Executive Summary

We will build an enterprise-grade, distributed, voice-powered assistant that combines local voice interaction, secure enterprise integrations, memory, workflow automation, admin governance, and extensible tool/plugin execution.

The product should feel like a private, always-available operator: users can speak naturally, ask questions, trigger workflows, dictate text, control approved local/enterprise tools, and receive spoken or visual responses. Unlike hobby Jarvis clones, this system must be secure, observable, multi-tenant, auditable, centrally governed, and deployable across desktops, servers, and managed enterprise environments.

The strongest inspiration comes from `isair/jarvis`: privacy-first local processing, natural wake-word placement, rolling transcript context, echo detection, long-term memory, MCP extensibility, smart tool selection, planner/evaluator loops, dictation, setup wizard, and evaluation suite. `atharva-shinde7/JARVIS-AI` contributes a simpler but useful first-layer decision-routing pattern, system/media automation, image generation, screen/camera analysis, and GUI-oriented assistant experience.

---

## 2. Recommended Primary Stack

### 2.1 Primary Language: TypeScript

**Recommendation:** Use **TypeScript** as the primary product language.

Rationale:
- Strong fit for distributed enterprise SaaS/control-plane development.
- Excellent ecosystem for APIs, realtime communication, admin UIs, auth, workflow orchestration, and integrations.
- MCP ecosystem is already heavily TypeScript-friendly.
- Shared types can span backend, SDK, web console, plugin schemas, and desktop UI.
- Easier enterprise hiring/maintenance than a pure Python desktop monolith or pure Rust distributed system.
- Fast iteration while still supporting strict typing, code generation, and policy enforcement.

### 2.2 Primary Backend Framework: NestJS

**Recommendation:** Build the enterprise control plane with **NestJS**.

Rationale:
- Opinionated modular architecture suitable for large teams.
- First-class support for REST, GraphQL, WebSockets, dependency injection, background workers, validation, guards, interceptors, and testing.
- Works well with OpenAPI, gRPC, queues, Prisma/Drizzle, OpenTelemetry, and enterprise auth patterns.
- Maps cleanly to bounded contexts: identity, devices, tenants, policies, memory, tools, workflows, audit, billing, admin.

### 2.3 Edge/Desktop Runtime: Tauri v2 + React UI + Rust-First Core

**Recommendation:** Build the desktop/edge client as a **Tauri v2** app with a **React/TypeScript UI shell** over a **Rust-first core** — not just a thin Rust capability layer under a JS app, but Rust as the default for anything real-time, security-critical, or hot-path.

**Rust owns (edge side):**
- Audio capture, VAD, wake-word detection, echo/interruption detection.
- Transcript buffering, rolling context window management.
- Intent judge (fast local classification pass).
- STT/TTS engine bindings — prefer native Rust crates/bindings (e.g. `whisper-rs`, Rust ONNX/Piper bindings) over spawning separate Python sidecars where a mature Rust binding exists, to avoid IPC overhead and reduce the process/dependency surface.
- Privileged OS capabilities: global hotkeys, mic/audio device access, screen/camera capture, clipboard, local file mediation, app launching, local process supervision.
- Local encrypted session/cache store and secure IPC to the control plane.

**TypeScript/React owns (edge side):**
- UI shell only: settings, chat transcript display, device enrollment flow, logs/status, onboarding wizard.
- No hot-path logic — the React layer renders state produced by the Rust core; it does not own audio, transcript, or policy logic.

Rationale: real-time audio (wake detection, echo/interrupt handling, transcript timing) is latency-sensitive and benefits from Rust's performance and lack of GC pauses. Keeping this logic in Rust rather than JS+native-shim also lets the edge core share code paths with server-side Rust services (policy evaluation, tool broker) via shared crates.

### 2.4 AI/Voice Runtime Sidecars (where native Rust bindings aren't mature enough)

Where a solid Rust-native binding doesn't yet exist or a provider is cloud/Python-only, fall back to sidecar processes:
- **STT:** faster-whisper (Python) as fallback if `whisper-rs` doesn't cover a needed model/feature; otherwise prefer `whisper-rs`/whisper.cpp via Rust bindings.
- **TTS:** Piper/Kokoro/Chatterbox via Rust bindings where available; ElevenLabs/enterprise cloud adapters as HTTP clients (language-agnostic, thin Rust or TS client is fine).
- **Local LLM:** Ollama, llama.cpp (has Rust bindings — `llama-cpp-rs`), vLLM, or OpenAI-compatible endpoint accessed via HTTP client.
- **Vision:** local screenshot/camera model where possible; cloud vision adapters where policy allows.

Sidecars (when used) should communicate over localhost gRPC/HTTP with authenticated IPC tokens and strict capability boundaries. Default bias: Rust-native binding over Python sidecar when a maintained option exists.

### 2.5 Data and Infrastructure

Recommended baseline:
- **PostgreSQL** for canonical enterprise data, tenants, policies, audit, devices, memory metadata.
- **pgvector** for memory/tool/document embeddings at enterprise scale.
- **Redis** for ephemeral state, hot sessions, rate limits, queues where appropriate.
- **NATS** for event bus and edge/control-plane messaging.
- **Temporal** for durable multi-step workflows and long-running automations.
- **OpenTelemetry + Prometheus/Grafana + Loki** for traces, metrics, and logs.
- **Kubernetes** for enterprise/server deployment; packaged single-node option for SMB/self-hosted.

---

## 3. Product Goals

### 3.1 User Goals

- Speak naturally to an assistant without rigid command syntax.
- Use wake word anywhere in a sentence.
- Ask follow-up questions without repeating context.
- Dictate into any application with a hotkey.
- Automate local and enterprise tasks safely.
- Ask the assistant to inspect screen/camera content where permitted.
- Search web, documents, tools, tickets, messages, and organizational knowledge.
- Receive accurate responses with citations and confidence where relevant.
- Choose local/private AI processing or governed cloud AI processing.

### 3.2 Enterprise Goals

- Multi-tenant architecture with strong tenant isolation.
- SSO/SAML/OIDC, SCIM, RBAC/ABAC, device enrollment, and admin governance.
- Central policy controls for what the assistant can hear, store, execute, and send externally.
- Complete audit trails for tool calls, privileged actions, policy decisions, memory writes, and external communications.
- Pluggable integrations through MCP and first-party connectors.
- Secrets never exposed to models or plugins unless explicitly mediated through capability-scoped references.
- Evals, regression tests, and observability to prove assistant quality over time.

---

## 4. Inspiration Repo Analysis

## 4.1 `isair/jarvis`

### Useful Features to Adopt

- **Privacy-first local operation:** Ollama/OpenAI-compatible local model support; no default cloud dependency.
- **Natural wake-word handling:** assistant name can appear anywhere in the sentence.
- **Rolling transcript context:** the system understands surrounding conversation before the wake word.
- **Hot-window follow-up mode:** allows immediate follow-ups after assistant responses without repeating the wake word.
- **Echo detection:** ignores its own TTS output and handles stop/interruption logic.
- **Whisper STT:** local ASR with confidence and no-speech thresholds to reduce hallucinated transcriptions.
- **Local TTS:** Piper and Chatterbox support, including possible voice cloning.
- **Dictation mode:** global hotkey, hold-to-talk, hands-free mode, filler removal, custom dictionary, paste into any app.
- **Memory:** diary summaries, knowledge graph, semantic recall, memory viewer UI, nutrition/domain-specific memory examples.
- **Smart tool selection:** keyword/embedding/LLM strategies to avoid loading every tool into every prompt.
- **MCP integration:** external tools without tightly coupling core assistant logic.
- **Tool-discovery escape hatch:** assistant can discover additional tools mid-turn when needed.
- **Planner:** decomposes multi-step queries before execution.
- **Evaluator:** validates agentic loop performance.
- **Digest passes:** memory/tool-result compression for small models and context-window control.
- **Setup wizard/settings UI:** practical onboarding for models, Whisper, dictation, MCP, and local providers.
- **Evals:** explicit evaluation suite for wake detection, intent judge, memory, tool routing, planner, web search, complex flows.

### Patterns to Reuse Conceptually

- Transcript-first listening pipeline.
- Separate fast model for intent/tool routing from main response model.
- Model-size-aware prompts and context budget strategy.
- Tool result digestion before final answer generation.
- Memory recall gate to avoid over-injecting irrelevant memory.
- MCP runtime abstraction and cached tool catalogue.
- Local location/time context with privacy-preserving behavior.
- Automatic redaction before memory persistence.

### Limitations to Avoid

- Primarily single-user desktop architecture.
- Python desktop packaging complexity at enterprise scale.
- Limited mobile/web/admin governance story.
- Local-only design does not directly solve fleet management, tenant governance, audit, SSO, or compliance.
- Voice-only limitation in some flows; enterprise product needs voice, text, API, admin, and automation channels.

## 4.2 `atharva-shinde7/JARVIS-AI`

### Useful Features to Adopt

- **First-layer decision model:** classifies user requests into categories before execution.
- **Query routing:** general knowledge vs realtime search vs automation vs image generation vs vision.
- **System automation:** open/close apps, media control, volume controls, web search, YouTube playback.
- **Screen/camera analysis:** vision-powered analysis of user environment.
- **GUI assistant experience:** visible chat transcript and status indicators.
- **Hybrid cloud/local architecture:** cloud LLMs where helpful, local automation where needed.
- **Source-aware realtime search:** search, synthesize, cite.

### Limitations to Avoid

- Hobby-project monolith structure.
- API-key-in-env pattern without enterprise secret management.
- Limited security model for system commands.
- Limited memory/context architecture.
- Limited test/eval/observability posture.
- Browser/Selenium speech recognition and cloud-heavy dependencies are not ideal for enterprise privacy/reliability.
- Windows-primary assumptions.

---

## 5. Product Principles

1. **Local first, cloud optional:** sensitive audio and context should remain local unless policy allows cloud processing.
2. **Policy before action:** every external/privileged action must pass tenant, user, device, and capability policy checks.
3. **Capability-based execution:** tools receive narrow, auditable capabilities rather than raw secrets or broad permissions.
4. **Human interruptibility:** users can stop speech, cancel workflows, and inspect pending actions.
5. **Transparent memory:** users and admins can inspect, edit, export, and delete memory according to policy.
6. **Observable by design:** traces, metrics, logs, evals, and audit events are first-class, not afterthoughts.
7. **Composable integrations:** MCP and connector SDKs should make integrations easy without compromising governance.
8. **Graceful degradation:** local mode should continue for voice/dictation/basic tools when the control plane or cloud model is unavailable.
9. **Least privilege:** local automation and enterprise integrations default to deny.
10. **Model portability:** support local, private cloud, and commercial LLM/STT/TTS providers behind stable adapters.

---

## 6. Target User Experience

### 6.1 Modes

- **Wake-word mode:** passive listening for configured wake word/phrase.
- **Push-to-talk mode:** press and hold a hotkey to speak a command.
- **Dictation mode:** press/hold or hands-free hotkey to transcribe into active app.
- **Text mode:** same assistant through desktop chat, web console, Slack/Teams, or API.
- **Hot-window mode:** short follow-up interval after assistant speech.
- **Meeting mode:** optional meeting-aware context capture with consent, diarization, summaries, and action extraction.
- **Automation mode:** user-approved workflows with progress display and cancellation.

### 6.2 Example Enterprise Flows

- “Assistant, summarize the customer escalation on my screen and draft a response.”
- “File a Jira ticket from this conversation and assign it to platform support.”
- “Start a Zoom meeting notes session and send me action items after.”
- “Search our runbooks for Redis failover and walk me through it.”
- “Dictate this into Slack: I’ll be five minutes late.”
- “Open the latest QBR deck for Acme and summarize slide 12.”
- “Create an incident report from the last hour of alerts.”

---

## 7. High-Level Architecture

```text
+--------------------------------------------------------------+
|                      Enterprise Control Plane                 |
|                                                              |
|  NestJS API Gateway (business/CRUD/integrations)              |
|  - Auth / SSO / SCIM                                         |
|  - Tenant / RBAC / ABAC                                      |
|  - Device registry                                            |
|  - Workflow orchestration API                                 |
|  - Admin console backend                                      |
|                                                              |
|  Rust Services (hot-path/security-critical, e.g. Axum)        |
|  - Policy engine core                                          |
|  - MCP / tool gateway (capability broker)                     |
|  - AI router (model/provider dispatch)                        |
|  - Audit event ingestion                                      |
|                                                              |
|  PostgreSQL + pgvector     Redis      NATS      Temporal      |
+--------------------------+------------------+----------------+
                           | secure mTLS/WebSocket/NATS
                           v
+--------------------------------------------------------------+
|                         Edge Agent                            |
|                                                              |
|  Tauri Shell: React/TS UI (settings, chat display, enrollment,|
|  logs, onboarding) — no hot-path logic                      |
|                                                              |
|  Rust Core (shares crates with server-side Rust services):    |
|  - Wake detection / VAD / echo-interrupt                      |
|  - Audio capture                                              |
|  - STT/TTS engine bindings (native Rust crates preferred)     |
|  - Intent judge                                               |
|  - Local transcript buffer                                    |
|  - Local tool executor                                        |
|  - Screen/camera/clipboard mediators (permission-scoped)       |
|  - Local encrypted cache                                      |
|  - Secure IPC to control plane                                 |
+--------------------------+-----------------------------------+
                           |
                           v
+--------------------------------------------------------------+
|                  AI / Tool Execution Layer                   |
|                                                              |
|  LLM Router                                                  |
|  - local Ollama/llama.cpp (llama-cpp-rs binding preferred)     |
|  - private vLLM endpoint                                     |
|  - OpenAI-compatible provider                                |
|                                                              |
|  MCP / Connectors (mediated by Rust tool gateway)              |
|  - GitHub, Slack, Jira, Google Workspace, Notion, databases   |
|  - Browser automation                                        |
|  - RPA/system automation                                     |
|  - Custom enterprise plugins                                 |
+--------------------------------------------------------------+
```

---

## 8. Core Services and Components

### 8.1 Edge Voice Agent

Responsibilities: see §2.3 (Rust core owns audio/wake/STT/TTS/intent/local-tool-execution/capabilities; React/TS owns UI shell only) and §9 (Voice Pipeline) for the runtime flow. Not re-described here to avoid duplication — this section covers only sync/lifecycle responsibilities not already specified:

- Sync events, memory candidates, and audit data to the control plane.
- Device identity certificate for secure control-plane connection (mTLS).
- Rust-native sidecar/process supervisor for any AI engine that isn't a native Rust binding (see §2.4).
- Local encrypted SQLite (or equivalent embedded store) cache for offline session state.

### 8.2 Control Plane API

Responsibilities:
- Tenant and org management.
- User identity, SSO, SCIM, RBAC/ABAC.
- Device enrollment and revocation.
- Central policy distribution.
- Tool registry and integration catalog.
- MCP server configuration with secret references.
- Memory indexing and governance.
- Workflow orchestration and approvals.
- Audit and compliance reports.

### 8.3 Policy Engine

Every tool call and privileged action must answer:
- Who is requesting this?
- From which tenant, device, session, and network posture?
- What tool/capability is being requested?
- What data will be read, written, transmitted, or remembered?
- Is human confirmation required?
- Should the result be redacted before model exposure or memory persistence?

Implementation:
- Rust policy evaluation core (shared crate between edge and control plane) as the default — not a TypeScript module — consistent with the Rust-first hot-path decision in §2.3–2.4. Declarative policy rules (YAML/JSON) are compiled/loaded by the Rust evaluator; NestJS calls into it via a local gRPC service or native binding rather than reimplementing evaluation logic in JS.
- Add OPA/Rego or Cedar as an alternative/supplement only if policy authoring complexity outgrows the custom Rust evaluator — evaluate this at Phase 2 exit, not upfront.
- Cache signed policy bundles on edge devices for offline enforcement.

### 8.4 Memory System

Memory layers:
1. **Hot context:** current utterance, recent transcript, active task state.
2. **Session memory:** recent conversation and tool results.
3. **User memory:** preferences, durable facts, personal workflows.
4. **Tenant knowledge:** docs, runbooks, FAQs, approved knowledge bases.
5. **Workflow memory:** state for long-running tasks.
6. **Audit memory:** immutable event trail, not injected into models by default.

Capabilities:
- Semantic search with pgvector.
- Entity and topic extraction.
- Knowledge graph for durable facts/relationships.
- Memory viewer/editor.
- Policy-based retention and deletion.
- PII/secrets redaction before persistence.
- Memory provenance: every memory item should know its source, timestamp, confidence, owner, and visibility scope.

### 8.5 Tool and Integration Layer

Tool types:
- **Local tools:** app launch, clipboard, file read/write, screen capture, camera, OS automation.
- **Enterprise SaaS tools:** Slack, Teams, Jira, GitHub, GitLab, Google Workspace, Microsoft 365, Notion, Salesforce, ServiceNow.
- **Data tools:** Postgres, Snowflake, BigQuery, Elasticsearch, internal APIs.
- **Browser tools:** controlled browser automation with explicit user/session policy.
- **RPA tools:** deterministic automation for legacy apps.
- **MCP tools:** third-party MCP servers mediated by enterprise policy.

Required guardrails:
- Tool schema validation.
- Capability scopes.
- SecretReference-based credentials.
- Prompt-injection fencing for tool outputs.
- SSRF and egress controls for web fetch/search.
- Confirmation gates for write/send/delete/financial/admin actions.
- Tool result digest before model injection when results are large or untrusted.

### 8.6 Workflow Orchestrator

Use Temporal for:
- Multi-step automations.
- Approval waits.
- Retryable tasks.
- Scheduled follow-ups.
- Long-running enterprise workflows.
- Human-in-the-loop state machines.

Example workflow:
1. User asks assistant to create an incident report.
2. Assistant plans subtasks.
3. Policy engine checks permissions.
4. Temporal workflow gathers alerts/logs/tickets.
5. LLM drafts report with citations.
6. User approves.
7. Tool executor creates ticket/postmortem page.
8. Audit event records all steps.

### 8.7 AI Router

The AI router abstracts provider/model choices:
- Fast model: intent classification, tool routing, redaction classification.
- Main model: conversation and synthesis.
- Planner model: task decomposition.
- Evaluator model: response/tool-call validation.
- Embedding model: memory and tool search.
- Vision model: screenshot/camera/document analysis.
- STT/TTS providers: local/cloud policy-aware adapters.

Routing inputs:
- Tenant policy.
- Data sensitivity.
- Latency requirements.
- Cost constraints.
- Offline/online state.
- Model capability requirements: tool calling, JSON mode, multimodal, embeddings.

---

## 9. Voice Pipeline

### 9.1 Listening Flow

1. Audio capture from microphone.
2. Voice activity detection.
3. Wake word or push-to-talk trigger.
4. STT transcription.
5. Confidence/no-speech filtering.
6. Rolling transcript context extraction.
7. Echo detection and stop command handling.
8. Intent judge classifies:
   - ignored/background conversation
   - assistant-directed request
   - stop/cancel
   - dictation
   - follow-up
   - local command
   - enterprise workflow
9. Query normalization and context synthesis.
10. Policy/memory/tool routing.
11. Response generation and/or tool execution.
12. TTS output and hot-window activation.

### 9.2 Echo and Interruption

Must support:
- Rejecting assistant’s own speech from being reprocessed.
- “Stop” or configured interruption commands during TTS.
- Partial echo cleanup when user speaks over assistant.
- Energy/timing/text-similarity heuristics plus model fallback.

### 9.3 Dictation

Required features:
- Global hotkey.
- Hold-to-dictate.
- Double-tap hands-free mode.
- Local transcription by default.
- Filler-word cleanup option.
- Custom dictionary.
- App-aware paste.
- Dictation history with policy-controlled retention.
- Main voice listener pauses during dictation.

---

## 10. Agentic Reasoning Pipeline

Recommended turn pipeline:

1. **Input sanitation:** normalize transcript and remove obvious ASR artifacts.
2. **Sensitivity classification:** identify secrets, regulated data, PII, and policy-sensitive content.
3. **Recall gate:** decide whether memory retrieval is useful.
4. **Memory retrieval:** fetch only relevant scoped memory.
5. **Tool selection:** choose a small set of relevant tools.
6. **Planner:** decompose multi-step requests.
7. **Policy preflight:** determine whether requested actions are allowed or need approval.
8. **Agentic loop:** call tools, inspect results, continue until done or blocked.
9. **Tool-result digest:** compress untrusted/large results into attributed facts.
10. **Evaluator:** check completeness, tool correctness, citation sufficiency, and policy compliance.
11. **Final response:** speak and/or display.
12. **Memory candidate extraction:** propose durable memories with confidence/provenance.
13. **Audit event emission:** record the full policy/tool/action trace.

---

## 11. Security Requirements

### 11.1 Hard Invariants

- No raw secret values are persisted in logs, memory, prompt dumps, traces, or tool outputs.
- Secrets are accessed only through brokered SecretReferences.
- Tool calls must be schema-validated and policy-authorized.
- External/untrusted content must be fenced before model exposure.
- Tenant isolation must be enforced at the data-access layer, not only application filters.
- Device enrollment must be revocable.
- Local automation must be least-privilege and user-visible.
- Destructive, external, financial, admin, or reputation-impacting actions require explicit confirmation unless covered by pre-approved policy.
- Prompt injection from webpages, emails, documents, tickets, and tool outputs must not be treated as instructions.
- Audio capture state must be visible and auditable.

### 11.2 Enterprise Controls

- SSO/OIDC/SAML.
- SCIM provisioning.
- RBAC and ABAC.
- Tenant/org/project scopes.
- Device posture checks.
- Data residency controls.
- Configurable retention policies.
- DLP/redaction pipeline.
- Audit export.
- Admin approval workflows.
- Integration credential rotation.
- Model/provider allowlists.
- Egress allowlists.

---

## 12. Observability and Quality

### 12.1 Observability

Capture:
- Voice pipeline latency: VAD, STT, intent judge, LLM, tool calls, TTS.
- Wake false positives/false negatives.
- STT confidence and no-speech drops.
- Tool selection accuracy.
- Planner/evaluator outcomes.
- Policy denials and approvals.
- Memory retrieval precision.
- Cost per turn.
- Provider/model errors.
- Local sidecar health.

### 12.2 Evaluation Suite

Build evals from day one:
- Wake word placement.
- Background conversation ignore behavior.
- Echo detection.
- Stop/interruption.
- Follow-up context.
- Dictation accuracy.
- Tool routing.
- Planner decomposition.
- Memory recall relevance.
- Prompt-injection resistance.
- Tool output digestion.
- Enterprise policy enforcement.
- Multi-tenant isolation.
- Offline behavior.
- Regression tests for known failure cases.

---

## 13. Data Model Sketch

Core entities:
- `Tenant`
- `User`
- `Group`
- `Role`
- `Policy`
- `Device`
- `Session`
- `Utterance`
- `TranscriptSegment`
- `AssistantTurn`
- `ToolDefinition`
- `ToolCapability`
- `ToolInvocation`
- `ConnectorInstallation`
- `SecretReference`
- `WorkflowDefinition`
- `WorkflowRun`
- `ApprovalRequest`
- `MemoryItem`
- `MemorySource`
- `KnowledgeEntity`
- `AuditEvent`
- `ModelProvider`
- `ModelRoute`
- `EvalCase`
- `EvalRun`

Important fields for `MemoryItem`:
- `tenant_id`
- `owner_user_id`
- `visibility_scope`
- `source_id`
- `source_type`
- `content_redacted`
- `embedding`
- `confidence`
- `sensitivity_label`
- `retention_expires_at`
- `created_at`
- `updated_at`

Important fields for `ToolInvocation`:
- `tenant_id`
- `user_id`
- `device_id`
- `session_id`
- `tool_id`
- `capability_scope`
- `input_redacted`
- `output_redacted`
- `policy_decision_id`
- `approval_request_id`
- `status`
- `latency_ms`
- `created_at`

---

## 14. API Surface

### 14.1 Control Plane APIs

- `POST /v1/devices/enroll`
- `POST /v1/sessions`
- `POST /v1/turns`
- `POST /v1/tools/invoke`
- `GET /v1/tools`
- `POST /v1/workflows/run`
- `GET /v1/workflows/:id`
- `POST /v1/approvals/:id/approve`
- `POST /v1/approvals/:id/reject`
- `GET /v1/memory/search`
- `POST /v1/memory`
- `PATCH /v1/memory/:id`
- `DELETE /v1/memory/:id`
- `GET /v1/audit/events`
- `GET /v1/admin/policies`
- `PUT /v1/admin/policies/:id`

### 14.2 Edge IPC APIs

- `voice.startListening`
- `voice.stopListening`
- `voice.setMode`
- `dictation.start`
- `dictation.stop`
- `tts.speak`
- `tts.stop`
- `screen.capture`
- `camera.capture`
- `clipboard.write`
- `app.open`
- `app.close`
- `sidecar.health`
- `sidecar.restart`

---

## 15. Development Roadmap

## Phase 0 — Product Foundation

Deliverables:
- Final product requirements document.
- Threat model.
- Architecture decision records.
- Data classification model.
- Initial integration catalog.
- Evaluation plan.

Exit criteria:
- Security invariants accepted.
- Stack agreed.
- MVP scope frozen.

## Phase 1 — Local MVP

**Locked scope decision (was previously an open question — resolved to avoid stalling execution):**
- Platforms: macOS + Windows first (Linux deferred to a later phase).
- Interaction mode: push-to-talk first; passive wake-word listening added once push-to-talk pipeline is proven and evals pass.
- Stack: Rust core (per §2.3) for audio/wake/STT/TTS/intent-judge/local-tool-execution; Tauri v2 shell with React/TS UI for chat/settings/onboarding only.

Deliverables:
- Tauri desktop shell (React/TS UI, Rust core).
- Local push-to-talk (wake-word detection added post-MVP once evals pass).
- Local STT via native Rust binding (`whisper-rs`/whisper.cpp) with Python sidecar fallback only if a required model/feature isn't covered.
- Local TTS via native Rust binding (Piper/Kokoro) with cloud adapter fallback.
- Basic chat UI (React/TS shell).
- Local LLM/OpenAI-compatible router (Rust, `llama-cpp-rs` preferred for local models).
- Simple tool executor: weather/search/local app open (Rust, policy-gated even at MVP stage).
- Dictation hotkey (Rust, via Tauri global-shortcut plugin).
- Local encrypted session store (Rust, via Tauri sql/stronghold plugins).

Exit criteria:
- User can speak (push-to-talk), get response, use dictation, and trigger a safe local tool.
- Basic evals for voice pipeline pass.

## Phase 2 — Control Plane MVP

Deliverables:
- NestJS API (business/CRUD/integrations: tenants, users, SSO, workflow definitions, admin console backend).
- Rust policy engine core service (Axum), called by NestJS via local gRPC/native binding.
- PostgreSQL schema.
- Device enrollment.
- User auth.
- Tool registry.
- Audit events (Rust ingestion service preferred for throughput; see §2.3).
- Admin UI basics.
- Edge-to-cloud secure channel (mTLS).

Exit criteria:
- Managed device can enroll, receive policy from the Rust policy core, execute allowed tools, and emit audit events.

## Phase 3 — Enterprise Memory and Tooling

Deliverables:
- pgvector memory search.
- Memory viewer/editor.
- Redaction pipeline.
- MCP connector runtime.
- Slack/GitHub/Jira/Google Workspace first-party connectors.
- Tool selection router.
- Tool-result digest.

Exit criteria:
- Assistant can search governed enterprise memory and safely invoke approved integrations.

## Phase 4 — Agentic Workflow Automation

Deliverables:
- Planner.
- Evaluator.
- Temporal workflow runner.
- Approval requests.
- Human-in-the-loop automations.
- Workflow templates.
- Long-running task state.

Exit criteria:
- Assistant can run auditable multi-step workflows with approvals and retries.

## Phase 5 — Vision, Meetings, and Advanced UX

Deliverables:
- Screen analysis.
- Camera analysis where policy permits.
- Meeting mode.
- Speaker diarization.
- Summaries/action items.
- Better interruption/duplex speech behavior.
- Mobile companion app assessment.

Exit criteria:
- Vision and meeting workflows pass security review and evals.

## Phase 6 — Hardening and Enterprise Release

Deliverables:
- SOC2-oriented controls.
- Pen test remediation.
- Load testing.
- Offline/failover testing.
- Installer/update signing.
- Fleet deployment docs.
- Admin/compliance reports.
- Full eval dashboard.

Exit criteria:
- Production-ready enterprise release candidate.

---

## 16. Suggested Repository Structure

```text
ralleh/
  apps/
    control-plane-api/        # NestJS API (business/CRUD/integrations/admin backend)
    admin-web/                # React admin console
    desktop-edge/             # Tauri app (React/TS UI shell + Rust core)
    docs-web/                 # Product/docs site
  packages/
    shared-types/             # Zod/OpenAPI-generated types (TS side)
    tool-sdk/                 # Tool/plugin SDK (TS, for connector authors)
    evals/                    # Evals and fixtures
    observability/            # Telemetry helpers
  crates/
    ralleh-core/               # Shared Rust core: audio, wake-word, VAD, echo/interrupt, transcript buffer, intent judge
    policy-core/               # Rust policy evaluation engine (shared edge + control-plane)
    mcp-gateway/               # Rust MCP/tool capability broker
    ai-router/                 # Rust model/provider dispatch
    memory-core/               # Memory schemas/retrieval utilities (Rust or thin Rust+SQL)
    audit-core/                # Rust audit event ingestion
  sidecars/
    stt-fallback/              # Python STT fallback (faster-whisper) only if no mature Rust binding covers a needed model
    tts-fallback/              # Python/cloud TTS fallback only if no mature Rust binding covers a needed voice
  infra/
    docker/
    helm/
    terraform/
    temporal/
  docs/
    architecture/
    threat-model/
    adr/
    runbooks/
```

---

## 17. Build vs Adopt Recommendations

Adopt where possible:
- STT: `whisper-rs`/whisper.cpp (Rust-native preferred); faster-whisper (Python) only as fallback.
- TTS: Piper/Kokoro Rust bindings preferred; Chatterbox/ElevenLabs cloud adapter as HTTP client.
- Local LLM: `llama-cpp-rs`/Ollama (Rust-native preferred); vLLM/OpenAI-compatible APIs via HTTP client.
- Workflow: Temporal.
- Messaging: NATS.
- Auth: OIDC/SAML provider integration rather than custom auth.
- Observability: OpenTelemetry.
- MCP: existing MCP protocol and SDKs, wrapped by our Rust gateway.
- Vector search: pgvector initially.

Build custom:
- Rust policy engine core (edge + control plane shared).
- Rust MCP/tool capability broker.
- Rust audio/voice pipeline orchestration (wake, VAD, echo/interrupt, transcript buffer).
- Edge/control-plane secure sync.
- Memory governance and provenance layer.
- Admin console tailored to assistant governance (NestJS/React).
- Evals specific to voice + enterprise automation.

---

## 18. Open Questions

1. Should the product be primarily self-hosted, SaaS, or hybrid from day one?
2. Which enterprise identity providers are mandatory first: Google Workspace, Microsoft Entra, Okta?
3. Is offline local-only mode a hard enterprise requirement or premium feature?
4. What is the first target platform: Windows, macOS, Linux, or all three?
5. What are the first 5 enterprise integrations we care about?
6. Should meeting mode be included in MVP or later?
7. What regulated data regimes must be supported initially: HIPAA, SOC2, GDPR, FINRA, CJIS?
8. What level of user-visible memory editing is required at launch?
9. Will customers allow cloud LLMs, or must all inference support private/local deployment?
10. Should the assistant be branded as a personal operator, enterprise copilot, or automation fabric?

---

## 19. MVP Recommendation

The best first MVP is not a full “Jarvis for everything.” It should prove the hardest product thesis safely:

**MVP:** A managed desktop voice assistant that supports local wake/push-to-talk, local dictation, governed enterprise search, and one approved workflow integration.

Suggested MVP scope:
- Desktop app for macOS + Windows.
- Push-to-talk first; wake word second if needed.
- Local STT and TTS.
- Text fallback chat UI.
- Control plane with device enrollment, policy, audit.
- Memory search over a small enterprise knowledge base.
- GitHub/Jira/Slack or Google Workspace integration.
- Human confirmation for all write/send actions.
- Evals for voice, routing, memory, and policy.

This gives us the enterprise spine early: device management, policy, audit, memory, and tool execution. More magical features like passive wake-word conversation, screen/camera vision, and meeting mode can then be added without rewriting the foundation.

---

## 20. Key Architectural Decisions

### ADR-001: Polyglot Split — Rust for Hot-Path/Security-Critical, TypeScript for Business Logic

Decision: Use **Rust** as the default for real-time, security-critical, and hot-path components (edge voice/audio core, policy engine, MCP/tool gateway, AI router, audit ingestion). Use **TypeScript + NestJS** for business-CRUD control-plane logic (tenants, RBAC, SSO, billing, workflow definitions, admin console backend) and the admin/desktop UI shells.

Reason: Real-time audio (wake detection, echo/interrupt, transcript timing) and the security spine (policy evaluation, tool brokering, audit) benefit from Rust's performance, memory safety, and lack of GC pauses — and can share crates between edge and server. Business logic and enterprise integrations (OIDC, SaaS SDKs, admin UI) remain faster to build and hire for in TypeScript/NestJS. This supersedes the earlier "TypeScript primary language" framing (v0.1) — Rust is now co-primary, scoped by where hot-path/security-critical logic lives.

### ADR-002: Tauri v2 Desktop Edge Client, Rust-First Core

Decision: Use Tauri v2 for the desktop assistant, with a **Rust-first core** (not a thin capability shim) handling audio/wake/STT/TTS/intent/local-tool-execution/privileged OS capabilities, and a React/TypeScript UI shell handling only display/settings/onboarding.

Reason: Secure, lightweight, cross-platform (Tauri uses the OS's native webview, not a bundled browser engine). Tauri's permission/capability system provides a built-in, auditable grant model for privileged operations, which pairs directly with our own tenant/device policy engine. Keeping hot-path logic in Rust rather than JS+native-shim lets the edge core share code with server-side Rust services.

### ADR-003: Rust-Native AI Bindings Preferred Over Python Sidecars

Decision: Prefer native Rust bindings (`whisper-rs`, `llama-cpp-rs`, Rust Piper/Kokoro bindings) for STT/TTS/local-LLM engines. Fall back to Python or cloud sidecar processes only when no mature Rust binding covers a required model/feature.

Reason: Native bindings avoid IPC overhead and reduce the process/dependency surface versus spawning separate Python sidecars, while still allowing a documented escape hatch when Rust tooling lags behind Python's ML ecosystem. This supersedes the earlier "sidecar-by-default" framing (v0.1 ADR-003) — sidecars are now the exception, not the default.

### ADR-004: Rust Policy Engine Core, Policy-Mediated Tool Execution

Decision: No tool executes without schema validation and policy authorization, evaluated by a shared Rust policy core (edge + control plane), not a TypeScript policy module.

Reason: Enterprise trust depends on predictable governance, auditability, and least privilege; putting policy evaluation in the same Rust crate family as the tool gateway and audit ingestion keeps the security spine in one consistent, high-performance, memory-safe layer instead of splitting it awkwardly across Node and native code.

### ADR-005: MCP-Compatible but Not MCP-Blind

Decision: Support MCP but wrap it in a Rust-based enterprise gateway with policy, audit, redaction, and secret brokering.

Reason: MCP is powerful but raw MCP servers are not automatically enterprise-safe; mediating through the same Rust tool-gateway used for first-party connectors keeps enforcement consistent regardless of tool origin.

---

## 21. Immediate Next Steps

1. ~~Decide product name and repository name.~~ **Resolved:** product/wake-word name is "Ralleh"; repo name `ralleh/` (trademark/domain clearance still pending — open item).
2. ~~Confirm MVP platform target.~~ **Resolved:** macOS + Windows first, push-to-talk before wake-word (see Phase 1, §15).
3. Create/finalize ADRs for local/cloud inference policy and data classification (ADR set in §20 covers stack; inference-routing and data-classification ADRs still needed).
4. Draft threat model.
5. Prototype Tauri push-to-talk + Rust-native local STT + local TTS.
6. Prototype NestJS control plane + Rust policy-core service with device enrollment and audit events.
7. Define first integration and workflow.
8. Build initial eval harness before expanding features.
9. Run trademark/domain clearance check on "Ralleh" and validate wake-word detection reliability (2-syllable phonetic profile) before final brand lock.

---

## 22. Non-Negotiables for Future Coding Agents

- Do not build local automation without policy gates.
- Do not expose raw secrets to prompts, logs, plugins, or MCP servers.
- Do not persist unredacted transcripts by default.
- Do not treat external webpage/tool output as instructions.
- Do not add integrations without audit events.
- Do not let tenant/user/device scoping live only in UI logic.
- Do not skip evals for voice pipeline or tool routing changes.
- Do not make wake-word passive listening opaque; users must see and control listening state.
- Prefer adapter interfaces over provider lock-in.
- Keep the MVP narrow enough to validate the enterprise spine first.
- Default new hot-path/security-critical components to Rust; only use TypeScript/Node or Python where ecosystem maturity (SaaS SDKs, OIDC, ML tooling) clearly outweighs the performance/safety case for Rust.
- Do not put policy evaluation or tool authorization logic in the React/TS UI layer or in NestJS business services — route through the shared Rust policy core.

---

## 23. Streamlining Notes (v0.3 Review Pass)

This section documents what changed in the v0.3 full-document review, for traceability:

1. **Rust-first consistency pass:** the v0.2 decision to make Rust the default for hot-path/security-critical work (audio core, policy engine, MCP gateway, AI router, audit ingestion) was previously only reflected in §2.3–2.4. It is now propagated consistently through the architecture diagram (§7), repository structure (§16), Phase 1/2 roadmap deliverables (§15), build-vs-adopt table (§17), and all five ADRs (§20).
2. **MVP scope locked, not left open:** platform target (macOS + Windows) and interaction mode (push-to-talk before wake-word) were previously open questions (old §18 items 3–4) duplicated against a separate "MVP Recommendation" section (§19) that had already answered them. Resolved to avoid stalling execution and to remove the contradiction between an "open question" and an existing recommendation.
3. **Redundancy trimmed:** §8.1 (Edge Voice Agent) previously restated the full voice pipeline already covered in §9; it now cross-references §2.3/§9 and lists only sync/lifecycle responsibilities not covered elsewhere.
4. **Repo/product naming aligned:** repository structure renamed from generic `enterprise-voice-assistant/` to `ralleh/`, matching the locked product/wake-word name.
5. **Immediate Next Steps updated:** items resolved during this conversation (product name, MVP platform target) are marked resolved rather than left as pending action items, and a new item was added for trademark/domain clearance and wake-word detection validation on "Ralleh."
6. **Non-Negotiables extended:** added two rules codifying the Rust-first default and prohibiting policy/authorization logic from leaking into UI or Node business-service layers, closing a gap where future coding agents could reintroduce policy logic in the wrong layer.

No product goals, security invariants, or phase sequencing were changed in substance — this pass is architecture-consistency and scope-locking, not a strategy change.
