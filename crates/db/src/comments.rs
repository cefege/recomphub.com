//! Port comments and star ratings. Users write many comments per port but hold at most one
//! rating per port; a rating does not depend on any comment.

use crate::{DbError, DbResult};
use chrono::{DateTime, Utc};
use sqlx::{PgExecutor, PgPool, Postgres, QueryBuilder};

pub const MAX_BODY_CHARS: usize = 2000;
pub const MAX_REASON_CHARS: usize = 500;

/// A user may post this many comments per window.
pub const POST_LIMIT: i64 = 5;
pub const POST_WINDOW_MINUTES: i64 = 10;
/// The same body may not be posted again by the same user within this window.
pub const DUPLICATE_WINDOW_MINUTES: i64 = 60;

/// Sort order for a port's comment list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommentSort {
    #[default]
    Newest,
    Highest,
    Lowest,
}

/// Which comments a list query returns.
#[derive(Debug, Clone, Copy)]
pub struct CommentFilter {
    pub port_id: i64,
    pub sort: CommentSort,
    /// Only comments whose author rated the port this many stars.
    pub stars: Option<i16>,
    /// Hidden comments are included only for moderators.
    pub include_hidden: bool,
}

/// A comment joined with its author and the author's rating of the same port.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CommentRow {
    pub id: i64,
    pub port_id: i64,
    pub user_id: i64,
    pub body: String,
    pub tested_target: Option<String>,
    pub created_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub hidden: bool,
    pub hidden_reason: Option<String>,
    pub author_name: String,
    pub author_github_login: Option<String>,
    pub author_avatar_media_id: Option<i64>,
    pub author_rating: Option<i16>,
}

const COLUMNS: &str = "SELECT c.id, c.port_id, c.user_id, c.body, c.tested_target, c.created_at, \
     c.edited_at, c.hidden, c.hidden_reason, u.display_name AS author_name, \
     u.github_login AS author_github_login, \
     u.avatar_media_id AS author_avatar_media_id, r.stars AS author_rating";

/// Banned users' ratings are ignored everywhere, so their stars disappear from summaries.
const FROM: &str = " FROM comments c \
     JOIN users u ON u.id = c.user_id \
     LEFT JOIN port_ratings r ON r.port_id = c.port_id AND r.user_id = c.user_id AND NOT u.banned";

fn push_filter<'a>(qb: &mut QueryBuilder<'a, Postgres>, filter: &CommentFilter) {
    qb.push(" WHERE c.port_id = ").push_bind(filter.port_id);
    if !filter.include_hidden {
        qb.push(" AND NOT c.hidden");
    }
    if let Some(stars) = filter.stars {
        qb.push(" AND r.stars = ").push_bind(stars);
    }
}

/// One page of comments for a port, in the requested order.
pub async fn list(
    pool: &PgPool,
    filter: &CommentFilter,
    offset: i64,
    limit: i64,
) -> DbResult<Vec<CommentRow>> {
    let mut qb = QueryBuilder::<Postgres>::new(COLUMNS);
    qb.push(FROM);
    push_filter(&mut qb, filter);
    qb.push(match filter.sort {
        CommentSort::Newest => " ORDER BY c.created_at DESC, c.id DESC",
        CommentSort::Highest => " ORDER BY r.stars DESC NULLS LAST, c.created_at DESC, c.id DESC",
        CommentSort::Lowest => " ORDER BY r.stars ASC NULLS LAST, c.created_at DESC, c.id DESC",
    });
    qb.push(" LIMIT ").push_bind(limit);
    qb.push(" OFFSET ").push_bind(offset);
    Ok(qb.build_query_as::<CommentRow>().fetch_all(pool).await?)
}

/// Number of comments matching the filter.
pub async fn count(pool: &PgPool, filter: &CommentFilter) -> DbResult<i64> {
    let mut qb = QueryBuilder::<Postgres>::new("SELECT count(*)");
    qb.push(FROM);
    push_filter(&mut qb, filter);
    Ok(qb.build_query_scalar::<i64>().fetch_one(pool).await?)
}

/// One comment by id.
pub async fn get(ex: impl PgExecutor<'_>, id: i64) -> DbResult<CommentRow> {
    let sql = format!("{COLUMNS}{FROM} WHERE c.id = $1");
    sqlx::query_as::<_, CommentRow>(&sql)
        .bind(id)
        .fetch_optional(ex)
        .await?
        .ok_or_else(|| DbError::NotFound("comment".to_string()))
}

/// Posts a comment. Enforces the per-user rate limit and the duplicate-body rule, and
/// rejects ports that are not published. The poster's `users` row is locked for the
/// transaction, so concurrent posts by one user serialise and the limits cannot be raced.
pub async fn create(
    pool: &PgPool,
    port_id: i64,
    user_id: i64,
    body: &str,
    tested_target: Option<&str>,
) -> DbResult<i64> {
    let mut tx = pool.begin().await?;

    sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR NO KEY UPDATE")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    let published = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM ports WHERE id = $1 AND status = 'published')",
    )
    .bind(port_id)
    .fetch_one(&mut *tx)
    .await?;
    if !published {
        return Err(DbError::NotFound("port".to_string()));
    }

    let (recent, retry_secs) = sqlx::query_as::<_, (i64, Option<f64>)>(
        "SELECT count(*), EXTRACT(EPOCH FROM (min(created_at) + make_interval(mins => $2::int) - now()))::float8
         FROM comments
         WHERE user_id = $1 AND created_at > now() - make_interval(mins => $2::int)",
    )
    .bind(user_id)
    .bind(POST_WINDOW_MINUTES as i32)
    .fetch_one(&mut *tx)
    .await?;
    if recent >= POST_LIMIT {
        let secs = retry_secs.unwrap_or(0.0).ceil().max(1.0) as i64;
        return Err(DbError::RateLimited(secs));
    }

    let duplicate = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
             SELECT 1 FROM comments
             WHERE user_id = $1 AND body = $2 AND created_at > now() - make_interval(mins => $3::int))",
    )
    .bind(user_id)
    .bind(body)
    .bind(DUPLICATE_WINDOW_MINUTES as i32)
    .fetch_one(&mut *tx)
    .await?;
    if duplicate {
        return Err(DbError::InvalidState(
            "you already posted this comment recently".to_string(),
        ));
    }

    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO comments (port_id, user_id, body, tested_target)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(port_id)
    .bind(user_id)
    .bind(body)
    .bind(tested_target)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(id)
}

/// Rewrites a comment's body and tested platform. Callers check ownership and hidden state.
pub async fn edit(
    ex: impl PgExecutor<'_>,
    id: i64,
    body: &str,
    tested_target: Option<&str>,
) -> DbResult<()> {
    let done = sqlx::query(
        "UPDATE comments SET body = $2, tested_target = $3, edited_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(body)
    .bind(tested_target)
    .execute(ex)
    .await?;
    if done.rows_affected() == 0 {
        return Err(DbError::NotFound("comment".to_string()));
    }
    Ok(())
}

/// Deletes a comment. The author's rating is untouched.
pub async fn delete(ex: impl PgExecutor<'_>, id: i64) -> DbResult<()> {
    let done = sqlx::query("DELETE FROM comments WHERE id = $1")
        .bind(id)
        .execute(ex)
        .await?;
    if done.rows_affected() == 0 {
        return Err(DbError::NotFound("comment".to_string()));
    }
    Ok(())
}

/// Hides or unhides a comment. Hiding can also remove the author's rating for that port;
/// unhiding never touches ratings.
pub async fn set_hidden(
    pool: &PgPool,
    id: i64,
    moderator_id: i64,
    hidden: bool,
    reason: Option<&str>,
    remove_rating: bool,
) -> DbResult<()> {
    let mut tx = pool.begin().await?;
    let done = sqlx::query(
        "UPDATE comments
         SET hidden = $2,
             hidden_by = CASE WHEN $2 THEN $3::bigint ELSE NULL END,
             hidden_reason = CASE WHEN $2 THEN $4::text ELSE NULL END
         WHERE id = $1",
    )
    .bind(id)
    .bind(hidden)
    .bind(moderator_id)
    .bind(reason)
    .execute(&mut *tx)
    .await?;
    if done.rows_affected() == 0 {
        return Err(DbError::NotFound("comment".to_string()));
    }
    if hidden && remove_rating {
        sqlx::query(
            "DELETE FROM port_ratings r
             USING comments c
             WHERE c.id = $1 AND r.port_id = c.port_id AND r.user_id = c.user_id",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Sets or changes a user's rating of a port. Fails with `NotFound` unless the port is
/// published.
pub async fn rate(ex: impl PgExecutor<'_>, port_id: i64, user_id: i64, stars: i16) -> DbResult<()> {
    if !(1..=5).contains(&stars) {
        return Err(DbError::InvalidPayload(
            "stars must be between 1 and 5".to_string(),
        ));
    }
    let done = sqlx::query(
        "INSERT INTO port_ratings (port_id, user_id, stars)
         SELECT id, $2, $3 FROM ports WHERE id = $1 AND status = 'published'
         ON CONFLICT (port_id, user_id)
         DO UPDATE SET stars = EXCLUDED.stars, updated_at = now()",
    )
    .bind(port_id)
    .bind(user_id)
    .bind(stars)
    .execute(ex)
    .await?;
    if done.rows_affected() == 0 {
        return Err(DbError::NotFound("port".to_string()));
    }
    Ok(())
}

/// Removes a user's rating of a port. Returns whether a rating existed.
pub async fn unrate(ex: impl PgExecutor<'_>, port_id: i64, user_id: i64) -> DbResult<bool> {
    let done = sqlx::query("DELETE FROM port_ratings WHERE port_id = $1 AND user_id = $2")
        .bind(port_id)
        .bind(user_id)
        .execute(ex)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// A user's current rating of a port, if any.
pub async fn my_rating(
    ex: impl PgExecutor<'_>,
    port_id: i64,
    user_id: i64,
) -> DbResult<Option<i16>> {
    let stars = sqlx::query_scalar::<_, i16>(
        "SELECT stars FROM port_ratings WHERE port_id = $1 AND user_id = $2",
    )
    .bind(port_id)
    .bind(user_id)
    .fetch_optional(ex)
    .await?;
    Ok(stars)
}

/// Number of ratings per star value for a port, index 0 holding the 1-star count. Ratings
/// from banned users are excluded.
pub async fn rating_distribution(ex: impl PgExecutor<'_>, port_id: i64) -> DbResult<[i64; 5]> {
    let rows = sqlx::query_as::<_, (i16, i64)>(
        "SELECT r.stars, count(*)
         FROM port_ratings r
         JOIN users u ON u.id = r.user_id
         WHERE r.port_id = $1 AND NOT u.banned
         GROUP BY r.stars",
    )
    .bind(port_id)
    .fetch_all(ex)
    .await?;
    let mut distribution = [0i64; 5];
    for (stars, count) in rows {
        if (1..=5).contains(&stars) {
            distribution[(stars - 1) as usize] = count;
        }
    }
    Ok(distribution)
}
