//! Comment and rating behaviour. Each test gets a fresh database with the migrations applied.

use recomphub_db::comments::{self, CommentFilter, CommentRow, CommentSort};
use recomphub_db::games::{self, IgdbGame};
use recomphub_db::submissions::{self, ApproveOverrides, NewSubmission};
use recomphub_db::{DbError, DbResult, PgPool};
use recomphub_shared::{RatingSummary, SubmissionKind};
use serde_json::json;

async fn make_user(pool: &PgPool, login: &str, approved: i32) -> DbResult<i64> {
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (display_name, github_login, role, approved_count)
         VALUES ($1, $2, 'user', $3) RETURNING id",
    )
    .bind(login)
    .bind(login)
    .bind(approved)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// A published port, approved by a moderator.
async fn make_port(pool: &PgPool) -> DbResult<i64> {
    let mut conn = pool.acquire().await?;
    let g = IgdbGame {
        igdb_id: 1,
        name: "Game A".to_string(),
        slug: "game-a".to_string(),
        ..Default::default()
    };
    let game = games::sync_igdb(&mut conn, &g).await?;
    drop(conn);
    let owner = make_user(pool, "owner", 3).await?;
    let sub = submissions::create(
        pool,
        &NewSubmission {
            kind: SubmissionKind::NewPort,
            source: "user".to_string(),
            submitter_id: Some(owner),
            payload: json!({
                "game_id": game,
                "name": "Port A",
                "kind": "static_recomp",
                "repo_url": "https://github.com/a/port-a",
                "playability": "complete"
            }),
            target_game_id: None,
            target_port_id: None,
        },
    )
    .await?;
    let moderator = sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (display_name, github_login, role) VALUES ('moderator', 'moderator', 'moderator') RETURNING id",
    )
    .fetch_one(pool)
    .await?;
    let approved =
        submissions::approve(pool, sub.id, moderator, &ApproveOverrides::default(), None).await?;
    Ok(approved
        .target_port_id
        .expect("approved submission sets the port id"))
}

fn filter(
    port_id: i64,
    sort: CommentSort,
    stars: Option<i16>,
    include_hidden: bool,
) -> CommentFilter {
    CommentFilter {
        port_id,
        sort,
        stars,
        include_hidden,
    }
}

fn bodies(rows: &[CommentRow]) -> Vec<&str> {
    rows.iter().map(|r| r.body.as_str()).collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn posts_list_newest_first_and_enforce_the_rate_limit(pool: PgPool) -> DbResult<()> {
    let port = make_port(&pool).await?;
    let user = make_user(&pool, "poster", 0).await?;
    for i in 0..comments::POST_LIMIT {
        comments::create(
            &pool,
            port,
            user,
            &format!("Runs well, run {i}"),
            Some("windows"),
        )
        .await?;
    }
    let err = comments::create(&pool, port, user, "One more", None)
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::RateLimited(secs) if secs > 0));

    let f = filter(port, CommentSort::Newest, None, false);
    let rows = comments::list(&pool, &f, 0, 20).await?;
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].body, "Runs well, run 4");
    assert_eq!(rows[0].tested_target.as_deref(), Some("windows"));
    assert_eq!(rows[0].author_name, "poster");
    assert_eq!(rows[0].author_github_login.as_deref(), Some("poster"));
    assert_eq!(comments::count(&pool, &f).await?, 5);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn rejects_a_repeated_body_within_the_hour(pool: PgPool) -> DbResult<()> {
    let port = make_port(&pool).await?;
    let user = make_user(&pool, "repeater", 0).await?;
    comments::create(&pool, port, user, "Same text", None).await?;
    let err = comments::create(&pool, port, user, "Same text", None)
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::InvalidState(_)));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn stores_platforms_in_their_as_str_form(pool: PgPool) -> DbResult<()> {
    let port = make_port(&pool).await?;
    let user = make_user(&pool, "vita", 0).await?;
    let id = comments::create(&pool, port, user, "Plays on the Vita", Some("ps_vita")).await?;
    assert_eq!(
        comments::get(&pool, id).await?.tested_target.as_deref(),
        Some("ps_vita")
    );

    // The URL slug is not a stored value, so the CHECK constraint rejects it.
    let err = comments::create(&pool, port, user, "Slug spelling", Some("ps-vita"))
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::Sql(_)));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn unpublished_ports_take_no_comments_or_ratings(pool: PgPool) -> DbResult<()> {
    let port = make_port(&pool).await?;
    sqlx::query("UPDATE ports SET status = 'hidden' WHERE id = $1")
        .bind(port)
        .execute(&pool)
        .await?;
    let user = make_user(&pool, "late", 0).await?;
    let err = comments::create(&pool, port, user, "Hello", None)
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::NotFound(_)));
    let err = comments::rate(&pool, port, user, 4).await.unwrap_err();
    assert!(matches!(err, DbError::NotFound(_)));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn hiding_hides_from_the_public_and_can_remove_the_rating(pool: PgPool) -> DbResult<()> {
    let port = make_port(&pool).await?;
    let author = make_user(&pool, "author", 0).await?;
    let moderator = make_user(&pool, "mod", 0).await?;
    let id = comments::create(&pool, port, author, "Crashes on boot", None).await?;
    comments::rate(&pool, port, author, 2).await?;

    comments::set_hidden(&pool, id, moderator, true, Some("spam"), false).await?;
    let public = filter(port, CommentSort::Newest, None, false);
    assert!(comments::list(&pool, &public, 0, 20).await?.is_empty());
    assert_eq!(comments::count(&pool, &public).await?, 0);
    let mods = filter(port, CommentSort::Newest, None, true);
    let rows = comments::list(&pool, &mods, 0, 20).await?;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].hidden);
    assert_eq!(rows[0].hidden_reason.as_deref(), Some("spam"));
    assert_eq!(comments::my_rating(&pool, port, author).await?, Some(2));

    comments::set_hidden(&pool, id, moderator, true, None, true).await?;
    assert_eq!(comments::my_rating(&pool, port, author).await?, None);

    comments::set_hidden(&pool, id, moderator, false, None, false).await?;
    let rows = comments::list(&pool, &public, 0, 20).await?;
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].hidden);
    assert_eq!(rows[0].hidden_reason, None);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn editing_and_deleting_only_touch_the_comment(pool: PgPool) -> DbResult<()> {
    let port = make_port(&pool).await?;
    let user = make_user(&pool, "editor", 0).await?;
    let id = comments::create(&pool, port, user, "First draft", None).await?;
    comments::edit(&pool, id, "Second draft", Some("linux")).await?;
    let row = comments::get(&pool, id).await?;
    assert_eq!(row.body, "Second draft");
    assert_eq!(row.tested_target.as_deref(), Some("linux"));
    assert!(row.edited_at.is_some());
    comments::delete(&pool, id).await?;
    assert!(matches!(
        comments::get(&pool, id).await.unwrap_err(),
        DbError::NotFound(_)
    ));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn rating_is_one_row_per_user_and_port(pool: PgPool) -> DbResult<()> {
    let port = make_port(&pool).await?;
    let user = make_user(&pool, "rater", 0).await?;
    comments::rate(&pool, port, user, 3).await?;
    comments::rate(&pool, port, user, 5).await?;
    assert_eq!(comments::my_rating(&pool, port, user).await?, Some(5));
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM port_ratings WHERE user_id = $1")
        .bind(user)
        .fetch_one(&pool)
        .await?;
    assert_eq!(rows, 1);

    assert!(comments::unrate(&pool, port, user).await?);
    assert!(!comments::unrate(&pool, port, user).await?);
    assert_eq!(comments::my_rating(&pool, port, user).await?, None);

    let err = comments::rate(&pool, port, user, 0).await.unwrap_err();
    assert!(matches!(err, DbError::InvalidPayload(_)));
    let err = comments::rate(&pool, port, user, 6).await.unwrap_err();
    assert!(matches!(err, DbError::InvalidPayload(_)));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn rating_distribution_and_average_skip_banned_users(pool: PgPool) -> DbResult<()> {
    let port = make_port(&pool).await?;
    for (login, stars) in [("u10", 5), ("u11", 5), ("u12", 2)] {
        let user = make_user(&pool, login, 0).await?;
        comments::rate(&pool, port, user, stars).await?;
    }
    let banned = make_user(&pool, "banned", 0).await?;
    comments::rate(&pool, port, banned, 1).await?;
    sqlx::query("UPDATE users SET banned = true WHERE id = $1")
        .bind(banned)
        .execute(&pool)
        .await?;

    let distribution = comments::rating_distribution(&pool, port).await?;
    assert_eq!(distribution, [0, 1, 0, 0, 2]);
    let summary = RatingSummary::from_distribution(distribution);
    assert_eq!(summary.count, 3);
    assert_eq!(summary.average, Some(4.0));

    let empty = RatingSummary::from_distribution([0; 5]);
    assert_eq!(empty.count, 0);
    assert_eq!(empty.average, None);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn comments_sort_by_author_rating_and_filter_by_stars(pool: PgPool) -> DbResult<()> {
    let port = make_port(&pool).await?;
    let a = make_user(&pool, "a", 0).await?;
    let b = make_user(&pool, "b", 0).await?;
    let c = make_user(&pool, "c", 0).await?;
    comments::create(&pool, port, a, "A", None).await?;
    comments::create(&pool, port, b, "B", None).await?;
    comments::create(&pool, port, c, "C", None).await?;
    comments::rate(&pool, port, a, 5).await?;
    comments::rate(&pool, port, b, 2).await?;

    let highest = comments::list(
        &pool,
        &filter(port, CommentSort::Highest, None, false),
        0,
        20,
    )
    .await?;
    assert_eq!(bodies(&highest), ["A", "B", "C"]);
    assert_eq!(highest[0].author_rating, Some(5));
    assert_eq!(highest[2].author_rating, None);

    let lowest = comments::list(
        &pool,
        &filter(port, CommentSort::Lowest, None, false),
        0,
        20,
    )
    .await?;
    assert_eq!(bodies(&lowest), ["B", "A", "C"]);

    let five = filter(port, CommentSort::Newest, Some(5), false);
    let rows = comments::list(&pool, &five, 0, 20).await?;
    assert_eq!(bodies(&rows), ["A"]);
    assert_eq!(comments::count(&pool, &five).await?, 1);
    Ok(())
}
