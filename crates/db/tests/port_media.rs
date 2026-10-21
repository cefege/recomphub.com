//! README-derived port screenshots. Each test gets a fresh database with the migrations applied.

use recomphub_db::media;
use recomphub_db::port_media;
use recomphub_db::{DbResult, PgPool};

async fn make_game(pool: &PgPool) -> DbResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO games (igdb_id, slug, name, popularity, want_count, port_count)
         VALUES (1, 'game', 'Game', 0, 0, 0) RETURNING id",
    )
    .fetch_one(pool)
    .await?)
}

async fn make_port(pool: &PgPool, game_id: i64, slug: &str, repo_url: &str) -> DbResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO ports (game_id, slug, name, kind, repo_url, declared_platforms,
                            github_stars, star_count, status)
         VALUES ($1, $2, $3, 'static_recomp', $4, '{}', 5, 5, 'published') RETURNING id",
    )
    .bind(game_id)
    .bind(slug)
    .bind(format!("Port {slug}"))
    .bind(repo_url)
    .fetch_one(pool)
    .await?)
}

async fn make_user(pool: &PgPool, login: &str) -> DbResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (display_name, github_login, role, approved_count)
         VALUES ($1, $1, 'user', 0) RETURNING id",
    )
    .bind(login)
    .fetch_one(pool)
    .await?)
}

/// The `media.id` queued for a URL by `record_readme_images`.
async fn media_id_for(pool: &PgPool, url: &str) -> DbResult<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT id FROM media WHERE source_url = $1 AND kind = 'port_screenshot'",
    )
    .bind(url)
    .fetch_one(pool)
    .await?)
}

async fn record(pool: &PgPool, port: i64, urls: &[String]) -> DbResult<()> {
    let images: Vec<(String, String)> = urls
        .iter()
        .cloned()
        .map(|url| (url, String::new()))
        .collect();
    port_media::record_readme_images(pool, port, &images, "game-port").await
}

/// Marks a queued README image as rendered exactly as the production media pipeline does.
async fn render(pool: &PgPool, url: &str) -> DbResult<()> {
    let id = media_id_for(pool, url).await?;
    media::mark_ready(pool, id, vec![480, 960], 960, 540, 2).await?;
    port_media::approve_readme_after_render(pool, id, 960).await
}

#[sqlx::test(migrations = "./migrations")]
async fn pending_readme_image_is_not_shown_until_rendered(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool).await?;
    let port = make_port(&pool, game, "port", "https://github.com/a/b").await?;
    let url = "https://raw.githubusercontent.com/a/b/main/one.png".to_string();

    record(&pool, port, std::slice::from_ref(&url)).await?;
    assert!(port_media::approved_for_port(&pool, port).await?.is_empty());

    render(&pool, &url).await?;
    let approved = port_media::approved_for_port(&pool, port).await?;
    assert_eq!(approved.len(), 1);
    assert_eq!(approved[0].source_url, url);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn readme_images_show_in_readme_order(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool).await?;
    let port = make_port(&pool, game, "port", "https://github.com/a/b").await?;
    // Deliberately not alphabetical: README order, not insertion/sort order, must survive.
    let urls = vec![
        "https://raw.githubusercontent.com/a/b/main/two.png".to_string(),
        "https://raw.githubusercontent.com/a/b/main/one.png".to_string(),
    ];
    record(&pool, port, &urls).await?;
    for url in &urls {
        render(&pool, url).await?;
    }

    let approved = port_media::approved_for_port(&pool, port).await?;
    let got: Vec<&str> = approved.iter().map(|m| m.source_url.as_str()).collect();
    assert_eq!(got, vec![urls[0].as_str(), urls[1].as_str()]);
    assert!(approved.iter().all(|m| m.kind == "port_screenshot"));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn repeated_scan_does_not_duplicate_rows(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool).await?;
    let port = make_port(&pool, game, "port", "https://github.com/a/b").await?;
    let urls = vec!["https://raw.githubusercontent.com/a/b/main/one.png".to_string()];

    record(&pool, port, &urls).await?;
    record(&pool, port, &urls).await?;
    render(&pool, &urls[0]).await?;

    let approved = port_media::approved_for_port(&pool, port).await?;
    assert_eq!(approved.len(), 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn an_image_dropped_from_the_readme_is_pruned_on_rescan(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool).await?;
    let port = make_port(&pool, game, "port", "https://github.com/a/b").await?;
    let one = "https://raw.githubusercontent.com/a/b/main/one.png".to_string();
    let two = "https://raw.githubusercontent.com/a/b/main/two.png".to_string();

    record(&pool, port, &[one.clone(), two.clone()]).await?;
    render(&pool, &one).await?;
    render(&pool, &two).await?;
    assert_eq!(port_media::approved_for_port(&pool, port).await?.len(), 2);

    // The project replaced its first screenshot with a new one; "one.png" is gone from the README.
    let three = "https://raw.githubusercontent.com/a/b/main/three.png".to_string();
    record(&pool, port, &[two.clone(), three.clone()]).await?;
    render(&pool, &three).await?;

    let approved = port_media::approved_for_port(&pool, port).await?;
    let got: Vec<&str> = approved.iter().map(|m| m.source_url.as_str()).collect();
    assert_eq!(got, vec![two.as_str(), three.as_str()]);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn readme_images_are_capped(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool).await?;
    let port = make_port(&pool, game, "port", "https://github.com/a/b").await?;
    let urls: Vec<String> = (0..10)
        .map(|i| format!("https://raw.githubusercontent.com/a/b/main/{i}.png"))
        .collect();

    record(&pool, port, &urls).await?;
    for url in urls.iter().take(port_media::README_IMAGE_CAP) {
        render(&pool, url).await?;
    }

    let approved = port_media::approved_for_port(&pool, port).await?;
    assert_eq!(approved.len(), port_media::README_IMAGE_CAP);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_render_under_the_minimum_width_is_excluded(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool).await?;
    let port = make_port(&pool, game, "port", "https://github.com/a/b").await?;
    let url = "https://raw.githubusercontent.com/a/b/main/tiny.png".to_string();

    record(&pool, port, std::slice::from_ref(&url)).await?;
    let id = media_id_for(&pool, &url).await?;
    media::mark_ready(&pool, id, vec![120], 120, 60, 2).await?;
    port_media::approve_readme_after_render(&pool, id, 120).await?;

    assert!(port_media::approved_for_port(&pool, port).await?.is_empty());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn no_images_is_a_no_op(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool).await?;
    let port = make_port(&pool, game, "port", "https://github.com/a/b").await?;

    record(&pool, port, &[]).await?;

    assert!(port_media::approved_for_port(&pool, port).await?.is_empty());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_second_upload_of_the_same_file_reuses_the_pending_row(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool).await?;
    let port = make_port(&pool, game, "port", "https://github.com/a/b").await?;
    let user = make_user(&pool, "uploader").await?;
    let media_id =
        media::enqueue(&pool, "port_screenshot", "upload:deadbeef", "test-upload").await?;

    let first = port_media::insert_pending(&pool, port, media_id, user).await?;
    let second = port_media::insert_pending(&pool, port, media_id, user).await?;

    assert_eq!(first, second);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn game_screenshot_filenames_follow_stable_relation_order(pool: PgPool) -> DbResult<()> {
    let game = make_game(&pool).await?;
    let first = media::enqueue(
        &pool,
        "screenshot",
        "https://example.test/first.png",
        "unrelated-hint",
    )
    .await?;
    let second = media::enqueue(
        &pool,
        "screenshot",
        "https://example.test/second.png",
        "unrelated-hint",
    )
    .await?;
    sqlx::query("INSERT INTO game_media (game_id, media_id, position) VALUES ($1, $2, $3)")
        .bind(game)
        .bind(first)
        .bind(0)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO game_media (game_id, media_id, position) VALUES ($1, $2, $3)")
        .bind(game)
        .bind(second)
        .bind(1)
        .execute(&pool)
        .await?;

    let first_row = media::get(&pool, first).await?.expect("queued first media");
    let second_row = media::get(&pool, second).await?.expect("queued second media");
    let (first_base, first_number) = media::public_filename_input(&pool, &first_row).await?;
    let (second_base, second_number) = media::public_filename_input(&pool, &second_row).await?;
    assert_eq!((first_base.as_str(), first_number), ("game-screenshot", 1));
    assert_eq!((second_base.as_str(), second_number), ("game-screenshot", 2));

    let (first_name, second_name) = tokio::try_join!(
        media::assign_public_filename(&pool, first, &first_base, first_number),
        media::assign_public_filename(&pool, second, &second_base, second_number),
    )?;
    assert_eq!(first_name, "game-screenshot-1.avif");
    assert_eq!(second_name, "game-screenshot-2.avif");
    assert!(!first_name.contains('@'));
    assert!(!first_name.contains(&first_row.hash));
    Ok(())
}
