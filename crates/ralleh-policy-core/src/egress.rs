//! Outbound-URL policy for completion backends and (future) other
//! network-facing callers.
//!
//! ## Threat model
//!
//! The `desktop-edge` shell stores completion API keys in the OS
//! keychain. That is only half of the enterprise security story:
//! keys are only as safe as the destinations we're willing to send
//! them to. Without an egress policy, an attacker who can either
//! tamper with `edge-settings.json` on disk, coax the operator into
//! pasting a malicious `base_url` into the settings UI, or exploit
//! any future config-import flow, can turn our secure key store
//! into a one-shot exfiltration channel: point the backend at
//! `https://attacker.example/v1`, and the shell will happily
//! attach a `Bearer sk-...` header on the very first request.
//!
//! This module is the second half. Every call site that resolves a
//! completion URL into a real HTTP request must first pass the URL
//! through [`EgressPolicy::check_url`]. The default policy allows
//! the well-known hosted providers plus loopback (for local Ollama /
//! LM Studio / vLLM). Enterprises with self-hosted endpoints set
//! `RALLEH_COMPLETION_ALLOWED_HOSTS` at deployment time to replace
//! the default list, exactly as they would configure any other
//! outbound-network policy on a managed device.
//!
//! ## Design principles
//!
//! - **Deny by default at the edges of tolerance.** The allowlist
//!   is small and specific; wildcard subdomains aren't supported in
//!   this pass because they materially expand the trust surface and
//!   we haven't seen a real deployment need. Add them explicitly
//!   when a real customer asks.
//! - **HTTPS enforced off-loopback.** Cleartext HTTP is allowed
//!   only when the host is a loopback address; every other host
//!   must use `https://`. This prevents key-leakage over
//!   coffee-shop Wi-Fi even if an attacker somehow persuades the
//!   operator to save an `http://api.openai.com`-flavoured URL.
//! - **No I/O.** The policy is a pure function on strings so it
//!   can run at every layer (save, test, request-build) without
//!   dragging in async or the file system.
//! - **Auditable.** [`EgressDenied`] carries the specific reason
//!   so the caller can log or surface it. The UI-side error text
//!   should match the reason variant, not be free-form.

use std::collections::HashSet;

/// Default set of hosts the shell is willing to send completion
/// traffic to when the operator has NOT configured an override via
/// `RALLEH_COMPLETION_ALLOWED_HOSTS`. Covers the two hosted
/// providers we ship first-class support for plus loopback for the
/// local-LLM crowd (Ollama, LM Studio, vLLM, llama.cpp server).
///
/// Kept as a static array so the default list is discoverable via
/// code search and doesn't get out of sync with docs.
pub const DEFAULT_ALLOWED_HOSTS: &[&str] = &[
    "api.openai.com",
    "api.anthropic.com",
    "localhost",
    "127.0.0.1",
    "0.0.0.0",
    "::1",
];

/// Env var read by [`EgressPolicy::from_env`]. Comma-separated list
/// of hostnames (no schemes, no ports); overrides the default list
/// entirely so an enterprise policy can be strictly narrower than
/// the built-in one.
pub const ALLOWED_HOSTS_ENV: &str = "RALLEH_COMPLETION_ALLOWED_HOSTS";

/// Policy handle. Cheap to clone and pass through call chains. The
/// internal shape is a `HashSet` for O(1) exact matches — the
/// allowlist is expected to be small enough that any structure
/// works, but the set keeps the lookup crisp and future-proofs the
/// common case where the list grows to dozens of internal
/// endpoints on an enterprise deployment.
#[derive(Debug, Clone)]
pub struct EgressPolicy {
    allowed_hosts: HashSet<String>,
}

impl Default for EgressPolicy {
    fn default() -> Self {
        Self {
            allowed_hosts: DEFAULT_ALLOWED_HOSTS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

impl EgressPolicy {
    /// Build a policy from an explicit list of hosts. Empty entries
    /// and whitespace are trimmed. Empty input produces an empty
    /// policy that denies *everything* — that is the correct
    /// enterprise behavior for "no destinations configured" and is
    /// what a misconfigured env var should yield rather than
    /// silently falling back to permissive defaults.
    pub fn from_hosts<I, S>(hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let allowed_hosts = hosts
            .into_iter()
            .map(Into::into)
            .map(|h| h.trim().to_ascii_lowercase())
            .filter(|h| !h.is_empty())
            .collect();
        Self { allowed_hosts }
    }

    /// Read the policy from the process environment, falling back
    /// to the default policy when the override env var is unset.
    /// Set-but-empty (`RALLEH_COMPLETION_ALLOWED_HOSTS=""`) yields
    /// an empty (deny-all) policy — that's the operator explicitly
    /// disabling every outbound completion, which is a reasonable
    /// airgap-testing configuration.
    pub fn from_env() -> Self {
        match std::env::var(ALLOWED_HOSTS_ENV) {
            Ok(raw) => Self::from_hosts(raw.split(',')),
            Err(_) => Self::default(),
        }
    }

    /// Number of allowed hosts (for diagnostics and tests).
    pub fn allowed_count(&self) -> usize {
        self.allowed_hosts.len()
    }

    /// Human-readable, comma-joined list of allowed hosts. Sorted
    /// so error messages are stable across processes (a `HashSet`
    /// yields non-deterministic iteration order otherwise).
    pub fn allowed_hosts_display(&self) -> String {
        let mut hosts: Vec<&String> = self.allowed_hosts.iter().collect();
        hosts.sort();
        hosts.into_iter().cloned().collect::<Vec<_>>().join(", ")
    }

    /// Check that `url` targets an allowed host over an allowed
    /// scheme. Returns `Ok(())` when the URL is permitted, and
    /// `Err(EgressDenied)` carrying the specific reason otherwise.
    ///
    /// Reasons the caller should distinguish:
    /// - `MalformedUrl` — the input isn't a URL we can parse. The
    ///   settings-UI layer should treat this as a validation
    ///   failure and reject the input before it reaches disk.
    /// - `UnsupportedScheme` — scheme other than `http` / `https`.
    /// - `InsecureScheme` — `http://` used with a non-loopback
    ///   host. Never allowed off-loopback because the key travels
    ///   in the `Authorization` header.
    /// - `HostNotAllowed` — host isn't in the allowlist. This is
    ///   the exfiltration-blocking case.
    pub fn check_url(&self, url: &str) -> Result<(), EgressDenied> {
        let parsed = parse_url_scheme_host(url).ok_or_else(|| EgressDenied {
            host: String::new(),
            reason: EgressDenialReason::MalformedUrl,
        })?;
        match parsed.scheme {
            "http" if !is_loopback_host(parsed.host) => {
                return Err(EgressDenied {
                    host: parsed.host.to_string(),
                    reason: EgressDenialReason::InsecureScheme,
                });
            }
            "http" | "https" => {}
            _ => {
                return Err(EgressDenied {
                    host: parsed.host.to_string(),
                    reason: EgressDenialReason::UnsupportedScheme,
                });
            }
        }
        let host_lc = parsed.host.to_ascii_lowercase();
        if self.allowed_hosts.contains(&host_lc) {
            Ok(())
        } else {
            Err(EgressDenied {
                host: parsed.host.to_string(),
                reason: EgressDenialReason::HostNotAllowed,
            })
        }
    }
}

/// Denial record produced when [`EgressPolicy::check_url`] refuses
/// to authorize a URL. Carries the host we objected to so the
/// caller can log or surface it without re-parsing the URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressDenied {
    pub host: String,
    pub reason: EgressDenialReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressDenialReason {
    MalformedUrl,
    UnsupportedScheme,
    InsecureScheme,
    HostNotAllowed,
}

impl std::fmt::Display for EgressDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.reason {
            EgressDenialReason::MalformedUrl => {
                write!(f, "URL could not be parsed")
            }
            EgressDenialReason::UnsupportedScheme => {
                write!(
                    f,
                    "URL scheme not supported (only http/https are allowed); host was {}",
                    self.host
                )
            }
            EgressDenialReason::InsecureScheme => {
                write!(
                    f,
                    "insecure http:// is only allowed for loopback hosts; refusing to send credentials to {} in cleartext",
                    self.host
                )
            }
            EgressDenialReason::HostNotAllowed => {
                write!(
                    f,
                    "host {} is not in the completion egress allowlist (set {} to override)",
                    self.host, ALLOWED_HOSTS_ENV
                )
            }
        }
    }
}

impl std::error::Error for EgressDenied {}

struct ParsedUrl<'a> {
    scheme: &'a str,
    host: &'a str,
}

/// Minimal URL parser that extracts the scheme and host. We
/// deliberately avoid pulling in the `url` crate here — the policy
/// engine is meant to stay dep-light, and the input surface is
/// small enough (operator-entered URLs, no fragments) that a
/// hand-rolled extractor is safer than the surprise-heavy grammar
/// of a full URL parser. Anything we can't cleanly decompose is
/// treated as `MalformedUrl` — a strictly safer failure than a
/// silent misparse.
fn parse_url_scheme_host(url: &str) -> Option<ParsedUrl<'_>> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty() {
        return None;
    }
    // Strip any userinfo prefix (`user:pass@host` isn't something
    // we accept from callers, but rejecting cleanly is better than
    // matching an attacker-supplied `evil.example@api.openai.com`
    // shape).
    let after_userinfo = match rest.rsplit_once('@') {
        Some((_, host_part)) => host_part,
        None => rest,
    };
    // Trim path / query / fragment.
    let host_and_port = after_userinfo
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_userinfo);
    if host_and_port.is_empty() {
        return None;
    }
    // Handle bracketed IPv6 literals: `[::1]:8080` → host = `::1`.
    let host = if let Some(stripped) = host_and_port.strip_prefix('[') {
        let end = stripped.find(']')?;
        &stripped[..end]
    } else {
        // Non-IPv6: strip a trailing `:port` if any.
        host_and_port
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(host_and_port)
    };
    if host.is_empty() {
        return None;
    }
    Some(ParsedUrl { scheme, host })
}

fn is_loopback_host(host: &str) -> bool {
    matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "0.0.0.0" | "::1"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- URL parser -----------------------------------------------

    #[test]
    fn parses_simple_https_url() {
        let p = parse_url_scheme_host("https://api.openai.com/v1").unwrap();
        assert_eq!(p.scheme, "https");
        assert_eq!(p.host, "api.openai.com");
    }

    #[test]
    fn parses_url_with_port() {
        let p = parse_url_scheme_host("http://localhost:11434/v1").unwrap();
        assert_eq!(p.scheme, "http");
        assert_eq!(p.host, "localhost");
    }

    #[test]
    fn parses_ipv6_bracketed_url() {
        let p = parse_url_scheme_host("http://[::1]:8080/v1").unwrap();
        assert_eq!(p.host, "::1");
    }

    #[test]
    fn strips_userinfo_prefix() {
        // Defense against `evil@api.openai.com` obfuscation.
        let p = parse_url_scheme_host("https://evil@api.openai.com/v1").unwrap();
        assert_eq!(p.host, "api.openai.com");
    }

    #[test]
    fn rejects_missing_scheme() {
        assert!(parse_url_scheme_host("api.openai.com/v1").is_none());
    }

    #[test]
    fn rejects_empty_host() {
        assert!(parse_url_scheme_host("https:///v1").is_none());
    }

    // ---- EgressPolicy ---------------------------------------------

    #[test]
    fn default_policy_allows_openai() {
        let p = EgressPolicy::default();
        assert!(p.check_url("https://api.openai.com/v1").is_ok());
    }

    #[test]
    fn default_policy_allows_anthropic() {
        let p = EgressPolicy::default();
        assert!(p.check_url("https://api.anthropic.com").is_ok());
    }

    #[test]
    fn default_policy_allows_local_loopback() {
        let p = EgressPolicy::default();
        assert!(p.check_url("http://localhost:11434/v1").is_ok());
        assert!(p.check_url("http://127.0.0.1:11434/v1").is_ok());
    }

    #[test]
    fn default_policy_denies_random_host() {
        let p = EgressPolicy::default();
        let err = p.check_url("https://attacker.example/v1").unwrap_err();
        assert_eq!(err.reason, EgressDenialReason::HostNotAllowed);
        assert_eq!(err.host, "attacker.example");
    }

    #[test]
    fn default_policy_denies_http_to_non_loopback() {
        // Even if the host were allowed, http:// off-loopback is
        // never OK because the api_key travels in the header.
        let p = EgressPolicy::from_hosts(["api.openai.com"]);
        let err = p.check_url("http://api.openai.com/v1").unwrap_err();
        assert_eq!(err.reason, EgressDenialReason::InsecureScheme);
    }

    #[test]
    fn default_policy_denies_userinfo_impersonation() {
        // Classic phishing shape -- confirm we route based on the
        // real host, not the userinfo prefix.
        let p = EgressPolicy::default();
        let err = p
            .check_url("https://api.openai.com@attacker.example/v1")
            .unwrap_err();
        assert_eq!(err.reason, EgressDenialReason::HostNotAllowed);
        assert_eq!(err.host, "attacker.example");
    }

    #[test]
    fn default_policy_denies_unsupported_scheme() {
        let p = EgressPolicy::default();
        let err = p.check_url("file:///etc/passwd").unwrap_err();
        // `file:` has no host, so we may report either
        // MalformedUrl (empty host) or UnsupportedScheme depending
        // on how the URL is written; both are safe denials. Test
        // asserts we DO deny, without over-fitting to the reason.
        assert!(matches!(
            err.reason,
            EgressDenialReason::UnsupportedScheme | EgressDenialReason::MalformedUrl
        ));
    }

    #[test]
    fn empty_env_produces_deny_all_policy() {
        // An operator can explicitly set the env var to an empty
        // string to disable all outbound completions (airgap test
        // configuration). This must NOT silently fall back to the
        // default list.
        let p = EgressPolicy::from_hosts(std::iter::empty::<String>());
        assert_eq!(p.allowed_count(), 0);
        assert!(p.check_url("https://api.openai.com").is_err());
    }

    #[test]
    fn from_hosts_normalizes_case_and_whitespace() {
        let p = EgressPolicy::from_hosts(["  API.OpenAI.com  ", ""]);
        assert_eq!(p.allowed_count(), 1);
        assert!(p.check_url("https://api.openai.com").is_ok());
        assert!(p.check_url("https://API.OPENAI.COM").is_ok());
    }

    #[test]
    fn allowed_hosts_display_is_sorted_and_stable() {
        // HashSet iteration is non-deterministic. The display
        // helper is used in error messages that show up in tests
        // and audit logs, so its output MUST be stable.
        let p = EgressPolicy::from_hosts(["zeta.example", "alpha.example", "beta.example"]);
        let s = p.allowed_hosts_display();
        assert_eq!(s, "alpha.example, beta.example, zeta.example");
    }

    #[test]
    fn denial_display_names_the_env_override() {
        // Enterprise ops want the error message to tell them what
        // to change. Include the env var name so a Ctrl-F in the
        // stdout stream lands directly on the remediation.
        let p = EgressPolicy::default();
        let err = p.check_url("https://internal.corp/v1").unwrap_err();
        let text = format!("{err}");
        assert!(text.contains(ALLOWED_HOSTS_ENV), "{text}");
        assert!(text.contains("internal.corp"), "{text}");
    }
}
