//! IGDB request bodies, title matching and catalog parsing. No network.

use chrono::NaiveDate;
use recomphub_ingest::igdb::{
    by_ids_body, catalog_body, clean_search_title, parse_games, search_body, title_matches,
};

#[test]
fn catalog_body_filters_allowed_types_and_pages_by_rating() {
    let body = catalog_body(&[7, 8, 9], 1000);
    assert!(body.contains("game_type = (7,8,9)"), "{body}");
    assert!(body.contains("total_rating_count > 0"), "{body}");
    assert!(body.contains("version_parent = null"), "{body}");
    assert!(body.contains("sort total_rating_count desc"), "{body}");
    assert!(body.contains("limit 500"), "{body}");
    assert!(body.contains("offset 1000"), "{body}");
}

#[test]
fn by_ids_body_fetches_exactly_the_requested_ids() {
    let body = by_ids_body(&[11, 22, 33]);
    assert!(body.contains("where id = (11,22,33)"), "{body}");
    assert!(body.contains("limit 500"), "{body}");
}

#[test]
fn search_body_restricts_to_platform_only_when_given() {
    let scoped = search_body("Majora's Mask", Some(4));
    assert!(scoped.starts_with("search \"Majora's Mask\";"), "{scoped}");
    assert!(scoped.contains("where platforms = (4)"), "{scoped}");

    let unscoped = search_body("Majora's Mask", None);
    assert!(!unscoped.contains("platforms ="), "{unscoped}");
}

#[test]
fn clean_search_title_removes_quotes_and_collapses_whitespace() {
    assert_eq!(clean_search_title("  Say \"Hi\"   there "), "Say Hi there");
}

#[test]
fn title_matches_on_normalized_name_or_alternative_name() {
    // The local name is "The Legend of Zelda: Majora's Mask"; the leading "the " is stripped when normalized.
    let full = recomphub_shared::normalize_title("The Legend of Zelda: Majora's Mask");
    assert_eq!(full, "legend of zelda majora s mask");
    assert!(title_matches(
        &full,
        "The Legend of Zelda: Majora's Mask",
        &[]
    ));
    // A short title only matches through an alternative name.
    let alts = vec!["Majora's Mask".to_string()];
    assert!(title_matches(
        &recomphub_shared::normalize_title("Majora's Mask"),
        "Other",
        &alts
    ));
    // Ocarina of Time matches neither the name nor the alternative.
    assert!(!title_matches(
        &full,
        "The Legend of Zelda: Ocarina of Time",
        &alts
    ));
}

#[test]
fn parse_games_maps_metadata_media_and_popularity() {
    let body = r#"[
        {
            "id": 1942,
            "name": "The Legend of Zelda: Majora's Mask",
            "slug": "the-legend-of-zelda-majoras-mask",
            "summary": "A short adventure.",
            "first_release_date": 1000000000,
            "alternative_names": [{"name": "Majora's Mask"}],
            "platforms": [4],
            "cover": {"image_id": "co1"},
            "screenshots": [
                {"image_id": "s1"}, {"image_id": "s2"}, {"image_id": "s3"},
                {"image_id": "s4"}, {"image_id": "s5"}, {"image_id": "s6"},
                {"image_id": "s7"}, {"image_id": "s8"}
            ],
            "genres": [{"name": "Adventure"}],
            "involved_companies": [
                {"company": {"name": "Nintendo"}, "developer": true, "publisher": true},
                {"company": {"name": "Nintendo EAD"}, "developer": true, "publisher": false}
            ],
            "total_rating_count": 42,
            "game_type": 0
        },
        {
            "id": 2,
            "name": "Bare Game",
            "slug": "bare-game"
        }
    ]"#;

    let games = parse_games(body).expect("valid IGDB JSON");
    assert_eq!(games.len(), 2);

    let mm = &games[0];
    assert_eq!(mm.igdb_id, 1942);
    assert_eq!(mm.slug, "the-legend-of-zelda-majoras-mask");
    assert_eq!(mm.alt_names, vec!["Majora's Mask".to_string()]);
    assert_eq!(mm.first_release_date, NaiveDate::from_ymd_opt(2001, 9, 9));
    assert_eq!(mm.developer.as_deref(), Some("Nintendo, Nintendo EAD"));
    assert_eq!(mm.publisher.as_deref(), Some("Nintendo"));
    assert_eq!(mm.popularity, 42);
    assert_eq!(mm.platform_igdb_ids, vec![4]);
    assert_eq!(
        mm.cover_url.as_deref(),
        Some("https://images.igdb.com/igdb/image/upload/t_cover_big_2x/co1.jpg")
    );
    // Only the first six screenshots are kept, at 1080p.
    assert_eq!(mm.screenshot_urls.len(), 6);
    assert_eq!(
        mm.screenshot_urls[0],
        "https://images.igdb.com/igdb/image/upload/t_1080p/s1.jpg"
    );
    // The OG image is the first screenshot at 1080p.
    assert_eq!(mm.og_url.as_deref(), Some(mm.screenshot_urls[0].as_str()));

    let bare = &games[1];
    assert_eq!(bare.popularity, 0);
    assert_eq!(bare.cover_url, None);
    assert_eq!(bare.og_url, None);
    assert!(bare.screenshot_urls.is_empty());
    assert_eq!(bare.first_release_date, None);
}
