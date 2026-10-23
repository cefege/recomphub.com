//! Native account policy. Tokens never leave the Rust-side session controller.

use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use recomphub_shared::SessionUser;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthStatus {
    Checking,
    SignedOut,
    WaitingForBrowser,
    AuthenticatedOnline,
    AuthenticatedOffline,
    Expired,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AuthView {
    pub status: AuthStatus,
    pub user: Option<SessionUser>,
    pub message: Option<String>,
}

impl AuthView {
    pub fn unlocked(&self) -> bool {
        matches!(
            self.status,
            AuthStatus::AuthenticatedOnline | AuthStatus::AuthenticatedOffline
        ) && self.user.is_some()
    }

    pub fn online(&self) -> bool {
        self.status == AuthStatus::AuthenticatedOnline && self.user.is_some()
    }
}

// Deliberately no Debug: the envelope contains a bearer credential.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct SessionEnvelope {
    pub version: u8,
    pub origin: String,
    pub session_token: String,
    pub expires_at: DateTime<Utc>,
    pub validated_user: Option<SessionUser>,
}

impl SessionEnvelope {
    pub fn usable(&self, origin: &str, now: DateTime<Utc>) -> bool {
        self.version == 1
            && self.origin == origin
            && self.expires_at > now
            && self.validated_user.as_ref().is_some_and(|user| user.id > 0)
            && valid_token(&self.session_token)
    }

    pub fn signed_out(origin: String) -> Self {
        Self {
            version: 1,
            origin,
            session_token: String::new(),
            expires_at: DateTime::UNIX_EPOCH,
            validated_user: None,
        }
    }
}

pub(super) fn valid_token(value: &str) -> bool {
    let mut decoded = [0u8; 32];
    value.len() == 43
        && URL_SAFE_NO_PAD
            .decode_slice(value, &mut decoded)
            .is_ok_and(|count| count == decoded.len())
}

pub(super) struct SessionState {
    pub generation: u64,
    pub view: AuthView,
    pub envelope: Option<SessionEnvelope>,
    pub allow_stored_restore: bool,
    pub cleanup_pending: bool,
    pub cleanup_generation: u64,
    deadline: Option<Instant>,
}

impl SessionState {
    pub fn new() -> Self {
        Self {
            generation: 0,
            view: AuthView {
                status: AuthStatus::Checking,
                user: None,
                message: None,
            },
            envelope: None,
            allow_stored_restore: true,
            cleanup_pending: false,
            cleanup_generation: 0,
            deadline: None,
        }
    }

    pub fn begin(&mut self, status: AuthStatus) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        if !self.view.unlocked() || status == AuthStatus::WaitingForBrowser {
            self.view = AuthView {
                status,
                user: None,
                message: None,
            };
        }
        self.generation
    }

    pub fn lock(&mut self, status: AuthStatus, message: Option<String>) {
        self.generation = self.generation.wrapping_add(1);
        self.envelope = None;
        self.deadline = None;
        self.view = AuthView {
            status,
            user: None,
            message,
        };
        if matches!(status, AuthStatus::Expired | AuthStatus::SignedOut) {
            self.cleanup_pending = true;
            self.cleanup_generation = self.generation;
            self.allow_stored_restore = false;
        }
    }

    pub fn accept(
        &mut self,
        generation: u64,
        envelope: SessionEnvelope,
        online: bool,
        origin: &str,
        now: DateTime<Utc>,
        monotonic_now: Instant,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        if !envelope.usable(origin, now) {
            self.lock(
                AuthStatus::Expired,
                Some("Your session expired. Continue with Google to sign in again.".into()),
            );
            return false;
        }
        let remaining = (envelope.expires_at - now)
            .to_std()
            .unwrap_or(Duration::ZERO);
        let Some(candidate_deadline) = monotonic_now.checked_add(remaining) else {
            self.lock(
                AuthStatus::Error,
                Some("The server returned an invalid session lifetime.".into()),
            );
            return false;
        };
        // Revalidation, network recovery and wall-clock rollback must not extend
        // the deadline established for this exact session during this process.
        let same_session = self
            .envelope
            .as_ref()
            .is_some_and(|current| current.session_token == envelope.session_token);
        self.deadline = Some(if same_session {
            self.deadline
                .map_or(candidate_deadline, |old| old.min(candidate_deadline))
        } else {
            candidate_deadline
        });
        self.view = AuthView {
            status: if online { AuthStatus::AuthenticatedOnline } else { AuthStatus::AuthenticatedOffline },
            user: envelope.validated_user.clone(),
            message: (!online).then(|| "Account sync is offline; device-local games and public installers remain available.".into()),
        };
        self.envelope = Some(envelope);
        self.allow_stored_restore = true;
        self.expire(now, monotonic_now);
        self.view.unlocked()
    }

    pub fn expire(&mut self, now: DateTime<Utc>, monotonic_now: Instant) -> bool {
        if self.view.unlocked()
            && (self
                .envelope
                .as_ref()
                .is_none_or(|value| value.expires_at <= now)
                || self
                    .deadline
                    .is_none_or(|deadline| monotonic_now >= deadline))
        {
            self.lock(
                AuthStatus::Expired,
                Some("Your session expired. Continue with Google to sign in again.".into()),
            );
            return true;
        }
        false
    }

    pub fn require(
        &mut self,
        online: bool,
        now: DateTime<Utc>,
        monotonic_now: Instant,
    ) -> Result<(), String> {
        self.expire(now, monotonic_now);
        if !self.view.unlocked() {
            return Err("Sign in to use account sync.".into());
        }
        if online && !self.view.online() {
            return Err("This account sync action needs an online session. Reconnect and refresh your account.".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(now: DateTime<Utc>) -> SessionEnvelope {
        SessionEnvelope {
            version: 1,
            origin: "https://recomphub.com".into(),
            session_token: URL_SAFE_NO_PAD.encode([7; 32]),
            expires_at: now + chrono::Duration::seconds(60),
            validated_user: Some(SessionUser {
                id: 7,
                display_name: "Fixture".into(),
                github_login: None,
                role: "member".into(),
                avatar: None,
            }),
        }
    }

    #[test]
    fn offline_requires_validated_unexpired_identity_and_denies_network_actions() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let clock = Instant::now();
        let mut state = SessionState::new();
        assert!(state.require(false, now, clock).is_err());
        let mut unvalidated = envelope(now);
        unvalidated.validated_user = None;
        assert!(!state.accept(0, unvalidated, false, "https://recomphub.com", now, clock));
        let generation = state.begin(AuthStatus::Checking);
        assert!(state.accept(
            generation,
            envelope(now),
            false,
            "https://recomphub.com",
            now,
            clock
        ));
        assert!(state.require(false, now, clock).is_ok());
        assert!(state.require(true, now, clock).is_err());
        assert!(
            state
                .require(false, now + chrono::Duration::seconds(60), clock)
                .is_err()
        );
        assert_eq!(state.view.status, AuthStatus::Expired);
    }

    #[test]
    fn monotonic_expiry_survives_wall_clock_rollback_and_refresh() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let clock = Instant::now();
        let mut state = SessionState::new();
        assert!(state.accept(0, envelope(now), true, "https://recomphub.com", now, clock));
        let generation = state.begin(AuthStatus::Checking);
        assert!(state.accept(
            generation,
            envelope(now),
            true,
            "https://recomphub.com",
            now - chrono::Duration::hours(1),
            clock + Duration::from_secs(30)
        ));
        assert!(
            state
                .require(
                    false,
                    now - chrono::Duration::hours(1),
                    clock + Duration::from_secs(60)
                )
                .is_err()
        );
    }

    #[test]
    fn cancelled_and_old_account_responses_cannot_unlock() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let clock = Instant::now();
        let mut state = SessionState::new();
        let old = state.begin(AuthStatus::WaitingForBrowser);
        state.lock(AuthStatus::SignedOut, None);
        let current = state.begin(AuthStatus::WaitingForBrowser);
        let mut account_b = envelope(now);
        account_b.validated_user.as_mut().unwrap().id = 8;
        assert!(state.accept(
            current,
            account_b,
            true,
            "https://recomphub.com",
            now,
            clock
        ));
        assert!(!state.accept(
            old,
            envelope(now),
            true,
            "https://recomphub.com",
            now,
            clock
        ));
        assert_eq!(state.view.user.as_ref().unwrap().id, 8);
        state.lock(AuthStatus::SignedOut, None);
        assert!(state.require(false, now, clock).is_err());
    }
}
