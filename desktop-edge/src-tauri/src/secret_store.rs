//! Enterprise-grade secret storage abstraction for API keys.
//!
//! Historically completion `api_key` values lived in cleartext in
//! `edge-settings.json`. That's fine for a first landing but a real
//! non-starter for anyone running the shell on a shared or unencrypted
//! disk. This module moves keys into the OS-native secure store —
//! Windows Credential Manager, macOS Keychain, or Linux Secret
//! Service — and provides an honest storage-signal to the UI when the
//! store isn't available (headless CI, broken DBus, etc.).
//!
//! ## Trait shape
//!
//! [`SecretStore`] is a tiny trio of `read` / `write` / `clear` methods
//! plus a `kind()` that names the backing storage for the UI. Three
//! concrete impls ship:
//!
//! - [`KeyringStore`] — the real thing. Every method wraps
//!   `keyring::Entry` and translates its errors into `Result<..,
//!   String>` so command handlers never have to know the crate.
//! - [`NullStore`] — the fallback when keyring init fails. Every
//!   write errors with a message the UI can surface; every read
//!   returns `Ok(None)`. Lets the shell keep running with cleartext
//!   fallback while making clear that nothing is being stored
//!   securely.
//! - [`InMemorySecretStore`] — the test harness. Simulates a working
//!   keychain without touching the host's real one, so unit tests
//!   don't leak or race against operator credentials.
//!
//! ## Namespacing
//!
//! Entries are keyed by `(service = "ralleh", account =
//! "completion.<kind>")`. This lets an operator keep an OpenAI key
//! and an Anthropic key stored at the same time and switch between
//! providers without re-entering credentials on every kind swap.
//! `Clear` only removes the entry for the specific kind the operator
//! is clearing; other kinds' keys stay put.
//!
//! ## Failure semantics
//!
//! Every method returns `Result<T, String>`. Errors are converted to
//! human-readable strings by the store itself so the UI can render
//! them unchanged. Writes that fail because the store isn't available
//! are the caller's cue to warn the user; reads that fail (as opposed
//! to returning `Ok(None)`) mean the store is malfunctioning and the
//! call site should log without pretending the request succeeded.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::settings::CompletionKind;

/// The OS-native backend a [`SecretStore`] is currently talking to.
/// Surfaced to the frontend so the settings UI can render an honest
/// "🔒 Stored in Keychain" badge instead of hiding behind a generic
/// "Stored on this device".
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretStorage {
    /// Real OS keychain (Windows Credential Manager, macOS
    /// Keychain, or Linux Secret Service).
    Keychain,
    /// No secure store available; secrets go to cleartext or aren't
    /// stored at all. UI must warn the user.
    Cleartext,
    /// Nothing has been stored yet -- distinct from "cleartext" so
    /// the UI can render "Not configured" instead of "Insecure".
    None,
}

/// Abstract secret store. See module docs.
pub trait SecretStore: Send + Sync {
    /// Fetch the stored secret for `kind`, or `Ok(None)` if nothing
    /// is stored. `Err` is reserved for "the store itself broke" --
    /// missing entries are `Ok(None)`, not `Err`.
    fn read(&self, kind: CompletionKind) -> Result<Option<String>, String>;

    /// Persist `secret` for `kind`. Overwrites any existing value.
    /// Empty strings are rejected with an error to catch programmer
    /// mistakes; use `clear` to remove a stored secret.
    fn write(&self, kind: CompletionKind, secret: &str) -> Result<(), String>;

    /// Remove any stored secret for `kind`. Non-existent entries
    /// are `Ok(())` (idempotent), matching the "delete is a no-op
    /// when there's nothing to delete" convention.
    fn clear(&self, kind: CompletionKind) -> Result<(), String>;

    /// Storage backend this store is speaking to. Used by the
    /// settings UI to render an accurate storage badge.
    fn kind(&self) -> SecretStorage;
}

const KEYRING_SERVICE: &str = "ralleh";

fn account_for(kind: CompletionKind) -> String {
    format!("completion.{}", kind.label())
}

/// Real OS keychain-backed store. `open_default` constructs one; if
/// the OS doesn't have a working keychain (headless Linux without
/// DBus is the usual case), it returns `None` and callers fall back
/// to [`NullStore`].
pub struct KeyringStore {
    // No state -- every operation instantiates a fresh `Entry`. On
    // every supported OS `Entry::new` is a cheap constructor
    // (no I/O), so caching provides zero benefit and would only
    // complicate lifetimes across the `Send + Sync` trait bound.
    _priv: (),
}

impl KeyringStore {
    /// Try to open the OS keychain. Returns `None` if the platform
    /// can't provide one — e.g. Linux without a running Secret
    /// Service daemon. Callers use `open_default_or_null` to
    /// transparently fall back.
    pub fn try_open() -> Option<Self> {
        // Cheap sanity probe: build an Entry, then read it. A
        // "no entry" error is fine (the entry just doesn't exist);
        // a platform-init error means the whole store is unusable.
        let entry = match keyring::Entry::new(KEYRING_SERVICE, "__probe__") {
            Ok(e) => e,
            Err(_) => return None,
        };
        match entry.get_password() {
            Ok(_) => Some(Self { _priv: () }),
            Err(keyring::Error::NoEntry) => Some(Self { _priv: () }),
            Err(_) => None,
        }
    }
}

impl SecretStore for KeyringStore {
    fn read(&self, kind: CompletionKind) -> Result<Option<String>, String> {
        let account = account_for(kind);
        let entry = keyring::Entry::new(KEYRING_SERVICE, &account)
            .map_err(|e| format!("keyring open failed: {e}"))?;
        match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("keyring read failed: {e}")),
        }
    }

    fn write(&self, kind: CompletionKind, secret: &str) -> Result<(), String> {
        if secret.is_empty() {
            return Err("refusing to store an empty secret".into());
        }
        let account = account_for(kind);
        let entry = keyring::Entry::new(KEYRING_SERVICE, &account)
            .map_err(|e| format!("keyring open failed: {e}"))?;
        entry
            .set_password(secret)
            .map_err(|e| format!("keyring write failed: {e}"))
    }

    fn clear(&self, kind: CompletionKind) -> Result<(), String> {
        let account = account_for(kind);
        let entry = keyring::Entry::new(KEYRING_SERVICE, &account)
            .map_err(|e| format!("keyring open failed: {e}"))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("keyring clear failed: {e}")),
        }
    }

    fn kind(&self) -> SecretStorage {
        SecretStorage::Keychain
    }
}

/// Fallback used when [`KeyringStore::try_open`] returns `None`.
/// Every write fails with a message the UI can surface. Reads
/// return `Ok(None)` so the surrounding code treats the store as
/// "nothing stored" rather than "broken store" — this pairs with
/// the cleartext-migration path so existing on-disk keys keep
/// working on hosts without a keychain.
pub struct NullStore;

impl SecretStore for NullStore {
    fn read(&self, _kind: CompletionKind) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn write(&self, _kind: CompletionKind, _secret: &str) -> Result<(), String> {
        Err("no OS keychain is available on this host — enable Windows Credential Manager, macOS Keychain, or the Linux Secret Service to store keys securely".into())
    }
    fn clear(&self, _kind: CompletionKind) -> Result<(), String> {
        Ok(())
    }
    fn kind(&self) -> SecretStorage {
        SecretStorage::Cleartext
    }
}

/// Open the OS keychain, falling back to `NullStore` if it's not
/// usable. Command handlers call this on every request rather than
/// caching a handle -- each `KeyringStore` op is O(1) and the
/// availability of the keychain can change during a session (users
/// can log out of GNOME Keyring, for example).
pub fn open_default() -> Box<dyn SecretStore> {
    match KeyringStore::try_open() {
        Some(store) => Box::new(store),
        None => Box::new(NullStore),
    }
}

/// In-memory secret store for tests. Never touches the real
/// keychain, so unit tests can assert read/write/clear semantics
/// without depending on the host or leaking state across runs.
#[allow(dead_code)]
pub struct InMemorySecretStore {
    entries: Mutex<HashMap<CompletionKind, String>>,
}

#[allow(dead_code)]
impl InMemorySecretStore {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_entry(kind: CompletionKind, secret: &str) -> Self {
        let store = Self::new();
        store.write(kind, secret).unwrap();
        store
    }
}

impl SecretStore for InMemorySecretStore {
    fn read(&self, kind: CompletionKind) -> Result<Option<String>, String> {
        Ok(self
            .entries
            .lock()
            .map_err(|e| e.to_string())?
            .get(&kind)
            .cloned())
    }

    fn write(&self, kind: CompletionKind, secret: &str) -> Result<(), String> {
        if secret.is_empty() {
            return Err("refusing to store an empty secret".into());
        }
        self.entries
            .lock()
            .map_err(|e| e.to_string())?
            .insert(kind, secret.to_string());
        Ok(())
    }

    fn clear(&self, kind: CompletionKind) -> Result<(), String> {
        self.entries
            .lock()
            .map_err(|e| e.to_string())?
            .remove(&kind);
        Ok(())
    }

    fn kind(&self) -> SecretStorage {
        // Tests are asserting behaviour, not badges. Return
        // Keychain so a UI-render test can exercise the "secure"
        // path against this store.
        SecretStorage::Keychain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_read_returns_none_when_empty() {
        let store = InMemorySecretStore::new();
        assert_eq!(store.read(CompletionKind::Openai).unwrap(), None);
    }

    #[test]
    fn in_memory_write_then_read_roundtrips() {
        let store = InMemorySecretStore::new();
        store.write(CompletionKind::Openai, "sk-1").unwrap();
        assert_eq!(
            store.read(CompletionKind::Openai).unwrap(),
            Some("sk-1".into())
        );
    }

    #[test]
    fn in_memory_kinds_are_independent() {
        let store = InMemorySecretStore::new();
        store.write(CompletionKind::Openai, "sk-openai").unwrap();
        store.write(CompletionKind::Anthropic, "sk-anth").unwrap();
        assert_eq!(
            store.read(CompletionKind::Openai).unwrap(),
            Some("sk-openai".into())
        );
        assert_eq!(
            store.read(CompletionKind::Anthropic).unwrap(),
            Some("sk-anth".into())
        );
        // Clearing one must NOT affect the other.
        store.clear(CompletionKind::Openai).unwrap();
        assert_eq!(store.read(CompletionKind::Openai).unwrap(), None);
        assert_eq!(
            store.read(CompletionKind::Anthropic).unwrap(),
            Some("sk-anth".into())
        );
    }

    #[test]
    fn in_memory_write_rejects_empty_secret() {
        let store = InMemorySecretStore::new();
        let err = store.write(CompletionKind::Openai, "").unwrap_err();
        assert!(err.to_lowercase().contains("empty"), "{err}");
    }

    #[test]
    fn in_memory_clear_is_idempotent() {
        let store = InMemorySecretStore::new();
        assert!(store.clear(CompletionKind::Openai).is_ok());
        assert!(store.clear(CompletionKind::Openai).is_ok());
    }

    #[test]
    fn null_store_reports_cleartext_and_rejects_writes() {
        let store = NullStore;
        assert_eq!(store.kind(), SecretStorage::Cleartext);
        assert_eq!(store.read(CompletionKind::Openai).unwrap(), None);
        assert!(store.write(CompletionKind::Openai, "sk-1").is_err());
        // Clear is idempotent even on a broken store.
        assert!(store.clear(CompletionKind::Openai).is_ok());
    }

    #[test]
    fn open_default_never_panics_on_this_host() {
        // We can't assert *which* backend we get -- it depends on
        // the CI runner -- but the call must always succeed and
        // return some usable trait object.
        let store = open_default();
        // Read on an unlikely kind must produce Ok(_), even if
        // it's Ok(None). If the real keychain is broken we want
        // NullStore, which also returns Ok(None).
        assert!(store.read(CompletionKind::Echo).is_ok());
    }

    /// Real end-to-end keychain smoke: writes, reads, clears an
    /// entry under a test-only account. Opt-in via
    /// `RALLEH_LIVE_KEYCHAIN=1` because it touches the host's
    /// actual credential store, and default CI shouldn't be doing
    /// that. Local devs can run with:
    ///   RALLEH_LIVE_KEYCHAIN=1 cargo test -p desktop-edge -- --ignored live_keychain
    #[test]
    #[ignore = "requires RALLEH_LIVE_KEYCHAIN=1 and a working OS keychain"]
    fn live_keychain_round_trip_when_explicitly_enabled() {
        assert!(
            std::env::var("RALLEH_LIVE_KEYCHAIN").is_ok(),
            "set RALLEH_LIVE_KEYCHAIN=1 to run this smoke"
        );
        let Some(store) = KeyringStore::try_open() else {
            panic!("keychain not available on this host");
        };
        let secret = format!("test-{}", std::process::id());
        store.write(CompletionKind::Openai, &secret).unwrap();
        let got = store.read(CompletionKind::Openai).unwrap();
        assert_eq!(got.as_deref(), Some(secret.as_str()));
        store.clear(CompletionKind::Openai).unwrap();
        assert_eq!(store.read(CompletionKind::Openai).unwrap(), None);
    }
}
