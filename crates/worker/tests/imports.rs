//! Importer behavior: tarball filtering, source mapping from the real fixtures, and placement
//! against a real database. None of these tests touch the network.

use std::collections::HashMap;
use std::io::Write;

use recomphub_db::PgPool;
use recomphub_db::ports::PortFull;
use recomphub_ingest::igdb::IgdbClient;
use recomphub_ingest::portsdr::parse_cards;
use recomphub_ingest::recompendium::parse_entry;
use recomphub_worker::imports::{
    Placement, SourcePort, place, portsdr_source, recompendium_files, recompendium_source,
};

const ZELDA_YAML: &str = include_str!("../../ingest/tests/fixtures/zelda-majoras-mask.yaml");
const PORTSDR_HTML: &str = include_str!("../../ingest/tests/fixtures/portsdr_card.html");
const MAJORAS_TITLE: &str = "The Legend of Zelda: Majora's Mask";

fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(gz);
    for (path, content) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        builder
            .append_data(&mut header, path, content.as_bytes())
            .expect("appending tar entry");
    }
    let mut gz = builder.into_inner().expect("finishing tar");
    gz.flush().expect("flushing gzip");
    gz.finish().expect("finishing gzip")
}

fn zelda_source() -> SourcePort {
    let entry = parse_entry("src/data/recomps/zelda-majoras-mask.yaml", ZELDA_YAML)
        .expect("parsing fixture")
        .expect("fixture has a repo");
    recompendium_source(&entry)
}

fn offline_client() -> IgdbClient {
    IgdbClient::new("client-id".to_string(), "client-secret".to_string())
        .expect("building the IGDB client does not touch the network")
}

#[test]
fn tarball_keeps_only_recomp_data_files_without_the_top_directory() {
    let archive = tarball(&[
        (
            "nio03-unricopie-1a2b3c/src/data/recomps/zelda.yaml",
            "name: a",
        ),
        ("nio03-unricopie-1a2b3c/src/data/ports/bar.yaml", "name: b"),
        (
            "nio03-unricopie-1a2b3c/src/data/decomps/baz.yaml",
            "name: c",
        ),
        ("nio03-unricopie-1a2b3c/src/data/mods/skip.yaml", "name: d"),
        ("nio03-unricopie-1a2b3c/src/data/tools/skip.yaml", "name: e"),
        (
            "nio03-unricopie-1a2b3c/src/data/recomps/notes.txt",
            "name: f",
        ),
        ("nio03-unricopie-1a2b3c/README.md", "# readme"),
    ]);

    let files = recompendium_files(&archive).expect("reading the tarball");
    let mut paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    paths.sort_unstable();

    assert_eq!(
        paths,
        vec![
            "src/data/decomps/baz.yaml",
            "src/data/ports/bar.yaml",
            "src/data/recomps/zelda.yaml",
        ]
    );
    let zelda = files
        .iter()
        .find(|(p, _)| p == "src/data/recomps/zelda.yaml")
        .expect("zelda entry present");
    assert_eq!(zelda.1, "name: a");
}

#[test]
fn recompendium_fixture_maps_to_a_complete_human_made_static_recomp() {
    let src = zelda_source();

    assert_eq!(src.source, "import:recompendium");
    assert_eq!(src.game_title, MAJORAS_TITLE);
    assert_eq!(src.platform_slug, Some("n64"));
    assert_eq!(src.port.kind, "static_recomp");
    assert_eq!(src.port.playability, "complete");
    assert_eq!(src.port.ai_usage, "none");
    assert_eq!(src.port.ai_source.as_deref(), Some("imported"));
    assert!(
        src.port
            .repo_url
            .eq_ignore_ascii_case("https://github.com/zelda64recomp/zelda64recomp"),
        "repo_url was {}",
        src.port.repo_url
    );

    let names: Vec<&str> = src.authors.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["Zelda64Recomp", "Mr. Wiseguy"]);
    assert!(
        src.authors
            .iter()
            .all(|a| a.role == "credited" && a.github_login.is_none())
    );
}

#[test]
fn portsdr_fixture_maps_to_a_windows_port_with_unknown_ai() {
    let card = parse_cards(PORTSDR_HTML)
        .into_iter()
        .next()
        .expect("fixture has one card");
    let src = portsdr_source(&card);

    assert_eq!(src.source, "import:portsdr");
    assert_eq!(src.game_title, "Ace Combat 6: Fires of Liberation");
    assert_eq!(src.port.name, "AC6_recomp");
    assert!(
        src.port
            .repo_url
            .eq_ignore_ascii_case("https://github.com/sal063/ac6_recomp"),
        "repo_url was {}",
        src.port.repo_url
    );
    assert_eq!(src.port.declared_platforms, vec!["windows".to_string()]);
    assert_eq!(src.port.ai_usage, "unknown");
    assert_eq!(src.port.ai_source, None);
    assert_eq!(src.platform_slug, Some("xbox360"));
    assert!(src.authors.is_empty());
}

async fn seed_n64(pool: &PgPool) -> anyhow::Result<i64> {
    let platform_id: i32 = sqlx::query_scalar(
        "INSERT INTO platforms (igdb_id, slug, name) VALUES (4, 'n64', 'Nintendo 64') RETURNING id",
    )
    .fetch_one(pool)
    .await?;
    let game_id: i64 = sqlx::query_scalar(
        "INSERT INTO games (igdb_id, slug, name) VALUES (1009, 'the-legend-of-zelda-majoras-mask', $1) RETURNING id",
    )
    .bind(MAJORAS_TITLE)
    .fetch_one(pool)
    .await?;
    sqlx::query("INSERT INTO game_platforms (game_id, platform_id) VALUES ($1, $2)")
        .bind(game_id)
        .bind(platform_id)
        .execute(pool)
        .await?;
    Ok(game_id)
}

fn n64_platforms() -> HashMap<String, i32> {
    HashMap::from([("n64".to_string(), 4)])
}

#[sqlx::test(migrations = "../db/migrations")]
async fn matched_game_publishes_the_port_with_sources_and_credited_authors(
    pool: PgPool,
) -> anyhow::Result<()> {
    seed_n64(&pool).await?;
    let client = offline_client();

    let placement = place(&pool, &client, &n64_platforms(), zelda_source()).await?;
    let Placement::Published(port_id) = placement else {
        anyhow::bail!("expected Published, got {placement:?}");
    };

    let (status, sources): (String, Vec<String>) =
        sqlx::query_as("SELECT status, sources FROM ports WHERE id = $1")
            .bind(port_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(status, "published");
    assert_eq!(sources, vec!["import:recompendium".to_string()]);

    let authors: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM port_authors WHERE port_id = $1 AND role = 'credited' ORDER BY position",
    )
    .bind(port_id)
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        authors,
        vec!["Zelda64Recomp".to_string(), "Mr. Wiseguy".to_string()]
    );
    Ok(())
}

#[sqlx::test(migrations = "../db/migrations")]
async fn repeat_import_fills_empty_fields_of_the_existing_port(pool: PgPool) -> anyhow::Result<()> {
    seed_n64(&pool).await?;
    let client = offline_client();

    let mut first = zelda_source();
    first.port.import_description = None;
    let Placement::Published(port_id) = place(&pool, &client, &n64_platforms(), first).await?
    else {
        anyhow::bail!("first import should publish");
    };

    let second = zelda_source();
    let expected_blurb = second.port.import_description.clone();
    let placement = place(&pool, &client, &n64_platforms(), second).await?;
    assert_eq!(placement, Placement::Filled(port_id));

    let blurb: Option<String> =
        sqlx::query_scalar("SELECT import_description FROM ports WHERE id = $1")
            .bind(port_id)
            .fetch_one(&pool)
            .await?;
    assert!(
        expected_blurb.is_some(),
        "fixture should carry a description"
    );
    assert_eq!(blurb, expected_blurb);

    let port_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ports WHERE repo_url LIKE '%zelda64recomp%'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        port_count, 1,
        "a repeat import must not insert a second port"
    );
    Ok(())
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unmatched_platform_queues_one_candidate_and_dedupes_repeats(
    pool: PgPool,
) -> anyhow::Result<()> {
    let client = offline_client();
    let mut src = zelda_source();
    src.platform_slug = None;

    let first = place(&pool, &client, &HashMap::new(), src.clone()).await?;
    assert_eq!(first, Placement::Candidate { inserted: true });
    let second = place(&pool, &client, &HashMap::new(), src).await?;
    assert_eq!(second, Placement::Candidate { inserted: false });

    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT status, source, payload->>'needs_game_match' FROM submissions WHERE kind = 'candidate'",
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        rows,
        vec![(
            "pending".to_string(),
            "import:recompendium".to_string(),
            "true".to_string()
        )]
    );
    Ok(())
}

#[sqlx::test(migrations = "../db/migrations")]
async fn candidate_payload_deserializes_as_a_new_port_for_approval(
    pool: PgPool,
) -> anyhow::Result<()> {
    let client = offline_client();
    let mut src = zelda_source();
    src.platform_slug = None;
    place(&pool, &client, &HashMap::new(), src).await?;

    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM submissions WHERE kind = 'candidate' AND status = 'pending'",
    )
    .fetch_one(&pool)
    .await?;
    let full: PortFull = serde_json::from_value(payload.clone())?;

    assert_eq!(
        full.input.repo_url.to_lowercase(),
        "https://github.com/zelda64recomp/zelda64recomp"
    );
    assert_eq!(full.input.kind, "static_recomp");
    assert_eq!(full.authors.len(), 2);
    assert_eq!(payload["game_title"], MAJORAS_TITLE);
    assert_eq!(payload["console"], "n64");
    Ok(())
}
