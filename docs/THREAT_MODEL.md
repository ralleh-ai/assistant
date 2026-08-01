# Threat Model (Phase 0 draft)

Living draft of DEVELOPMENT.md §15 Phase 0 deliverable (“draft threat
model”). Scope today is the **Rust workspace that exists**
(`ralleh-mcp-server` + policy/gateway/audio/ai-router/audit), not the
full NestJS control plane or Tauri shell.

## Assets

| Asset | Why it matters |
|---|---|
| Policy decisions / audit trail | Prove who did what; compliance / forensics |
| Tool handlers (fs, http fetch) | Direct side effects on disk and network |
| Approval queue | Human gate for destructive actions |
| Tenant / device / actor identifiers | Isolation boundary |
| Model prompts & completions | Data exfil / prompt injection surface |
| Mic audio / transcripts (future STT) | Highly sensitive personal data |

## Trust boundaries

```text
[Caller HTTP] --tenant/device/actor claims--> [ralleh-mcp-server]
                                                    |
                                                    v
                                            [PolicyEngine]
                                                    |
                          +-------------------------+-------------------------+
                          |                         |                         |
                          v                         v                         v
                   [ToolGateway]            [AiRouter]              [ApprovalStore]
                          |                         |                         |
                          v                         v                         v
              [Fs / HttpFetch handlers]   [CompletionBackend]        [JSON snapshot]
                          |                         |
                          v                         v
                    local FS / egress          provider HTTP
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
| T3 | SSRF via http fetch | High | Hostname allowlist, no redirects, http(s) only, no userinfo | No DNS-rebinding / private-IP blocklist yet |
| T4 | Unapproved destructive write | High | `RequireApproval` + parked invocation; approve is one-shot | Approvals not yet cryptographically bound to approver identity |
| T5 | Audit gap / silent privilege | High | Every gateway outcome → `AuditSink` (JSONL) | JSONL not queryable / tamper-evident |
| T6 | Prompt injection → tool misuse | Med | Tools still policy-gated; writes need approval | No output fencing / digest before model injection yet |
| T7 | Approval replay after restart | Med | Durable JSON approval store (status transitions persist) | File not integrity-protected |
| T8 | Egress to unexpected hosts | Med | Empty `allowed_hosts` fails config validation | Operators must keep allowlists tight |
| T9 | Mic / transcript leakage | Med | Mic optional; STT mock by default; CLI STT opt-in | Retention policy TBD |
| T10 | Supply-chain / native STT | Low | `whisper` feature off; CLI binaries gitignored under `tools/` | Model/tool download scripts; verify checksums later |

## Non-goals for this draft

- Full STRIDE worksheets per NestJS service (nothing shipped yet).
- Formal residual-risk acceptance by a security reviewer.
- Device attestation / mTLS threat analysis (Phase 1/2).

## Next threat-model revisions

1. Add authenticated caller identity once control-plane auth exists.
2. Cover Tauri IPC surface when the desktop shell starts.
3. Expand STT/TTS data-flow once `whisper` / TTS land in default builds.
4. Move audit to append-only / hash-chained or DB-backed storage.
