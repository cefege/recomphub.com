//! AES-256-GCM sealing for provider credentials at rest.
//!
//! The key comes from `SECRETS_KEY`: 32 random bytes, base64 encoded
//! (`openssl rand -base64 32`). A sealed value is `nonce (12 bytes) || ciphertext || tag`.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine as _;
use rand::RngCore;

pub const SECRETS_KEY_ENV: &str = "SECRETS_KEY";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("SECRETS_KEY is not set")]
    Missing,
    #[error("SECRETS_KEY must be base64 of exactly 32 bytes")]
    InvalidKey,
    #[error("sealed value could not be encrypted")]
    Seal,
    #[error("sealed value could not be opened (wrong key or corrupted data)")]
    Open,
}

/// Seals and opens byte strings with the server key.
pub struct Sealer {
    cipher: Aes256Gcm,
}

impl Sealer {
    pub fn from_env() -> Result<Self, SecretsError> {
        let raw = std::env::var(SECRETS_KEY_ENV).map_err(|_| SecretsError::Missing)?;
        Self::from_base64(raw.trim())
    }

    pub fn from_base64(raw: &str) -> Result<Self, SecretsError> {
        let key = base64::engine::general_purpose::STANDARD
            .decode(raw)
            .map_err(|_| SecretsError::InvalidKey)?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| SecretsError::InvalidKey)?;
        Ok(Self { cipher })
    }

    pub fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, SecretsError> {
        let mut nonce = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher
            .encrypt(&Nonce::from(nonce), plaintext)
            .map_err(|_| SecretsError::Seal)?;
        let mut sealed = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&ciphertext);
        Ok(sealed)
    }

    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, SecretsError> {
        if sealed.len() < NONCE_LEN + TAG_LEN {
            return Err(SecretsError::Open);
        }
        let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
        let mut fixed = [0u8; NONCE_LEN];
        fixed.copy_from_slice(nonce);
        self.cipher
            .decrypt(&Nonce::from(fixed), ciphertext)
            .map_err(|_| SecretsError::Open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> String {
        base64::engine::general_purpose::STANDARD.encode([byte; 32])
    }

    #[test]
    fn round_trips_and_uses_fresh_nonces() {
        let sealer = Sealer::from_base64(&key(7)).unwrap();
        let a = sealer.seal(b"secret-token").unwrap();
        let b = sealer.seal(b"secret-token").unwrap();
        assert_ne!(a, b, "each seal must use a fresh nonce");
        assert_eq!(sealer.open(&a).unwrap(), b"secret-token");
    }

    #[test]
    fn rejects_wrong_key_and_tampering() {
        let sealed = Sealer::from_base64(&key(1)).unwrap().seal(b"x").unwrap();
        assert!(Sealer::from_base64(&key(2)).unwrap().open(&sealed).is_err());
        let mut tampered = sealed.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert!(
            Sealer::from_base64(&key(1))
                .unwrap()
                .open(&tampered)
                .is_err()
        );
        assert!(
            Sealer::from_base64(&key(1))
                .unwrap()
                .open(&sealed[..10])
                .is_err()
        );
    }

    #[test]
    fn rejects_keys_of_the_wrong_length() {
        let short = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        assert!(matches!(
            Sealer::from_base64(&short),
            Err(SecretsError::InvalidKey)
        ));
        assert!(matches!(
            Sealer::from_base64("not base64!"),
            Err(SecretsError::InvalidKey)
        ));
    }
}
