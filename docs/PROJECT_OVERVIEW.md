# Project Overview

## What Ralleh is

Ralleh is an enterprise-grade, distributed, voice-powered assistant. Product
vision: users speak naturally, ask questions, trigger workflows, dictate
text, control approved local/enterprise tools, and receive spoken or visual
responses — but unlike hobby "Jarvis clone" projects, it must be secure,
observable, multi-tenant, auditable, centrally governed, and deployable
across desktops, servers, and managed enterprise environments.

Full product/architecture reasoning, inspiration analysis, and phased
roadmap: see [`DEVELOPMENT.md`](./DEVELOPMENT.md) in this folder (copied
from the original planning doc at
`projects/voice-assistant/DEVELOPMENT.md` in the wider workspace — that
original location may not exist in a new environment, which is exactly why
it's duplicated here).

## What this repository is

This repo (`ralleh-ai/assistant` on GitHub) is the **buildable code
repository** — the Rust implementation of the hot-path/security-critical
"spine" described in `DEVELOPMENT.md`'s Rust-first architecture (ADR-001).
It is *not* the full product (no Tauri desktop shell, no NestJS control
plane, no React UI yet) — it is the foundational Rust crate workspace that
those layers will eventually sit on top of / call into.

Guiding rule (from the original README, preserved): **build in small,
independently validated steps.** Each core module must have a real automated
test suite and pass before the next module is layered on top.

## Current implementation status (high level)

As of the last working session, this repo has:

- A working **policy engine** (`ralleh-policy-core`) — allow/deny/require-approval rules, tenant/device/actor/capability-prefix/sensitivity matching, first-match-wins semantics.
- A working **tool gateway** (`ralleh-tool-gateway`) — capability-based dispatch, policy-gated, with two real (non-mocked) handlers: sandboxed filesystem read and write.
- A working **audit persistence layer** (`ralleh-audit-store`) — every gateway dispatch event is durably written to an append-only JSONL log before the caller gets a response.
- A working **AI completion router** (`ralleh-ai-router`) — policy-gated routing to a pluggable `CompletionBackend`, with both a test-only `EchoBackend` and a real `HttpCompletionBackend` (OpenAI-compatible `/chat/completions` wire format, works against OpenAI/Ollama/vLLM/llama.cpp).
- A working **HTTP server** (`ralleh-mcp-server`) — Axum-based, wires the gateway + router + audit sink together behind `/v1/tools/dispatch`, `/v1/completions`, `/healthz`.
- A stubbed **audio core** (`ralleh-audio-core`) — VAD, wake-word, and audio-source abstractions with mock/test sources; no real microphone capture or STT/TTS bindings yet (Phase 1 of the roadmap, not yet started for real I/O).

97 tests passing across the workspace as of the last session. See
[`STATUS.md`](./STATUS.md) for the precise, dated snapshot and
[`ARCHITECTURE.md`](./ARCHITECTURE.md) for how each crate maps to the
DEVELOPMENT.md plan.

## What this repo is explicitly *not* yet

- No Tauri desktop shell / React UI (Phase 1 deliverable per DEVELOPMENT.md §15, not started).
- No NestJS control plane, no Postgres/pgvector, no Redis, no NATS, no Temporal (Phase 2+).
- No real audio capture, no real STT/TTS engine bindings (`whisper-rs`, Piper/Kokoro) — `ralleh-audio-core` currently operates on mock/synthetic audio frames for VAD/wake-word logic testing only.
- No MCP connector runtime, no first-party SaaS connectors (Slack/Jira/GitHub/Google Workspace) — Phase 3.
- No SSO/SCIM/RBAC/multi-tenant control plane — the policy engine supports tenant/device/actor scoping in its data model, but there is no actual multi-tenant control plane wired around it yet.
- Policy rules are currently **hardcoded in `main.rs`**, not loaded from a config file or database — this is a known, deliberate gap for the current stage (see [`NEXT_STEPS.md`](./NEXT_STEPS.md)).
