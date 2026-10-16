//! Google sign-in: the OpenID Connect authorization-code flow with PKCE.
//!
//! `authorize_url` sends the browser to Google. Google redirects back with a code,
//! and `fetch_account` exchanges it at the token endpoint. The ID token arrives
//! straight from that endpoint over TLS, authenticated with the client secret, so its
//! signature is not checked. OIDC Core 3.1.3.7 allows this. The claims are checked
//! instead: issuer, audience, authorized party, expiry, nonce and subject.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];
const USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";

/// What Google vouches for about the account that signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleAccount {
    /// Google's stable id for the account (`sub`). The identity key.
    pub subject: String,
    pub name: Option<String>,
    /// Set only when Google reports the email as verified. Never used as a key.
    pub email: Option<String>,
    pub picture: Option<String>,
}

/// The ID token claims RecompHub reads. Other claims are ignored.
#[derive(Debug, Deserialize)]
pub struct IdTokenClaims {
    pub iss: String,
    pub sub: String,
    pub aud: Audience,
    #[serde(default)]
    pub azp: Option<String>,
    pub exp: i64,
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: bool,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
}

/// The `aud` claim is a string or a list of strings.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn list(&self) -> Vec<&str> {
        match self {
            Audience::One(one) => vec![one.as_str()],
            Audience::Many(many) => many.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct TokenReply {
    id_token: Option<String>,
}

/// The S256 PKCE challenge for a verifier (RFC 7636 section 4.2).
pub fn code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The Google authorization URL. `select_account` forces the account picker. Linking
/// needs it, so a second Google account can be chosen instead of the one in use.
pub fn authorize_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    nonce: &str,
    challenge: &str,
    select_account: bool,
) -> Result<String> {
    let mut params = vec![
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", "openid email profile"),
        ("state", state),
        ("nonce", nonce),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ];
    if select_account {
        params.push(("prompt", "select_account"));
    }
    Ok(reqwest::Url::parse_with_params(AUTHORIZE_URL, &params)?.to_string())
}

/// Exchanges `code` for tokens and returns the account the ID token describes.
pub async fn fetch_account(
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
    expected_nonce: &str,
) -> Result<GoogleAccount> {
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(20))
        .build()
        .context("building the HTTP client")?;
    // The token endpoint takes a form body. This reqwest build has no form feature,
    // so the pairs are encoded with the URL serializer.
    let form = reqwest::Url::parse_with_params(
        "https://localhost/",
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri),
        ],
    )?;
    let body = form.query().unwrap_or_default().to_string();

    let reply: TokenReply = http
        .post(TOKEN_URL)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await
        .context("requesting the Google token")?
        .error_for_status()
        .context("Google rejected the token request")?
        .json()
        .await
        .context("reading the Google token reply")?;
    let id_token = reply
        .id_token
        .ok_or_else(|| anyhow!("Google returned no ID token"))?;

    let claims = decode_id_token(&id_token)?;
    check_id_token_claims(&claims, client_id, expected_nonce, Utc::now().timestamp())?;
    Ok(GoogleAccount {
        subject: claims.sub,
        name: claims.name,
        email: if claims.email_verified {
            claims.email
        } else {
            None
        },
        picture: claims.picture,
    })
}

/// Reads the claims from a JWT's payload segment. The signature is not checked here.
/// See the module docs for why that is acceptable on this path.
fn decode_id_token(token: &str) -> Result<IdTokenClaims> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| anyhow!("Google returned a malformed ID token"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .context("decoding the ID token payload")?;
    serde_json::from_slice(&bytes).context("reading the ID token claims")
}

/// Checks the claims that bind an ID token to this sign-in.
pub fn check_id_token_claims(
    claims: &IdTokenClaims,
    client_id: &str,
    expected_nonce: &str,
    now: i64,
) -> Result<()> {
    if !ISSUERS.contains(&claims.iss.as_str()) {
        bail!("ID token issuer is not Google");
    }
    let audiences = claims.aud.list();
    if !audiences.contains(&client_id) {
        bail!("ID token audience is not this client");
    }
    if audiences.len() > 1 && claims.azp.as_deref() != Some(client_id) {
        bail!("ID token has several audiences and is not issued to this client");
    }
    if claims.azp.as_deref().is_some_and(|azp| azp != client_id) {
        bail!("ID token is issued to another client");
    }
    if claims.exp <= now {
        bail!("ID token has expired");
    }
    if claims.nonce.as_deref() != Some(expected_nonce) {
        bail!("ID token nonce does not match this sign-in");
    }
    if claims.sub.is_empty() {
        bail!("ID token has no subject");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CLIENT: &str = "client-id.apps.googleusercontent.com";
    const NOW: i64 = 1_700_000_000;

    fn claims(value: serde_json::Value) -> IdTokenClaims {
        serde_json::from_value(value).expect("valid claims")
    }

    fn good() -> IdTokenClaims {
        claims(json!({
            "iss": "https://accounts.google.com",
            "sub": "1234",
            "aud": CLIENT,
            "exp": NOW + 300,
            "nonce": "n0",
        }))
    }

    #[test]
    fn accepts_a_matching_token() {
        assert!(check_id_token_claims(&good(), CLIENT, "n0", NOW).is_ok());
    }

    #[test]
    fn accepts_the_bare_issuer_form() {
        let mut c = good();
        c.iss = "accounts.google.com".to_string();
        assert!(check_id_token_claims(&c, CLIENT, "n0", NOW).is_ok());
    }

    #[test]
    fn rejects_another_issuer() {
        let mut c = good();
        c.iss = "https://login.example.com".to_string();
        assert!(check_id_token_claims(&c, CLIENT, "n0", NOW).is_err());
    }

    #[test]
    fn rejects_another_audience() {
        let c = claims(json!({
            "iss": "https://accounts.google.com", "sub": "1", "aud": "someone-else", "exp": NOW + 1, "nonce": "n0",
        }));
        assert!(check_id_token_claims(&c, CLIENT, "n0", NOW).is_err());
    }

    #[test]
    fn accepts_several_audiences_when_azp_is_this_client() {
        let c = claims(json!({
            "iss": "https://accounts.google.com", "sub": "1", "aud": [CLIENT, "other"],
            "azp": CLIENT, "exp": NOW + 1, "nonce": "n0",
        }));
        assert!(check_id_token_claims(&c, CLIENT, "n0", NOW).is_ok());
    }

    #[test]
    fn rejects_several_audiences_without_azp() {
        let c = claims(json!({
            "iss": "https://accounts.google.com", "sub": "1", "aud": [CLIENT, "other"],
            "exp": NOW + 1, "nonce": "n0",
        }));
        assert!(check_id_token_claims(&c, CLIENT, "n0", NOW).is_err());
    }

    #[test]
    fn rejects_an_azp_for_another_client() {
        let mut c = good();
        c.azp = Some("other-client".to_string());
        assert!(check_id_token_claims(&c, CLIENT, "n0", NOW).is_err());
    }

    #[test]
    fn rejects_an_expired_token() {
        let mut c = good();
        c.exp = NOW;
        assert!(check_id_token_claims(&c, CLIENT, "n0", NOW).is_err());
    }

    #[test]
    fn rejects_a_nonce_mismatch() {
        assert!(check_id_token_claims(&good(), CLIENT, "n1", NOW).is_err());
    }

    #[test]
    fn rejects_a_missing_nonce() {
        let mut c = good();
        c.nonce = None;
        assert!(check_id_token_claims(&c, CLIENT, "n0", NOW).is_err());
    }

    #[test]
    fn rejects_an_empty_subject() {
        let mut c = good();
        c.sub = String::new();
        assert!(check_id_token_claims(&c, CLIENT, "n0", NOW).is_err());
    }

    #[test]
    fn code_challenge_matches_the_rfc_7636_example() {
        assert_eq!(
            code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn decodes_the_payload_segment() {
        let payload = URL_SAFE_NO_PAD.encode(
            json!({"iss": "https://accounts.google.com", "sub": "42", "aud": CLIENT, "exp": NOW + 1})
                .to_string(),
        );
        let token = format!("eyJhbGciOiJSUzI1NiJ9.{payload}.sig");
        let decoded = decode_id_token(&token).expect("decodes");
        assert_eq!(decoded.sub, "42");
    }
}
