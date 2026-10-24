//! Process configuration, read from the environment once at startup.

use std::{path::PathBuf, sync::OnceLock};

use anyhow::{Context, Result};

/// Settings every server-side module reads. Secrets stay here and are never rendered.
#[derive(Debug)]
pub struct Config {
    pub database_url: String,
    /// Public origin without a trailing slash, for example `https://recomphub.com`.
    pub site_url: String,
    pub media_dir: PathBuf,
    /// Google OAuth client credentials for sign-in. Without them, sign-in is off.
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,
    pub github_token: Option<String>,
    /// Lowercased emails that receive the `admin` role on sign-in. Only verified emails count.
    pub admin_emails: Vec<String>,
    /// Amazon Associates tag for affiliate links. Without it, no affiliate shelf is shown.
    pub amazon_tag: Option<String>,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

impl Config {
    /// Reads the configuration from environment variables. `DATABASE_URL` is required.
    pub fn from_env() -> Result<Self> {
        let database_url = env_var("DATABASE_URL").context("DATABASE_URL must be set")?;
        let site_url = env_var("SITE_URL")
            .unwrap_or_else(|| "http://localhost:8080".to_string())
            .trim_end_matches('/')
            .to_string();
        let media_dir = env_var("MEDIA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./data/media"));
        let admin_emails = split_list(env_var("ADMIN_EMAILS"));
        Ok(Self {
            database_url,
            site_url,
            media_dir,
            google_client_id: env_var("GOOGLE_OAUTH_CLIENT_ID"),
            google_client_secret: env_var("GOOGLE_OAUTH_CLIENT_SECRET"),
            github_token: env_var("GITHUB_TOKEN"),
            admin_emails,
            amazon_tag: env_var("AMAZON_ASSOCIATE_TAG"),
        })
    }
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Splits a comma-separated list into trimmed, lowercased, non-empty entries.
fn split_list(raw: Option<String>) -> Vec<String> {
    raw.unwrap_or_default()
        .split(',')
        .map(|item| item.trim().to_ascii_lowercase())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Installs the process configuration. Later calls keep the first value.
pub fn install(config: Config) -> &'static Config {
    CONFIG.get_or_init(|| config)
}

/// The configuration installed by [`install`]. Panics only if called before startup installed it.
pub fn get() -> &'static Config {
    CONFIG
        .get()
        .expect("config::install runs during startup, before any request is served")
}
