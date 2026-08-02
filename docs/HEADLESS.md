# Headless vs Desktop Development

Default `cargo test --workspace` is **headless-safe**: no microphone, no
display, no Whisper/Piper binaries required.

## What runs on a headless host

| Surface | Default behavior |
|---------|------------------|
| VAD / wake-word | `MockAudioSource` frames only |
| STT / TTS | `MockStt` / `MockTts` |
| Frame assembly | Pure `FrameAssembler` (no `cpal`) |
| Pipeline smoke | mock mic → VAD → STT → TTS |
| Live mic (`cpal`) | **Not compiled** (`mic` feature off) |
| Whisper / Piper CLI e2e | `#[ignore]` — need env + downloaded tools |
| Tauri / clipboard / screen | Not implemented yet |

```bash
./scripts/bootstrap.sh
# or
cargo test --workspace
```

## Desktop / hardware opt-in

### Live microphone

```bash
# Linux: apt install libasound2-dev pkg-config
cargo test -p ralleh-audio-core --features mic
RALLEH_LIVE_MIC=1 cargo test -p ralleh-audio-core --features mic -- --ignored live_mic
```

- `try_open_default()` soft-fails (`Ok(None)`) on missing/broken devices,
  `RALLEH_SKIP_LIVE_AUDIO`, or `CI` without `RALLEH_LIVE_MIC=1`.
- `open_default()` still returns hard errors for real desktop apps.

### Real STT / TTS models

```bash
# Windows scripts under scripts/; download ggml + whisper-cli / piper + voice
cargo test -p ralleh-audio-core -- --ignored whisper_cli_e2e
cargo test -p ralleh-audio-core -- --ignored piper_cli_e2e
```

Env: `WHISPER_CLI_PATH`, `WHISPER_MODEL_PATH`, `PIPER_CLI_PATH`,
`PIPER_MODEL_PATH` — see [`ENVIRONMENT.md`](./ENVIRONMENT.md).

### In-process whisper-rs

```bash
cargo test -p ralleh-audio-core --features whisper -- --ignored whisper_rs_e2e
```

Needs cmake + libclang; often fails bindgen on Windows MSVC — use CLI path.

## Env knobs

| Variable | Effect |
|----------|--------|
| `RALLEH_LIVE_MIC=1` | Allow ignored live-mic smoke; disable CI auto-skip |
| `RALLEH_SKIP_LIVE_AUDIO` | Force soft-skip of live open |
| `CI` | Soft-skip live open unless `RALLEH_LIVE_MIC=1` |

## Rule for new desktop capabilities

Clipboard, screen capture, hotkeys, camera: ship **trait + mock** first,
gate OS bindings behind a cargo feature, and put hardware e2e behind
`#[ignore]` + an explicit env flag. Never let default `cargo test
--workspace` require a display or device.
