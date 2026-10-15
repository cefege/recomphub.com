//! YouTube query templates, title filtering and response parsing. No network.

use recomphub_ingest::youtube::{
    is_quota_error, parse_search, query_for, thumb_url, title_matches,
};

#[test]
fn query_templates_follow_the_target() {
    assert_eq!(
        query_for("windows", "Majora's Mask", "Majora's Mask Recomp").unwrap(),
        "Majora's Mask Majora's Mask Recomp PC gameplay"
    );
    assert_eq!(
        query_for("web", "Ace Combat 6", "AC6").unwrap(),
        "Ace Combat 6 browser port"
    );
    assert_eq!(
        query_for("any", "Ace Combat 6", "AC6").unwrap(),
        "Ace Combat 6 recomp"
    );
}

#[test]
fn unknown_target_is_an_error_not_a_guess() {
    assert!(query_for("playstation", "Ace Combat 6", "AC6").is_err());
}

#[test]
fn title_must_share_a_real_token_with_the_game_name() {
    assert!(title_matches(
        "Majora's Mask PC gameplay",
        "The Legend of Zelda: Majora's Mask"
    ));
    // Only "6" is shared, and tokens shorter than three characters do not count.
    assert!(!title_matches("Cats 6 hours of napping", "Ace Combat 6"));
    assert!(!title_matches("Unrelated video", "Ace Combat 6"));
}

#[test]
fn thumbnail_url_uses_the_hqdefault_pattern() {
    assert_eq!(
        thumb_url("abc123"),
        "https://i.ytimg.com/vi/abc123/hqdefault.jpg"
    );
}

#[test]
fn quota_error_requires_403_with_quota_exceeded_reason() {
    let quota = r#"{"error":{"code":403,"errors":[{"reason":"quotaExceeded"}]}}"#;
    let other = r#"{"error":{"code":403,"errors":[{"reason":"forbidden"}]}}"#;
    assert!(is_quota_error(403, quota));
    assert!(!is_quota_error(403, other));
    assert!(!is_quota_error(500, quota));
    assert!(!is_quota_error(403, "not json"));
}

#[test]
fn search_parsing_keeps_video_results_and_drops_others() {
    let body = r#"{
        "items": [
            {
                "id": {"kind": "youtube#video", "videoId": "vid1"},
                "snippet": {
                    "title": "Majora's Mask PC",
                    "channelTitle": "Some Channel",
                    "publishedAt": "2024-05-06T07:08:09Z"
                }
            },
            {
                "id": {"kind": "youtube#channel"},
                "snippet": {"title": "A channel", "channelTitle": "Ch"}
            },
            {
                "id": {"kind": "youtube#video", "videoId": "vid2"},
                "snippet": {"title": "No channel name", "channelTitle": ""}
            }
        ]
    }"#;
    let hits = parse_search(body).expect("valid search body");
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].youtube_id, "vid1");
    assert_eq!(hits[0].channel.as_deref(), Some("Some Channel"));
    assert_eq!(
        hits[0].published_at.unwrap().to_rfc3339(),
        "2024-05-06T07:08:09+00:00"
    );
    assert_eq!(hits[1].youtube_id, "vid2");
    assert_eq!(hits[1].channel, None);
}

#[test]
fn search_parsing_rejects_a_bad_timestamp() {
    let body =
        r#"{"items":[{"id":{"videoId":"v"},"snippet":{"title":"t","publishedAt":"yesterday"}}]}"#;
    assert!(parse_search(body).is_err());
}
