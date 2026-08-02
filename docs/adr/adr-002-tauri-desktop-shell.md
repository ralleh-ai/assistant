# ADR-002: Tauri v2 Desktop Edge Client, Rust-First Core

**Status:** Accepted (planning-time decision, copied from DEVELOPMENT.md §20) — **not yet implemented**

## Decision

Use Tauri v2 for the desktop assistant, with a **Rust-first core** (not a
thin capability shim) handling audio/wake/STT/TTS/intent/local-tool-
execution/privileged OS capabilities, and a React/TypeScript UI shell
handling only display/settings/onboarding.

## Reason

Secure, lightweight, cross-platform (Tauri uses the OS's native webview,
not a bundled browser engine). Tauri's permission/capability system
provides a built-in, auditable grant model for privileged operations,
which pairs directly with our own tenant/device policy engine. Keeping
hot-path logic in Rust rather than JS+native-shim lets the edge core share
code with server-side Rust services.

## Implementation status

Not started — **next implementation track** per
[`../NEXT_STEPS.md`](../NEXT_STEPS.md). `ralleh-audio-core` is the voice
logic that should sit behind this shell (VAD, wake-word, optional `mic`).
Forward threats: [`../THREAT_MODEL.md`](../THREAT_MODEL.md) T11–T16.
Headless rules for OS caps: [`../HEADLESS.md`](../HEADLESS.md).
