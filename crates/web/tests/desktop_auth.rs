#![cfg(feature = "server")]

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use dioxus::server::{
    DioxusRouterExt, FullstackState,
    axum::{Router, serve},
};
use rand::Rng;
use recomphub::server::{
    config::{self, Config},
    http::with_site_routes,
    session, state,
};
use recomphub_db::{
    identities::{self, IdentityProfile},
    users_sessions,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, PgPool, postgres::PgPoolOptions};
use tokio::{net::TcpListener, task::JoinHandle};

struct TestDb {
    pool: PgPool,
    admin_url: String,
    name: String,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_approved_authorization_runs_over_loopback() {
    let db = create_db().await;
    assert!(
        state::init_pool(db.pool.clone()).is_ok(),
        "integration binary requires one process-global pool"
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let base = format!("http://{address}");
    config::install(Config {
        database_url: "disposable-test-database".into(),
        site_url: base.clone(),
        media_dir: std::env::temp_dir(),
        google_client_id: None,
        google_client_secret: None,
        github_token: None,
        admin_emails: vec![],
        amazon_tag: None,
    });
    let app = with_site_routes(
        Router::new()
            .register_server_functions()
            .with_state(FullstackState::headless()),
    );
    let server: JoinHandle<()> = tokio::spawn(async move {
        serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let response = client
        .post(format!("{base}/api/v1/desktop/auth/start"))
        .json(&json!({"challenge": URL_SAFE_NO_PAD.encode(Sha256::digest([7_u8; 32]))}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let started: Value = response.json().await.unwrap();
    let request_id = started["request_id"].as_str().unwrap().to_owned();
    let authorization_url = started["authorization_url"].as_str().unwrap();
    assert!(authorization_url.ends_with(&format!("/auth/desktop?request_id={request_id}")));

    let redirect = client
        .get(format!("{base}/auth/desktop?request_id={request_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(redirect.status(), 302);
    assert_eq!(redirect.headers()["cache-control"], "private, no-store");
    assert_eq!(redirect.headers()["referrer-policy"], "no-referrer");
    assert!(
        redirect.headers()["location"]
            .to_str()
            .unwrap()
            .starts_with("/auth/google?next=")
    );
    let unknown = URL_SAFE_NO_PAD.encode([99_u8; 32]);
    let bad = client
        .get(format!("{base}/auth/desktop?request_id={unknown}"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
    assert_eq!(bad.headers()["cache-control"], "private, no-store");
    assert_eq!(bad.headers()["referrer-policy"], "no-referrer");

    let profile = IdentityProfile {
        subject: format!("desktop-auth-{}", rand::rng().random::<u64>()),
        email: Some("desktop@example.invalid".into()),
        name: Some("<img src=x onerror=alert(1)>".into()),
        avatar_media_id: None,
    };
    let user = identities::sign_in(&db.pool, &profile, false)
        .await
        .unwrap();
    assert_eq!(
        identities::sign_in(&db.pool, &profile, false)
            .await
            .unwrap()
            .id,
        user.id
    );
    let browser = session::new_token();
    users_sessions::create_session(&db.pool, &browser, user.id, Utc::now() + Duration::days(30))
        .await
        .unwrap();
    let form = client
        .get(format!("{base}/auth/desktop?request_id={request_id}"))
        .header("cookie", format!("rh_session={browser}"))
        .send()
        .await
        .unwrap();
    assert_eq!(form.status(), 200);
    assert_eq!(
        form.headers()["content-security-policy"],
        "default-src 'none'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
    );
    assert_eq!(form.headers()["referrer-policy"], "same-origin");
    assert_eq!(form.headers()["cache-control"], "private, no-store");
    let html = form.text().await.unwrap();
    assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
    assert!(!html.contains("<img src=x"));
    let csrf = html
        .split("name=\"csrf_token\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string();
    let other_browser = session::new_token();
    users_sessions::create_session(
        &db.pool,
        &other_browser,
        user.id,
        Utc::now() + Duration::days(30),
    )
    .await
    .unwrap();
    let session_swap = client
        .get(format!("{base}/auth/desktop?request_id={request_id}"))
        .header("cookie", format!("rh_session={other_browser}"))
        .send()
        .await
        .unwrap();
    assert_eq!(session_swap.status(), 403);

    let pending = client
        .post(format!("{base}/api/v1/desktop/auth/exchange"))
        .json(&json!({"request_id":request_id,"verifier":URL_SAFE_NO_PAD.encode([7_u8; 32])}))
        .send()
        .await
        .unwrap();
    let wrong = client
        .post(format!("{base}/api/v1/desktop/auth/exchange"))
        .json(&json!({"request_id":request_id,"verifier":URL_SAFE_NO_PAD.encode([8_u8; 32])}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 400);
    assert_eq!(pending.status(), 202);
    let no_origin = client
        .post(format!("{base}/auth/desktop/approve"))
        .header("cookie", format!("rh_session={browser}"))
        .form(&[
            ("request_id", request_id.as_str()),
            ("csrf_token", csrf.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(no_origin.status(), 403);
    assert_eq!(no_origin.headers()["referrer-policy"], "no-referrer");
    let opaque_origin = client
        .post(format!("{base}/auth/desktop/approve"))
        .header("origin", "null")
        .header("cookie", format!("rh_session={browser}"))
        .form(&[
            ("request_id", request_id.as_str()),
            ("csrf_token", csrf.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(opaque_origin.status(), 403);
    let swapped_csrf = URL_SAFE_NO_PAD.encode([4_u8; 32]);
    let csrf_swap = client
        .post(format!("{base}/auth/desktop/approve"))
        .header("origin", base.as_str())
        .header("cookie", format!("rh_session={browser}"))
        .form(&[
            ("request_id", request_id.as_str()),
            ("csrf_token", swapped_csrf.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(csrf_swap.status(), 403);
    let rejected = client
        .post(format!("{base}/auth/desktop/approve"))
        .header("origin", "http://evil.invalid")
        .header("cookie", format!("rh_session={browser}"))
        .form(&[
            ("request_id", request_id.as_str()),
            ("csrf_token", csrf.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 403);
    let approved = client
        .post(format!("{base}/auth/desktop/approve"))
        .header("origin", base.as_str())
        .header("cookie", format!("rh_session={browser}"))
        .form(&[
            ("request_id", request_id.as_str()),
            ("csrf_token", csrf.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(approved.status(), 200);
    assert_eq!(approved.headers()["cache-control"], "private, no-store");
    let swapped = client
        .post(format!("{base}/auth/desktop/approve"))
        .header("origin", base.as_str())
        .header("cookie", format!("rh_session={}", session::new_token()))
        .form(&[
            ("request_id", request_id.as_str()),
            ("csrf_token", csrf.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(swapped.status(), 409);

    let exchange = || {
        client
            .post(format!("{base}/api/v1/desktop/auth/exchange"))
            .json(&json!({"request_id":request_id,"verifier":URL_SAFE_NO_PAD.encode([7_u8; 32])}))
    };
    let (first, second) = tokio::join!(exchange().send(), exchange().send());
    let first = first.unwrap();
    let second = second.unwrap();
    assert!(
        (first.status() == 200 && second.status() == 410)
            || (first.status() == 410 && second.status() == 200)
    );
    let complete = if first.status() == 200 { first } else { second };
    let issued: Value = complete.json().await.unwrap();
    let native = issued["session_token"].as_str().unwrap();
    assert!(
        native != browser,
        "desktop must receive an independent session"
    );
    let replay = exchange().send().await.unwrap();
    assert_eq!(replay.status(), 410);
    let me = client
        .get(format!("{base}/api/me"))
        .header("cookie", format!("rh_session={native}"))
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);
    let me: Option<recomphub_shared::SessionUser> = me.json().await.unwrap();
    assert_eq!(me.expect("native identity").id, user.id);
    let logout = client
        .post(format!("{base}/api/v1/desktop/auth/logout"))
        .header("cookie", format!("rh_session={native}"))
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 204);
    assert!(
        users_sessions::find_session_user(&db.pool, &browser)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        users_sessions::find_session_user(&db.pool, native)
            .await
            .unwrap()
            .is_none()
    );
    let rollback_id = session::new_token();
    let rollback_challenge = [31_u8; 32];
    let rollback_csrf: [u8; 32] = Sha256::digest(b"rollback-csrf").into();
    recomphub_db::desktop_auth::start_request(&db.pool, &rollback_id, &rollback_challenge)
        .await
        .unwrap();
    recomphub_db::desktop_auth::bind_confirmation(&db.pool, &rollback_id, &browser, &rollback_csrf)
        .await
        .unwrap();
    recomphub_db::desktop_auth::approve_request(&db.pool, &rollback_id, &browser, &rollback_csrf)
        .await
        .unwrap();
    assert!(
        recomphub_db::desktop_auth::exchange_request(
            &db.pool,
            &rollback_id,
            &rollback_challenge,
            &browser
        )
        .await
        .is_err()
    );
    assert!(matches!(
        recomphub_db::desktop_auth::exchange_request(
            &db.pool,
            &rollback_id,
            &rollback_challenge,
            "replacement-native-session"
        )
        .await,
        Ok(recomphub_db::desktop_auth::ExchangeResult::Complete { .. })
    ));
    let banned_profile = IdentityProfile {
        subject: format!("desktop-banned-{}", rand::rng().random::<u64>()),
        email: None,
        name: Some("Banned test user".into()),
        avatar_media_id: None,
    };
    let banned_user = identities::sign_in(&db.pool, &banned_profile, false)
        .await
        .unwrap();
    let banned_browser = session::new_token();
    users_sessions::create_session(
        &db.pool,
        &banned_browser,
        banned_user.id,
        Utc::now() + Duration::days(30),
    )
    .await
    .unwrap();
    let banned_verifier = [42_u8; 32];
    let banned_challenge = Sha256::digest(banned_verifier);
    let banned_start = client
        .post(format!("{base}/api/v1/desktop/auth/start"))
        .json(&json!({"challenge":URL_SAFE_NO_PAD.encode(banned_challenge)}))
        .send()
        .await
        .unwrap();
    let banned_request: Value = banned_start.json().await.unwrap();
    let banned_request_id = banned_request["request_id"].as_str().unwrap();
    let consent = client
        .get(format!(
            "{base}/auth/desktop?request_id={banned_request_id}"
        ))
        .header("cookie", format!("rh_session={banned_browser}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let banned_csrf = consent
        .split("name=\"csrf_token\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let consent_result = client
        .post(format!("{base}/auth/desktop/approve"))
        .header("origin", base.as_str())
        .header("cookie", format!("rh_session={banned_browser}"))
        .form(&[
            ("request_id", banned_request_id),
            ("csrf_token", banned_csrf),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(consent_result.status(), 200);
    sqlx::query("UPDATE users SET banned = true WHERE id = $1")
        .bind(banned_user.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let banned_exchange = client.post(format!("{base}/api/v1/desktop/auth/exchange"))
        .json(&json!({"request_id":banned_request_id,"verifier":URL_SAFE_NO_PAD.encode(banned_verifier)}))
        .send().await.unwrap();
    assert_eq!(banned_exchange.status(), 403);
    sqlx::query("UPDATE users SET banned = false WHERE id = $1")
        .bind(banned_user.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let deleted_profile = IdentityProfile {
        subject: format!("desktop-deleted-{}", rand::rng().random::<u64>()),
        email: None,
        name: Some("Deleted fixture account".into()),
        avatar_media_id: None,
    };
    let deleted = identities::sign_in(&db.pool, &deleted_profile, false)
        .await
        .unwrap();
    let deleted_browser = session::new_token();
    users_sessions::create_session(
        &db.pool,
        &deleted_browser,
        deleted.id,
        Utc::now() + Duration::days(30),
    )
    .await
    .unwrap();
    let deleted_request = session::new_token();
    let deleted_verifier = [44_u8; 32];
    let deleted_hash: [u8; 32] = Sha256::digest(deleted_verifier).into();
    let deleted_csrf = [45_u8; 32];
    recomphub_db::desktop_auth::start_request(&db.pool, &deleted_request, &deleted_hash)
        .await
        .unwrap();
    recomphub_db::desktop_auth::bind_confirmation(
        &db.pool,
        &deleted_request,
        &deleted_browser,
        &deleted_csrf,
    )
    .await
    .unwrap();
    recomphub_db::desktop_auth::approve_request(
        &db.pool,
        &deleted_request,
        &deleted_browser,
        &deleted_csrf,
    )
    .await
    .unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(deleted.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let refused = client.post(format!("{base}/api/v1/desktop/auth/exchange"))
        .json(&json!({"request_id":deleted_request,"verifier":URL_SAFE_NO_PAD.encode(deleted_verifier)}))
        .send().await.unwrap();
    assert!(
        refused.status().is_client_error(),
        "a deleted user must not receive a desktop session"
    );

    let malformed = client
        .post(format!("{base}/api/v1/desktop/auth/start"))
        .header("content-type", "application/json")
        .body("{")
        .send()
        .await
        .unwrap();
    assert!(malformed.status().is_client_error());
    assert_eq!(malformed.headers()["cache-control"], "private, no-store");
    let expired_id = session::new_token();
    desktop_request(&db.pool, &expired_id).await;
    sqlx::query("UPDATE desktop_login_requests SET expires_at = now() - interval '1 second' WHERE request_hash = $1")
        .bind(users_sessions::session_hash(&expired_id)).execute(&db.pool).await.unwrap();
    let expired = client
        .get(format!("{base}/auth/desktop?request_id={expired_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(expired.status(), 410);
    assert_eq!(expired.headers()["cache-control"], "private, no-store");
    // Isolate capacity from earlier flow scenarios, then race admission through
    // the real listener. Exactly 1,000 independent grants may be admitted.
    sqlx::query("DELETE FROM desktop_login_requests")
        .execute(&db.pool)
        .await
        .unwrap();
    let mut requests = Vec::with_capacity(1_001);
    // Bound sockets, not admission tasks: this exercises the transaction race
    // without making the OS TCP listen backlog the subject of the test.
    let connections = std::sync::Arc::new(tokio::sync::Semaphore::new(32));
    for _ in 0..1_001 {
        let client = client.clone();
        let url = format!("{base}/api/v1/desktop/auth/start");
        let connections = connections.clone();
        requests.push(tokio::spawn(async move {
            let _connection = connections.acquire_owned().await.unwrap();
            let response = client
                .post(url)
                .json(&json!({"challenge":URL_SAFE_NO_PAD.encode([3_u8; 32])}))
                .send()
                .await
                .unwrap();
            let status = response.status();
            if status == 503 {
                assert_eq!(response.headers()["retry-after"], "60");
            }
            status.as_u16()
        }));
    }
    let mut admitted = 0;
    let mut refused = 0;
    for request in requests {
        match request.await.unwrap() {
            200 => admitted += 1,
            503 => refused += 1,
            status => panic!("unexpected admission status {status}"),
        }
    }
    assert_eq!(admitted, 1_000);
    assert_eq!(refused, 1);
    // A consumed grant still occupies admission capacity until its original TTL.
    sqlx::query("UPDATE desktop_login_requests SET consumed_at = now()")
        .execute(&db.pool)
        .await
        .unwrap();
    let overflow = client
        .post(format!("{base}/api/v1/desktop/auth/start"))
        .json(&json!({"challenge":URL_SAFE_NO_PAD.encode([3_u8; 32])}))
        .send()
        .await
        .unwrap();
    assert_eq!(overflow.status(), 503);
    assert_eq!(overflow.headers()["retry-after"], "60");
    sqlx::query("UPDATE desktop_login_requests SET created_at = now() - interval '11 minutes', expires_at = now() - interval '1 minute'")
        .execute(&db.pool).await.unwrap();
    desktop_request(&db.pool, &session::new_token()).await;
    server.abort();
    drop_db(db).await;
}

async fn create_db() -> TestDb {
    let url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must point to disposable Postgres");
    let (base, _) = url.rsplit_once('/').expect("database URL");
    let admin_url = format!("{base}/postgres");
    let name = format!("recomphub_auth_{:016x}", rand::rng().random::<u64>());
    let mut admin = PgConnection::connect(&admin_url)
        .await
        .expect("connect to disposable Postgres");
    sqlx::query(&format!("CREATE DATABASE \"{name}\""))
        .execute(&mut admin)
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&format!("{base}/{name}"))
        .await
        .unwrap();
    recomphub_db::migrate(&pool).await.unwrap();
    TestDb {
        pool,
        admin_url,
        name,
    }
}

async fn drop_db(db: TestDb) {
    let TestDb {
        pool,
        admin_url,
        name,
    } = db;
    pool.close().await;
    let mut admin = PgConnection::connect(&admin_url).await.unwrap();
    sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .execute(&mut admin)
        .await
        .unwrap();
}

async fn desktop_request(pool: &PgPool, request_id: &str) {
    recomphub_db::desktop_auth::start_request(pool, request_id, &[1; 32])
        .await
        .unwrap();
}
