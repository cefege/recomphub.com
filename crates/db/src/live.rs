//! Live presence persistence: daily viewer minutes per game and peak online per site.

use crate::DbResult;
use chrono::NaiveDate;
use sqlx::PgPool;

/// Adds one flush interval of viewing to the daily totals, in one transaction.
///
/// `per_game` holds `(game_id, viewers)` for every game with viewers > 0. Each viewer
/// counts as one minute, since the flush runs every minute. `site_peak` is the online count
/// at the time of the flush, merged with `greatest` into the day's peak.
pub async fn flush_viewer_minutes(
    pool: &PgPool,
    day: NaiveDate,
    per_game: &[(i64, i32)],
    site_peak: i32,
) -> DbResult<()> {
    let mut tx = pool.begin().await?;
    if !per_game.is_empty() {
        let ids: Vec<i64> = per_game.iter().map(|(g, _)| *g).collect();
        let counts: Vec<i32> = per_game.iter().map(|(_, v)| *v).collect();
        sqlx::query(
            "INSERT INTO game_live_daily (game_id, day, viewer_minutes, peak_viewers)
             SELECT g, $1, v, v FROM unnest($2::bigint[], $3::int[]) AS t(g, v)
             ON CONFLICT (game_id, day) DO UPDATE SET
                viewer_minutes = game_live_daily.viewer_minutes + EXCLUDED.viewer_minutes,
                peak_viewers = greatest(game_live_daily.peak_viewers, EXCLUDED.peak_viewers)",
        )
        .bind(day)
        .bind(ids)
        .bind(counts)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO site_live_daily (day, peak_online) VALUES ($1, $2)
         ON CONFLICT (day) DO UPDATE SET peak_online = greatest(site_live_daily.peak_online, EXCLUDED.peak_online)",
    )
    .bind(day)
    .bind(site_peak)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
