//! Ports: CRUD, authors, GitHub and AI sync, import fill, and listings.
//!
//! Field ownership: automated jobs never write a field listed in `locked_fields`.
//! Sync jobs write only GitHub-derived columns, and AI columns only when `ai_source` is not `declared`.

use crate::{DbError, DbResult, activity, games};
use chrono::{DateTime, Utc};
use recomphub_shared::{
    AiSource, AiUsage, MaintenanceLevel, Playability, PortKind, TargetPlatform, slugify,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, PgExecutor};

/// One AI detection signal, shown as evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiEvidence {
    pub signal: String,
    pub detail: String,
}

/// The editable fields of a port. Enum-valued fields are stored as their `as_str()` values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PortInput {
    pub name: String,
    pub slug: Option<String>,
    pub kind: String,
    pub source_platform_id: Option<i32>,
    pub repo_url: String,
    pub homepage_url: Option<String>,
    pub description: Option<String>,
    pub import_description: Option<String>,
    pub requirements: Option<String>,
    pub toolchain: Option<String>,
    pub license: Option<String>,
    pub playability: String,
    pub declared_platforms: Vec<String>,
    pub detected_platforms: Vec<String>,
    pub ai_usage: String,
    pub ai_tools: Vec<String>,
    pub ai_source: Option<String>,
    pub ai_evidence: Vec<AiEvidence>,
}

impl Default for PortInput {
    fn default() -> Self {
        PortInput {
            name: String::new(),
            slug: None,
            kind: String::new(),
            source_platform_id: None,
            repo_url: String::new(),
            homepage_url: None,
            description: None,
            import_description: None,
            requirements: None,
            toolchain: None,
            license: None,
            playability: "unknown".to_string(),
            declared_platforms: Vec::new(),
            detected_platforms: Vec::new(),
            ai_usage: "unknown".to_string(),
            ai_tools: Vec::new(),
            ai_source: None,
            ai_evidence: Vec::new(),
        }
    }
}

/// One author of a port.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorInput {
    pub github_login: Option<String>,
    pub name: String,
    pub avatar_media_id: Option<i64>,
    /// `owner`, `contributor` or `credited`.
    pub role: String,
    pub commits: Option<i32>,
}

/// A port with its authors. Used for creating, editing and reverting ports.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PortFull {
    #[serde(flatten)]
    pub input: PortInput,
    pub authors: Vec<AuthorInput>,
}

/// A row of the `ports` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PortRow {
    pub id: i64,
    pub game_id: i64,
    pub slug: String,
    pub name: String,
    pub kind: String,
    pub source_platform_id: Option<i32>,
    pub repo_url: String,
    pub github_repo_id: Option<i64>,
    pub homepage_url: Option<String>,
    pub description: Option<String>,
    pub import_description: Option<String>,
    pub requirements: Option<String>,
    pub toolchain: Option<String>,
    pub license: Option<String>,
    pub playability: String,
    pub latest_version: Option<String>,
    pub latest_release_url: Option<String>,
    pub latest_release_at: Option<DateTime<Utc>>,
    pub latest_is_prerelease: Option<bool>,
    pub pushed_at: Option<DateTime<Utc>>,
    pub archived: bool,
    pub github_stars: i32,
    pub github_forks: i32,
    pub declared_platforms: Vec<String>,
    pub detected_platforms: Vec<String>,
    pub ai_usage: String,
    pub ai_tools: Vec<String>,
    pub ai_source: Option<String>,
    pub ai_evidence: serde_json::Value,
    pub star_count: i32,
    pub status: String,
    pub sources: Vec<String>,
    pub locked_fields: Vec<String>,
    pub github_synced_at: Option<DateTime<Utc>>,
    pub ai_scanned_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_commit_at: Option<DateTime<Utc>>,
    pub commits_30d: i32,
    pub commits_90d: i32,
    pub commits_365d: i32,
    pub committers_180d: i32,
    pub releases_365d: i32,
    pub weekly_commits: Vec<i32>,
    pub maintenance_level: Option<String>,
    pub maintenance_score: Option<i16>,
    pub maintenance_computed_at: Option<DateTime<Utc>>,
}

impl PortRow {
    /// Effective platforms: `declared_platforms` if non-empty, otherwise `detected_platforms`.
    /// Unknown stored values are skipped.
    pub fn platforms(&self) -> Vec<TargetPlatform> {
        let list = if self.declared_platforms.is_empty() {
            &self.detected_platforms
        } else {
            &self.declared_platforms
        };
        list.iter()
            .filter_map(|s| s.parse::<TargetPlatform>().ok())
            .collect()
    }

    /// The stored AI evidence.
    pub fn evidence(&self) -> DbResult<Vec<AiEvidence>> {
        Ok(serde_json::from_value(self.ai_evidence.clone())?)
    }

    /// The editable fields of this port.
    pub fn to_input(&self) -> DbResult<PortInput> {
        Ok(PortInput {
            name: self.name.clone(),
            slug: Some(self.slug.clone()),
            kind: self.kind.clone(),
            source_platform_id: self.source_platform_id,
            repo_url: self.repo_url.clone(),
            homepage_url: self.homepage_url.clone(),
            description: self.description.clone(),
            import_description: self.import_description.clone(),
            requirements: self.requirements.clone(),
            toolchain: self.toolchain.clone(),
            license: self.license.clone(),
            playability: self.playability.clone(),
            declared_platforms: self.declared_platforms.clone(),
            detected_platforms: self.detected_platforms.clone(),
            ai_usage: self.ai_usage.clone(),
            ai_tools: self.ai_tools.clone(),
            ai_source: self.ai_source.clone(),
            ai_evidence: self.evidence()?,
        })
    }
}

/// A row of the `port_authors` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AuthorRow {
    pub port_id: i64,
    pub position: i32,
    pub github_login: Option<String>,
    pub name: String,
    pub avatar_media_id: Option<i64>,
    pub role: String,
    pub commits: Option<i32>,
}

impl AuthorRow {
    /// The editable fields of this author.
    pub fn to_input(&self) -> AuthorInput {
        AuthorInput {
            github_login: self.github_login.clone(),
            name: self.name.clone(),
            avatar_media_id: self.avatar_media_id,
            role: self.role.clone(),
            commits: self.commits,
        }
    }
}

/// A port with the fields needed for a card: its game's slug, name and cover.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PortWithGame {
    #[sqlx(flatten)]
    pub port: PortRow,
    pub game_slug: String,
    pub game_name: String,
    pub game_cover_media_id: Option<i64>,
}

const PORT_CARD_SQL: &str = "SELECT p.*, g.slug AS game_slug, g.name AS game_name,
        g.cover_media_id AS game_cover_media_id
    FROM ports p JOIN games g ON g.id = p.game_id";

/// Canonical GitHub repo URL: `https://github.com/{owner}/{repo}`, lowercased, without `.git`.
pub fn normalize_repo_url(input: &str) -> DbResult<String> {
    let s = input.trim();
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let mut parts = rest.trim_end_matches('/').split('/');
    let host = parts.next().unwrap_or("");
    let owner = parts.next().unwrap_or("");
    let repo = parts.next().unwrap_or("");
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if !host.eq_ignore_ascii_case("github.com") || owner.is_empty() || repo.is_empty() {
        return Err(DbError::InvalidPayload(format!(
            "not a GitHub repository URL: {input}"
        )));
    }
    Ok(format!(
        "https://github.com/{}/{}",
        owner.to_lowercase(),
        repo.to_lowercase()
    ))
}

/// Checks enum values, required fields and length limits of a port.
pub fn validate_input(input: &PortInput) -> DbResult<()> {
    if input.name.trim().is_empty() {
        return Err(DbError::InvalidPayload("name is required".to_string()));
    }
    input.kind.parse::<PortKind>()?;
    input.playability.parse::<Playability>()?;
    input.ai_usage.parse::<AiUsage>()?;
    if let Some(src) = &input.ai_source {
        src.parse::<AiSource>()?;
    }
    for p in input
        .declared_platforms
        .iter()
        .chain(input.detected_platforms.iter())
    {
        p.parse::<TargetPlatform>()?;
    }
    normalize_repo_url(&input.repo_url)?;
    if input
        .description
        .as_deref()
        .is_some_and(|d| d.chars().count() > 2000)
    {
        return Err(DbError::InvalidPayload(
            "description is longer than 2000 characters".to_string(),
        ));
    }
    if input
        .requirements
        .as_deref()
        .is_some_and(|r| r.chars().count() > 500)
    {
        return Err(DbError::InvalidPayload(
            "requirements are longer than 500 characters".to_string(),
        ));
    }
    Ok(())
}

async fn unique_slug(conn: &mut PgConnection, game_id: i64, base: &str) -> DbResult<String> {
    let mut base = slugify(base);
    if base.is_empty() {
        base = "port".to_string();
    }
    let mut candidate = base.clone();
    let mut n = 2;
    loop {
        let taken = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM ports WHERE game_id = $1 AND slug = $2)",
        )
        .bind(game_id)
        .bind(candidate.clone())
        .fetch_one(&mut *conn)
        .await?;
        if !taken {
            return Ok(candidate);
        }
        candidate = format!("{base}-{n}");
        n += 1;
    }
}

/// Inserts a published port with its authors and refreshes the game's rollup.
/// Fails with `InvalidState` when the repo is already listed.
pub async fn insert_port(
    conn: &mut PgConnection,
    game_id: i64,
    full: &PortFull,
    sources: &[String],
) -> DbResult<i64> {
    validate_input(&full.input)?;
    let repo = normalize_repo_url(&full.input.repo_url)?;
    if find_by_repo_url(&mut *conn, &repo).await?.is_some() {
        return Err(DbError::InvalidState(format!("{repo} is already listed")));
    }
    let base = full
        .input
        .slug
        .as_deref()
        .unwrap_or(full.input.name.as_str());
    let slug = unique_slug(&mut *conn, game_id, base).await?;
    let i = &full.input;
    let port_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO ports (game_id, slug, name, kind, source_platform_id, repo_url, homepage_url,
            description, import_description, requirements, toolchain, license, playability,
            declared_platforms, detected_platforms, ai_usage, ai_tools, ai_source, ai_evidence,
            sources, status)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18,
                $19, $20, 'published')
         RETURNING id",
    )
    .bind(game_id)
    .bind(slug)
    .bind(i.name.trim().to_string())
    .bind(i.kind.clone())
    .bind(i.source_platform_id)
    .bind(repo)
    .bind(i.homepage_url.clone())
    .bind(i.description.clone())
    .bind(i.import_description.clone())
    .bind(i.requirements.clone())
    .bind(i.toolchain.clone())
    .bind(i.license.clone())
    .bind(i.playability.clone())
    .bind(i.declared_platforms.clone())
    .bind(i.detected_platforms.clone())
    .bind(i.ai_usage.clone())
    .bind(i.ai_tools.clone())
    .bind(i.ai_source.clone())
    .bind(serde_json::to_value(&i.ai_evidence)?)
    .bind(sources.to_vec())
    .fetch_one(&mut *conn)
    .await?;
    replace_authors(&mut *conn, port_id, &full.authors).await?;
    games::refresh_rollup(&mut *conn, game_id).await?;
    Ok(port_id)
}

/// Overwrites every editable field of a port and replaces its authors.
/// `slug` is kept when `input.slug` is `None`.
pub async fn update_port(conn: &mut PgConnection, port_id: i64, full: &PortFull) -> DbResult<()> {
    validate_input(&full.input)?;
    let repo = normalize_repo_url(&full.input.repo_url)?;
    if find_by_repo_url(&mut *conn, &repo)
        .await?
        .is_some_and(|other| other.id != port_id)
    {
        return Err(DbError::InvalidState(format!("{repo} is already listed")));
    }
    let i = &full.input;
    let game_id = sqlx::query_scalar::<_, i64>(
        "UPDATE ports SET slug = COALESCE($2::text, slug), name = $3, kind = $4,
            source_platform_id = $5, repo_url = $6, homepage_url = $7, description = $8,
            import_description = $9, requirements = $10, toolchain = $11, license = $12,
            playability = $13, declared_platforms = $14, detected_platforms = $15,
            ai_usage = $16, ai_tools = $17, ai_source = $18, ai_evidence = $19, updated_at = now()
         WHERE id = $1
         RETURNING game_id",
    )
    .bind(port_id)
    .bind(i.slug.clone())
    .bind(i.name.trim().to_string())
    .bind(i.kind.clone())
    .bind(i.source_platform_id)
    .bind(repo)
    .bind(i.homepage_url.clone())
    .bind(i.description.clone())
    .bind(i.import_description.clone())
    .bind(i.requirements.clone())
    .bind(i.toolchain.clone())
    .bind(i.license.clone())
    .bind(i.playability.clone())
    .bind(i.declared_platforms.clone())
    .bind(i.detected_platforms.clone())
    .bind(i.ai_usage.clone())
    .bind(i.ai_tools.clone())
    .bind(i.ai_source.clone())
    .bind(serde_json::to_value(&i.ai_evidence)?)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("port {port_id}")))?;
    replace_authors(&mut *conn, port_id, &full.authors).await?;
    games::refresh_rollup(&mut *conn, game_id).await?;
    Ok(())
}

/// A port row by id. Returns `NotFound` when missing.
pub async fn get_row(ex: impl PgExecutor<'_>, id: i64) -> DbResult<PortRow> {
    sqlx::query_as::<_, PortRow>("SELECT * FROM ports WHERE id = $1")
        .bind(id)
        .fetch_optional(ex)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("port {id}")))
}

/// A port and its authors as editable input.
pub async fn load_full(conn: &mut PgConnection, id: i64) -> DbResult<PortFull> {
    let row = get_row(&mut *conn, id).await?;
    let authors = authors(&mut *conn, id).await?;
    Ok(PortFull {
        input: row.to_input()?,
        authors: authors.iter().map(AuthorRow::to_input).collect(),
    })
}

/// A published port by game slug and port slug.
pub async fn find_published(
    ex: impl PgExecutor<'_>,
    game_slug: &str,
    port_slug: &str,
) -> DbResult<Option<PortWithGame>> {
    let sql =
        format!("{PORT_CARD_SQL} WHERE g.slug = $1 AND p.slug = $2 AND p.status = 'published'");
    let row = sqlx::query_as::<_, PortWithGame>(&sql)
        .bind(game_slug)
        .bind(port_slug)
        .fetch_optional(ex)
        .await?;
    Ok(row)
}

/// A port by repo URL. The URL is normalized first, so any GitHub spelling works.
pub async fn find_by_repo_url(
    ex: impl PgExecutor<'_>,
    repo_url: &str,
) -> DbResult<Option<PortRow>> {
    let repo = normalize_repo_url(repo_url)?;
    let row = sqlx::query_as::<_, PortRow>("SELECT * FROM ports WHERE repo_url = $1")
        .bind(repo)
        .fetch_optional(ex)
        .await?;
    Ok(row)
}

/// Another port that already owns this GitHub repository, by id or by canonical URL.
pub async fn other_port_with_repo(
    ex: impl PgExecutor<'_>,
    port_id: i64,
    github_repo_id: i64,
    repo_url: &str,
) -> DbResult<Option<i64>> {
    let id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM ports WHERE id <> $1 AND (github_repo_id = $2 OR repo_url = $3) LIMIT 1",
    )
    .bind(port_id)
    .bind(github_repo_id)
    .bind(repo_url)
    .fetch_optional(ex)
    .await?;
    Ok(id)
}

/// Authors of a port in position order.
pub async fn authors(ex: impl PgExecutor<'_>, port_id: i64) -> DbResult<Vec<AuthorRow>> {
    let rows = sqlx::query_as::<_, AuthorRow>(
        "SELECT * FROM port_authors WHERE port_id = $1 ORDER BY position",
    )
    .bind(port_id)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Published, non-decompilation ports of a game: complete first, then by GitHub stars.
pub async fn list_for_game(ex: impl PgExecutor<'_>, game_id: i64) -> DbResult<Vec<PortRow>> {
    let rows = sqlx::query_as::<_, PortRow>(
        "SELECT * FROM ports
         WHERE game_id = $1 AND status = 'published' AND kind <> 'decompilation'
         ORDER BY CASE playability WHEN 'complete' THEN 0 WHEN 'playable' THEN 1
                  WHEN 'in_progress' THEN 2 ELSE 3 END,
                  github_stars DESC, id",
    )
    .bind(game_id)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Published decompilation projects of a game.
pub async fn list_decompilations(ex: impl PgExecutor<'_>, game_id: i64) -> DbResult<Vec<PortRow>> {
    let rows = sqlx::query_as::<_, PortRow>(
        "SELECT * FROM ports
         WHERE game_id = $1 AND status = 'published' AND kind = 'decompilation'
         ORDER BY github_stars DESC, id",
    )
    .bind(game_id)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Published ports that run on `target`, optionally only for games from one original platform.
/// Ordered by GitHub stars.
pub async fn list_by_target(
    ex: impl PgExecutor<'_>,
    target: &str,
    platform_id: Option<i32>,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<PortWithGame>> {
    let sql = format!(
        "{PORT_CARD_SQL}
         WHERE p.status = 'published' AND p.kind <> 'decompilation'
           AND $1::text = ANY(port_platforms(p))
           AND ($2::int IS NULL OR EXISTS (
                SELECT 1 FROM game_platforms gp WHERE gp.game_id = g.id AND gp.platform_id = $2::int))
         ORDER BY p.github_stars DESC, p.id
         LIMIT $3 OFFSET $4"
    );
    let rows = sqlx::query_as::<_, PortWithGame>(&sql)
        .bind(target)
        .bind(platform_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(ex)
        .await?;
    Ok(rows)
}

/// Number of ports counted by `list_by_target`.
pub async fn count_by_target(
    ex: impl PgExecutor<'_>,
    target: &str,
    platform_id: Option<i32>,
) -> DbResult<i64> {
    let n = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM ports p JOIN games g ON g.id = p.game_id
         WHERE p.status = 'published' AND p.kind <> 'decompilation'
           AND $1::text = ANY(port_platforms(p))
           AND ($2::int IS NULL OR EXISTS (
                SELECT 1 FROM game_platforms gp WHERE gp.game_id = g.id AND gp.platform_id = $2::int))",
    )
    .bind(target)
    .bind(platform_id)
    .fetch_one(ex)
    .await?;
    Ok(n)
}

/// Each runs-on target with the number of published ports that run on it.
pub async fn runs_on_counts(ex: impl PgExecutor<'_>) -> DbResult<Vec<(String, i64)>> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT u.t, count(*)::int8
         FROM ports p CROSS JOIN LATERAL unnest(port_platforms(p)) AS u(t)
         WHERE p.status = 'published' AND p.kind <> 'decompilation'
         GROUP BY u.t ORDER BY u.t",
    )
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Published ports by the later of their creation and latest release.
pub async fn just_released(
    ex: impl PgExecutor<'_>,
    limit: i64,
    offset: i64,
) -> DbResult<Vec<PortWithGame>> {
    let sql = format!(
        "{PORT_CARD_SQL}
         WHERE p.status = 'published' AND p.kind <> 'decompilation'
         ORDER BY greatest(p.created_at, p.latest_release_at) DESC, p.id DESC
         LIMIT $1 OFFSET $2"
    );
    let rows = sqlx::query_as::<_, PortWithGame>(&sql)
        .bind(limit)
        .bind(offset)
        .fetch_all(ex)
        .await?;
    Ok(rows)
}

/// Published ports whose name is similar to `q`, or whose name or repo contains it.
pub async fn search_ports(
    ex: impl PgExecutor<'_>,
    q: &str,
    limit: i64,
) -> DbResult<Vec<PortWithGame>> {
    let pattern = format!("%{}%", q.replace('%', "\\%").replace('_', "\\_"));
    let sql = format!(
        "{PORT_CARD_SQL}
         WHERE p.status = 'published'
           AND (similarity(p.name, $1) > 0.3 OR p.name ILIKE $2 OR p.repo_url ILIKE $2)
         ORDER BY similarity(p.name, $1) DESC, p.github_stars DESC, p.id
         LIMIT $3"
    );
    let rows = sqlx::query_as::<_, PortWithGame>(&sql)
        .bind(q)
        .bind(pattern)
        .bind(limit)
        .fetch_all(ex)
        .await?;
    Ok(rows)
}

/// Published ports where `login` is an author (case-insensitive).
pub async fn by_author(ex: impl PgExecutor<'_>, login: &str) -> DbResult<Vec<PortWithGame>> {
    let sql = format!(
        "{PORT_CARD_SQL}
         WHERE p.status = 'published' AND EXISTS (
            SELECT 1 FROM port_authors a WHERE a.port_id = p.id AND lower(a.github_login) = lower($1))
         ORDER BY p.github_stars DESC, p.id"
    );
    let rows = sqlx::query_as::<_, PortWithGame>(&sql)
        .bind(login)
        .fetch_all(ex)
        .await?;
    Ok(rows)
}

/// Published ports due for a GitHub sync: never synced, or last synced more than 6 hours ago.
pub async fn due_github_sync(ex: impl PgExecutor<'_>, limit: i64) -> DbResult<Vec<PortRow>> {
    let rows = sqlx::query_as::<_, PortRow>(
        "SELECT * FROM ports
         WHERE status = 'published' AND (github_synced_at IS NULL OR github_synced_at < now() - interval '6 hours')
         ORDER BY github_synced_at NULLS FIRST, id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Published ports due for a weekly AI scan: never scanned, or last scanned more than 7 days ago.
pub async fn due_ai_scan(ex: impl PgExecutor<'_>, limit: i64) -> DbResult<Vec<PortRow>> {
    let rows = sqlx::query_as::<_, PortRow>(
        "SELECT * FROM ports
         WHERE status = 'published' AND (ai_scanned_at IS NULL OR ai_scanned_at < now() - interval '7 days')
         ORDER BY ai_scanned_at NULLS FIRST, id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(ex)
    .await?;
    Ok(rows)
}

/// Sets a port to `published` or `hidden`, refreshes its game's rollup, and returns the game id.
pub async fn set_status(conn: &mut PgConnection, port_id: i64, status: &str) -> DbResult<i64> {
    if status != "published" && status != "hidden" {
        return Err(DbError::InvalidPayload(format!(
            "invalid port status {status}"
        )));
    }
    let game_id = sqlx::query_scalar::<_, i64>(
        "UPDATE ports SET status = $2, updated_at = now() WHERE id = $1 RETURNING game_id",
    )
    .bind(port_id)
    .bind(status)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("port {port_id}")))?;
    games::refresh_rollup(&mut *conn, game_id).await?;
    Ok(game_id)
}

/// Replaces all authors of a port. Roles must be `owner`, `contributor` or `credited`.
pub async fn replace_authors(
    conn: &mut PgConnection,
    port_id: i64,
    authors: &[AuthorInput],
) -> DbResult<()> {
    for a in authors {
        if !matches!(a.role.as_str(), "owner" | "contributor" | "credited") {
            return Err(DbError::InvalidPayload(format!(
                "invalid author role {}",
                a.role
            )));
        }
    }
    sqlx::query("DELETE FROM port_authors WHERE port_id = $1")
        .bind(port_id)
        .execute(&mut *conn)
        .await?;
    for (position, a) in authors.iter().enumerate() {
        sqlx::query(
            "INSERT INTO port_authors (port_id, position, github_login, name, avatar_media_id, role, commits)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(port_id)
        .bind(position as i32)
        .bind(a.github_login.clone())
        .bind(a.name.clone())
        .bind(a.avatar_media_id)
        .bind(a.role.clone())
        .bind(a.commits)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// The locked field names of a port.
pub async fn locked_fields(ex: impl PgExecutor<'_>, port_id: i64) -> DbResult<Vec<String>> {
    sqlx::query_scalar::<_, Vec<String>>("SELECT locked_fields FROM ports WHERE id = $1")
        .bind(port_id)
        .fetch_optional(ex)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("port {port_id}")))
}

/// Authors from GitHub. Writes only when `authors` is unlocked and the port has no credited authors.
/// Returns whether the rows were written.
pub async fn replace_github_authors(
    conn: &mut PgConnection,
    port_id: i64,
    authors: &[AuthorInput],
) -> DbResult<bool> {
    if locked_fields(&mut *conn, port_id)
        .await?
        .iter()
        .any(|f| f == "authors")
    {
        return Ok(false);
    }
    let has_credited = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM port_authors WHERE port_id = $1 AND role = 'credited')",
    )
    .bind(port_id)
    .fetch_one(&mut *conn)
    .await?;
    if has_credited {
        return Ok(false);
    }
    replace_authors(conn, port_id, authors).await?;
    Ok(true)
}

/// Authors from an import. Writes only when `authors` is unlocked and the port has no authors.
/// Every row gets role `credited`. Returns whether the rows were written.
pub async fn set_credited_authors(
    conn: &mut PgConnection,
    port_id: i64,
    authors: &[AuthorInput],
) -> DbResult<bool> {
    if locked_fields(&mut *conn, port_id)
        .await?
        .iter()
        .any(|f| f == "authors")
    {
        return Ok(false);
    }
    let has_any = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM port_authors WHERE port_id = $1)",
    )
    .bind(port_id)
    .fetch_one(&mut *conn)
    .await?;
    if has_any {
        return Ok(false);
    }
    let credited: Vec<AuthorInput> = authors
        .iter()
        .map(|a| AuthorInput {
            role: "credited".to_string(),
            ..a.clone()
        })
        .collect();
    replace_authors(conn, port_id, &credited).await?;
    Ok(true)
}

/// GitHub metadata of one repo, as fetched by `github-sync`.
#[derive(Debug, Clone)]
pub struct GithubSync {
    pub github_repo_id: i64,
    pub repo_url: String,
    pub homepage_url: Option<String>,
    pub license: Option<String>,
    pub github_stars: i32,
    pub github_forks: i32,
    pub pushed_at: Option<DateTime<Utc>>,
    /// When the GitHub repository was created. `None` keeps the stored date.
    pub repo_created_at: Option<DateTime<Utc>>,
    pub archived: bool,
    pub latest_version: Option<String>,
    pub latest_release_url: Option<String>,
    pub latest_release_at: Option<DateTime<Utc>>,
    pub latest_is_prerelease: Option<bool>,
    pub detected_platforms: Vec<String>,
}

/// Result of an AI scan. `usage: None` means no conclusion (both human and AI signals), so the AI fields keep their values.
#[derive(Debug, Clone)]
pub struct AiSync {
    pub usage: Option<String>,
    pub tools: Vec<String>,
    pub evidence: Vec<AiEvidence>,
}

async fn lock_row(conn: &mut PgConnection, port_id: i64) -> DbResult<PortRow> {
    sqlx::query_as::<_, PortRow>("SELECT * FROM ports WHERE id = $1 FOR UPDATE")
        .bind(port_id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("port {port_id}")))
}

fn keep_or<T: Clone>(locked: bool, current: &T, new: &T) -> T {
    if locked { current.clone() } else { new.clone() }
}

fn keep_or_fill(locked: bool, current: &Option<String>, new: &Option<String>) -> Option<String> {
    if locked || new.is_none() {
        current.clone()
    } else {
        new.clone()
    }
}

/// Applies a GitHub sync to a port, and an AI scan when `ai` is `Some`.
/// Inserts a `release` activity when the latest version changes on a port that was synced before.
pub async fn update_github(
    conn: &mut PgConnection,
    port_id: i64,
    sync: &GithubSync,
    ai: Option<&AiSync>,
) -> DbResult<()> {
    let cur = lock_row(&mut *conn, port_id).await?;
    let locked = |f: &str| cur.locked_fields.iter().any(|x| x == f);
    let repo_url = keep_or(
        locked("repo_url"),
        &cur.repo_url,
        &normalize_repo_url(&sync.repo_url)?,
    );
    let homepage = keep_or_fill(
        locked("homepage_url"),
        &cur.homepage_url,
        &sync.homepage_url,
    );
    let license = keep_or_fill(locked("license"), &cur.license, &sync.license);
    let detected = keep_or(
        locked("detected_platforms"),
        &cur.detected_platforms,
        &sync.detected_platforms,
    );

    let is_new_release = sync
        .latest_version
        .as_ref()
        .filter(|v| cur.latest_version.as_deref() != Some(v.as_str()))
        .filter(|_| cur.github_synced_at.is_some());
    if let Some(v) = is_new_release {
        activity::insert(
            &mut *conn,
            "release",
            cur.game_id,
            Some(port_id),
            &json!({ "version": v }),
            None,
        )
        .await?;
    }

    let may_write_ai =
        cur.ai_source.as_deref() != Some("declared") && !locked("ai_usage") && !locked("ai_tools");
    let (ai_usage, ai_tools, ai_evidence, ai_source) = match ai {
        Some(AiSync {
            usage: Some(u),
            tools,
            evidence,
        }) if may_write_ai => (
            u.clone(),
            tools.clone(),
            serde_json::to_value(evidence)?,
            Some("detected".to_string()),
        ),
        _ => (
            cur.ai_usage.clone(),
            cur.ai_tools.clone(),
            cur.ai_evidence.clone(),
            cur.ai_source.clone(),
        ),
    };
    let stamp_ai = ai.is_some();

    sqlx::query(
        "UPDATE ports SET github_repo_id = $2, repo_url = $3, homepage_url = $4, license = $5,
            github_stars = $6, github_forks = $7, pushed_at = $8, archived = $9,
            latest_version = $10, latest_release_url = $11, latest_release_at = $12,
            latest_is_prerelease = $13, detected_platforms = $14,
            ai_usage = $15, ai_tools = $16, ai_evidence = $17, ai_source = $18,
            ai_scanned_at = CASE WHEN $19::bool THEN now() ELSE ai_scanned_at END,
            repo_created_at = COALESCE($20, repo_created_at),
            github_synced_at = now(), updated_at = now()
         WHERE id = $1",
    )
    .bind(port_id)
    .bind(sync.github_repo_id)
    .bind(repo_url)
    .bind(homepage)
    .bind(license)
    .bind(sync.github_stars)
    .bind(sync.github_forks)
    .bind(sync.pushed_at)
    .bind(sync.archived)
    .bind(sync.latest_version.clone())
    .bind(sync.latest_release_url.clone())
    .bind(sync.latest_release_at)
    .bind(sync.latest_is_prerelease)
    .bind(detected)
    .bind(ai_usage)
    .bind(ai_tools)
    .bind(ai_evidence)
    .bind(ai_source)
    .bind(stamp_ai)
    .bind(sync.repo_created_at)
    .execute(&mut *conn)
    .await?;
    games::refresh_rollup(&mut *conn, cur.game_id).await?;
    Ok(())
}

/// Maintenance signals computed by `github-sync`, written to the maintenance columns of a port.
#[derive(Debug, Clone)]
pub struct MaintenanceSync {
    pub last_commit_at: Option<DateTime<Utc>>,
    pub commits_30d: i32,
    pub commits_90d: i32,
    pub commits_365d: i32,
    pub committers_180d: i32,
    pub releases_365d: i32,
    /// `None` keeps the stored weeks, for when GitHub has not finished computing its stats.
    pub weekly_commits: Option<Vec<i32>>,
    pub level: MaintenanceLevel,
    pub score: u8,
}

/// Writes the maintenance columns of a port. Does not touch `github_synced_at` or the locks.
pub async fn update_maintenance(
    conn: &mut PgConnection,
    port_id: i64,
    maintenance: &MaintenanceSync,
) -> DbResult<()> {
    sqlx::query(
        "UPDATE ports SET
            last_commit_at = $2,
            commits_30d = $3,
            commits_90d = $4,
            commits_365d = $5,
            committers_180d = $6,
            releases_365d = $7,
            weekly_commits = COALESCE($8, weekly_commits),
            maintenance_level = $9,
            maintenance_score = $10,
            maintenance_computed_at = now()
         WHERE id = $1",
    )
    .bind(port_id)
    .bind(maintenance.last_commit_at)
    .bind(maintenance.commits_30d)
    .bind(maintenance.commits_90d)
    .bind(maintenance.commits_365d)
    .bind(maintenance.committers_180d)
    .bind(maintenance.releases_365d)
    .bind(maintenance.weekly_commits.clone())
    .bind(maintenance.level.key())
    .bind(i16::from(maintenance.score))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

fn fill_text(current: Option<String>, new: Option<String>, locked: bool) -> Option<String> {
    if locked || current.as_deref().is_some_and(|s| !s.is_empty()) {
        return current;
    }
    new.filter(|s| !s.is_empty()).or(current)
}

/// Fills only NULL or empty columns and unlocked fields from an import.
/// AI usage and playability are filled only while they are `unknown`.
pub async fn fill_empty(conn: &mut PgConnection, port_id: i64, input: &PortInput) -> DbResult<()> {
    let cur = lock_row(&mut *conn, port_id).await?;
    let locked = |f: &str| cur.locked_fields.iter().any(|x| x == f);
    let homepage = fill_text(
        cur.homepage_url.clone(),
        input.homepage_url.clone(),
        locked("homepage_url"),
    );
    let import_description = fill_text(
        cur.import_description.clone(),
        input.import_description.clone(),
        locked("import_description"),
    );
    let requirements = fill_text(
        cur.requirements.clone(),
        input.requirements.clone(),
        locked("requirements"),
    );
    let toolchain = fill_text(
        cur.toolchain.clone(),
        input.toolchain.clone(),
        locked("toolchain"),
    );
    let license = fill_text(
        cur.license.clone(),
        input.license.clone(),
        locked("license"),
    );
    let source_platform_id = if locked("source_platform_id") || cur.source_platform_id.is_some() {
        cur.source_platform_id
    } else {
        input.source_platform_id
    };
    let declared = if locked("declared_platforms") || !cur.declared_platforms.is_empty() {
        cur.declared_platforms.clone()
    } else {
        input.declared_platforms.clone()
    };
    let playability =
        if cur.playability == "unknown" && !locked("playability") && input.playability != "unknown"
        {
            input.playability.clone()
        } else {
            cur.playability.clone()
        };
    let ai_fillable = cur.ai_usage == "unknown"
        && !locked("ai_usage")
        && matches!(input.ai_usage.as_str(), "none" | "assisted");
    let (ai_usage, ai_tools, ai_evidence, ai_source) = if ai_fillable {
        (
            input.ai_usage.clone(),
            input.ai_tools.clone(),
            serde_json::to_value(&input.ai_evidence)?,
            input.ai_source.clone(),
        )
    } else {
        (
            cur.ai_usage.clone(),
            cur.ai_tools.clone(),
            cur.ai_evidence.clone(),
            cur.ai_source.clone(),
        )
    };

    sqlx::query(
        "UPDATE ports SET homepage_url = $2, import_description = $3, requirements = $4,
            toolchain = $5, license = $6, source_platform_id = $7, declared_platforms = $8,
            playability = $9, ai_usage = $10, ai_tools = $11, ai_evidence = $12, ai_source = $13,
            updated_at = now()
         WHERE id = $1",
    )
    .bind(port_id)
    .bind(homepage)
    .bind(import_description)
    .bind(requirements)
    .bind(toolchain)
    .bind(license)
    .bind(source_platform_id)
    .bind(declared)
    .bind(playability)
    .bind(ai_usage)
    .bind(ai_tools)
    .bind(ai_evidence)
    .bind(ai_source)
    .execute(&mut *conn)
    .await?;
    games::refresh_rollup(&mut *conn, cur.game_id).await?;
    Ok(())
}

/// Adds a source tag (for example `import:portsdr`) if it is not already present.
pub async fn add_source(conn: &mut PgConnection, port_id: i64, source: &str) -> DbResult<()> {
    sqlx::query(
        "UPDATE ports SET sources = array_append(sources, $2::text)
         WHERE id = $1 AND NOT ($2::text = ANY(sources))",
    )
    .bind(port_id)
    .bind(source)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Adds field names to `locked_fields`. Duplicates are removed.
pub async fn lock_fields(conn: &mut PgConnection, port_id: i64, fields: &[String]) -> DbResult<()> {
    sqlx::query(
        "UPDATE ports SET locked_fields = ARRAY(
            SELECT DISTINCT f FROM unnest(locked_fields || $2::text[]) AS f)
         WHERE id = $1",
    )
    .bind(port_id)
    .bind(fields.to_vec())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_repo_urls() {
        assert_eq!(
            normalize_repo_url("https://www.GitHub.com/Owner/Repo.git/").unwrap(),
            "https://github.com/owner/repo"
        );
        assert_eq!(
            normalize_repo_url("github.com/a/b/tree/main").unwrap(),
            "https://github.com/a/b"
        );
    }

    #[test]
    fn rejects_non_github_urls() {
        assert!(normalize_repo_url("https://gitlab.com/a/b").is_err());
        assert!(normalize_repo_url("https://github.com/onlyowner").is_err());
    }

    #[test]
    fn fill_text_keeps_existing_values() {
        assert_eq!(
            fill_text(Some("a".into()), Some("b".into()), false),
            Some("a".into())
        );
        assert_eq!(
            fill_text(Some(String::new()), Some("b".into()), false),
            Some("b".into())
        );
        assert_eq!(fill_text(None, Some("b".into()), true), None);
        assert_eq!(fill_text(None, Some(String::new()), false), None);
    }

    #[test]
    fn validation_rejects_bad_enum_and_long_description() {
        let mut input = PortInput {
            name: "Port".into(),
            kind: "static_recomp".into(),
            repo_url: "https://github.com/a/b".into(),
            ..PortInput::default()
        };
        assert!(validate_input(&input).is_ok());
        input.kind = "nonsense".into();
        assert!(validate_input(&input).is_err());
        input.kind = "static_recomp".into();
        input.description = Some("x".repeat(2001));
        assert!(validate_input(&input).is_err());
    }
}
