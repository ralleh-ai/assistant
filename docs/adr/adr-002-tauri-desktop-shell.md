# ADR-002: Tauri v2 Desktop Edge Client, Rust-First Core

**Status:** Accepted — Phase 1 scaffold in progress (`desktop-edge/`)

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

**Scaffold + voice smoke + station log:** [`../../desktop-edge/`](../../desktop-edge/)
Tauri v2 + React/TS; IPC `core_ping`, `voice_smoke` (mock pipeline via
`ralleh-audio-core`), and edge settings load/save (app config dir only —
no webview FS). Live mic and OS capabilities not wired yet.
Forward threats T11–T16; headless rules in [`../HEADLESS.md`](../HEADLESS.md).
