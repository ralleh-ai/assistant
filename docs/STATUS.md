# Status — Last Validated Snapshot

**As of:** 2026-08-01 (http-fetch SSRF harden + mic-capture)

## Build/test state

```
cargo test --workspace → headless-safe default features
```

## Highlights

- **HttpFetchHandler** — private/link-local/special IP block + DNS
  rebinding guard (hostname must resolve public; loopback only via
  explicit IP allowlist). Threat model T3 closed for current surface.
- **mic-capture** binary (`--features mic`) for local WAV recording.
- Headless audio defaults; Whisper/Piper CLI e2e opt-in; Anthropic + T1 auth.

## Next up

OIDC/device attestation; Cargo.lock; naming reconcile — see NEXT_STEPS.md.
