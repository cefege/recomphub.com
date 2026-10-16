//! Donation links for GitHub authors (`author_funding`). Rows are keyed by lowercase GitHub
//! login; `author_funding_checks` records every login looked up, including those with no links.

use crate::{DbResult, PgConnection, PgExecutor};
use chrono::{DateTime, Utc};
use recomphub_shared::FundingPlatform;
use sqlx::FromRow;

/// One stored link, as read back for the API layer.
#[derive(Debug, Clone, FromRow)]
pub struct AuthorFundingRow {
    pub login: String,
    pub platform: String,
    pub url: String,
}

/// Where a link was found. Stored as text; the CHECK constraint mirrors these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FundingSource {
    /// A `FUNDING.yml` in one of the author's port repositories.
    Repo,
    /// A `FUNDING.yml` in the author's `<login>/.github` profile repository.
    Profile,
    /// A GitHub Sponsors listing for the login.
    Sponsors,
}

impl FundingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            FundingSource::Repo => "repo",
            FundingSource::Profile => "profile",
            FundingSource::Sponsors => "sponsors",
        }
    }
}

/// One link to store for a login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FundingRecord {
    pub platform: FundingPlatform,
    pub url: String,
    pub source: FundingSource,
}

/// Stored links for the given logins, matched case-insensitively and ordered for stable display.
pub async fn for_logins(
    ex: impl PgExecutor<'_>,
    logins: &[String],
) -> DbResult<Vec<AuthorFundingRow>> {
    let lowered: Vec<String> = logins.iter().map(|l| l.to_ascii_lowercase()).collect();
    Ok(sqlx::query_as::<_, AuthorFundingRow>(
        "SELECT login, platform, url FROM author_funding \
         WHERE login = ANY($1) ORDER BY login, platform, url",
    )
    .bind(lowered)
    .fetch_all(ex)
    .await?)
}

/// Logins of authors on published ports whose funding was never checked or was checked before
/// `checked_before`. Never-checked logins come first, then the stalest.
pub async fn due_logins(
    ex: impl PgExecutor<'_>,
    checked_before: DateTime<Utc>,
    limit: i64,
) -> DbResult<Vec<String>> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT logins.login FROM ( \
             SELECT DISTINCT lower(pa.github_login) AS login \
             FROM port_authors pa \
             JOIN ports p ON p.id = pa.port_id \
             WHERE p.status = 'published' AND pa.github_login IS NOT NULL \
         ) logins \
         LEFT JOIN author_funding_checks c ON c.login = logins.login \
         WHERE c.checked_at IS NULL OR c.checked_at < $1 \
         ORDER BY c.checked_at NULLS FIRST, logins.login \
         LIMIT $2",
    )
    .bind(checked_before)
    .bind(limit)
    .fetch_all(ex)
    .await?)
}

/// Repository URLs of published ports owned by `login` (lowercase), most-starred first.
pub async fn owned_repo_urls(
    ex: impl PgExecutor<'_>,
    login: &str,
    limit: i64,
) -> DbResult<Vec<String>> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT repo_url FROM ports \
         WHERE status = 'published' AND repo_url ILIKE 'https://github.com/%' \
           AND lower(split_part(repo_url, '/', 4)) = $1 \
         ORDER BY github_stars DESC NULLS LAST, id \
         LIMIT $2",
    )
    .bind(login)
    .bind(limit)
    .fetch_all(ex)
    .await?)
}

/// Replaces every stored link for `login` and stamps the check time. Run inside a transaction
/// so readers never see a half-written set.
pub async fn replace_links(
    conn: &mut PgConnection,
    login: &str,
    links: &[FundingRecord],
) -> DbResult<()> {
    let login = login.to_ascii_lowercase();
    let login = login.as_str();
    mark_checked(&mut *conn, login).await?;
    sqlx::query("DELETE FROM author_funding WHERE login = $1")
        .bind(login)
        .execute(&mut *conn)
        .await?;
    for link in links {
        sqlx::query(
            "INSERT INTO author_funding (login, platform, url, source) VALUES ($1, $2, $3, $4) \
             ON CONFLICT DO NOTHING",
        )
        .bind(login)
        .bind(link.platform.as_str())
        .bind(&link.url)
        .bind(link.source.as_str())
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Stamps the check time for `login` without changing its links.
pub async fn mark_checked(conn: &mut PgConnection, login: &str) -> DbResult<()> {
    sqlx::query(
        "INSERT INTO author_funding_checks (login, checked_at) VALUES ($1, now()) \
         ON CONFLICT (login) DO UPDATE SET checked_at = now()",
    )
    .bind(login.to_ascii_lowercase())
    .execute(&mut *conn)
    .await?;
    Ok(())
}
