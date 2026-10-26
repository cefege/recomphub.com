//! Posts new published ports and releases to the Discord server's `#announcements` channel, so
//! Discord members see the same signal the public RSS feed (`GET /feed.xml`,
//! `crates/web/src/server/http.rs`) already shows on the website — without anyone checking
//! either by hand. Titles and links match the feed exactly, so the two never disagree.
//!
//! Scheduled by the worker's `discord-announce` command. Separate from `discord-setup`, which
//! only configures the server's channels and permissions once, by hand.
//!
//! Progress is tracked in `settings` as the last posted `activity.id`, not a timestamp: two
//! activity rows can share a millisecond, and an id-based watermark can never re-announce or
//! permanently skip a row because of that. It advances after each successful post, so a failed
//! send retries next run instead of duplicating everything already posted.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use recomphub_db::PgPool;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

const API: &str = "https://discord.com/api/v10";
const GUILD_ID: &str = "1561674685316472994";
const CHANNEL_NAME: &str = "announcements";
/// Discord permission bit for sending messages, matching `discord-setup`'s `SEND_MESSAGES`.
const SEND_MESSAGES: u64 = 1 << 11;
/// Discord's "Blurple" brand color (https://discord.com/branding), matching the website's
/// `BUTTON_DISCORD` Join Discord CTAs.
const BRAND_COLOR: u32 = 0x5865F2;
const LAST_POSTED_KEY: &str = "discord_announce_last_posted_id";
/// Messages sent per run. A long outage (job suspended, token revoked) then replays as one
/// bounded burst instead of flooding the channel; the rest posts on the following runs.
const MAX_PER_RUN: usize = 20;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub posted: usize,
    /// More rows were due than `MAX_PER_RUN`; the remainder posts next run.
    pub truncated: bool,
}

/// One activity row, already turned into what gets posted.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Announcement {
    activity_id: i64,
    title: String,
    url: String,
}

#[derive(Clone)]
pub struct DiscordClient {
    http: Client,
    token: String,
}

impl DiscordClient {
    /// Reads `DISCORD_BOT_TOKEN`, the same token `discord-setup` uses.
    pub fn from_env() -> Result<Self> {
        let token = std::env::var("DISCORD_BOT_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .context("DISCORD_BOT_TOKEN is not set")?;
        let http = Client::builder()
            .user_agent("recomphub/1.0 (+https://recomphub.com)")
            .timeout(Duration::from_secs(30))
            .build()
            .context("building Discord HTTP client")?;
        Ok(Self { http, token })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{API}{path}"))
            .header("Authorization", format!("Bot {}", self.token))
    }

    async fn channel_id(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct Channel {
            id: String,
            name: String,
            #[serde(rename = "type")]
            kind: u8,
        }
        let response = self
            .request(reqwest::Method::GET, &format!("/guilds/{GUILD_ID}/channels"))
            .send()
            .await
            .context("listing Discord channels")?;
        let status = response.status();
        let body = response.text().await.context("reading Discord response")?;
        if !status.is_success() {
            bail!("listing Discord channels failed ({status}): {body}");
        }
        let channels: Vec<Channel> =
            serde_json::from_str(&body).context("decoding Discord channels")?;
        channels
            .into_iter()
            .find(|channel| channel.kind == 0 && channel.name == CHANNEL_NAME)
            .map(|channel| channel.id)
            .with_context(|| format!("#{CHANNEL_NAME} channel not found"))
    }

    /// `discord-setup` makes `#announcements` read-only for `@everyone`, so the bot needs its
    /// own explicit allow overwrite to post there regardless of how it was invited. Idempotent;
    /// safe to call every run.
    async fn ensure_send_permission(&self, channel_id: &str) -> Result<()> {
        #[derive(Deserialize)]
        struct CurrentUser {
            id: String,
        }
        let me: CurrentUser = self
            .request(reqwest::Method::GET, "/users/@me")
            .send()
            .await
            .context("fetching the bot's own user id")?
            .error_for_status()
            .context("fetching the bot's own user id")?
            .json()
            .await
            .context("decoding the bot's own user")?;
        let response = self
            .request(
                reqwest::Method::PUT,
                &format!("/channels/{channel_id}/permissions/{}", me.id),
            )
            .json(&json!({
                "id": me.id,
                "type": 1,
                "allow": SEND_MESSAGES.to_string(),
                "deny": "0",
            }))
            .send()
            .await
            .context("granting the bot send permission")?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("granting the bot send permission failed ({status}): {body}");
        }
        Ok(())
    }

    async fn post(&self, channel_id: &str, announcement: &Announcement) -> Result<()> {
        let body = json!({
            "embeds": [{
                "title": announcement.title,
                "url": announcement.url,
                "color": BRAND_COLOR,
            }],
        });
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("/channels/{channel_id}/messages"),
            )
            .json(&body)
            .send()
            .await
            .with_context(|| format!("posting activity {}", announcement.activity_id))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!(
                "posting activity {} failed ({status}): {body}",
                announcement.activity_id
            );
        }
        Ok(())
    }
}

/// The stored watermark, or the current activity high-water mark when the job has never run
/// before — so the first run announces nothing from before the bot existed, only what happens
/// after it starts.
async fn last_posted_id(pool: &PgPool) -> Result<i64> {
    let value: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = $1")
            .bind(LAST_POSTED_KEY)
            .fetch_optional(pool)
            .await?;
    match value.and_then(|v| v.as_i64()) {
        Some(id) => Ok(id),
        None => {
            let baseline = recomphub_db::activity::latest_id(pool).await?;
            set_last_posted_id(pool, baseline).await?;
            Ok(baseline)
        }
    }
}

async fn set_last_posted_id(pool: &PgPool, id: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(LAST_POSTED_KEY)
    .bind(json!(id))
    .execute(pool)
    .await?;
    Ok(())
}

/// Title text, matching `/feed.xml`'s wording exactly so Discord and RSS never disagree.
fn title(kind: &str, version: Option<&str>, port_name: &str, game_name: &str) -> String {
    match (kind, version) {
        ("release", Some(version)) => format!("{port_name} {version} released"),
        ("release", None) => format!("{port_name} released"),
        _ => format!("New port: {port_name} for {game_name}"),
    }
}

type ActivityRow = (i64, String, Option<String>, String, String, String, String);

/// Port additions and releases on published ports since `after_id`, oldest first. Same filter
/// as `/feed.xml`.
async fn due(pool: &PgPool, after_id: i64, site: &str) -> Result<Vec<Announcement>> {
    let rows = sqlx::query_as::<_, ActivityRow>(
        "SELECT a.id, a.kind, a.data->>'version', g.slug, g.name, p.slug, p.name \
         FROM activity a \
         JOIN games g ON g.id = a.game_id \
         JOIN ports p ON p.id = a.port_id \
         WHERE a.kind IN ('port_added', 'release') AND p.status = 'published' AND a.id > $1 \
         ORDER BY a.id ASC",
    )
    .bind(after_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, kind, version, game_slug, game_name, port_slug, port_name)| Announcement {
                activity_id: id,
                title: title(&kind, version.as_deref(), &port_name, &game_name),
                url: format!("{site}/games/{game_slug}/{port_slug}"),
            },
        )
        .collect())
}

/// What would post since the last run, without a Discord token, a client, or posting anything.
/// Never advances the watermark past establishing it, except the very first time this ever
/// runs against a database: that call still records today's activity high-water mark as the
/// baseline, so later runs (dry or live) only ever see activity from after the bot started.
pub async fn preview(pool: &PgPool, site: &str) -> Result<Vec<(String, String)>> {
    let after_id = last_posted_id(pool).await?;
    let announcements = due(pool, after_id, site).await?;
    Ok(announcements
        .into_iter()
        .map(|a| (a.title, a.url))
        .collect())
}

/// Posts every published port addition and release since the last run to `#announcements`, in
/// order, advancing the watermark after each successful post.
pub async fn run(pool: &PgPool, discord: &DiscordClient, site: &str) -> Result<Report> {
    let after_id = last_posted_id(pool).await?;
    let mut announcements = due(pool, after_id, site).await?;
    let truncated = announcements.len() > MAX_PER_RUN;
    announcements.truncate(MAX_PER_RUN);
    if announcements.is_empty() {
        return Ok(Report::default());
    }

    let channel_id = discord.channel_id().await?;
    discord.ensure_send_permission(&channel_id).await?;
    for announcement in &announcements {
        discord.post(&channel_id, announcement).await?;
        set_last_posted_id(pool, announcement.activity_id).await?;
    }
    Ok(Report {
        posted: announcements.len(),
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use recomphub_db::activity;
    use recomphub_db::games::{self, IgdbGame};
    use recomphub_db::ports::{self, PortFull, PortInput};

    #[test]
    fn release_title_includes_version_when_known() {
        assert_eq!(
            title("release", Some("v1.2.0"), "Super Port", "Super Game"),
            "Super Port v1.2.0 released"
        );
        assert_eq!(
            title("release", None, "Super Port", "Super Game"),
            "Super Port released"
        );
    }

    #[test]
    fn port_added_title_names_the_game() {
        assert_eq!(
            title("port_added", None, "Super Port", "Super Game"),
            "New port: Super Port for Super Game"
        );
    }

    async fn make_game(pool: &PgPool, igdb_id: i64, slug: &str) -> Result<i64> {
        let mut conn = pool.acquire().await?;
        Ok(games::sync_igdb(
            &mut conn,
            &IgdbGame {
                igdb_id,
                name: slug.to_string(),
                slug: slug.to_string(),
                ..Default::default()
            },
        )
        .await?)
    }

    async fn make_port(pool: &PgPool, game_id: i64, slug: &str, status: &str) -> Result<i64> {
        let mut conn = pool.acquire().await?;
        let port_id = ports::insert_port(
            &mut conn,
            game_id,
            &PortFull {
                input: PortInput {
                    name: slug.to_string(),
                    slug: Some(slug.to_string()),
                    kind: "static_recomp".to_string(),
                    repo_url: format!("https://github.com/example/{slug}"),
                    playability: "playable".to_string(),
                    ..Default::default()
                },
                authors: vec![],
            },
            &[format!("import:{slug}")],
        )
        .await?;
        // `insert_port` always inserts as published; flip to hidden for the "not published" case.
        if status != "published" {
            ports::set_status(&mut conn, port_id, "hidden").await?;
        }
        Ok(port_id)
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn unpublished_and_unrelated_activity_is_excluded(pool: PgPool) -> Result<()> {
        let game = make_game(&pool, 1, "game-a").await?;
        let published = make_port(&pool, game, "port-a", "published").await?;
        let hidden = make_port(&pool, game, "port-b", "hidden").await?;

        activity::insert(&pool, "port_added", game, Some(published), &json!({}), None).await?;
        activity::insert(&pool, "port_added", game, Some(hidden), &json!({}), None).await?;
        activity::insert(&pool, "starred", game, Some(published), &json!({}), None).await?;

        let announcements = due(&pool, 0, "https://recomphub.com").await?;
        assert_eq!(announcements.len(), 1);
        assert_eq!(announcements[0].title, "New port: port-a for game-a");
        assert_eq!(
            announcements[0].url,
            "https://recomphub.com/games/game-a/port-a"
        );
        Ok(())
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn first_run_skips_existing_activity_and_only_announces_what_comes_after(
        pool: PgPool,
    ) -> Result<()> {
        let game = make_game(&pool, 2, "game-b").await?;
        let port = make_port(&pool, game, "port-c", "published").await?;
        activity::insert(&pool, "port_added", game, Some(port), &json!({}), None).await?;

        // Never run before: the baseline swallows history already in the table.
        let baseline = last_posted_id(&pool).await?;
        assert_eq!(due(&pool, baseline, "https://recomphub.com").await?.len(), 0);

        activity::insert(
            &pool,
            "release",
            game,
            Some(port),
            &json!({"version": "v2.0"}),
            None,
        )
        .await?;
        let announcements = due(&pool, baseline, "https://recomphub.com").await?;
        assert_eq!(announcements.len(), 1);
        assert_eq!(announcements[0].title, "port-c v2.0 released");
        Ok(())
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn watermark_advances_only_past_successfully_posted_rows(pool: PgPool) -> Result<()> {
        let game = make_game(&pool, 3, "game-c").await?;
        let port = make_port(&pool, game, "port-d", "published").await?;
        activity::insert(&pool, "port_added", game, Some(port), &json!({}), None).await?;

        let baseline = last_posted_id(&pool).await?;
        let announcements = due(&pool, baseline - 1, "https://recomphub.com").await?;
        assert_eq!(announcements.len(), 1);

        set_last_posted_id(&pool, announcements[0].activity_id).await?;
        assert_eq!(last_posted_id(&pool).await?, announcements[0].activity_id);
        assert_eq!(
            due(&pool, announcements[0].activity_id, "https://recomphub.com")
                .await?
                .len(),
            0
        );
        Ok(())
    }
}
