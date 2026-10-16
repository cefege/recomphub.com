//! Sign-in through Cloudflare Access. An Access application protects only `/auth/cloudflare`.
//! Cloudflare signs the user in and forwards the request with the `Cf-Access-Jwt-Assertion`
//! header. This module verifies that RS256 JWT against the team's JWKS, finds or creates the
//! user keyed by `sub`, and starts the usual local session.
//!
//! References:
//! - Validating the JWT: https://developers.cloudflare.com/cloudflare-one/identity/authorization-cookie/validating-json/
//! - Application token claims: https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/application-token/
//! - Session management and logout: https://developers.cloudflare.com/cloudflare-one/access-controls/access-settings/session-management/

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::Utc;
use dioxus::server::axum::extract::Query;
use dioxus::server::axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use dioxus::server::axum::response::{IntoResponse, Response};
use recomphub_db::identities::{self, IdentityProfile, Provider};
use recomphub_db::users_sessions;
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
use serde::Deserialize;
use tokio::sync::Mutex;

use super::{
    config::{self, Config},
    session, state,
};

/// The header Cloudflare Access adds to every request it lets through.
pub const ASSERTION_HEADER: &str = "cf-access-jwt-assertion";
const CERTS_PATH: &str = "/cdn-cgi/access/certs";
const IDENTITY_PATH: &str = "/cdn-cgi/access/get-identity";
/// Keys are refetched after this long even when every `kid` is known.
const KEY_MAX_AGE: Duration = Duration::from_secs(3600);
/// Minimum gap between JWKS fetches triggered by an unknown `kid`.
const REFETCH_COOLDOWN: Duration = Duration::from_secs(10);
const USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";

/// Verifies Access JWTs for one team and application audience.
pub struct AccessVerifier {
    issuer: String,
    audience: String,
    certs_url: String,
    cooldown: Duration,
    http: reqwest::Client,
    keys: Mutex<KeyCache>,
}

#[derive(Default)]
struct KeyCache {
    /// `kid` to DER-encoded RSA public key (PKCS#1 `RSAPublicKey`).
    keys: HashMap<String, Vec<u8>>,
    fetched_at: Option<Instant>,
}

/// The claims this site relies on. `aud` and `iss` are checked in [`check_claims`].
#[derive(Debug, Deserialize)]
pub struct AccessClaims {
    pub iss: String,
    pub aud: Audience,
    pub exp: i64,
    #[serde(default)]
    pub nbf: Option<i64>,
    pub sub: String,
    pub email: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn contains(&self, audience: &str) -> bool {
        match self {
            Audience::One(one) => one == audience,
            Audience::Many(many) => many.iter().any(|item| item == audience),
        }
    }
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: Option<String>,
}

impl AccessVerifier {
    /// A verifier for `issuer` (`https://<team>.cloudflareaccess.com`) and `audience` (the AUD tag).
    pub fn new(issuer: &str, audience: &str) -> Result<Self> {
        Self::with_certs_url(
            issuer,
            audience,
            &format!("{issuer}{CERTS_PATH}"),
            REFETCH_COOLDOWN,
        )
    }

    /// Like [`AccessVerifier::new`] with an explicit JWKS URL and refetch cooldown.
    pub fn with_certs_url(
        issuer: &str,
        audience: &str,
        certs_url: &str,
        cooldown: Duration,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()
            .context("building the Access HTTP client")?;
        Ok(Self {
            issuer: issuer.to_string(),
            audience: audience.to_string(),
            certs_url: certs_url.to_string(),
            cooldown,
            http,
            keys: Mutex::new(KeyCache::default()),
        })
    }

    /// Verifies signature, algorithm, `iss`, `aud`, `exp`, `nbf`, `sub` and `email`.
    /// Returns the claims only when every check passes.
    pub async fn verify(&self, token: &str) -> Result<AccessClaims> {
        let mut parts = token.split('.');
        let (Some(head), Some(payload), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            bail!("token is not a three-part JWT");
        };
        let header: Header = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(head)?)
            .context("reading the token header")?;
        if header.alg != "RS256" {
            bail!("token algorithm {} is not RS256", header.alg);
        }
        let kid = header.kid.ok_or_else(|| anyhow!("token has no kid"))?;
        let der = self.key_for(&kid).await?;

        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .context("decoding the token signature")?;
        UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, der)
            .verify(format!("{head}.{payload}").as_bytes(), &signature)
            .map_err(|_| anyhow!("token signature does not verify"))?;

        let claims: AccessClaims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload)?)
            .context("reading the token claims")?;
        check_claims(
            &claims,
            &self.issuer,
            &self.audience,
            Utc::now().timestamp(),
        )?;
        Ok(claims)
    }

    /// The key for `kid`. Refetches the JWKS when the key is unknown (rotation), when the
    /// cache is older than [`KEY_MAX_AGE`], or on first use. Unknown-kid refetches honour the cooldown.
    async fn key_for(&self, kid: &str) -> Result<Vec<u8>> {
        let mut cache = self.keys.lock().await;
        let due = match cache.fetched_at {
            None => true,
            Some(at) => {
                at.elapsed() >= KEY_MAX_AGE
                    || (!cache.keys.contains_key(kid) && at.elapsed() >= self.cooldown)
            }
        };
        if due {
            cache.keys = self.fetch_keys().await?;
            cache.fetched_at = Some(Instant::now());
        }
        cache
            .keys
            .get(kid)
            .cloned()
            .ok_or_else(|| anyhow!("no Access signing key with kid {kid}"))
    }

    async fn fetch_keys(&self) -> Result<HashMap<String, Vec<u8>>> {
        #[derive(Deserialize)]
        struct Jwks {
            keys: Vec<Jwk>,
        }
        #[derive(Deserialize)]
        struct Jwk {
            kid: Option<String>,
            kty: String,
            n: Option<String>,
            e: Option<String>,
        }

        let jwks: Jwks = self
            .http
            .get(&self.certs_url)
            .send()
            .await
            .context("requesting the Access certs")?
            .error_for_status()
            .context("Access refused the certs request")?
            .json()
            .await
            .context("reading the Access certs")?;

        let mut keys = HashMap::new();
        for key in jwks.keys {
            if key.kty != "RSA" {
                continue;
            }
            let (Some(kid), Some(n), Some(e)) = (key.kid, key.n, key.e) else {
                continue;
            };
            let n = URL_SAFE_NO_PAD
                .decode(n)
                .context("decoding a key modulus")?;
            let e = URL_SAFE_NO_PAD
                .decode(e)
                .context("decoding a key exponent")?;
            keys.insert(kid, rsa_public_key_der(&n, &e));
        }
        if keys.is_empty() {
            bail!("Access published no RSA keys");
        }
        Ok(keys)
    }
}

/// Checks the claims that bind a token to this application and to a real user.
pub fn check_claims(claims: &AccessClaims, issuer: &str, audience: &str, now: i64) -> Result<()> {
    if claims.iss != issuer {
        bail!("token issuer is not this Access team");
    }
    if !claims.aud.contains(audience) {
        bail!("token audience is not this Access application");
    }
    if claims.exp <= now {
        bail!("token has expired");
    }
    if claims.nbf.is_some_and(|nbf| nbf > now) {
        bail!("token is not valid yet");
    }
    if claims.sub.trim().is_empty() {
        bail!("token has no subject");
    }
    if claims.email.trim().is_empty() {
        bail!("token has no email");
    }
    Ok(())
}

/// DER `RSAPublicKey ::= SEQUENCE { modulus INTEGER, publicExponent INTEGER }`, as ring expects.
fn rsa_public_key_der(n: &[u8], e: &[u8]) -> Vec<u8> {
    let body = [der_uint(n), der_uint(e)].concat();
    let mut out = vec![0x30];
    out.extend(der_len(body.len()));
    out.extend(body);
    out
}

fn der_uint(bytes: &[u8]) -> Vec<u8> {
    let start = bytes
        .iter()
        .position(|&b| b != 0)
        .unwrap_or(bytes.len().saturating_sub(1));
    let trimmed = &bytes[start..];
    let mut body = Vec::with_capacity(trimmed.len() + 1);
    if trimmed.first().is_some_and(|&b| b & 0x80 != 0) {
        body.push(0);
    }
    body.extend_from_slice(trimmed);
    let mut out = vec![0x02];
    out.extend(der_len(body.len()));
    out.extend(body);
    out
}

fn der_len(len: usize) -> Vec<u8> {
    if len < 0x80 {
        return vec![len as u8];
    }
    let bytes = len.to_be_bytes();
    let first = bytes
        .iter()
        .position(|&b| b != 0)
        .unwrap_or(bytes.len() - 1);
    let mut out = vec![0x80 | (bytes.len() - first) as u8];
    out.extend_from_slice(&bytes[first..]);
    out
}

#[derive(Deserialize)]
pub struct LoginQuery {
    next: Option<String>,
}

/// `GET /auth/cloudflare?next=`: the only route behind the Access application. Signs the
/// verified user in and redirects to `next`.
pub async fn login(headers: HeaderMap, Query(query): Query<LoginQuery>) -> Response {
    let cfg = config::get();
    let (issuer, audience) = match settings(cfg) {
        Ok(pair) => pair,
        Err(message) => return plain(StatusCode::SERVICE_UNAVAILABLE, message),
    };
    let Some(token) = headers
        .get(ASSERTION_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|token| !token.is_empty())
    else {
        return plain(
            StatusCode::UNAUTHORIZED,
            "sign-in must come through Cloudflare Access",
        );
    };
    let verifier = match VERIFIER
        .get_or_init(|| AccessVerifier::new(&issuer, &audience).map_err(|err| format!("{err:#}")))
    {
        Ok(verifier) => verifier,
        Err(err) => {
            tracing::error!(error = %err, "building the Access verifier failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
        }
    };
    let claims = match verifier.verify(token).await {
        Ok(claims) => claims,
        Err(err) => {
            tracing::warn!(error = %err, "rejected a Cloudflare Access token");
            return plain(
                StatusCode::UNAUTHORIZED,
                "could not verify the sign-in; try again",
            );
        }
    };

    let email = claims.email.trim().to_string();
    let name = fetch_name(&verifier.http, &issuer, token).await;
    let profile = IdentityProfile {
        provider: Provider::Cloudflare,
        subject: claims.sub.clone(),
        login: None,
        email: Some(email.clone()),
        name,
        avatar_media_id: None,
    };
    let is_admin = is_admin_email(&email, &cfg.admin_emails);

    let pool = match state::pool() {
        Ok(pool) => pool,
        Err(err) => {
            tracing::error!(error = %err, "database pool unavailable during sign-in");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
        }
    };
    let user = match identities::sign_in(pool, &profile, is_admin).await {
        Ok(user) if user.banned => return plain(StatusCode::FORBIDDEN, "this account is banned"),
        Ok(user) => user,
        Err(err) => {
            tracing::error!(error = ?err, "saving the sign-in failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
        }
    };

    let token = session::new_token();
    let expires_at = Utc::now() + chrono::Duration::days(session::SESSION_DAYS);
    if let Err(err) = users_sessions::create_session(pool, &token, user.id, expires_at).await {
        tracing::error!(error = ?err, "creating the session failed");
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
    }

    let max_age = session::SESSION_DAYS * 24 * 60 * 60;
    let cookie = format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        session::SESSION_COOKIE,
        secure_attr(cfg)
    );
    let next = safe_next(query.next.as_deref());
    redirect(&next, &[cookie])
}

/// The team issuer and application audience, or a 503 message naming the missing variable.
fn settings(cfg: &Config) -> std::result::Result<(String, String), String> {
    match (cfg.cf_team_domain.as_deref(), cfg.cf_access_aud.as_deref()) {
        (Some(team), Some(aud)) => Ok((format!("https://{team}"), aud.to_string())),
        (None, _) => Err("Sign-in is not set up yet: CF_ACCESS_TEAM_DOMAIN is not set".to_string()),
        (Some(_), None) => Err("Sign-in is not set up yet: CF_ACCESS_AUD is not set".to_string()),
    }
}

static VERIFIER: OnceLock<std::result::Result<AccessVerifier, String>> = OnceLock::new();

/// Admin role for a verified email listed in `ADMIN_EMAILS`. Lists are lowercased at startup.
pub fn is_admin_email(email: &str, admins: &[String]) -> bool {
    admins.contains(&email.trim().to_ascii_lowercase())
}

/// The display name from `get-identity`, when Cloudflare has one. Best effort: any failure
/// returns None and the email's local part is used instead.
async fn fetch_name(http: &reqwest::Client, issuer: &str, token: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Identity {
        name: Option<String>,
    }
    let reply: Identity = http
        .get(format!("{issuer}{IDENTITY_PATH}"))
        .header(header::COOKIE, format!("CF_Authorization={token}"))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    reply
        .name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

/// Redirects with the given `Set-Cookie` values. Cookie text that cannot be sent is a 500.
pub(super) fn redirect(location: &str, cookies: &[String]) -> Response {
    let mut response = (
        StatusCode::FOUND,
        [(header::LOCATION, location.to_string())],
    )
        .into_response();
    for cookie in cookies {
        match HeaderValue::from_str(cookie) {
            Ok(value) => {
                response.headers_mut().append(header::SET_COOKIE, value);
            }
            Err(err) => {
                tracing::error!(error = %err, "refusing to send an invalid cookie");
                return plain(StatusCode::INTERNAL_SERVER_ERROR, "sign-in failed");
            }
        }
    }
    response
}

pub(super) fn plain(status: StatusCode, message: impl Into<String>) -> Response {
    (status, message.into()).into_response()
}

/// Accepts only same-origin paths that are safe inside a `Location` header and a cookie.
pub fn safe_next(raw: Option<&str>) -> String {
    match raw {
        Some(next)
            if next.starts_with('/')
                && !next.starts_with("//")
                && next.bytes().all(|b| {
                    b.is_ascii_graphic() && !matches!(b, b';' | b',' | b'"' | b'\\' | b'|')
                }) =>
        {
            next.to_string()
        }
        _ => "/".to_string(),
    }
}

/// `POST /auth/logout`: deletes the local session and expires its cookie. When Access is
/// configured, the browser then goes to the application's Access logout, which clears the
/// `CF_Authorization` cookie so the next visit signs in again.
pub async fn logout(headers: HeaderMap) -> Response {
    let cfg = config::get();
    if let Some(token) = session::token_from_headers(&headers) {
        let pool = match state::pool() {
            Ok(pool) => pool,
            Err(err) => {
                tracing::error!(error = %err, "database pool unavailable during logout");
                return plain(StatusCode::INTERNAL_SERVER_ERROR, "logout failed");
            }
        };
        if let Err(err) = users_sessions::delete_session(pool, &token).await {
            tracing::error!(error = ?err, "deleting the session failed");
            return plain(StatusCode::INTERNAL_SERVER_ERROR, "logout failed");
        }
    }
    let clear = format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        session::SESSION_COOKIE,
        secure_attr(cfg)
    );
    let location = if cfg.cf_team_domain.is_some() {
        format!("{}/cdn-cgi/access/logout", cfg.site_url)
    } else {
        "/".to_string()
    };
    redirect(&location, &[clear])
}

/// `Secure` on the session cookie whenever the site is served over HTTPS.
pub(super) fn secure_attr(cfg: &Config) -> &'static str {
    if cfg.site_url.starts_with("https://") {
        "; Secure"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::LazyLock;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rsa::RsaPrivateKey;
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::traits::PublicKeyParts;
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    const TEAM: &str = "https://team.cloudflareaccess.com";
    const AUD: &str = "app-aud-tag";

    /// One RSA key shared by the tests. Generating 2048-bit keys is slow in debug builds.
    fn key() -> &'static RsaPrivateKey {
        static KEY: LazyLock<RsaPrivateKey> = LazyLock::new(|| {
            RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).expect("generates a key")
        });
        &KEY
    }

    fn jwks_entry(kid: &str, key: &RsaPrivateKey) -> Value {
        let public = key.to_public_key();
        json!({
            "kid": kid,
            "kty": "RSA",
            "alg": "RS256",
            "use": "sig",
            "n": URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
            "e": URL_SAFE_NO_PAD.encode(public.e().to_bytes_be()),
        })
    }

    fn jwks(entries: &[(&str, &RsaPrivateKey)]) -> String {
        let keys: Vec<Value> = entries
            .iter()
            .map(|(kid, key)| jwks_entry(kid, key))
            .collect();
        json!({ "keys": keys, "public_certs": [], "public_cert": {} }).to_string()
    }

    fn sign(key: &RsaPrivateKey, kid: &str, claims: Value) -> String {
        let head =
            URL_SAFE_NO_PAD.encode(json!({"alg": "RS256", "typ": "JWT", "kid": kid}).to_string());
        let body = URL_SAFE_NO_PAD.encode(claims.to_string());
        let input = format!("{head}.{body}");
        let rng = ring::rand::SystemRandom::new();
        let pair = ring::signature::RsaKeyPair::from_pkcs8(
            key.to_pkcs8_der().expect("encodes the key").as_bytes(),
        )
        .expect("loads the key");
        let mut signature = vec![0; pair.public().modulus_len()];
        pair.sign(
            &ring::signature::RSA_PKCS1_SHA256,
            &rng,
            input.as_bytes(),
            &mut signature,
        )
        .expect("signs");
        format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature))
    }

    fn now() -> i64 {
        Utc::now().timestamp()
    }

    fn good_claims() -> Value {
        json!({
            "iss": TEAM,
            "aud": [AUD],
            "sub": "user-sub-1",
            "email": "member@example.com",
            "iat": now(),
            "nbf": now() - 10,
            "exp": now() + 300,
        })
    }

    /// A JWKS server on localhost. Serves the current body and counts requests.
    struct Certs {
        url: String,
        body: Arc<std::sync::Mutex<String>>,
        hits: Arc<AtomicUsize>,
    }

    async fn serve_certs(initial: String) -> Certs {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds");
        let addr = listener.local_addr().expect("address");
        let body = Arc::new(std::sync::Mutex::new(initial));
        let hits = Arc::new(AtomicUsize::new(0));
        let (served, counted) = (body.clone(), hits.clone());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let payload = served.lock().expect("lock").clone();
                counted.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                        payload.len()
                    );
                    let _ = socket.write_all(reply.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Certs {
            url: format!("http://{addr}{CERTS_PATH}"),
            body,
            hits,
        }
    }

    fn verifier(certs: &Certs) -> AccessVerifier {
        AccessVerifier::with_certs_url(TEAM, AUD, &certs.url, Duration::ZERO).expect("builds")
    }

    #[tokio::test]
    async fn accepts_a_valid_token() {
        let certs = serve_certs(jwks(&[("k1", key())])).await;
        let token = sign(key(), "k1", good_claims());
        let claims = verifier(&certs).verify(&token).await.expect("verifies");
        assert_eq!(claims.sub, "user-sub-1");
        assert_eq!(claims.email, "member@example.com");
    }

    #[tokio::test]
    async fn rejects_another_audience() {
        let certs = serve_certs(jwks(&[("k1", key())])).await;
        let mut claims = good_claims();
        claims["aud"] = json!(["someone-else"]);
        let token = sign(key(), "k1", claims);
        assert!(verifier(&certs).verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn rejects_another_issuer() {
        let certs = serve_certs(jwks(&[("k1", key())])).await;
        let mut claims = good_claims();
        claims["iss"] = json!("https://other.cloudflareaccess.com");
        let token = sign(key(), "k1", claims);
        assert!(verifier(&certs).verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn rejects_an_expired_token() {
        let certs = serve_certs(jwks(&[("k1", key())])).await;
        let mut claims = good_claims();
        claims["exp"] = json!(now() - 1);
        let token = sign(key(), "k1", claims);
        assert!(verifier(&certs).verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn rejects_a_signature_from_another_key() {
        let other = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).expect("key");
        let certs = serve_certs(jwks(&[("k1", key())])).await;
        let token = sign(&other, "k1", good_claims());
        assert!(verifier(&certs).verify(&token).await.is_err());
    }

    #[tokio::test]
    async fn refetches_the_certs_for_an_unknown_kid() {
        let certs = serve_certs(jwks(&[("old", key())])).await;
        let v = verifier(&certs);
        let first = sign(key(), "old", good_claims());
        v.verify(&first).await.expect("cached key verifies");
        assert_eq!(certs.hits.load(Ordering::SeqCst), 1);

        // Cloudflare rotates: the new kid appears in the JWKS.
        let rotated = RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).expect("key");
        *certs.body.lock().expect("lock") = jwks(&[("old", key()), ("new", &rotated)]);
        let second = sign(&rotated, "new", good_claims());
        v.verify(&second)
            .await
            .expect("rotated key verifies after a refetch");
        assert_eq!(certs.hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_unknown_kid_that_stays_unknown_is_rejected() {
        let certs = serve_certs(jwks(&[("k1", key())])).await;
        let token = sign(key(), "ghost", good_claims());
        assert!(verifier(&certs).verify(&token).await.is_err());
        assert_eq!(certs.hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn admin_grant_matches_the_verified_email_case_insensitively() {
        let admins = vec!["mateiasmihaiandrei@gmail.com".to_string()];
        assert!(is_admin_email("MateiasMihaiAndrei@Gmail.com", &admins));
        assert!(!is_admin_email("someone@example.com", &admins));
    }

    #[test]
    fn safe_next_keeps_same_origin_paths_and_rejects_the_rest() {
        assert_eq!(safe_next(Some("/account?x=1")), "/account?x=1");
        for raw in ["//evil", "/\\evil", "https://x", "/a b", "", "/a;b", "/a|b"] {
            assert_eq!(safe_next(Some(raw)), "/", "{raw:?} should fall back to /");
        }
        assert_eq!(safe_next(None), "/");
    }

    #[test]
    fn der_encoding_matches_the_rsa_public_key_layout() {
        // A modulus with the high bit set needs the 0x00 pad; the exponent 65537 is 0x010001.
        let der = rsa_public_key_der(&[0x80, 0x01], &[0x01, 0x00, 0x01]);
        assert_eq!(
            der,
            vec![
                0x30, 0x0a, 0x02, 0x03, 0x00, 0x80, 0x01, 0x02, 0x03, 0x01, 0x00, 0x01
            ]
        );
    }
}
