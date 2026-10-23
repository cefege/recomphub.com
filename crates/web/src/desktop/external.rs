//! Safe handoff of external HTTP(S) destinations to the operating system browser.

use reqwest::Url;

/// Open a parsed HTTP(S) URL in the user's system browser.
pub fn open_external_http(url: &str) -> Result<(), String> {
    let parsed =
        Url::parse(url).map_err(|_| "This external link is not a valid URL.".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("Only external HTTP and HTTPS links can be opened.".to_string());
    }
    webbrowser::open(parsed.as_str())
        .map(|_| ())
        .map_err(|error| format!("Could not open the system browser: {error}"))
}
