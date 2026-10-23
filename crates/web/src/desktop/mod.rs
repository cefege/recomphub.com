//! Native bootstrap and process-wide desktop context.
//!
//! The desktop renderer uses the same server functions as the website, but all requests are
//! pinned to one validated API origin and redirect following is disabled before any component can
//! make a request.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use dioxus::fullstack::{GLOBAL_REQUEST_CLIENT, set_server_url};
use recomphub_desktop::LibraryManager;
use reqwest::Url;

pub mod account;
pub mod auth;
pub mod external;
pub mod install_actions;
pub mod library;
pub mod session;
pub mod workspace;

pub struct NativeRuntime {
    pub origin: String,
    pub root: PathBuf,
    pub api: reqwest::Client,
    pub cookies: Arc<reqwest::cookie::Jar>,
    pub anonymous_api: reqwest::Client,
    pub account: account::Account,
    library: OnceLock<Arc<LibraryManager>>,
    library_open: tokio::sync::Mutex<()>,
}

static ORIGIN: OnceLock<String> = OnceLock::new();

static RUNTIME: OnceLock<Arc<NativeRuntime>> = OnceLock::new();

impl NativeRuntime {
    /// Opens the device-local library independently of account state.
    pub async fn library(self: &Arc<Self>) -> Result<Arc<LibraryManager>, String> {
        if let Some(library) = self.library.get() {
            return Ok(library.clone());
        }
        let _opening = self.library_open.lock().await;
        if let Some(library) = self.library.get() {
            return Ok(library.clone());
        }
        let root = self.root.clone();
        let library = tokio::task::spawn_blocking(move || LibraryManager::open(root))
            .await
            .map_err(|_| "The library initialization worker stopped unexpectedly.".to_string())?
            .map_err(|error| error.to_string())?;
        let library = Arc::new(library);
        let _ = self.library.set(library.clone());
        Ok(library)
    }
}

const DESKTOP_USER_AGENT: &str = concat!("RecompHub/", env!("CARGO_PKG_VERSION"));

pub fn initialize() -> Result<(), String> {
    let origin = configured_origin()?;
    let root = configured_library_root()?;
    let cookies = Arc::new(reqwest::cookie::Jar::default());
    let client = reqwest::Client::builder()
        .user_agent(DESKTOP_USER_AGENT)
        .cookie_provider(cookies.clone())
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|error| format!("building desktop API client: {error}"))?;
    let anonymous_api = reqwest::Client::builder()
        .user_agent(DESKTOP_USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|error| format!("building desktop browser-approval client: {error}"))?;
    GLOBAL_REQUEST_CLIENT
        .set(client.clone())
        .map_err(|_| "desktop API client was initialized more than once".to_string())?;
    let origin = ORIGIN.get_or_init(|| origin).as_str();
    set_server_url(origin);
    RUNTIME
        .set(Arc::new(NativeRuntime {
            origin: origin.to_string(),
            root,
            api: client,
            cookies,
            anonymous_api,
            account: account::Account::new(),
            library: OnceLock::new(),
            library_open: tokio::sync::Mutex::new(()),
        }))
        .map_err(|_| "desktop runtime was initialized more than once".to_string())
}

pub fn runtime() -> Arc<NativeRuntime> {
    RUNTIME
        .get()
        .expect("desktop runtime must be initialized before App launches")
        .clone()
}

fn configured_origin() -> Result<String, String> {
    let candidate = if cfg!(debug_assertions) {
        std::env::var("RECOMPHUB_API_URL").unwrap_or_else(|_| "https://recomphub.com".to_string())
    } else {
        "https://recomphub.com".to_string()
    };
    validate_api_origin(&candidate, cfg!(debug_assertions))
}

/// Validates one API origin without reading mutable process configuration. Release builds call
/// this with `allow_debug_loopback = false`, so HTTP is never accepted in customer artifacts.
pub fn validate_api_origin(value: &str, allow_debug_loopback: bool) -> Result<String, String> {
    let url = Url::parse(value).map_err(|_| "API origin must be an HTTP(S) origin".to_string())?;
    let loopback = url.host_str().is_some_and(|host| {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.host_str().is_none()
        || (url.scheme() == "http" && (!allow_debug_loopback || !loopback))
    {
        return Err(
            "API origin must be HTTPS; debug HTTP is loopback-only and origins cannot contain a path, credentials, query, or fragment".to_string(),
        );
    }
    Ok(url.origin().ascii_serialization())
}

fn configured_library_root() -> Result<PathBuf, String> {
    if cfg!(debug_assertions)
        && let Some(value) = std::env::var_os("RECOMPHUB_LIBRARY_ROOT")
    {
        return Ok(PathBuf::from(value));
    }
    LibraryManager::default_root().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::validate_api_origin;

    #[test]
    fn origin_validation_is_pure_and_rejects_non_origins() {
        assert_eq!(
            validate_api_origin("https://recomphub.com", false).unwrap(),
            "https://recomphub.com"
        );
        assert!(validate_api_origin("https://example.test/path", true).is_err());
        assert!(validate_api_origin("https://user@example.test", true).is_err());
        assert!(validate_api_origin("http://example.test", true).is_err());
    }

    #[test]
    fn debug_http_is_limited_to_loopback() {
        assert_eq!(
            validate_api_origin("http://127.0.0.1:8080", true).unwrap(),
            "http://127.0.0.1:8080"
        );
        assert!(validate_api_origin("http://[::1]:8080", true).is_ok());
        assert!(validate_api_origin("http://localhost:8080", false).is_err());
    }

    #[tokio::test]
    async fn library_opens_without_an_account() {
        let root =
            std::env::temp_dir().join(format!("recomphub-guest-{:016x}", rand::random::<u64>()));
        let client = reqwest::Client::new();
        let runtime = std::sync::Arc::new(super::NativeRuntime {
            origin: "https://recomphub.com".into(),
            root: root.clone(),
            api: client.clone(),
            anonymous_api: client,
            cookies: std::sync::Arc::new(reqwest::cookie::Jar::default()),
            account: super::account::Account::new(),
            library: std::sync::OnceLock::new(),
            library_open: tokio::sync::Mutex::new(()),
        });
        assert!(runtime.library().await.is_ok());
        assert!(
            root.exists(),
            "the guest device library should be initialized"
        );
    }
}
