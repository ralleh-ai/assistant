# Threat Model (Phase 0 draft + forward surfaces)

Living draft of DEVELOPMENT.md §15 Phase 0 deliverable (“draft threat
model”). Scope is layered:

1. **Implemented now** — Rust workspace (`ralleh-mcp-server` +
   policy/gateway/audio/ai-router/audit).
2. **Forward-looking** — Tauri desktop shell and NestJS control plane
   (not shipped; threats listed so Phase 1/2 design stays honest).

## Assets

| Asset | Why it matters |
|---|---|
| Policy decisions / audit trail | Prove who did what; compliance / forensics |
| Tool handlers (fs, http fetch) | Direct side effects on disk and network |
| Approval queue | Human gate for destructive actions |
| Tenant / device / actor identifiers | Isolation boundary |
| Model prompts & completions | Data exfil / prompt injection surface |
| Mic audio / transcripts | Highly sensitive personal data |
| Desktop OS capabilities (future) | Clipboard, screen, files, hotkeys — high blast radius |
| Control-plane tenant config / secrets (future) | SSO, API keys, billing, device enrollment |

## Trust boundaries (current Rust spine)

```text
[Caller HTTP] --Bearer token?--> [ralleh-mcp-server]
                                       |
                                       v
                                 [PolicyEngine]
                                       |
         +-----------------------------+-----------------------------+
         |                             |                             |
         v                             v                             v
  [ToolGateway]                  [AiRouter]                 [ApprovalStore]
         |                             |                             |
         v                             v                             v
 [Fs / HttpFetch]              [CompletionBackend]           [JSON snapshot]
         |                             |
         v                             v
   local FS / egress             provider HTTP
```

Caller-supplied `tenant_id` / `actor_id` are authenticated when
`RALLEH_API_TOKENS` or `RALLEH_API_TOKENS_FILE` is set (shared-secret Bearer
tokens bound to tenant/actor[/device]). Without tokens configured, the
server still accepts body claims as labels only (dev mode) and logs a
warning. Real OIDC / device attestation remains Phase 2.

## Key threats and mitigations (current code)

| ID | Threat | Severity | Mitigation today | Gap |
|---|---|---|---|---|
| T1 | Cross-tenant capability use | High | Policy tenant scoping; **Bearer tokens bind tenant/actor(/device) when configured** | No OIDC/device attestation yet |
| T2 | Path traversal via fs tools | High | Canonicalize + sandbox root in handlers (independent of policy) | — |
| T3 | SSRF via http fetch | High | Hostname allowlist; no redirects; http(s) only; no userinfo; **block link-local/special IPs; hostname must resolve to public IPs (DNS-rebinding guard); private/loopback only via explicit IP allowlist** | Optional hostname→private flag still TBD |
| T4 | Unapproved destructive write | High | `RequireApproval` + parked invocation; approve is one-shot | Approvals not yet cryptographically bound to approver identity |
| T5 | Audit gap / silent privilege | High | Every gateway outcome → `AuditSink` (JSONL) | JSONL not queryable / tamper-evident |
| T6 | Prompt injection → tool misuse | Med | Tools still policy-gated; writes need approval | No output fencing / digest before model injection yet |
| T7 | Approval replay after restart | Med | Durable JSON approval store (status transitions persist) | File not integrity-protected |
| T8 | Egress to unexpected hosts | Med | Empty `allowed_hosts` fails config validation | Operators must keep allowlists tight |
| T9 | Mic / transcript leakage | Med | Mic optional (`mic` feature); STT mock by default; CLI STT opt-in | Retention / encryption at rest TBD |
| T10 | Supply-chain / native STT | Low | `whisper` feature off; CLI binaries gitignored under `tools/` | Checksums for downloaded tools/models |

## Forward: Tauri desktop shell (not implemented)

Planned trust boundary (ADR-002):

```text
[React UI webview] --Tauri IPC/commands--> [Rust edge core]
                                                    |
                        +---------------------------+------------------+
                        |                           |                  |
                        v                           v                  v
                 [audio/STT/TTS]            [local tools/OS]    [HTTP to mcp-server]
```

| ID | Threat | Severity | Planned mitigation | Notes |
|---|---|---|---|---|
| T11 | UI→Rust IPC capability bypass | High | Tauri capabilities allowlist; never expose raw FS/net to JS | Mirror tool-gateway policy for OS caps |
| T12 | Malicious/compromised webview content | High | Minimal CSP; no remote untrusted UI; ship UI with app | No `dangerousRemoteDomain*` |
| T13 | Clipboard / screen capture abuse | High | Trait + mock + feature gate (HEADLESS.md); policy capability per action | Explicit user grant + audit |
| T14 | Always-on mic exfiltration | High | Push-to-talk / wake gating; local processing preference; retention policy | Pair with T9 |
| T15 | Local secret theft from disk | Med | OS keychain / encrypted store; no plaintext API keys in config | Control plane issues short-lived tokens |
| T16 | Auto-update supply chain | Med | Signed updates; pinned update endpoint | Phase 1 packaging |

## Forward: NestJS control plane (not implemented)

Planned trust boundary (ADR-001):

```text
[Admin console / SSO] --> [NestJS control plane] --> [Postgres]
                                |        |
                                v        v
                         [device enroll] [policy push / config]
                                |
                                v
                      [edge agents / mcp-server]
```

| ID | Threat | Severity | Planned mitigation | Notes |
|---|---|---|---|---|
| T17 | Broken tenant isolation in CRUD APIs | High | Row-level tenant_id on every query; integration tests | Same invariant as Rust policy engine |
| T18 | SSO / session fixation / token theft | High | OIDC via proven IdP; httpOnly secure cookies or mTLS for agents | Replaces shared-secret Bearer (T1 gap) |
| T19 | Privilege escalation via RBAC bugs | High | Least-privilege roles; deny-by-default admin routes | Audit all admin mutations |
| T20 | Poisoned policy push to edges | High | Signed config bundles; version pins; edge rejects unsigned | Pair with device attestation |
| T21 | Secrets in env/logs | Med | Secret manager; redacted structured logs | No raw keys in Nest logs |
| T22 | Multi-tenant data in shared DB | High | Strict tenant predicates; consider schema-per-tenant later | Backups inherit isolation rules |

## Non-goals for this draft

- Formal residual-risk acceptance by a security reviewer.
- Device attestation / mTLS deep-dive (tracked under T18/T20).
- Full STRIDE worksheets per future NestJS microservice.

## Next threat-model revisions

1. Replace Bearer shared secrets with OIDC/device identity (close T1/T18).
2. When Tauri IPC lands, map each command to T11–T16 with test evidence.
3. When NestJS lands, add per-route tenant isolation proofs (T17).
4. Move audit to append-only / hash-chained or DB-backed storage (T5).
