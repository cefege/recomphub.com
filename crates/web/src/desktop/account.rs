//! Rust-side account controller and OS-credential I/O. No tokens enter UI state.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use dioxus::fullstack::RequestError;
use dioxus::prelude::ServerFnError;
use recomphub_desktop::credentials::{CredentialBackend, CredentialStore};
use recomphub_shared::SessionUser;
use tokio::sync::watch;

use super::NativeRuntime;
use super::session::{AuthStatus, AuthView, SessionEnvelope, SessionState};

pub struct Account {
    state: Mutex<SessionState>,
    updates: watch::Sender<AuthView>,
    credential_io: tokio::sync::Mutex<()>,
    credentials: Arc<dyn CredentialBackend>,
}

impl Default for Account {
    fn default() -> Self {
        Self::new()
    }
}

impl Account {
    pub fn new() -> Self {
        Self::with_credentials(Arc::new(CredentialStore::new()))
    }

    fn with_credentials(credentials: Arc<dyn CredentialBackend>) -> Self {
        let state = SessionState::new();
        let (updates, _) = watch::channel(state.view.clone());
        Self {
            state: Mutex::new(state),
            updates,
            credential_io: tokio::sync::Mutex::new(()),
            credentials,
        }
    }

    fn change<T>(&self, action: impl FnOnce(&mut SessionState) -> T) -> T {
        // Mutations in this lock contain no fallible I/O or user callbacks.
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let result = action(&mut state);
        self.updates.send_if_modified(|view| {
            if *view == state.view {
                return false;
            }
            *view = state.view.clone();
            true
        });
        result
    }

    pub fn subscribe(&self) -> watch::Receiver<AuthView> {
        self.updates.subscribe()
    }

    pub fn view(&self) -> AuthView {
        self.change(|state| {
            state.expire(Utc::now(), Instant::now());
            state.view.clone()
        })
    }

    /// Kept for account-scoped operations; device-local actions never call this.
    pub fn require(&self, online: bool) -> Result<(), String> {
        self.change(|state| state.require(online, Utc::now(), Instant::now()))
    }

    pub fn current(&self, generation: u64) -> bool {
        self.change(|state| state.generation == generation)
    }

    pub fn begin_browser(&self) -> u64 {
        self.change(|state| state.begin(AuthStatus::WaitingForBrowser))
    }

    pub fn fail(&self, generation: u64, message: String) {
        self.lock_current(generation, AuthStatus::Error, Some(message));
    }

    fn lock_current(
        &self,
        generation: u64,
        status: AuthStatus,
        message: Option<String>,
    ) -> Option<u64> {
        self.change(|state| {
            if state.generation != generation {
                return None;
            }
            state.lock(status, message);
            Some(state.generation)
        })
    }

    pub fn cancel(&self) {
        self.change(|state| state.lock(AuthStatus::SignedOut, None));
    }

    fn notice(&self, generation: u64, message: String) {
        self.change(|state| {
            if state.generation == generation {
                match &mut state.view.message {
                    Some(previous) if !previous.contains(&message) => {
                        previous.push('\n');
                        previous.push_str(&message);
                    }
                    None => state.view.message = Some(message),
                    _ => {}
                }
            }
        });
    }

    pub(super) async fn accept_online(
        &self,
        runtime: &Arc<NativeRuntime>,
        generation: u64,
        mut envelope: SessionEnvelope,
    ) -> Result<(), String> {
        let checked = check_identity().await;
        if !self.current(generation) {
            return Ok(());
        }
        match checked {
            IdentityCheck::Online(user) => envelope.validated_user = Some(user),
            IdentityCheck::Rejected => {
                clear_cookie(runtime);
                self.lock_current(
                    generation,
                    AuthStatus::SignedOut,
                    Some(
                        "The desktop session was rejected. Continue with Google to try again."
                            .into(),
                    ),
                );
                return Err("The desktop session was rejected.".into());
            }
            IdentityCheck::Unavailable | IdentityCheck::Invalid => {
                clear_cookie(runtime);
                return Err("Could not validate the new session. Retry sign-in when the server is reachable.".into());
            }
        }
        let accepted = self.change(|state| {
            state.accept(
                generation,
                envelope.clone(),
                true,
                &runtime.origin,
                Utc::now(),
                Instant::now(),
            )
        });
        if !accepted {
            return Ok(());
        }
        self.save(runtime, generation, envelope).await;
        Ok(())
    }

    async fn save(&self, runtime: &Arc<NativeRuntime>, generation: u64, envelope: SessionEnvelope) {
        let _io = self.credential_io.lock().await;
        let cleanup_complete = self.cleanup_locked(runtime).await;
        if !self.current(generation) {
            return;
        }
        let origin = runtime.origin.clone();
        let store = self.credentials.clone();
        let saved = tokio::task::spawn_blocking(move || {
            let bytes = serde_json::to_vec(&envelope)
                .map_err(|_| "Could not encode the remembered session.".to_string())?;
            store
                .save(&origin, &bytes)
                .map_err(|error| error.to_string())
        })
        .await;
        if !matches!(saved, Ok(Ok(()))) {
            let message = if cleanup_complete {
                "Not remembered on this device: the OS credential store refused to save this session. You can use it until you close the app or it expires."
            } else {
                "Not remembered on this device. The OS also refused to clear the previous account: persistent sign-out is not confirmed. Sign out and retry credential removal before closing the app."
            };
            self.notice(generation, message.into());
        } else if !cleanup_complete {
            self.change(|state| {
                if state.generation == generation {
                    state.view.message = None;
                }
            });
        }
    }

    async fn delete(&self, runtime: &Arc<NativeRuntime>) -> bool {
        let _io = self.credential_io.lock().await;
        self.cleanup_locked(runtime).await
    }

    /// Credential cleanup is an ordered store operation, not an auth response.
    /// A new sign-in generation must not discard an earlier explicit sign-out.
    async fn cleanup_locked(&self, runtime: &Arc<NativeRuntime>) -> bool {
        let epoch = self.change(|state| state.cleanup_pending.then_some(state.cleanup_generation));
        let Some(epoch) = epoch else {
            return true;
        };
        let origin = runtime.origin.clone();
        let store = self.credentials.clone();
        let result = tokio::task::spawn_blocking(move || {
            if store.delete(&origin).is_ok() {
                return Ok(());
            }
            let tombstone = serde_json::to_vec(&SessionEnvelope::signed_out(origin.clone()))
                .map_err(|_| "Could not encode the signed-out record.".to_string())?;
            store
                .save(&origin, &tombstone)
                .map_err(|error| error.to_string())
        })
        .await;
        let generation = self.change(|state| {
            if state.cleanup_generation == epoch {
                state.cleanup_pending = false;
            }
            state.generation
        });
        let removed = matches!(result, Ok(Ok(())));
        if !removed {
            self.notice(generation, "Signed out in this app, but the OS refused to remove or replace the saved credential. Persistent sign-out is not confirmed. Retry credential removal before closing the app.".into());
        }
        removed
    }
}

#[derive(Debug, PartialEq)]
enum IdentityCheck {
    Online(SessionUser),
    Rejected,
    Unavailable,
    Invalid,
}

async fn check_identity() -> IdentityCheck {
    match tokio::time::timeout(Duration::from_secs(10), crate::api::me()).await {
        Ok(result) => classify_identity(result),
        Err(_) => IdentityCheck::Unavailable,
    }
}

fn classify_identity(result: Result<Option<SessionUser>, ServerFnError>) -> IdentityCheck {
    match result {
        Ok(Some(user)) if user.id > 0 => IdentityCheck::Online(user),
        Ok(_) => IdentityCheck::Rejected,
        Err(error) => match error {
            ServerFnError::ServerError {
                code: 401 | 403, ..
            } => IdentityCheck::Rejected,
            ServerFnError::ServerError {
                code: 500..=599, ..
            } => IdentityCheck::Unavailable,
            ServerFnError::Request(error) => match error {
                RequestError::Status(_, 401 | 403) => IdentityCheck::Rejected,
                RequestError::Status(_, 500..=599)
                | RequestError::Timeout(_)
                | RequestError::Connect(_)
                | RequestError::Request(_)
                | RequestError::Body(_) => IdentityCheck::Unavailable,
                _ => IdentityCheck::Invalid,
            },
            ServerFnError::StreamError(_) => IdentityCheck::Unavailable,
            _ => IdentityCheck::Invalid,
        },
    }
}

pub(super) fn clear_cookie(runtime: &NativeRuntime) {
    if let Ok(url) = reqwest::Url::parse(&runtime.origin) {
        runtime
            .cookies
            .add_cookie_str("rh_session=; Path=/; Max-Age=0", &url);
    }
}

pub(super) fn install_cookie(
    runtime: &NativeRuntime,
    envelope: &SessionEnvelope,
) -> Result<(), String> {
    if !super::session::valid_token(&envelope.session_token) || envelope.expires_at <= Utc::now() {
        return Err("The server returned an invalid or expired desktop credential.".into());
    }
    let url = reqwest::Url::parse(&runtime.origin)
        .map_err(|_| "The configured API origin is invalid.".to_string())?;
    let secure = if url.scheme() == "https" {
        "; Secure"
    } else {
        ""
    };
    runtime.cookies.add_cookie_str(
        &format!(
            "rh_session={}; Path=/; HttpOnly; Expires={}{}",
            envelope.session_token,
            envelope.expires_at.format("%a, %d %b %Y %H:%M:%S GMT"),
            secure,
        ),
        &url,
    );
    Ok(())
}

/// First restore reads only the OS credential entry. Refresh uses the in-memory
/// envelope, so a failed persistence write does not turn a usable session into guest.
pub async fn restore(runtime: Arc<NativeRuntime>, cold_start: bool) {
    let (generation, cached, allow_stored_restore) = runtime.account.change(|state| {
        (
            state.begin(AuthStatus::Checking),
            state.envelope.clone(),
            state.allow_stored_restore,
        )
    });
    let envelope = if cached.is_some() || !cold_start || !allow_stored_restore {
        cached
    } else {
        let _io = runtime.account.credential_io.lock().await;
        if !runtime.account.current(generation) {
            return;
        }
        let origin = runtime.origin.clone();
        let store = runtime.account.credentials.clone();
        let read = tokio::task::spawn_blocking(move || store.load(&origin));
        match tokio::time::timeout(Duration::from_secs(10), read).await {
            Ok(Ok(Ok(Some(bytes)))) => serde_json::from_slice::<SessionEnvelope>(&bytes).ok(),
            Ok(Ok(Ok(None))) => None,
            _ => {
                runtime.account.fail(generation, "Could not read the OS credential store. Retry the session check or continue with Google; no plaintext fallback is used.".into());
                return;
            }
        }
    };
    if !runtime.account.current(generation) {
        return;
    }
    let Some(mut envelope) = envelope else {
        runtime
            .account
            .lock_current(generation, AuthStatus::SignedOut, None);
        return;
    };
    if !envelope.usable(&runtime.origin, Utc::now()) {
        let expired = envelope.validated_user.is_some() && envelope.expires_at <= Utc::now();
        let status = if expired {
            AuthStatus::Expired
        } else {
            AuthStatus::SignedOut
        };
        clear_cookie(&runtime);
        if runtime
            .account
            .lock_current(generation, status, None)
            .is_some()
        {
            runtime.account.delete(&runtime).await;
        }
        return;
    }
    if let Err(message) = install_cookie(&runtime, &envelope) {
        runtime.account.fail(generation, message);
        return;
    }
    let online = match check_identity().await {
        IdentityCheck::Online(user) => {
            envelope.validated_user = Some(user);
            true
        }
        IdentityCheck::Unavailable => false,
        IdentityCheck::Rejected => {
            if runtime.account.current(generation) {
                clear_cookie(&runtime);
                if runtime
                    .account
                    .lock_current(
                        generation,
                        AuthStatus::SignedOut,
                        Some("Your session was rejected. Sign in again.".into()),
                    )
                    .is_some()
                {
                    runtime.account.delete(&runtime).await;
                }
            }
            return;
        }
        IdentityCheck::Invalid => {
            runtime.account.fail(generation, "The server returned an invalid identity response. The library remains locked; retry the session check.".into());
            return;
        }
    };
    if runtime.account.change(|state| {
        state.accept(
            generation,
            envelope.clone(),
            online,
            &runtime.origin,
            Utc::now(),
            Instant::now(),
        )
    }) && online
    {
        runtime.account.save(&runtime, generation, envelope).await;
    }
}

pub async fn sign_out(runtime: Arc<NativeRuntime>) {
    let token = runtime.account.change(|state| {
        let token = state
            .envelope
            .as_ref()
            .map(|envelope| envelope.session_token.clone());
        state.lock(AuthStatus::SignedOut, None);
        state.allow_stored_restore = false;
        (state.generation, token)
    });
    clear_cookie(&runtime);
    // Independent request cookie: sign-out must not race a later sign-in on the shared jar.
    let revoke = async {
        if let Some(token) = token.1 {
            runtime
                .anonymous_api
                .post(format!("{}/api/v1/desktop/auth/logout", runtime.origin))
                .header(reqwest::header::COOKIE, format!("rh_session={token}"))
                .send()
                .await
                .map_err(|_| ())?
                .error_for_status()
                .map_err(|_| ())?;
        }
        Ok::<_, ()>(())
    };
    let (_, revoked) = tokio::join!(runtime.account.delete(&runtime), revoke);
    if revoked.is_err() {
        runtime.account.notice(token.0, "Signed out locally. The server could not confirm remote revocation; the old session retains its original server expiry.".into());
    }
}

pub async fn remove_saved_credential(runtime: Arc<NativeRuntime>) {
    let generation = runtime.account.change(|state| {
        if !matches!(
            state.view.status,
            AuthStatus::SignedOut | AuthStatus::Expired | AuthStatus::Error
        ) {
            return None;
        }
        state.cleanup_pending = true;
        state.cleanup_generation = state.generation;
        Some(state.generation)
    });
    if let Some(generation) = generation
        && runtime.account.delete(&runtime).await
    {
        runtime.account.notice(
            generation,
            "Saved credential removed from this device.".into(),
        );
    }
}

pub async fn enforce_expiry(runtime: Arc<NativeRuntime>) {
    let pending = runtime.account.change(|state| {
        state.expire(Utc::now(), Instant::now());
        state.cleanup_pending
    });
    if pending {
        runtime.account.delete(&runtime).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reachable_guest_or_ban_is_never_an_offline_fallback() {
        assert_eq!(classify_identity(Ok(None)), IdentityCheck::Rejected);
        for code in [401, 403] {
            let error = ServerFnError::ServerError {
                code,
                message: "fixture".into(),
                details: None,
            };
            assert_eq!(classify_identity(Err(error)), IdentityCheck::Rejected);
            let error = ServerFnError::Request(RequestError::Status("fixture".into(), code));
            assert_eq!(classify_identity(Err(error)), IdentityCheck::Rejected);
        }
    }

    #[test]
    fn server_failure_is_unavailable_but_invalid_payload_is_locked() {
        assert_eq!(
            classify_identity(Err(ServerFnError::new("fixture"))),
            IdentityCheck::Unavailable
        );
        assert_eq!(
            classify_identity(Err(ServerFnError::Request(RequestError::Timeout(
                "fixture".into()
            )))),
            IdentityCheck::Unavailable
        );
        assert_eq!(
            classify_identity(Err(ServerFnError::Deserialization("fixture".into()))),
            IdentityCheck::Invalid
        );
        assert_eq!(
            classify_identity(Err(ServerFnError::Request(RequestError::Builder(
                "fixture".into()
            )))),
            IdentityCheck::Invalid
        );
    }

    struct FaultStore {
        saved: Mutex<Option<Vec<u8>>>,
        refuse_write: bool,
    }

    impl CredentialBackend for FaultStore {
        fn load(
            &self,
            _: &str,
        ) -> Result<Option<Vec<u8>>, recomphub_desktop::credentials::CredentialError> {
            Ok(self.saved.lock().unwrap().clone())
        }
        fn save(
            &self,
            _: &str,
            bytes: &[u8],
        ) -> Result<(), recomphub_desktop::credentials::CredentialError> {
            if self.refuse_write {
                return Err(
                    recomphub_desktop::credentials::CredentialError::Unavailable(
                        "fixture OS refusal".into(),
                    ),
                );
            }
            *self.saved.lock().unwrap() = Some(bytes.to_vec());
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), recomphub_desktop::credentials::CredentialError> {
            Err(
                recomphub_desktop::credentials::CredentialError::Unavailable(
                    "fixture deletion refusal".into(),
                ),
            )
        }
    }

    fn fixture_runtime(store: Arc<dyn CredentialBackend>) -> Arc<NativeRuntime> {
        let client = reqwest::Client::new();
        Arc::new(NativeRuntime {
            origin: "https://recomphub.com".into(),
            root: std::env::temp_dir().join(format!(
                "recomphub-auth-fixture-{:016x}",
                rand::random::<u64>()
            )),
            api: client.clone(),
            anonymous_api: client,
            cookies: Arc::new(reqwest::cookie::Jar::default()),
            account: Account::with_credentials(store),
            library: std::sync::OnceLock::new(),
            library_open: tokio::sync::Mutex::new(()),
        })
    }

    fn fixture_session() -> SessionEnvelope {
        use base64::Engine;
        SessionEnvelope {
            version: 1,
            origin: "https://recomphub.com".into(),
            session_token: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([1; 32]),
            expires_at: Utc::now() + chrono::Duration::minutes(5),
            validated_user: Some(SessionUser {
                id: 1,
                display_name: "Fixture".into(),
                github_login: None,
                role: "member".into(),
                avatar: None,
            }),
        }
    }

    #[tokio::test]
    async fn refused_credential_write_keeps_validated_session_memory_only() {
        let store = Arc::new(FaultStore {
            saved: Mutex::new(None),
            refuse_write: true,
        });
        let runtime = fixture_runtime(store);
        let session = fixture_session();
        runtime.account.change(|state| {
            assert!(state.accept(
                0,
                session.clone(),
                true,
                &runtime.origin,
                Utc::now(),
                Instant::now()
            ));
        });
        runtime.account.save(&runtime, 0, session).await;
        assert!(runtime.account.require(false).is_ok());
        assert!(
            runtime
                .account
                .view()
                .message
                .unwrap()
                .contains("Not remembered on this device")
        );
        assert!(
            !runtime.root.exists(),
            "account persistence must not create library storage"
        );
    }

    #[tokio::test]
    async fn refused_delete_uses_a_locked_tombstone_or_reports_persistence_failure() {
        for refuse_write in [false, true] {
            let store = Arc::new(FaultStore {
                saved: Mutex::new(Some(serde_json::to_vec(&fixture_session()).unwrap())),
                refuse_write,
            });
            let runtime = fixture_runtime(store.clone());
            runtime
                .account
                .change(|state| state.lock(AuthStatus::SignedOut, None));
            runtime.account.delete(&runtime).await;
            assert!(runtime.account.require(false).is_err());
            assert!(!runtime.account.change(|state| state.allow_stored_restore));
            if refuse_write {
                assert!(
                    runtime
                        .account
                        .view()
                        .message
                        .unwrap()
                        .contains("Persistent sign-out is not confirmed")
                );
            } else {
                let bytes = store.load(&runtime.origin).unwrap().unwrap();
                let tombstone: SessionEnvelope = serde_json::from_slice(&bytes).unwrap();
                assert!(!tombstone.usable(&runtime.origin, Utc::now()));
                assert!(tombstone.validated_user.is_none());
                assert!(tombstone.session_token.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn a_failed_new_browser_attempt_cannot_cancel_earlier_persistent_signout() {
        let store = Arc::new(FaultStore {
            saved: Mutex::new(Some(serde_json::to_vec(&fixture_session()).unwrap())),
            refuse_write: false,
        });
        let runtime = fixture_runtime(store.clone());
        runtime
            .account
            .change(|state| state.lock(AuthStatus::SignedOut, None));
        let generation = runtime.account.begin_browser();
        runtime
            .account
            .fail(generation, "new browser attempt failed".into());
        runtime.account.delete(&runtime).await;
        let bytes = store.load(&runtime.origin).unwrap().unwrap();
        let remembered: SessionEnvelope = serde_json::from_slice(&bytes).unwrap();
        assert!(!remembered.usable(&runtime.origin, Utc::now()));
        assert!(runtime.account.require(false).is_err());
    }

    #[tokio::test]
    async fn stale_credential_removal_retry_preserves_a_new_signed_in_session() {
        let session = fixture_session();
        let store = Arc::new(FaultStore {
            saved: Mutex::new(Some(serde_json::to_vec(&session).unwrap())),
            refuse_write: false,
        });
        let runtime = fixture_runtime(store.clone());
        runtime.account.change(|state| {
            assert!(state.accept(
                0,
                session,
                true,
                &runtime.origin,
                Utc::now(),
                Instant::now()
            ));
        });
        remove_saved_credential(runtime.clone()).await;
        let bytes = store.load(&runtime.origin).unwrap().unwrap();
        let remembered: SessionEnvelope = serde_json::from_slice(&bytes).unwrap();
        assert!(remembered.usable(&runtime.origin, Utc::now()));
        assert!(runtime.account.require(true).is_ok());
    }
}
