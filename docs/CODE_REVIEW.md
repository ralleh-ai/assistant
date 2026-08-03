# Ralleh — Enterprise Hardening Code Review

> **Reviewer's framing.** This is a deep, security-first "read every load-bearing
> line" review of the Ralleh assistant workspace, written the way a principal
> engineer would hand it to the team before a security-sensitive GA. It is
> deliberately blunt about weaknesses, but it opens by saying the true thing:
> **this is unusually disciplined code.** The security architecture is coherent,
> the invariants are enforced in more than one layer, and the tests actually pin
> the invariants that matter. The findings below are the delta between "very
> good" and "enterprise-grade / auditable by a hostile third party."

- **Date:** 2026-08
- **Scope:** entire repo — Rust workspace (`crates/*`), desktop shell
  (`desktop-edge/`), presence prototype (`presence-prototype/`, `crates/presence-ipc`),
  CI/build/config, and the TypeScript/React frontend.
- **Companion docs:** builds on and cross-references
  [`THREAT_MODEL.md`](./THREAT_MODEL.md) (threat IDs `T1`–`T22` are reused here),
  [`ARCHITECTURE.md`](./ARCHITECTURE.md), and [`NEXT_STEPS.md`](./NEXT_STEPS.md).

---

## 0. A note on the stack

The engagement brief asked for a review of a **Go desktop application** against
**Go and TypeScript** best practices. The repository is not Go — it is a **Rust**
Cargo workspace plus a **Tauri v2** desktop shell with a **TypeScript/React**
webview. This review is written against the *actual* stack: idiomatic Rust
(ownership, error handling, `unsafe` avoidance, `clippy`), Tauri security
posture, and modern TypeScript/React. Where the brief's intent maps cleanly
("secure OS/mic/camera capabilities", "enterprise-grade", "bullet-proof"), that
intent drove the review regardless of language.

---

## 1. Executive summary

Ralleh is built around a **single security chokepoint** — every privileged
action funnels through a deny-by-default policy engine, a tool gateway, and an
audit sink — and that discipline holds consistently across crates. Secrets are
write-only from the UI's perspective and land in the OS keychain; egress is
allowlisted with real SSRF/DNS-rebinding defenses; the audit log is hash-chained
and tamper-evident; the Tauri capability surface is minimal. The TypeScript is
`strict`, uses discriminated unions well, and never reaches for
`dangerouslySetInnerHTML` or `eval`.

The gaps that stand between this and "enterprise-grade" are **not** architectural
— they are a focused set of hardening items:

| # | Severity | Finding | Where |
|---|----------|---------|-------|
| **C1** | Critical | Bearer-token comparison is not constant-time (timing side channel on the auth secret) | `ralleh-mcp-server/src/auth.rs` |
| **C2** | Critical | MCP server has no production hardening: no rate limit, body-size cap, request timeout, concurrency cap, graceful shutdown, or TLS | `ralleh-mcp-server` |
| **H1** | High | `fs_write_handler` can be escaped by a symlink in the final path component (and `exists()` follows symlinks) | `ralleh-tool-gateway/src/fs_write_handler.rs` |
| **H2** | High | HTTP-fetch SSRF check is TOCTOU: the handler resolves DNS to validate, `reqwest` re-resolves to connect | `ralleh-tool-gateway/src/http_fetch_handler.rs` |
| **H3** | High | Audit writes `flush()` but never `sync_all()` — a crash/power-loss can lose "durably recorded" events on both sinks | `ralleh-audit-store/src/sink.rs`, `desktop-edge/.../audit.rs` |
| **H4** | High | `panic!`/`unwrap()` on operational failures (server bootstrap, approval persistence) turns recoverable errors into crashes | `ralleh-mcp-server/src/main.rs`, `ralleh-tool-gateway/src/approval.rs` |
| **H5** | High | Tauri app-defined `invoke` commands are **not** ACL-restricted — the `core:default` capability + bare `build.rs` leave all ~30 custom commands (mic, secret-save, diagnostics…) reachable from the webview; the capability comment overclaims "allowlisted core IPC only" | `desktop-edge/src-tauri/{build.rs, capabilities/default.json}` |
| **H6** | High | Presence IPC reads stdin with unbounded `read_line` and an unbounded `mpsc::channel()` — a malformed/hostile peer can force unbounded memory growth (DoS) on both runtime and shell sides | `presence-runtime/src/ipc_stdin.rs`, `desktop-edge/.../presence.rs` |
| **H7** | High | `BackendSettings` resets the form on every 15s status poll (`useEffect` deps `[open, status]`) — silently wipes an operator's in-progress base-URL/model/API-key edits | `desktop-edge/src/BackendSettings.tsx` |
| **H8** | High | Audio utterance buffers are unbounded (`collect_utterance`, wake-word `current_utterance`) and CLI STT/TTS subprocesses have no timeout or input-size cap — stuck VAD / hung child → unbounded RAM + retained voice PII | `ralleh-audio-core/src/{pipeline.rs, wakeword.rs, stt.rs, tts.rs}` |
| **M1** | Medium | Desktop audit log re-reads the whole file on every append to find the last hash — O(n) per write, O(n²) to rotation | `desktop-edge/.../audit.rs` (`last_hash_in`) |
| **M2** | Medium | API tokens via `RALLEH_API_TOKENS` env var are process-listing-visible, char-restricted (`:`/`;`), and token files have no permission check | `ralleh-mcp-server/src/auth.rs` |
| **M3** | Medium | `WhisperCliStt` writes **raw microphone audio** to a predictable path in the shared temp dir with no crash-safe cleanup (RAII) | `ralleh-audio-core/src/stt.rs` |
| **M4** | Medium | Audit log is tamper-*evident* only; no signing/HMAC/external anchoring for tamper-*proof* | audit stores |
| **M5** | Medium | Live-mic callback silently drops frames under backpressure and never surfaces stream errors (no counter/metric) | `ralleh-audio-core/src/cpal_source.rs` |
| **M6** | Medium | No supply-chain gate in CI (`cargo-audit`/`cargo-deny`, `npm audit`) and no checksums for downloaded whisper models/binaries | CI, `T10` |
| **M7** | Medium | `AudioFrame` derives `Serialize` over raw PCM — an accidental log/IPC/audit serialize embeds the full voice waveform (PII) | `ralleh-audio-core/src/source.rs` |
| **M8** | Medium | `assistant_diagnostics_bundle` accepts an arbitrary `destDir` with no path confinement (unlike `reveal_path_in_file_manager`); UI passes `null` today but the IPC surface is broader than needed | `desktop-edge/.../diagnostics.rs` |
| **M9** | Medium | Presence render loop can stall: GPU `SurfaceError` early-returns skip `request_redraw()`, so heartbeats/IPC drain stop until the next window event; `OutOfMemory` logs "exiting" but doesn't | `presence-runtime/src/app.rs` |
| **M10** | Medium | Frontend async overlap: `refreshStatus`/status-poll/probe and Test-vs-Save have no in-flight guard, so responses can apply out of order | `desktop-edge/src/BackendSettings.tsx`, `PresenceStatusLine.tsx` |
| **M11** | Medium | `Conversation.tsx` streaming has no unmount/cancel guard — leaving Core mid-stream can `setState` on an unmounted tree | `desktop-edge/src/Conversation.tsx` |
| **M12** | Medium | Per-frame allocation of the full instance buffer (~120k particles, ~5–6 MB) causes allocator pressure / frame hitches | `presence-core/src/render/mod.rs` |
| **L1–L9** | Low/nit | CSP `style-src 'unsafe-inline'` + missing `base-uri`/`object-src`/`frame-ancestors`; `eprintln!` in a library; dev-mode auth default; approvals not bound to approver identity; wire-version exact-match; Google Fonts CDN; misc. | various |

Nothing here is a "stop the release" architectural rewrite. Every item is a
bounded, well-scoped change, and most are 1–3 files. Section 8 gives a
prioritized roadmap.

---

## 1a. Resolution status (2026-08)

**Every finding above (C1–C2, H1–H8, M1–M12, L1–L9, plus the follow-ups F3)
has been implemented.** The changes were made in the crates/files named in each
row; the remediation detail for each item lives in its numbered section below.
Highlights:

- **Auth & server (C1, C2, H4, L3):** constant-time bearer-token comparison,
  hardening middleware (rate limit, body-size cap, request timeout, concurrency
  cap, graceful shutdown), removal of bootstrap `panic!`/`unwrap`, and a
  fail-closed `RALLEH_REQUIRE_AUTH`.
- **Tool gateway (H1, H2, H4):** symlink-leaf rejection + atomic `create_new`
  writes, DNS-pinned SSRF connection, and fail-open-logged approval persistence.
- **Audit durability (H3, M1, L2):** `sync_all()` on both sinks, cached last
  hash on the desktop sink (O(1) append), structured logging.
- **Audio pipeline (H8, M3, M5, M7):** bounded utterance/wake-word buffers,
  timeout-bounded STT/TTS subprocesses with input caps, RAII secure temp files,
  mic drop-counter/error surfacing, and `#[serde(skip)]` over raw PCM.
- **Presence IPC (H6, M9, M12, L8):** bounded (capped) line reads and a bounded
  channel on both runtime and shell sides, redraw-on-`SurfaceError`, real exit
  on `OutOfMemory`, reused instance buffer with hysteresis, and a version
  *range* compatibility check.
- **Tauri shell (H5, M8, L1, L5, L9):** every custom `invoke` command is now
  declared in `build.rs` and explicitly allow-listed in the capability;
  diagnostics `destDir` confined under the app config dir; tightened production
  CSP (no `unsafe-inline`, added `object-src`/`base-uri`/`frame-ancestors`) with
  a separate dev CSP for Vite HMR; self-hosted fonts; Windows reveal-path fix.
- **Frontend (H7, M10, M11, F3):** form reset only on open transition, async
  overlap guards + stream cancellation, and a top-level error boundary.
- **Supply chain (M6):** `cargo-deny` + `npm audit` CI gate and SHA-256
  checksum verification for downloaded whisper/piper models and binaries.

**Local verification status.** Fully green:

- `cargo clippy --all-targets -- -D warnings` passes on **all three** Rust
  projects — the main workspace (including the `ring`-dependent
  `ralleh-mcp-server`, `ralleh-tool-gateway`, `ralleh-ai-router`,
  `ralleh-audit-store`), the `desktop-edge/src-tauri` Tauri shell, and the
  `presence-prototype` (wgpu) tree.
- `cargo fmt --check` is clean across all three projects.
- `ReadLints` is clean on every edited Rust/TypeScript file; the frontend
  `tsc --noEmit` and `npm run build` pass.

(Compiling the `ring`/C-linking crates locally requires a *complete* MSVC
toolchain + Windows SDK; a fresh VS preview toolset that ships without the
desktop x64 CRT import libraries will fail at the linker with `LNK1104:
msvcrt.lib`. Build from a "Native Tools" developer shell — or repair the
"Desktop development with C++" workload — so `rustc` links against a complete
toolset.)

---

## 2. What is genuinely excellent (keep doing this)

A review that only lists faults is misleading. These are real strengths and they
should be protected during any refactor:

1. **Deny-by-default policy with defense-in-depth validation.** `PolicyEngine::evaluate`
   validates the request *before* rule matching, and handlers (`fs_read`,
   `fs_write`, `http_fetch`) **re-enforce their own confinement regardless of what
   policy already allowed**. Two independent layers must both fail for an escape.
2. **Egress control that actually resists SSRF.** Hostname allowlist, HTTPS-only
   for non-loopback, no redirects, no URL userinfo, and a link-local/special-IP
   block with a DNS-rebinding guard. This is materially better than most shipping
   products (modulo the TOCTOU narrowing in H2).
3. **Write-only secret model, end to end.** The webview never receives the key
   (`hasApiKey: boolean` only); the `ApiKeyUpdate` union defaults to `{ op: "keep" }`
   so a key can never be cleared by omission; storage falls back from OS keychain
   to cleartext **with a visible warning badge** rather than silently. This is
   exactly the right shape.
4. **Tamper-evident audit chain with honest limits.** SHA-256 prev-hash chaining,
   a `verify()` that localizes tampering to a line number, and documentation that
   explicitly says "tamper-evident, not tamper-proof." Honesty in the doc is worth
   as much as the code.
5. **Minimal Tauri *plugin* attack surface.** `core:default` capability only — no
   `shell`, `fs`, or `http` plugins are exposed to JS — a locked-down CSP, no
   remote content, and every reverse-channel path bounded (`audit_tail` clamps
   `[1,500]`, `presence_log_tail` clamps `[1,1000]`). *Caveat:* this does not
   extend to the app's own `invoke` commands, which are not individually
   ACL-gated — see **H5**.
6. **DoS-aware streaming.** The SSE parser has a bounded buffer
   (`SSE_MAX_BUFFER_BYTES`); HTTP backends set explicit timeouts and disable
   redirects.
7. **Real-time-correct audio capture.** The `cpal` callback never blocks — it
   `try_send`s and drops on backpressure, which is the *correct* choice inside an
   audio callback (see M5 for the one refinement: make the drop observable).
8. **Wire-format hygiene.** `presence-ipc` versions every envelope, uses
   `#[non_exhaustive]` enums, `#[serde(default)]` for additive compatibility, and
   pins exact wire spellings with round-trip tests.
9. **Strict, modern TypeScript.** `strict`, `noUnusedLocals`,
   `noUnusedParameters`, `noFallthroughCasesInSwitch`; state modeled as
   discriminated unions; a `safeInvoke` wrapper so a misbehaving renderer can't
   surface IPC noise to users.
10. **The tests pin the invariants that matter** — sandbox escape rejection,
    audit tamper detection, egress denial, wake-word state machine, secret
    redaction — not just happy paths.

---

## 3. Critical findings

### C1 — Bearer-token comparison is not constant-time

**Where:** `crates/ralleh-mcp-server/src/auth.rs`, `TokenAuthenticator::authenticate`

```150:154:crates/ralleh-mcp-server/src/auth.rs
        self.by_token
            .get(token)
            .cloned()
            .ok_or(AuthError::UnknownToken)
```

**Problem.** `HashMap::get` compares the presented token against stored keys with
ordinary `==`, which short-circuits on the first differing byte, and the hash
lookup itself is input-dependent. Both leak timing information about a **secret**
(the shared bearer token). Over enough samples an attacker on the same host/network
can use timing to recover a valid token byte-by-byte. This is the textbook case
for constant-time comparison.

**Impact.** Authentication bypass via timing oracle. `T1` (cross-tenant capability
use) partially rests on these tokens, so this undermines the strongest current
auth control.

**Fix.** Compare in constant time and avoid using the secret as a hash key on the
hot path. Minimal change using a vetted primitive:

```toml
# crates/ralleh-mcp-server/Cargo.toml
subtle = "2"
```

```rust
use subtle::ConstantTimeEq;

pub fn authenticate(&self, authorization: Option<&str>) -> Result<CallerIdentity, AuthError> {
    let header = authorization.ok_or(AuthError::MissingToken)?;
    let presented = header
        .strip_prefix("Bearer ").or_else(|| header.strip_prefix("bearer "))
        .map(str::trim).filter(|t| !t.is_empty())
        .ok_or(AuthError::MalformedHeader)?;

    // Constant-time scan of all tokens; do not early-return on match so the
    // timing does not depend on WHERE the match is or WHETHER it exists.
    let mut found: Option<&CallerIdentity> = None;
    for (token, identity) in &self.by_token {
        let hit = token.as_bytes().ct_eq(presented.as_bytes());
        if bool::from(hit) {
            found = Some(identity);
        }
    }
    found.cloned().ok_or(AuthError::UnknownToken)
}
```

For larger token sets, prefer hashing tokens (e.g. SHA-256) at load time and
comparing fixed-length digests in constant time, which keeps the loop cheap while
removing the length/content leak. Also apply constant-time comparison to any
future secret comparisons (`enforce` compares tenant/actor labels, which are not
secrets and are fine as-is).

---

### C2 — The MCP HTTP server has no production hardening

**Where:** `crates/ralleh-mcp-server` (router/main)

**Problem.** The Axum surface, as configured, is missing the middleware every
internet- or LAN-exposed service needs:

- **No request rate limiting / concurrency cap** — a single caller can exhaust
  CPU, sockets, or the audit disk.
- **No explicit request body size limit** — relying on framework defaults is not
  an auditable control; large bodies reach JSON parsing.
- **No per-request timeout** — a slow-loris or a wedged handler ties up a worker
  indefinitely.
- **No graceful shutdown** — in-flight tool dispatches and audit writes can be
  cut mid-flight on SIGTERM/redeploy.
- **No TLS termination story** — bearer tokens (C1) traverse the wire; without
  TLS (or an explicit "must sit behind a TLS proxy" contract) they are exposed.

**Impact.** Availability (DoS) and confidentiality. For an "enterprise-grade"
bar this is table stakes.

**Fix.** Layer `tower-http` / `tower` middleware at the router root and add
graceful shutdown. Example:

```rust
use std::time::Duration;
use tower::{ServiceBuilder, limit::ConcurrencyLimitLayer};
use tower_http::{
    limit::RequestBodyLimitLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

let app = router
    .layer(
        ServiceBuilder::new()
            .layer(TraceLayer::new_for_http())
            .layer(TimeoutLayer::new(Duration::from_secs(30)))
            .layer(RequestBodyLimitLayer::new(1 * 1024 * 1024)) // 1 MiB, explicit
            .layer(ConcurrencyLimitLayer::new(256)),
    );

axum::serve(listener, app)
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
```

Add a real rate limiter (e.g. `tower_governor`) keyed by authenticated identity
+ peer IP. Document explicitly whether the server is expected to terminate TLS
itself (`axum-server` + `rustls`) or sit behind a trusted TLS proxy — either is
acceptable, but the choice must be written down and enforced, not implicit.

---

## 4. High findings

### H1 — Symlink escape in the filesystem write handler

**Where:** `crates/ralleh-tool-gateway/src/fs_write_handler.rs`, `resolve_within_root`

```64:77:crates/ralleh-tool-gateway/src/fs_write_handler.rs
        let parent = candidate
            .parent()
            .ok_or(FsWriteTextError::PathEscapesRoot)?;

        let canonical_parent = parent
            .canonicalize()
            .map_err(|_| FsWriteTextError::ParentMissing)?;

        if !canonical_parent.starts_with(&self.root) {
            return Err(FsWriteTextError::PathEscapesRoot);
        }

        Ok(canonical_parent.join(file_name))
```

**Problem.** The handler canonicalizes the *parent* directory and confirms it is
under `root`, then re-attaches the raw `file_name`. If that final component is a
**symlink** pointing outside the sandbox, `fs::write` follows it and writes
outside `root`. The subsequent overwrite guard uses `resolved.exists()`
(line 106), which *also* follows symlinks, so it doesn't help. The parent-canonicalization
correctly defeats `../` traversal in directory components, but not a symlink as
the leaf.

**Preconditions / impact.** An attacker must first place a symlink inside the
sandbox (another process, a shared/multi-tenant sandbox root, a restored backup,
or a prior tool that can create links). On Windows, symlink creation needs
privilege, lowering (not eliminating) risk; on Linux/macOS it's routine.
Cross-platform code should defend regardless. Result: sandbox escape → arbitrary
file overwrite. This is exactly the `T2` class the handler exists to prevent.

**Fix.** Refuse a symlink leaf, and open with no-follow / create-new semantics:

```rust
use std::fs::OpenOptions;

// After computing `resolved`:
if let Ok(meta) = std::fs::symlink_metadata(&resolved) {
    if meta.file_type().is_symlink() {
        return Err(FsWriteTextError::PathEscapesRoot); // never follow a leaf symlink
    }
}

// Prefer create_new to make the overwrite check atomic (removes the exists() TOCTOU too):
let mut opts = OpenOptions::new();
opts.write(true);
if overwrite { opts.create(true).truncate(true); } else { opts.create_new(true); }
#[cfg(unix)]
{ use std::os::unix::fs::OpenOptionsExt; opts.custom_flags(libc::O_NOFOLLOW); }
let mut f = opts.open(&resolved).map_err(/* map create_new-exists to RefusingOverwrite */)?;
```

Add a regression test: create a symlink inside the sandbox pointing outside it,
attempt a write through it, assert `PathEscapesRoot` and that the external target
is untouched (mirror the existing `rejects_path_traversal_outside_root` test).

---

### H2 — TOCTOU / DNS-rebinding window in HTTP fetch

**Where:** `crates/ralleh-tool-gateway/src/http_fetch_handler.rs`, `assert_safe_destination`

**Problem.** The handler resolves the hostname to validate that it points at a
public IP (the DNS-rebinding guard), but then hands the **hostname** to `reqwest`,
which performs its *own, independent* DNS resolution for the actual connection.
Between the two resolutions an attacker-controlled DNS record can flip from a
public IP (passes validation) to `169.254.169.254` / `127.0.0.1` / a private
range (used for the real request). The validation and the connection don't agree
on an IP.

**Impact.** SSRF to cloud metadata endpoints / internal services despite the
guard (`T3`).

**Fix.** Make validation and connection resolve to the *same* address. Resolve
once, validate every returned IP, then pin the connection to a validated IP:

```rust
use reqwest::dns::Resolve; // or:
let resolved: Vec<std::net::SocketAddr> = /* resolve host:port once */;
for addr in &resolved { reject_if_special_or_private(addr.ip())?; }

let client = reqwest::Client::builder()
    .redirect(reqwest::redirect::Policy::none())
    // Force reqwest to connect to the exact IP we validated, bypassing a
    // second DNS lookup entirely:
    .resolve_to_addrs(host, &resolved)
    .build()?;
```

`Client::resolve_to_addrs` (or a custom `dns::Resolve` that caches the validated
answer for the request's lifetime) closes the window. Keep the existing "must be
public unless explicitly IP-allowlisted" rule applied to the resolved set.

---

### H3 — Audit durability: `flush()` without `sync_all()`

**Where:** `crates/ralleh-audit-store/src/sink.rs` and `desktop-edge/src-tauri/src/audit.rs`

```153:157:crates/ralleh-audit-store/src/sink.rs
        let mut file = self.file.lock().expect("audit sink mutex poisoned");
        file.write_all(&line)
            .and_then(|_| file.flush())
            .map_err(|source| AuditSinkError::Write { source })
```

```379:381:desktop-edge/src-tauri/src/audit.rs
        file.write_all(line.as_bytes())
            .map_err(|e| format!("audit: write: {e}"))?;
        Ok(())
```

**Problem.** The server sink's own doc comment claims "a crash or `kill -9`
immediately after a successful `record()` cannot lose that record." That's true
for a *process* crash (the bytes are in the kernel page cache), but **not** for a
power loss or kernel panic — `flush()` only pushes libc buffers to the OS; it does
not force the OS to write to stable storage. `sync_all()` (fsync) is required for
the durability the comment promises. The desktop `write()` doesn't even
`flush()`. For a log whose entire selling point is "hand it to a customer's
security review," durable persistence is part of the contract.

**Impact.** Silent loss of audit evidence across a hard crash; weakens `T5`.

**Fix.** Add `file.sync_all()?` after the write on both paths. If per-event fsync
proves too slow under real load, switch to an explicit, documented durability
mode (batched group-commit with a bounded flush interval, or `O_DSYNC`), and make
the guarantee configurable — but do **not** let the doc claim a guarantee the
code doesn't provide. Also: the desktop `write()` re-opens the file handle on
every call (lines 374–378); holding the handle open (like the server sink does)
avoids an `open`/`close` syscall pair per event.

---

### H4 — Panics on operational failures

**Where:** `crates/ralleh-mcp-server/src/main.rs` (bootstrap `unwrap()`/`panic!`),
`crates/ralleh-tool-gateway/src/approval.rs` (`persist_or_expect`)

**Problem.** Server startup uses `unwrap()`/`panic!` for configuration and setup,
and the approval store's persistence path panics on I/O error
(`persist_or_expect`). A full disk, a revoked permission, or a transient FS error
becomes a **process crash** instead of a handled error. For a server that gates
destructive actions behind approvals, an approval-store write failure taking down
the whole service is a bad failure mode — and worse, a crash mid-approval can
leave the queue in a surprising state on restart.

**Impact.** Availability; `T4`/`T7` interactions (approval durability).

**Fix.**
- Bootstrap: return `Result` from `run()` / `main() -> anyhow::Result<()>`,
  propagate with `?`, and print a clear diagnostic + non-zero exit on failure
  instead of an unwrap backtrace.
- Approval persistence: return `Result` from the persist path and have callers
  decide. A failed persist should degrade to "in-memory only + loud warning +
  audit event," never a panic. Contrast with the *good* pattern already used by
  the audit sink's `GatewayAuditSink::record`, which is explicitly fail-open and
  logs rather than propagating a crash.

Grep the tree for `unwrap()`/`expect()`/`panic!` on non-invariant paths as part
of this pass; reserve them for genuine "this is a programmer bug" invariants
(the `Mutex` poisoning `expect`s are acceptable; I/O and config are not).

---

### H5 — Tauri app-defined commands are not ACL-restricted

**Where:** `desktop-edge/src-tauri/build.rs`, `desktop-edge/src-tauri/capabilities/default.json`

```1:3:desktop-edge/src-tauri/build.rs
fn main() {
    tauri_build::build()
}
```

```4:8:desktop-edge/src-tauri/capabilities/default.json
  "description": "Minimal Phase 1 capability — allowlisted core IPC only (T11)",
  "windows": ["main"],
  "permissions": [
    "core:default"
  ]
```

**Problem.** The capability description claims "allowlisted core IPC only (T11),"
but `core:default` only governs Tauri's *core plugin* commands. The application's
own `#[tauri::command]` handlers registered via `invoke_handler` (mic start/stop,
`assistant_save_backend`, `assistant_diagnostics_bundle`, audit read/verify,
secret-affecting flows — ~30 commands) are **not** individually gated by this
capability, and `build.rs` does nothing to restrict them. So the webview can
invoke every command. The comment overstates a control that isn't implemented,
which is exactly the kind of thing a security auditor flags: `T11`'s stated
mitigation ("Tauri capabilities allowlist; never expose raw FS/net to JS") is
only half-realized.

**Impact / context.** Realistic risk is **moderate, not severe**, because the
mitigations around it are strong: the webview ships with the app, loads no remote
content, and runs under a tight CSP — so reaching these commands requires a
webview compromise (XSS), which is already hardened against. But defense-in-depth
for an enterprise bar wants a compromised renderer to *still* be unable to save
secrets or start the mic. Rate High primarily because a shipped security comment
asserts a control that does not exist.

**Fix.** Restrict the command surface explicitly and make the capability honest:

```rust
// build.rs
fn main() {
    let manifest = tauri_build::AppManifest::new()
        .commands(&[
            "core_ping", "load_edge_settings", "assistant_think", /* …exact UI set… */
        ]);
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(manifest),
    ).expect("tauri build");
}
```

Then add matching `allow-<command>` permissions to `default.json`, prune any
command the UI never calls, and correct the description to state what is actually
allowlisted. (Verify the exact `tauri_build` API against the pinned Tauri 2
version before wiring.)

---

### H6 — Unbounded presence IPC input (stdin + queue)

**Where:** `presence-runtime/src/ipc_stdin.rs` (mirrored on the shell side in
`desktop-edge/src-tauri/src/presence.rs`)

```103:124:presence-prototype/presence-runtime/src/ipc_stdin.rs
pub(crate) fn run<R: BufRead>(mut input: R, tx: Sender<Command>) {
    let mut line = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) => return, // EOF
            Ok(_) => {}
            ...
        }
```

**Problem.** Two unbounded resources on the IPC path. (1) `read_line` has no
maximum line length — a peer that sends a large payload with no `\n` forces the
`String` to grow without bound. (2) The transport uses `mpsc::channel()`
(unbounded); a fast producer (mic pump + mode updates) with a stalled render loop
grows the queue without backpressure, and `active_modes: Vec<PresenceMode>`
decodes to an attacker-sized vector. The `decode()`/`drain()` logic itself is
nicely defensive once bytes are in memory — the gap is *before* and *around*
parse.

**Impact.** Memory-exhaustion DoS of the presence child and/or shell. The channel
is between trusted local processes today, so this is defense-in-depth, but it's
the kind of unbounded-input footgun that becomes real the moment the transport is
reused.

**Fix.** Replace `read_line` with `read_until(b'\n', ..)` under a hard byte budget
(e.g. 64–256 KiB; on exceed, log + discard to next newline). Use
`sync_channel(N)` (64–256) and drop+count on `TrySendError::Full`. Cap
`active_modes` length at decode time (there are only 6 modes; `≤ 16` is generous).
Optionally drain with a per-frame command budget.

---

### H7 — Backend settings form is wiped by the status poll

**Where:** `desktop-edge/src/BackendSettings.tsx`

```255:259:desktop-edge/src/BackendSettings.tsx
  useEffect(() => {
    if (open) setForm(stateFromStatus(status));
    if (open) setTest({ phase: "idle" });
    if (open) setSave({ phase: "idle" });
  }, [open, status]);
```

**Problem.** This effect resets the form whenever `open` *or* `status` changes.
`status` is refreshed by a 15-second poll (and by probes). If the operator is
part-way through typing a base URL / model / API key when a poll lands, their
edits are silently overwritten from persisted state. It's a data-loss UX bug on a
security-sensitive form (re-entering an API key is exactly the friction the
write-only design tried to avoid).

**Fix.** Reset only on the `false → true` open transition, not on every `status`
change:

```tsx
const prevOpen = useRef(false);
useEffect(() => {
  if (open && !prevOpen.current) {
    setForm(stateFromStatus(status));
    setTest({ phase: "idle" });
    setSave({ phase: "idle" });
  }
  prevOpen.current = open;
}, [open, status]);
```

Optionally pause the poll (or skip applying it to the form) while the panel is
open and dirty.

---

### H8 — Unbounded audio utterance buffers; no subprocess timeout / input cap

**Where:** `ralleh-audio-core/src/pipeline.rs` (`collect_utterance`),
`src/wakeword.rs` (`current_utterance`), `src/stt.rs` / `src/tts.rs` (CLI adapters)

**Problem.** Three related unbounded/blocking paths on the live-audio side:
- `collect_utterance` and the wake-word `Collecting` phase push every speech frame
  into a `Vec` until the VAD reports silence. `max_utterance_frames` only affects
  *trigger eligibility at the end* — it does not bound growth *during* capture. A
  stuck or noisy VAD (or a long monologue) grows RAM without limit and retains
  voice PII in memory.
- `WhisperCliStt`/`PiperCliTts` invoke the external binary with `output()` /
  `wait_with_output()` and **no timeout** — a hung child blocks the caller thread
  indefinitely and can orphan on parent abort.
- CLI STT writes any-length `samples` to a WAV with no size cap (a multi-hour
  buffer → huge temp file / OOM), and doesn't re-validate the 16 kHz sample-rate
  invariant the native `WhisperStt` enforces.

**Impact.** Memory-exhaustion / hang / PII-retention. `T9` interactions.

**Fix.** Add a `max_utterance_samples`/duration cap that stops collection (and
returns a truncated/aborted result) in both `collect_utterance` and the wake-word
detector. Wrap subprocess calls with a wall-clock timeout proportional to audio
length (`wait_timeout` + kill on expiry). Reject CLI STT input above a documented
bound (e.g. 60–120 s @ 16 kHz) and mirror the native sample-rate check. (This
finding is the buffer/robustness complement to **M3**'s temp-file PII fix.)

---

## 5. Medium findings

### M1 — Desktop audit log is O(n) per append (O(n²) to rotation)

**Where:** `desktop-edge/src-tauri/src/audit.rs`, `last_hash_in` called from `write`

```555:561:desktop-edge/src-tauri/src/audit.rs
fn last_hash_in(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    raw.lines()
        .rev()
        .find_map(|l| serde_json::from_str::<AuditEvent>(l).ok())
        .and_then(|e| e.hash)
}
```

**Problem.** Every single append reads the **entire** active file into memory to
find the previous line's hash. As the file grows toward the 4 MiB rotation cap
(~28k events), each write re-reads up to 4 MiB. Total work to fill one file is
quadratic in event count. It's bounded by rotation, but it's needless and it puts
avoidable I/O on a lock held during every audited action.

**Fix.** Cache the last hash in `AuditLogInner` (it already owns the write path
under the `Mutex`). Seed it once on open by reading the tail, update it in-memory
after each successful write, and reset it to `None` on rotation. This turns each
append into O(1) plus the append itself.

### M2 — Env-var API tokens: process-visible, char-restricted, unchecked file perms

**Where:** `crates/ralleh-mcp-server/src/auth.rs`, `parse_env_list` / `load_from_path`

**Problem.** `RALLEH_API_TOKENS` packs secrets as `token:tenant:actor[:device]`
separated by `;`. Consequences: (a) the token cannot contain `:` or `;`,
silently constraining entropy/rotation formats; (b) environment variables are
readable via `/proc/<pid>/environ`, `ps e`, container inspection, and crash
dumps — a poor place for a shared secret; (c) `load_from_path` reads the token
file with no check that it isn't world-readable.

**Fix.** Prefer `RALLEH_API_TOKENS_FILE` (already supported) and, on Unix, refuse
files with group/other permissions (`fs::metadata(..).permissions().mode() & 0o077 != 0`).
For deployment, document integration with a secret manager (Vault / cloud
KMS / Kubernetes secrets mounted as files) and treat the env-var packed form as
dev-only. Longer term this folds into the `T1`→`T18` OIDC migration.

### M3 — Raw microphone audio written to a predictable temp path with no crash-safe cleanup

**Where:** `crates/ralleh-audio-core/src/stt.rs`, `WhisperCliStt::transcribe`

```227:241:crates/ralleh-audio-core/src/stt.rs
        let dir = std::env::temp_dir();
        let uniq = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let wav_path = dir.join(format!(
            "ralleh-whisper-{}-{}.wav",
            std::process::id(),
            uniq
        ));
        crate::wav::write_pcm16_mono(&wav_path, samples, sample_rate_hz)
            .map_err(|e| SttError::Engine(e.to_string()))?;
        let result = self.transcribe_file(&wav_path);
        let _ = std::fs::remove_file(&wav_path);
        result
```

**Problem.** This is the most sensitive data in the system — a person's voice —
and it's written to the **shared** temp dir under a predictable name (pid +
timestamp). On a multi-user host another user can read it during the transcription
window; the predictable name invites a symlink/TOCTOU pre-creation attack; and if
the process panics or is killed between write and `remove_file`, the audio
**persists on disk** with no cleanup (`T9`). The external-process invocation
itself is fine (args are passed separately, no shell), and `cli_path`/`model_path`
come from operator env, not untrusted input.

**Fix.** Use the `tempfile` crate to create the file with `0600` permissions in a
private location, and make cleanup RAII so a panic still deletes it:

```rust
let mut tmp = tempfile::Builder::new()
    .prefix("ralleh-whisper-").suffix(".wav")
    .tempfile()?;                 // 0600, unpredictable name, auto-deleted on drop
crate::wav::write_pcm16_mono(tmp.path(), samples, sample_rate_hz)?;
let result = self.transcribe_file(tmp.path());
// `tmp` drops here (even on early return/panic-unwind) and removes the file.
```

Consider zeroizing the PCM buffer after use and documenting a retention/erasure
policy for transcripts to close the `T9` gap named in the threat model.

### M4 — Audit log is tamper-evident, not tamper-proof

**Where:** both audit stores.

**Problem.** The hash chain detects mutation of existing lines, but an attacker
with write access to the file can regenerate the entire chain (the code and docs
say so honestly). For a compliance/forensics asset, "an insider can rewrite
history undetectably" may not meet the bar.

**Fix (tiered, pick per threat model):**
- **Cheapest:** HMAC each line with a key held outside the file (OS keychain), so
  regeneration requires the key, not just FS write.
- **Stronger:** sign the chain head periodically with an offline/asymmetric key.
- **Strongest:** anchor the chain head to an external append-only service
  (SIEM ingest with server-side timestamping, or a transparency-log style notary).

This is explicitly listed as future work in `THREAT_MODEL.md` ("Move audit to
append-only / hash-chained or DB-backed storage"); calling it out here so it's
tracked as a hardening item, not an oversight.

### M5 — Live-mic frame drops and stream errors are invisible

**Where:** `crates/ralleh-audio-core/src/cpal_source.rs`

**Problem.** The audio callback correctly `try_send`s and drops on backpressure
(the right real-time behavior), but the drop is completely silent — no counter,
no metric — so sustained loss (a stalled consumer) is undiagnosable. Separately,
the stream error callback stores the error into `err_flag`, but nothing ever reads
it, so device errors are swallowed and `next_frame()` just quietly stops
producing.

**Fix.** Keep the non-blocking send, but increment an `AtomicU64` dropped-frame
counter on `try_send` failure and expose it (diagnostics bundle / presence
telemetry). Surface `err_flag` through `next_frame()` (e.g. return a
`Result`/`Poll`-like status, or expose a `last_error()` accessor) so a wedged
input device becomes visible instead of a silent stall.

### M6 — No supply-chain gate in CI; downloaded models/binaries unverified

**Where:** CI workflows; `T10`.

**Problem.** CI runs `fmt`/`clippy`/`test` and `tsc`/`vite build` (good), but
there's no `cargo audit` / `cargo deny` for known-vulnerable or unmaintained
crates, no `npm audit` for the webview deps, and the whisper CLI/model download
scripts don't verify checksums. For an enterprise SBOM/provenance story these are
expected.

**Fix.** Add a `cargo deny check advisories bans sources licenses` job and a
`cargo audit` job (fail on vuln); add `npm audit --audit-level=high` (or the
`pnpm`/`osv-scanner` equivalent) to the frontend job; pin and verify SHA-256
checksums in the `download-whisper-*` scripts. Consider `cargo-vet` for
transitive-dependency review at enterprise scale.

### M7 — `AudioFrame` derives `Serialize` over raw PCM

**Where:** `crates/ralleh-audio-core/src/source.rs`

**Problem.** `AudioFrame` carries the raw sample buffer *and* derives `Serialize`.
Any accidental `serde_json` log line, IPC hop, or audit serialization of a frame
embeds the full voice waveform — the most sensitive data in the system — into a
sink that was never meant to hold it. The type system currently makes leaking PII
*easy* rather than *hard*.

**Fix.** Split metadata from payload: a serde-able `AudioFrameMeta` (rate,
sequence, energy) and a non-serde live-capture type, or put `#[serde(skip)]` on
`samples`. Make "you cannot serialize a raw waveform by accident" a compile-time
property, consistent with the audit module's "no field wide enough for a secret"
design.

### M8 — Diagnostics bundle `destDir` is not path-confined

**Where:** `desktop-edge/src-tauri/src/diagnostics.rs` (command exposed as
`assistantDiagnosticsBundle` in `presence.ts`)

**Problem.** The command accepts an arbitrary `destDir` and writes the bundle
there, with none of the app-config-dir confinement that
`reveal_path_in_file_manager` correctly applies. The UI passes `null` today, so
it's not currently exploitable, but the IPC surface is broader than the UI needs
— and (per **H5**) any webview code can call it.

**Fix.** Confine `dest_dir` to the app config dir (mirror the reveal guard), or
drop the parameter until a real "choose location" flow exists. Combine with H5 so
the command is both ACL-gated and path-confined.

### M9 — Presence render loop can stall after a surface error

**Where:** `presence-runtime/src/app.rs`

**Problem.** Under `ControlFlow::Poll`, the early `return` on
`SurfaceError::Lost | Outdated | OutOfMemory` skips `window.request_redraw()`.
After a resize-on-error the loop can idle until another window event arrives, and
because heartbeats and IPC draining happen inside redraw, *those stop too* — the
shell then sees a false `presence-stalled`. Separately, the `OutOfMemory` branch
logs "exiting" but only returns.

**Fix.** Call `request_redraw()` before every early return in the redraw path;
on `OutOfMemory` actually `event_loop.exit()` (or enter a visible degraded state).

### M10 / M11 — Frontend async overlap and missing stream cancellation

**Where:** `desktop-edge/src/BackendSettings.tsx`, `PresenceStatusLine.tsx`,
`Conversation.tsx`

**Problem.** Several async paths lack in-flight guards: `refreshStatus` (initial
load + 15s poll + probe) can apply responses out of order; Test and Save can run
concurrently (each disables only its own button); the presence status poll can
stack calls if IPC is slow. And `Conversation.tsx`'s `assistantThinkStream` has
no unmount/abort guard, so leaving Core mid-stream can `setState` on an unmounted
tree.

**Fix.** Add a monotonic `requestId` / `AbortController` to the status fetches and
ignore stale results; share a single `busy` lock across Test/Save; skip a poll
tick while the previous is in flight (or use chained `setTimeout`). In
`Conversation.tsx`, track a `mounted`/`aborted` ref checked in the channel
handler and `finally`, and clear `channel.onmessage` on unmount (ideally pass an
abort signal to Rust to stop the stream server-side).

### M12 — Per-frame instance-buffer allocation in the renderer

**Where:** `presence-core/src/render/mod.rs`

**Problem.** Each frame allocates a fresh `Vec<InstanceRaw>` for ~120k particles
(~5–6 MB at the Balanced tier) before uploading to the GPU, adding allocator
pressure and periodic frame hitches on top of sim cost. Relatedly,
`ensure_instance_capacity` grows to peak and never shrinks on tier downgrade.

**Fix.** Reuse a persistent `Vec<InstanceRaw>` on `Renderer` (clear + reserve),
optionally double-buffered; shrink the GPU instance buffer with hysteresis when
`needed < capacity / 2`.

---

## 6. Low findings & nits

- **L1 — CSP `style-src 'unsafe-inline'`.** `desktop-edge/src-tauri/tauri.conf.json`
  allows inline styles. Low risk in a ship-with-app webview, but for defense in
  depth move to hashed/nonce styles or externalize CSS so the directive can drop
  `'unsafe-inline'`. `img-src ... data:` is also a (minor) broadening.
- **L2 — `eprintln!` in a library.** `ralleh-audit-store/src/sink.rs`
  (`GatewayAuditSink::record`) logs a persistence failure with `eprintln!`; the
  rest of the workspace uses `tracing`/`log`. Route it through the structured
  logger so audit-write failures are captured by the same pipeline as everything
  else.
- **L3 — Dev-mode auth accepts body-claim labels when no tokens configured.**
  Documented and warned, but ship a hard "require auth" switch
  (`RALLEH_REQUIRE_AUTH=1`) that refuses to start without tokens, so a production
  deploy can't accidentally run open.
- **L4 — Approvals not cryptographically bound to the approver.** `T4` gap: an
  approval is one-shot but doesn't capture *who* approved it in a
  non-repudiable way. When identity lands (`T18`), bind approvals to a signed
  approver identity and record it in the audit chain.
- **L5 — `reveal_path_in_file_manager` uses `format!("/select,{}")` on Windows.**
  The path is validated to be canonical under the app config dir (good), but pass
  it as a separate `arg` rather than string-formatting into the `explorer`
  argument to avoid any future argument-injection footgun.
- **L6 — Per-request egress re-check.** `AiRouter::route` relies on egress
  validation at backend-construction time; the desktop `build_backend_from_config`
  correctly re-checks. Consider asserting the egress invariant on the hot `route`
  path too (cheap allowlist lookup) so a future backend built by a path that
  forgets the check still can't exfiltrate.
- **L7 — Dropped-frame/heartbeat observability into the audit/diagnostics bundle.**
  You already capture `presence-stalled`; fold the mic drop counter (M5) and
  backend health transitions into the same diagnostics surface for a single
  operator-facing "is anything silently degrading?" view.
- **L8 — Presence wire version is exact-match.** `Envelope::is_current()` requires
  `version == VERSION`, so any bump breaks mixed-version rollout even for
  additive-compatible payloads. Accept `version <= VERSION` (reject only newer)
  or maintain an explicit supported range.
- **L9 — CSP hardening + self-hosted fonts.** Beyond `style-src 'unsafe-inline'`
  (L1), the CSP omits `base-uri 'self'`, `object-src 'none'`, and
  `frame-ancestors 'none'`, and `index.html` pulls Google Fonts from a CDN at
  runtime (external request + a `font-src`/`style-src` relaxation). Add the
  missing directives and self-host fonts (`@fontsource`) to tighten the policy
  and drop the third-party dependency. Consider enabling
  `noUncheckedIndexedAccess` / `exactOptionalPropertyTypes` in `tsconfig.json`
  incrementally for stricter enterprise typing.

---

## 7. TypeScript / React frontend

The frontend is in good shape and needs far less work than a typical Tauri
webview.

**Strengths:** `strict` + `noUnused*` + `noFallthroughCasesInSwitch`;
discriminated-union state machines (`ApiKeyInputMode`, `TestState`, `SaveState`,
`DiagnosticsState`) that make illegal states unrepresentable; the write-only
secret model mirrored faithfully in `presence.ts`; `safeInvoke` to contain IPC
errors; no `dangerouslySetInnerHTML`, no `eval`, no `any` sprinkled around;
literal-union wire types that turn a stale enum spelling into a compile error at
the callsite.

**Recommendations:**
- **F1 — Add ESLint + `@typescript-eslint` (type-checked) and Prettier to CI.**
  `tsc --noEmit` catches type errors but not lint-class issues (exhaustive
  `switch`, `no-floating-promises`, hook deps). `no-floating-promises` in
  particular would formalize the deliberate `void refreshStatus()` /
  `.catch(() => {})` patterns already in use.
- **F2 — Runtime-validate IPC payloads at the boundary.** The TS types
  (`EdgeSettingsResponse`, `BackendStatus`, `AuditEvent`) are compile-time only;
  a Rust shape drift becomes a silent `undefined` at runtime. A tiny `zod` (or
  hand-written guard) parse at each `invoke` boundary makes the contract
  enforced, not assumed. Keep it thin — these are trusted-origin payloads, so
  this is about *drift detection*, not untrusted-input defense.
- **F3 — Error boundary.** `main.tsx` renders `<App/>` with no React error
  boundary; an unexpected render throw white-screens the shell. Add a top-level
  boundary that shows a recoverable error panel (and can trigger the diagnostics
  bundle).
- **F4 — Centralize the "mirror of Rust types" contract.** `presence.ts` and
  `settings.ts` hand-mirror serde shapes. A generated types step (e.g. `ts-rs` on
  the Rust side) would remove the manual sync burden and the class of bug F2
  guards against. Optional, but it scales better than comments that say "keep in
  sync."

---

## 8. Prioritized remediation roadmap

**P0 — before any exposed deployment (days):**
1. C1 constant-time token compare (`subtle`).
2. C2 server middleware: body limit, timeout, concurrency, graceful shutdown; decide TLS story.
3. H1 symlink-leaf rejection + `create_new` in fs write handler (+ regression test).
4. H2 pin validated IP for http-fetch (`resolve_to_addrs`).
5. H5 restrict Tauri app-command surface (`AppManifest::commands`) + fix the overclaiming capability comment.
6. H7 fix the `BackendSettings` form-reset data-loss bug (open-transition guard).

**P1 — before "enterprise-grade / auditable" claim (1–2 weeks):**
7. H3 `sync_all()` on both audit paths; hold the desktop file handle open.
8. H4 remove operational panics (server bootstrap + approval persistence).
9. H6 bounded presence stdin reads + `sync_channel` + `active_modes` cap.
10. H8 utterance-length caps + subprocess timeouts + CLI STT input/rate checks.
11. M1 cache last audit hash in memory.
12. M2 token file perm checks + `RALLEH_REQUIRE_AUTH`; document secret-manager path (L3).
13. M3 `tempfile` + RAII for whisper/piper audio; transcript retention note.
14. M7 remove/gate serde on raw PCM (`AudioFrame`).
15. M8 confine diagnostics `destDir`.
16. M6 `cargo deny`/`cargo audit`/`npm audit` in CI; checksum the model/CLI downloads.

**P2 — hardening & scale (as the roadmap allows):**
17. M4 HMAC/sign/anchor the audit chain (pick a tier).
18. M5 mic drop counter + surfaced stream errors.
19. M9 presence render-loop redraw-on-error; M12 reuse instance buffer.
20. M10/M11 frontend async-overlap guards + `Conversation.tsx` stream cancellation.
21. F1–F4 frontend: ESLint, boundary IPC validation, error boundary, generated types.
22. L1–L9 nits.

---

## 9. Method & coverage

Reviewed first-hand (representative, not exhaustive): policy engine/rules/egress/
request/decision; tool gateway (registry, handlers for fs read/write and http
fetch, approval store, events); MCP server (auth, router, config, state, main);
AI router + Anthropic/HTTP backends + SSE parser; audit store (sink, record) and
the desktop audit log (chain + verify); OS capabilities (clipboard, screen,
hotkey); audio core (cpal source, STT incl. whisper CLI/rs, VAD, wake-word,
frames); `presence-ipc` wire types; the Tauri shell (`lib.rs`, `assistant.rs`,
`secret_store.rs`, `settings.rs`, `mic.rs`, `tauri.conf.json`, `capabilities/`);
and the TypeScript frontend (`App.tsx`, `BackendSettings.tsx`, `settings.ts`,
`presence.ts`, `main.tsx`, `tsconfig.json`, `vite.config.ts`).

The `presence-prototype` renderer internals, the audio pipeline's deeper
robustness surface, and the frontend's async/ACL details were corroborated by
three focused deep-dive passes whose findings are folded in above (H5–H8, M7–M12,
L8–L9): a `ralleh-audio-core` review, a `presence-prototype` review, and a
TypeScript/React frontend review. Notably, the frontend pass corrected an
over-optimistic first-pass reading of the Tauri capability posture (**H5**): the
`core:default` capability restricts *plugin* commands but not the app's own
`invoke` handlers.

Severity is assigned by exploitability × blast radius on the current
(pre-`T18`-identity) posture; several Medium/Low items rise or fall once real
device/OIDC identity lands. Line references point at the reviewed revision and
should be re-anchored if the files move.

---

*Prepared as an internal engineering artifact. Cross-reference `THREAT_MODEL.md`
for threat IDs and `NEXT_STEPS.md` for backlog integration.*
