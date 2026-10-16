//! Port descriptions: the repository snapshot that github-sync stores each weekly scan, and the
//! generated description built from it. The generation itself lives in `recomphub-ingest`.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgExecutor};

use crate::DbResult;

/// Repository text stored for one port. Written by github-sync; read by describe-ports.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct SourceSnapshot {
    pub repo_description: Option<String>,
    pub topics: Vec<String>,
    pub readme: Option<String>,
    pub release_name: Option<String>,
    pub release_notes: Option<String>,
    pub content_hash: String,
}

/// Replaces the stored snapshot. Does not touch the generation bookkeeping.
pub async fn upsert_sources(
    ex: impl PgExecutor<'_>,
    port_id: i64,
    s: &SourceSnapshot,
) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO port_description_sources
             (port_id, repo_description, topics, readme, release_name, release_notes, content_hash, fetched_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now())
         ON CONFLICT (port_id) DO UPDATE SET
             repo_description = EXCLUDED.repo_description,
             topics = EXCLUDED.topics,
             readme = EXCLUDED.readme,
             release_name = EXCLUDED.release_name,
             release_notes = EXCLUDED.release_notes,
             content_hash = EXCLUDED.content_hash,
             fetched_at = EXCLUDED.fetched_at",
    )
    .bind(port_id)
    .bind(&s.repo_description)
    .bind(&s.topics)
    .bind(&s.readme)
    .bind(&s.release_name)
    .bind(&s.release_notes)
    .bind(&s.content_hash)
    .execute(ex)
    .await?;
    Ok(())
}

/// Snapshot stored for one port, if github-sync has scanned it.
pub async fn get_sources(
    ex: impl PgExecutor<'_>,
    port_id: i64,
) -> DbResult<Option<SourceSnapshot>> {
    Ok(sqlx::query_as::<_, SourceSnapshot>(
        "SELECT repo_description, topics, readme, release_name, release_notes, content_hash
         FROM port_description_sources WHERE port_id = $1",
    )
    .bind(port_id)
    .fetch_optional(ex)
    .await?)
}

/// A published port with a stored snapshot, plus the facts its description is built from.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct Candidate {
    pub port_id: i64,
    pub content_hash: String,
    pub port_name: String,
    pub kind: String,
    pub game_name: String,
    pub game_targets: Vec<String>,
    pub platform_name: Option<String>,
    pub source_hash: Option<String>,
    pub describe_attempted_at: Option<DateTime<Utc>>,
    pub homepage_url: Option<String>,
    pub license: Option<String>,
}

/// Every published port with a stored snapshot. Least recently attempted first, so ports that
/// failed or were just regenerated go to the back of the queue.
pub async fn candidates(ex: impl PgExecutor<'_>) -> DbResult<Vec<Candidate>> {
    Ok(sqlx::query_as::<_, Candidate>(
        "SELECT s.port_id, s.content_hash, p.name AS port_name, p.kind,
                g.name AS game_name, g.targets AS game_targets,
                pl.name AS platform_name, d.source_hash, s.describe_attempted_at,
                p.homepage_url, p.license
         FROM port_description_sources s
         JOIN ports p ON p.id = s.port_id
         JOIN games g ON g.id = p.game_id
         LEFT JOIN platforms pl ON pl.id = p.source_platform_id
         LEFT JOIN port_descriptions d ON d.port_id = s.port_id
         WHERE p.status = 'published'
         ORDER BY s.describe_attempted_at ASC NULLS FIRST, s.port_id",
    )
    .fetch_all(ex)
    .await?)
}

/// Records that generation was attempted for a port. `error` is `None` on success.
pub async fn record_attempt(
    ex: impl PgExecutor<'_>,
    port_id: i64,
    error: Option<&str>,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE port_description_sources
         SET describe_attempted_at = now(), describe_error = $2
         WHERE port_id = $1",
    )
    .bind(port_id)
    .bind(error)
    .execute(ex)
    .await?;
    Ok(())
}

/// A generated description, ready to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPortDescription {
    pub port_id: i64,
    pub summary: String,
    pub technique: String,
    pub technique_note: String,
    pub features: Vec<String>,
    pub requirements: Vec<String>,
    pub limitations: Vec<String>,
    pub setup: Vec<String>,
    pub model: String,
    pub provider: String,
    pub source_hash: String,
}

/// Stored description for a port.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct PortDescriptionRow {
    pub port_id: i64,
    pub summary: String,
    pub technique: String,
    pub technique_note: String,
    pub features: Vec<String>,
    pub requirements: Vec<String>,
    pub limitations: Vec<String>,
    pub setup: Vec<String>,
    pub model: String,
    pub provider: Option<String>,
    pub source_hash: String,
    pub generated_at: DateTime<Utc>,
}

/// Inserts or replaces the description for a port.
pub async fn save(ex: impl PgExecutor<'_>, d: &NewPortDescription) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO port_descriptions
             (port_id, summary, technique, technique_note, features, requirements, limitations,
              setup, model, provider, source_hash, generated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, now())
         ON CONFLICT (port_id) DO UPDATE SET
             summary = EXCLUDED.summary,
             technique = EXCLUDED.technique,
             technique_note = EXCLUDED.technique_note,
             features = EXCLUDED.features,
             requirements = EXCLUDED.requirements,
             limitations = EXCLUDED.limitations,
             setup = EXCLUDED.setup,
             model = EXCLUDED.model,
             provider = EXCLUDED.provider,
             source_hash = EXCLUDED.source_hash,
             generated_at = EXCLUDED.generated_at",
    )
    .bind(d.port_id)
    .bind(&d.summary)
    .bind(&d.technique)
    .bind(&d.technique_note)
    .bind(&d.features)
    .bind(&d.requirements)
    .bind(&d.limitations)
    .bind(&d.setup)
    .bind(&d.model)
    .bind(&d.provider)
    .bind(&d.source_hash)
    .execute(ex)
    .await?;
    Ok(())
}

/// Stored description for a port, if one has been generated.
pub async fn get(ex: impl PgExecutor<'_>, port_id: i64) -> DbResult<Option<PortDescriptionRow>> {
    Ok(sqlx::query_as::<_, PortDescriptionRow>(
        "SELECT port_id, summary, technique, technique_note, features, requirements, limitations,
                setup, model, provider, source_hash, generated_at
         FROM port_descriptions WHERE port_id = $1",
    )
    .bind(port_id)
    .fetch_optional(ex)
    .await?)
}
