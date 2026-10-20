//! Plain HTTP routes served beside the Dioxus app: health, robots, sitemaps,
//! the RSS feed, media files and the Google sign-in endpoints. Also the
//! site-wide middleware (canonical redirects, cache headers, compression).

use chrono::{DateTime, Utc};
use dioxus::server::axum::Router;
use dioxus::server::axum::extract::{DefaultBodyLimit, Path, Request};
use dioxus::server::axum::http::{
    Extensions, HeaderMap, HeaderValue, Method, StatusCode, Uri, Version, header,
};
use dioxus::server::axum::middleware::{self, Next};
use dioxus::server::axum::response::{IntoResponse, Response};
use dioxus::server::axum::routing::{get, post};
use recomphub_shared::enums::TargetPlatform;
use recomphub_shared::seo::xml_escape;
use sqlx::PgPool;
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{DefaultPredicate, Predicate};
use tower_http::services::ServeDir;

use super::{config, oauth, state};

/// Google's limit per sitemap file.
const SITEMAP_LIMIT: usize = 45_000;
const HTML_CACHE: &str = "public, max-age=60, s-maxage=300";
const MEDIA_CACHE: &str = "public, max-age=31536000, immutable";
const SITEMAP_CACHE: &str = "public, max-age=3600";
const FEED_CACHE: &str = "public, max-age=300";
const FEED_ITEMS: i64 = 50;

/// Adds the site routes, the media service and the site-wide middleware to the Dioxus router.
/// Requires the configuration to be installed first.
pub fn with_site_routes(router: Router) -> Router {
    let site = Router::new()
        .route("/healthz", get(healthz))
        .route("/robots.txt", get(robots))
        .route("/ads.txt", get(ads_txt))
        .route("/sitemap.xml", get(sitemap_index))
        .route("/sitemaps/{name}", get(sitemap_file))
        .route("/feed.xml", get(feed))
        .route("/auth/google", get(oauth::login))
        .route("/auth/google/callback", get(oauth::callback))
        .route("/auth/logout", post(oauth::logout))
        .nest_service("/media", ServeDir::new(&config::get().media_dir));

    let compression = CompressionLayer::new().compress_when(DefaultPredicate::new().and(
        |status: StatusCode, _: Version, _: &HeaderMap, _: &Extensions| {
            // Websocket upgrades (101) must pass through untouched.
            status != StatusCode::SWITCHING_PROTOCOLS
        },
    ));

    // Axum's `Bytes`-based extractors (used under every JSON server-fn body, including the
    // screenshot upload, which sends base64 bytes as a JSON string) cap requests at 2 MB by
    // default. Screenshots run up to `MAX_UPLOAD_BYTES` (ingest::media) before base64 overhead;
    // this covers that with room to spare.
    router
        .merge(site)
        .layer(compression)
        .layer(middleware::from_fn(cache_headers))
        .layer(middleware::from_fn(canonical))
        .layer(DefaultBodyLimit::max(24 * 1024 * 1024))
}

async fn healthz() -> Response {
    let pool = match pool_or_500() {
        Ok(pool) => pool,
        Err(response) => return *response,
    };
    match sqlx::query("SELECT 1").execute(pool).await {
        Ok(_) => "ok".into_response(),
        Err(err) => {
            tracing::warn!(error = ?err, "health check could not reach the database");
            plain(StatusCode::SERVICE_UNAVAILABLE, "database unavailable")
        }
    }
}

async fn ads_txt() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        "google.com, pub-6004542868270159, DIRECT, f08c47fec0942fa0\n",
    )
        .into_response()
}

async fn robots() -> Response {
    let site = &config::get().site_url;
    let mut body = String::from("User-agent: *\nAllow: /\n");
    for path in [
        "/api/",
        "/search",
        "/submit",
        "/account",
        "/moderation",
        "/admin",
        "/auth/",
    ] {
        body.push_str(&format!("Disallow: {path}\n"));
    }
    body.push_str(&format!("\nSitemap: {site}/sitemap.xml\n"));
    text_response("text/plain; charset=utf-8", SITEMAP_CACHE, body)
}

async fn sitemap_index() -> Response {
    let pool = match pool_or_500() {
        Ok(pool) => pool,
        Err(response) => return *response,
    };
    let site = &config::get().site_url;
    match build_sitemaps(pool, site).await {
        Ok(files) => {
            let mut body = String::from(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
            );
            for (name, _) in &files {
                body.push_str("<sitemap><loc>");
                body.push_str(&xml_escape(&format!("{site}/sitemaps/{name}")));
                body.push_str("</loc></sitemap>\n");
            }
            body.push_str("</sitemapindex>\n");
            text_response("application/xml; charset=utf-8", SITEMAP_CACHE, body)
        }
        Err(err) => internal("building the sitemap index", err),
    }
}

async fn sitemap_file(Path(name): Path<String>) -> Response {
    let pool = match pool_or_500() {
        Ok(pool) => pool,
        Err(response) => return *response,
    };
    match build_sitemaps(pool, &config::get().site_url).await {
        Ok(files) => match files.into_iter().find(|(file, _)| *file == name) {
            Some((_, body)) => text_response("application/xml; charset=utf-8", SITEMAP_CACHE, body),
            None => plain(StatusCode::NOT_FOUND, "no such sitemap"),
        },
        Err(err) => internal("building a sitemap", err),
    }
}

/// The 50 most recent port additions and releases on published ports, as RSS 2.0.
async fn feed() -> Response {
    let pool = match pool_or_500() {
        Ok(pool) => pool,
        Err(response) => return *response,
    };
    let site = &config::get().site_url;
    let rows = sqlx::query_as::<_, FeedRow>(
        "SELECT a.id, a.kind, a.created_at, a.data->>'version', g.slug, g.name, p.slug, p.name \
         FROM activity a \
         JOIN games g ON g.id = a.game_id \
         JOIN ports p ON p.id = a.port_id \
         WHERE a.kind IN ('port_added', 'release') AND p.status = 'published' \
         ORDER BY a.id DESC LIMIT $1",
    )
    .bind(FEED_ITEMS)
    .fetch_all(pool)
    .await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(err) => return internal("loading the feed", err),
    };

    let mut items = String::new();
    for (id, kind, created_at, version, game_slug, game_name, port_slug, port_name) in rows {
        let title = match (kind.as_str(), version) {
            ("release", Some(version)) => format!("{port_name} {version} released"),
            ("release", None) => format!("{port_name} released"),
            _ => format!("New port: {port_name} for {game_name}"),
        };
        let link = format!("{site}/games/{game_slug}/{port_slug}");
        items.push_str(&format!(
            "<item><title>{}</title><link>{}</link><guid isPermaLink=\"false\">recomphub-activity-{id}</guid><pubDate>{}</pubDate></item>\n",
            xml_escape(&title),
            xml_escape(&link),
            created_at.to_rfc2822(),
        ));
    }
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\"><channel>\n\
         <title>RecompHub</title>\n<link>{}</link>\n\
         <description>New native ports, recompilations and releases of games</description>\n{items}\
         </channel></rss>\n",
        xml_escape(site),
    );
    text_response("application/rss+xml; charset=utf-8", FEED_CACHE, body)
}

/// Row of the feed query: activity id, kind, time, version, game slug and name, port slug and name.
type FeedRow = (
    i64,
    String,
    DateTime<Utc>,
    Option<String>,
    String,
    String,
    String,
    String,
);

/// One `<url>` entry, optionally with last-modified and approved AVIF image URLs.
struct Entry {
    path: String,
    lastmod: Option<String>,
    images: Vec<String>,
}

fn entry(path: String, lastmod: Option<DateTime<Utc>>) -> Entry {
    Entry {
        path,
        lastmod: lastmod.map(|at| at.format("%Y-%m-%d").to_string()),
        images: Vec::new(),
    }
}

fn static_entry(path: &str) -> Entry {
    Entry {
        path: path.to_string(),
        lastmod: None,
        images: Vec::new(),
    }
}

/// Every sitemap file as `(file name, XML)`. Sets with more than 45,000 URLs are split
/// into `name.xml`, `name-2.xml`, and so on. Every set has at least one file.
async fn build_sitemaps(pool: &PgPool, site: &str) -> anyhow::Result<Vec<(String, String)>> {
    let sets = [
        ("games", game_entries(pool).await?),
        ("ports", port_entries(pool).await?),
        ("hubs", hub_entries(pool).await?),
        ("authors", author_entries(pool).await?),
    ];
    let mut files = Vec::new();
    for (name, entries) in sets {
        let chunks: Vec<&[Entry]> = if entries.is_empty() {
            vec![&[]]
        } else {
            entries.chunks(SITEMAP_LIMIT).collect()
        };
        for (index, chunk) in chunks.into_iter().enumerate() {
            let file = if index == 0 {
                format!("{name}.xml")
            } else {
                format!("{name}-{}.xml", index + 1)
            };
            files.push((file, urlset(site, chunk)));
        }
    }
    Ok(files)
}

async fn game_entries(pool: &PgPool) -> anyhow::Result<Vec<Entry>> {
    let rows: Vec<(String, DateTime<Utc>)> =
        sqlx::query_as("SELECT slug, updated_at FROM games ORDER BY id")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(slug, updated)| entry(format!("/games/{slug}"), Some(updated)))
        .collect())
}

async fn port_entries(pool: &PgPool) -> anyhow::Result<Vec<Entry>> {
    let rows: Vec<(String, String, DateTime<Utc>, Option<String>, Option<String>, Option<String>, Option<i32>, Option<i32>)> =
        sqlx::query_as(
            "SELECT g.slug, p.slug, p.updated_at, m.kind, m.public_stem, m.hash, m.render_version, m.width \
             FROM ports p \
             JOIN games g ON g.id = p.game_id \
             LEFT JOIN port_media pm ON pm.port_id = p.id AND pm.approved_at IS NOT NULL \
             LEFT JOIN media m ON m.id = pm.media_id AND m.status = 'ready' \
             WHERE p.status = 'published' \
             ORDER BY g.slug, p.slug, pm.position, pm.id",
        )
        .fetch_all(pool)
        .await?;
    let mut entries = Vec::new();
    for (game, port, updated, kind, stem, hash, render_version, width) in rows {
        let path = format!("/games/{game}/{port}");
        let image = match (kind, stem, hash, render_version) {
            (Some(kind), Some(stem), Some(hash), Some(render_version)) => {
                sitemap_image_path(&kind, &stem, &hash, render_version, width)
            }
            _ => None,
        };
        if let Some(entry) = entries.last_mut().filter(|entry: &&mut Entry| entry.path == path) {
            if let Some(image) = image {
                entry.images.push(image);
            }
        } else {
            let mut entry = entry(path, Some(updated));
            if let Some(image) = image {
                entry.images.push(image);
            }
            entries.push(entry);
        }
    }
    Ok(entries)
}

async fn hub_entries(pool: &PgPool) -> anyhow::Result<Vec<Entry>> {
    let mut entries = vec![
        static_entry("/platforms"),
        static_entry("/stats"),
        static_entry("/trending"),
        static_entry("/wanted"),
        static_entry("/new"),
        static_entry("/about"),
        static_entry("/creators"),
        static_entry("/creators/recompile"),
        static_entry("/creators/platforms"),
        static_entry("/creators/videos"),
    ];

    let platforms: Vec<(String,)> = sqlx::query_as("SELECT slug FROM platforms ORDER BY slug")
        .fetch_all(pool)
        .await?;
    entries.extend(
        platforms
            .into_iter()
            .map(|(slug,)| entry(format!("/platforms/{slug}"), None)),
    );

    let runs_on = recomphub_db::ports::runs_on_counts(pool).await?;
    for (target, count) in runs_on {
        if count < 1 {
            continue;
        }
        if let Ok(target) = target.parse::<TargetPlatform>() {
            entries.push(entry(format!("/runs-on/{}", target.slug()), None));
        }
    }

    // Platform-and-target pairs with at least three published playable ports.
    let pairs: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT pl.slug, t.target, count(*) FROM ports p \
         JOIN platforms pl ON pl.id = p.source_platform_id \
         CROSS JOIN LATERAL unnest(port_platforms(p)) AS t(target) \
         WHERE p.status = 'published' AND p.kind <> 'decompilation' \
         GROUP BY pl.slug, t.target HAVING count(*) >= 3 ORDER BY pl.slug, t.target",
    )
    .fetch_all(pool)
    .await?;
    for (platform, target, _) in pairs {
        if let Ok(target) = target.parse::<TargetPlatform>() {
            entries.push(entry(
                format!("/platforms/{platform}/{}", target.slug()),
                None,
            ));
        }
    }
    Ok(entries)
}

async fn author_entries(pool: &PgPool) -> anyhow::Result<Vec<Entry>> {
    let logins: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT lower(a.github_login) FROM port_authors a \
         JOIN ports p ON p.id = a.port_id \
         WHERE p.status = 'published' AND a.github_login IS NOT NULL ORDER BY 1",
    )
    .fetch_all(pool)
    .await?;
    Ok(logins
        .into_iter()
        .map(|(login,)| entry(format!("/authors/{login}"), None))
        .collect())
}

fn urlset(site: &str, entries: &[Entry]) -> String {
    let mut body = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\" \
         xmlns:image=\"http://www.google.com/schemas/sitemap-image/1.1\">\n",
    );
    for entry in entries {
        body.push_str("<url><loc>");
        body.push_str(&xml_escape(&format!("{site}{}", entry.path)));
        body.push_str("</loc>");
        if let Some(lastmod) = &entry.lastmod {
            body.push_str("<lastmod>");
            body.push_str(lastmod);
            body.push_str("</lastmod>");
        }
        for image in &entry.images {
            body.push_str("<image:image><image:loc>");
            body.push_str(&xml_escape(&format!("{site}{image}")));
            body.push_str("</image:loc></image:image>");
        }
        body.push_str("</url>\n");
    }
    body.push_str("</urlset>\n");
    body
}

fn sitemap_image_path(
    kind: &str,
    stem: &str,
    hash: &str,
    render_version: i32,
    source_width: Option<i32>,
) -> Option<String> {
    let widths = match kind {
        "screenshot" | "port_screenshot" => &[480, 960, 1600][..],
        "cover" => &[132, 264, 528][..],
        "video_thumb" => &[320, 480][..],
        "avatar" => &[40, 80][..],
        _ => return None,
    };
    let source_width = source_width.unwrap_or_default().max(0) as u32;
    let width = widths.iter().copied().filter(|width| *width <= source_width).max()?;
    Some(format!("/media/{kind}/{stem}-{hash}-v{render_version}-{width}.avif"))
}

/// Redirects to the canonical URL: `www` host to the apex, no trailing slash, lowercase path.
/// GET and HEAD only, so POST server functions are never redirected.
async fn canonical(req: Request, next: Next) -> Response {
    if matches!(*req.method(), Method::GET | Method::HEAD)
        && let Some(location) = canonical_location(req.headers(), req.uri())
    {
        return (
            StatusCode::MOVED_PERMANENTLY,
            [(header::LOCATION, location)],
        )
            .into_response();
    }
    next.run(req).await
}

fn canonical_location(headers: &HeaderMap, uri: &Uri) -> Option<String> {
    let host_is_www = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| {
            host.split(':')
                .next()
                .unwrap_or(host)
                .eq_ignore_ascii_case("www.recomphub.com")
        });
    let path = canonical_path(uri.path());
    if !host_is_www && path == uri.path() {
        return None;
    }
    let query = uri
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    Some(format!("{}{path}{query}", config::get().site_url))
}

fn canonical_path(path: &str) -> String {
    let mut lower = path.to_ascii_lowercase();
    while lower.len() > 1 && lower.ends_with('/') {
        lower.pop();
    }
    lower
}

/// Cache headers: long-lived immutable media, short public caching for HTML.
/// Only GET responses are touched, and a handler's own `Cache-Control` always wins.
async fn cache_headers(req: Request, next: Next) -> Response {
    let is_get = *req.method() == Method::GET;
    let is_media = req.uri().path().starts_with("/media/");
    let mut response = next.run(req).await;
    if !is_get || response.headers().contains_key(header::CACHE_CONTROL) {
        return response;
    }
    if is_media {
        if response.status().is_success() {
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static(MEDIA_CACHE));
        }
    } else if is_html(response.headers()) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(HTML_CACHE));
    }
    response
}

fn is_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"))
}

fn pool_or_500() -> Result<&'static PgPool, Box<Response>> {
    state::pool().map_err(|err| {
        tracing::error!(error = %err, "database pool unavailable");
        Box::new(plain(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))
    })
}

/// Logs the error with its context and returns a generic 500.
fn internal(context: &str, err: impl std::fmt::Debug) -> Response {
    tracing::error!(error = ?err, "{context} failed");
    plain(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

fn plain(status: StatusCode, message: &'static str) -> Response {
    (status, message).into_response()
}

fn text_response(content_type: &'static str, cache: &'static str, body: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_path_lowercases_and_drops_trailing_slashes() {
        assert_eq!(canonical_path("/Games/Zelda/"), "/games/zelda");
        assert_eq!(canonical_path("/games/"), "/games");
        assert_eq!(canonical_path("/"), "/");
    }

    #[test]
    fn sitemap_urlset_escapes_urls_and_keeps_lastmod() {
        let entries = [
            entry("/games/a&b".to_string(), None),
            entry("/games/c".to_string(), DateTime::from_timestamp(0, 0)),
        ];
        let xml = urlset("https://recomphub.com", &entries);
        assert!(xml.contains("<loc>https://recomphub.com/games/a&amp;b</loc>"));
        assert!(xml.contains("<lastmod>1970-01-01</lastmod>"));
        assert_eq!(xml.matches("<url>").count(), 2);
    }
}
