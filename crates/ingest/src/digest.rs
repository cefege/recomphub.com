//! Admin digest: real member activity since the last time anyone checked — new comments,
//! ratings, submissions and reports. Read-only, no scheduled job runs it; the worker's `digest`
//! command is meant to be run on request ("anything new?") and prints a plain list with enough
//! to act on: who, what, a link.
//!
//! "Since" is tracked in `settings` under [`LAST_RUN_KEY`], separate from the admin-editable
//! integer settings in `recomphub_db::settings`, so asking twice in a row never repeats items.

use anyhow::Result;
use chrono::{DateTime, Utc};
use recomphub_db::PgPool;
use serde::Serialize;

/// Where the last run's timestamp is stored. Not one of `settings::KNOWN_KEYS`: it is not
/// admin-editable, just a marker.
const LAST_RUN_KEY: &str = "digest_last_run_at";
/// Used the first time the digest ever runs, when there is no stored marker yet.
const DEFAULT_LOOKBACK_HOURS: i64 = 24;

#[derive(Debug, Clone, Serialize)]
pub struct DigestComment {
    pub id: i64,
    pub author: String,
    pub email: Option<String>,
    pub body: String,
    pub game_name: String,
    pub game_slug: String,
    pub port_slug: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DigestRating {
    pub author: String,
    pub email: Option<String>,
    pub stars: i16,
    pub game_name: String,
    pub game_slug: String,
    pub port_slug: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DigestSubmission {
    pub id: i64,
    pub kind: String,
    pub status: String,
    pub author: String,
    pub email: Option<String>,
    /// The port or game name, or the start of a report's text.
    pub subject: String,
    pub repo_url: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Digest {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub comments: Vec<DigestComment>,
    pub ratings: Vec<DigestRating>,
    /// Submissions still waiting for review, regardless of when they were created: a pending
    /// item from before `since` is still worth seeing if nobody has looked at it yet.
    pub pending_submissions: Vec<DigestSubmission>,
    /// Submissions a moderator (not necessarily the one asking) decided on since `since`.
    pub reviewed_submissions: Vec<DigestSubmission>,
}

impl Digest {
    pub fn is_empty(&self) -> bool {
        self.comments.is_empty()
            && self.ratings.is_empty()
            && self.pending_submissions.is_empty()
            && self.reviewed_submissions.is_empty()
    }
}

/// The marker's current value, or `now - DEFAULT_LOOKBACK_HOURS` when the digest has never run.
async fn last_run(pool: &PgPool) -> Result<DateTime<Utc>> {
    let value: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = $1")
            .bind(LAST_RUN_KEY)
            .fetch_optional(pool)
            .await?;
    Ok(value
        .and_then(|v| v.as_str().map(str::to_string))
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now() - chrono::Duration::hours(DEFAULT_LOOKBACK_HOURS)))
}

async fn set_last_run(pool: &PgPool, at: DateTime<Utc>) -> Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(LAST_RUN_KEY)
    .bind(serde_json::Value::String(at.to_rfc3339()))
    .execute(pool)
    .await?;
    Ok(())
}

/// Builds the digest for everything since the last run (or the last 24 hours, the first time),
/// then moves the marker to `now`. Passing `since` (a manual lookback window) builds the same
/// report without moving the marker, so a one-off wider look does not make the next normal
/// check skip anything.
pub async fn run(pool: &PgPool, since: Option<DateTime<Utc>>) -> Result<Digest> {
    let advance = since.is_none();
    let since = match since {
        Some(s) => s,
        None => last_run(pool).await?,
    };
    let until = Utc::now();

    let comments = sqlx::query_as::<
        _,
        (
            i64,
            Option<String>,
            Option<String>,
            String,
            String,
            String,
            String,
            DateTime<Utc>,
        ),
    >(
        "SELECT c.id, u.display_name, i.email, c.body, g.name, g.slug, p.slug, c.created_at
         FROM comments c
         JOIN ports p ON p.id = c.port_id
         JOIN games g ON g.id = p.game_id
         LEFT JOIN users u ON u.id = c.user_id
         LEFT JOIN user_identities i ON i.user_id = c.user_id
         WHERE c.created_at > $1 AND NOT c.hidden
         ORDER BY c.created_at",
    )
    .bind(since)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(
        |(id, author, email, body, game_name, game_slug, port_slug, created_at)| DigestComment {
            id,
            author: author.unwrap_or_else(|| "Member".to_string()),
            email,
            body,
            game_name,
            game_slug,
            port_slug,
            created_at,
        },
    )
    .collect();

    let ratings = sqlx::query_as::<
        _,
        (
            Option<String>,
            Option<String>,
            i16,
            String,
            String,
            String,
            DateTime<Utc>,
        ),
    >(
        "SELECT u.display_name, i.email, r.stars, g.name, g.slug, p.slug, r.created_at
         FROM port_ratings r
         JOIN ports p ON p.id = r.port_id
         JOIN games g ON g.id = p.game_id
         LEFT JOIN users u ON u.id = r.user_id
         LEFT JOIN user_identities i ON i.user_id = r.user_id
         WHERE r.created_at > $1
         ORDER BY r.created_at",
    )
    .bind(since)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(
        |(author, email, stars, game_name, game_slug, port_slug, created_at)| DigestRating {
            author: author.unwrap_or_else(|| "Member".to_string()),
            email,
            stars,
            game_name,
            game_slug,
            port_slug,
            created_at,
        },
    )
    .collect();

    let pending_submissions = submission_rows(
        pool,
        "SELECT s.id, s.kind, s.status, u.display_name, i.email, s.payload, s.target_port_id, s.created_at
         FROM submissions s
         LEFT JOIN users u ON u.id = s.submitter_id
         LEFT JOIN user_identities i ON i.user_id = s.submitter_id
         WHERE s.status = 'pending' AND s.submitter_id IS NOT NULL
         ORDER BY s.created_at",
        since,
        false,
    )
    .await?;

    let reviewed_submissions = submission_rows(
        pool,
        "SELECT s.id, s.kind, s.status, u.display_name, i.email, s.payload, s.target_port_id, s.reviewed_at
         FROM submissions s
         LEFT JOIN users u ON u.id = s.submitter_id
         LEFT JOIN user_identities i ON i.user_id = s.submitter_id
         WHERE s.reviewed_at > $1 AND s.submitter_id IS NOT NULL
           AND s.status IN ('approved', 'auto_approved', 'rejected', 'reverted')
         ORDER BY s.reviewed_at",
        since,
        true,
    )
    .await?;

    if advance {
        set_last_run(pool, until).await?;
    }
    Ok(Digest {
        since,
        until,
        comments,
        ratings,
        pending_submissions,
        reviewed_submissions,
    })
}

/// Shared row-to-`DigestSubmission` mapping for the pending and reviewed queries, which only
/// differ in their WHERE clause and whether `since` is bound.
async fn submission_rows(
    pool: &PgPool,
    sql: &str,
    since: DateTime<Utc>,
    bind_since: bool,
) -> Result<Vec<DigestSubmission>> {
    let query = sqlx::query_as::<
        _,
        (
            i64,
            String,
            String,
            Option<String>,
            Option<String>,
            serde_json::Value,
            Option<i64>,
            DateTime<Utc>,
        ),
    >(sql);
    let rows = if bind_since {
        query.bind(since).fetch_all(pool).await?
    } else {
        query.fetch_all(pool).await?
    };
    let mut out = Vec::with_capacity(rows.len());
    for (id, kind, status, author, email, payload, target_port_id, created_at) in rows {
        let port: Option<(String, String)> = match target_port_id {
            Some(port_id) => {
                sqlx::query_as("SELECT name, repo_url FROM ports WHERE id = $1")
                    .bind(port_id)
                    .fetch_optional(pool)
                    .await?
            }
            None => None,
        };
        let repo_url = payload
            .get("repo_url")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| port.as_ref().map(|(_, url)| url.clone()));
        let subject = payload
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| port.as_ref().map(|(name, _)| format!("Edit to {name}")))
            .or_else(|| {
                payload
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .map(|t| t.trim().chars().take(80).collect())
            })
            .unwrap_or_else(|| format!("{kind} #{id}"));
        out.push(DigestSubmission {
            id,
            kind,
            status,
            author: author.unwrap_or_else(|| "Member".to_string()),
            email,
            subject,
            repo_url,
            created_at,
        });
    }
    Ok(out)
}
