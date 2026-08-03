//! Bearer-token caller authentication for the MCP HTTP surface.
//!
//! Addresses threat-model T1 at a spine-minimal level: when tokens are
//! configured, request `tenant_id` / `actor_id` (and optional `device_id`)
//! must match the identity bound to the presented Bearer token. This is
//! not OIDC/device attestation (Phase 2) — it is a shared-secret gate so
//! callers can no longer spoof arbitrary tenant labels on an open port.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

/// Refuse to load a token file that is readable or writable by group/other.
/// A shared-secret token store with `0644` permissions is a finding in its own
/// right; failing closed here turns a silent misconfiguration into a startup
/// error the operator must fix. No-op on non-Unix hosts, whose ACL model this
/// mode check does not map onto.
fn reject_insecure_token_file_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::metadata(path).map_err(|e| e.to_string())?;
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(format!(
                "token file {} is accessible by group/other (mode {:o}); \
                 restrict it with `chmod 600`",
                path.display(),
                mode & 0o777
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Compare two byte strings without short-circuiting on the first differing
/// byte. The running time depends only on the lengths (which for a bearer
/// token are not the sensitive part), never on *where* two strings first
/// differ — which is exactly the signal a timing attacker uses to recover a
/// secret one byte at a time. Kept dependency-free and `#[inline(never)]` so a
/// future optimizer pass can't reintroduce an early exit.
#[inline(never)]
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u64;
    let n = a.len().max(b.len());
    for i in 0..n {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= u64::from(x ^ y);
    }
    diff == 0
}

/// Authenticated caller identity derived from a configured API token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerIdentity {
    pub tenant_id: String,
    pub actor_id: String,
    pub device_id: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("missing Authorization bearer token")]
    MissingToken,
    #[error("invalid Authorization header (expected Bearer <token>)")]
    MalformedHeader,
    #[error("unknown or revoked API token")]
    UnknownToken,
    #[error("request tenant_id does not match authenticated token")]
    TenantMismatch,
    #[error("request actor_id does not match authenticated token")]
    ActorMismatch,
    #[error("request device_id does not match authenticated token")]
    DeviceMismatch,
}

#[derive(Debug, Clone, Deserialize)]
struct TokenRecord {
    token: String,
    tenant_id: String,
    actor_id: String,
    #[serde(default)]
    device_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TokenFile {
    tokens: Vec<TokenRecord>,
}

/// Lookup table of shared-secret tokens → bound identity.
#[derive(Debug, Clone, Default)]
pub struct TokenAuthenticator {
    by_token: HashMap<String, CallerIdentity>,
}

impl TokenAuthenticator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, token: impl Into<String>, identity: CallerIdentity) {
        self.by_token.insert(token.into(), identity);
    }

    pub fn is_empty(&self) -> bool {
        self.by_token.is_empty()
    }

    /// Load from a JSON file:
    /// `{ "tokens": [ { "token": "...", "tenant_id": "...", "actor_id": "...", "device_id": null } ] }`
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        reject_insecure_token_file_permissions(path)?;
        let raw = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let file: TokenFile = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let mut auth = Self::new();
        for rec in file.tokens {
            if rec.token.trim().is_empty() {
                return Err("token entry has empty token string".into());
            }
            auth.insert(
                rec.token,
                CallerIdentity {
                    tenant_id: rec.tenant_id,
                    actor_id: rec.actor_id,
                    device_id: rec.device_id,
                },
            );
        }
        Ok(auth)
    }

    /// Compact env form:
    /// `token:tenant:actor` or `token:tenant:actor:device`, entries separated by `;`.
    pub fn parse_env_list(raw: &str) -> Result<Self, String> {
        let mut auth = Self::new();
        for entry in raw.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let parts: Vec<&str> = entry.split(':').collect();
            if parts.len() < 3 || parts.len() > 4 {
                return Err(format!(
                    "invalid RALLEH_API_TOKENS entry '{entry}' (expected token:tenant:actor[:device])"
                ));
            }
            if parts[0].is_empty() || parts[1].is_empty() || parts[2].is_empty() {
                return Err(format!(
                    "invalid RALLEH_API_TOKENS entry '{entry}' (empty field)"
                ));
            }
            auth.insert(
                parts[0],
                CallerIdentity {
                    tenant_id: parts[1].to_string(),
                    actor_id: parts[2].to_string(),
                    device_id: parts.get(3).map(|s| (*s).to_string()),
                },
            );
        }
        Ok(auth)
    }

    /// Resolve from env: prefer `RALLEH_API_TOKENS_FILE`, else `RALLEH_API_TOKENS`.
    /// Returns `Ok(None)` when neither is set (open / dev mode).
    pub fn from_env() -> Result<Option<Self>, String> {
        if let Ok(path) = std::env::var("RALLEH_API_TOKENS_FILE") {
            let auth = Self::load_from_path(path)?;
            if auth.is_empty() {
                return Err("RALLEH_API_TOKENS_FILE contained no tokens".into());
            }
            return Ok(Some(auth));
        }
        if let Ok(list) = std::env::var("RALLEH_API_TOKENS") {
            let auth = Self::parse_env_list(&list)?;
            if auth.is_empty() {
                return Err("RALLEH_API_TOKENS was set but empty".into());
            }
            return Ok(Some(auth));
        }
        Ok(None)
    }

    pub fn authenticate(&self, authorization: Option<&str>) -> Result<CallerIdentity, AuthError> {
        let header = authorization.ok_or(AuthError::MissingToken)?;
        let token = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or(AuthError::MalformedHeader)?;

        // Constant-time lookup: compare the presented token against EVERY
        // configured token without short-circuiting, so neither the wall-clock
        // time nor the position of the first differing byte leaks information
        // about the secret (a `HashMap::get` + `==` would leak both). We scan
        // the whole table and never early-return on a hit for the same reason.
        let presented = token.as_bytes();
        let mut matched: Option<&CallerIdentity> = None;
        for (candidate, identity) in &self.by_token {
            if constant_time_eq(candidate.as_bytes(), presented) {
                matched = Some(identity);
            }
        }
        matched.cloned().ok_or(AuthError::UnknownToken)
    }

    /// Ensure the request's claimed labels match the authenticated identity.
    pub fn enforce(
        identity: &CallerIdentity,
        tenant_id: &str,
        actor_id: &str,
        device_id: &str,
    ) -> Result<(), AuthError> {
        if identity.tenant_id != tenant_id {
            return Err(AuthError::TenantMismatch);
        }
        if identity.actor_id != actor_id {
            return Err(AuthError::ActorMismatch);
        }
        if let Some(bound_device) = &identity.device_id {
            if bound_device != device_id {
                return Err(AuthError::DeviceMismatch);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_env_list_supports_optional_device() {
        let auth = TokenAuthenticator::parse_env_list(
            "tok-a:tenant-a:user-a:device-a;tok-b:tenant-b:user-b",
        )
        .unwrap();
        let a = auth.authenticate(Some("Bearer tok-a")).unwrap();
        assert_eq!(a.tenant_id, "tenant-a");
        assert_eq!(a.device_id.as_deref(), Some("device-a"));
        let b = auth.authenticate(Some("Bearer tok-b")).unwrap();
        assert_eq!(b.device_id, None);
    }

    #[test]
    fn authenticate_rejects_unknown_and_missing() {
        let mut auth = TokenAuthenticator::new();
        auth.insert(
            "good",
            CallerIdentity {
                tenant_id: "t1".into(),
                actor_id: "u1".into(),
                device_id: None,
            },
        );
        assert_eq!(
            auth.authenticate(None).unwrap_err(),
            AuthError::MissingToken
        );
        assert_eq!(
            auth.authenticate(Some("Basic x")).unwrap_err(),
            AuthError::MalformedHeader
        );
        assert_eq!(
            auth.authenticate(Some("Bearer bad")).unwrap_err(),
            AuthError::UnknownToken
        );
    }

    #[test]
    fn enforce_checks_tenant_actor_device() {
        let identity = CallerIdentity {
            tenant_id: "t1".into(),
            actor_id: "u1".into(),
            device_id: Some("d1".into()),
        };
        assert!(TokenAuthenticator::enforce(&identity, "t1", "u1", "d1").is_ok());
        assert_eq!(
            TokenAuthenticator::enforce(&identity, "t2", "u1", "d1").unwrap_err(),
            AuthError::TenantMismatch
        );
        assert_eq!(
            TokenAuthenticator::enforce(&identity, "t1", "u2", "d1").unwrap_err(),
            AuthError::ActorMismatch
        );
        assert_eq!(
            TokenAuthenticator::enforce(&identity, "t1", "u1", "d2").unwrap_err(),
            AuthError::DeviceMismatch
        );
    }

    #[test]
    fn constant_time_eq_matches_only_identical_bytes() {
        assert!(constant_time_eq(b"secret-token", b"secret-token"));
        assert!(!constant_time_eq(b"secret-token", b"secret-tokeX"));
        assert!(!constant_time_eq(b"secret", b"secret-token")); // length differs
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_group_or_other_accessible_token_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        fs::write(
            &path,
            r#"{"tokens":[{"token":"abc","tenant_id":"t1","actor_id":"u1"}]}"#,
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let err = TokenAuthenticator::load_from_path(&path).unwrap_err();
        assert!(err.contains("group/other"), "unexpected error: {err}");

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(TokenAuthenticator::load_from_path(&path).is_ok());
    }

    #[test]
    fn load_from_json_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tokens.json");
        fs::write(
            &path,
            r#"{
              "tokens": [
                {"token": "abc", "tenant_id": "t1", "actor_id": "u1", "device_id": "d1"}
              ]
            }"#,
        )
        .unwrap();
        // The loader refuses group/other-accessible token files; a real
        // deployment would `chmod 600`, so mirror that here.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let auth = TokenAuthenticator::load_from_path(&path).unwrap();
        let id = auth.authenticate(Some("Bearer abc")).unwrap();
        assert_eq!(id.actor_id, "u1");
    }
}
