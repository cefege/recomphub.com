//! OS-backed credential storage for desktop account envelopes.
//!
//! This module stores opaque envelope bytes only. The web shell owns the envelope
//! schema and never falls back to a plaintext file when the operating-system store
//! is unavailable.

use std::fmt;

pub const SERVICE: &str = "com.recomphub.desktop";

#[derive(Debug)]
pub enum CredentialError {
    Unavailable(String),
    InvalidOrigin,
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(message) => formatter.write_str(message),
            Self::InvalidOrigin => formatter.write_str("credential origin is invalid"),
        }
    }
}

impl std::error::Error for CredentialError {}

/// Persists opaque, serialized account envelopes under a normalized API origin.
///
/// `None` means no credential exists. Any error means the OS store could not be
/// consulted; callers must keep the account locked or use their in-memory session.
#[derive(Debug, Clone, Copy, Default)]
pub struct CredentialStore;

/// Injectable OS-store boundary for deterministic account error/restore tests.
pub trait CredentialBackend: Send + Sync {
    fn load(&self, origin: &str) -> Result<Option<Vec<u8>>, CredentialError>;
    fn save(&self, origin: &str, envelope: &[u8]) -> Result<(), CredentialError>;
    fn delete(&self, origin: &str) -> Result<(), CredentialError>;
}

impl CredentialStore {
    pub const fn new() -> Self {
        Self
    }
}

impl CredentialBackend for CredentialStore {
    fn load(&self, origin: &str) -> Result<Option<Vec<u8>>, CredentialError> {
        let account = normalized_origin(origin)?;
        platform::load(&account)
    }

    fn save(&self, origin: &str, envelope: &[u8]) -> Result<(), CredentialError> {
        let account = normalized_origin(origin)?;
        if envelope.is_empty() {
            return Err(CredentialError::Unavailable(
                "refusing to store an empty account envelope".to_string(),
            ));
        }
        platform::save(&account, envelope)
    }

    fn delete(&self, origin: &str) -> Result<(), CredentialError> {
        let account = normalized_origin(origin)?;
        platform::delete(&account)
    }
}

fn normalized_origin(origin: &str) -> Result<String, CredentialError> {
    let parsed = url::Url::parse(origin).map_err(|_| CredentialError::InvalidOrigin)?;
    if parsed.cannot_be_a_base()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(CredentialError::InvalidOrigin);
    }
    match parsed.scheme() {
        "https" | "http" => {}
        _ => return Err(CredentialError::InvalidOrigin),
    }
    if parsed.host_str().is_none() {
        return Err(CredentialError::InvalidOrigin);
    }
    Ok(parsed.origin().ascii_serialization())
}

#[cfg(any(target_os = "macos", windows))]
mod platform {
    use super::{CredentialError, SERVICE};

    fn entry(account: &str) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new(SERVICE, account)
            .map_err(|error| CredentialError::Unavailable(error.to_string()))
    }

    pub fn load(account: &str) -> Result<Option<Vec<u8>>, CredentialError> {
        match entry(account)?.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(CredentialError::Unavailable(error.to_string())),
        }
    }

    pub fn save(account: &str, envelope: &[u8]) -> Result<(), CredentialError> {
        entry(account)?
            .set_secret(envelope)
            .map_err(|error| CredentialError::Unavailable(error.to_string()))
    }

    pub fn delete(account: &str) -> Result<(), CredentialError> {
        match entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(CredentialError::Unavailable(error.to_string())),
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use super::CredentialError;

    pub fn load(_: &str) -> Result<Option<Vec<u8>>, CredentialError> {
        Err(CredentialError::Unavailable(
            "this platform has no native credential backend".to_string(),
        ))
    }

    pub fn save(_: &str, _: &[u8]) -> Result<(), CredentialError> {
        Err(CredentialError::Unavailable(
            "this platform has no native credential backend".to_string(),
        ))
    }

    pub fn delete(_: &str) -> Result<(), CredentialError> {
        Err(CredentialError::Unavailable(
            "this platform has no native credential backend".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    #[cfg(any(target_os = "macos", windows))]
    use super::CredentialBackend;
    use super::normalized_origin;

    #[test]
    fn origin_normalization_rejects_non_origins() {
        assert_eq!(
            normalized_origin("https://RECOMPHUB.com:443").unwrap(),
            "https://recomphub.com"
        );
        for value in [
            "https://recomphub.com/path",
            "https://user@recomphub.com",
            "ftp://recomphub.com",
        ] {
            assert!(normalized_origin(value).is_err(), "{value}");
        }
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    #[ignore = "requires access to the interactive OS credential store"]
    fn native_os_credential_roundtrip() {
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = super::CredentialStore::new().delete(&self.0);
            }
        }
        let account = Cleanup(format!(
            "https://{}.credential-smoke.invalid",
            uuid::Uuid::new_v4()
        ));
        let store = super::CredentialStore::new();
        assert!(
            store
                .load(&account.0)
                .expect("read test credential")
                .is_none()
        );
        store
            .save(&account.0, b"first native smoke envelope")
            .expect("save test credential");
        assert_eq!(
            store
                .load(&account.0)
                .expect("read saved credential")
                .as_deref(),
            Some(b"first native smoke envelope".as_slice())
        );
        store
            .save(&account.0, b"updated native smoke envelope")
            .expect("update test credential");
        assert_eq!(
            store
                .load(&account.0)
                .expect("read updated credential")
                .as_deref(),
            Some(b"updated native smoke envelope".as_slice())
        );
        store
            .delete(&account.0)
            .expect("delete only test credential");
        assert!(
            store
                .load(&account.0)
                .expect("read removed test credential")
                .is_none()
        );
    }
}
