//! Discovery keep rule and candidate payloads, plus media rendition planning. No network.

use recomphub_ingest::discover::{github_payload, keep, reddit_payload};
use recomphub_ingest::github::SearchRepo;
use recomphub_ingest::media::{extension_for, output_path, planned_widths, widths_for};
use recomphub_ingest::reddit::RedditRepo;
use std::path::Path;

#[test]
fn keep_rule_needs_five_stars_or_a_release() {
    assert!(!keep(4, false));
    assert!(keep(5, false));
    assert!(keep(0, true));
}

#[test]
fn github_candidate_payload_uses_lowercase_canonical_url() {
    let repo = SearchRepo {
        full_name: "Someone/Cool-Recomp".to_string(),
        name: "Cool-Recomp".to_string(),
        html_url: "https://github.com/Someone/Cool-Recomp".to_string(),
        description: Some("A port".to_string()),
        stars: 12,
    };
    let payload = github_payload(&repo);
    assert_eq!(
        payload["repo_url"],
        "https://github.com/someone/cool-recomp"
    );
    assert_eq!(payload["name"], "Cool-Recomp");
    assert_eq!(payload["description"], "A port");
    assert_eq!(payload["stars"], 12);
}

#[test]
fn reddit_candidate_payload_names_the_repo_and_keeps_its_source() {
    let repo = RedditRepo {
        repo_url: "https://github.com/owner/zelda-port".to_string(),
        source_url: "https://www.reddit.com/r/recomps/comments/abc/".to_string(),
        source_title: "New zelda port".to_string(),
    };
    let payload = reddit_payload(&repo);
    assert_eq!(payload["repo_url"], "https://github.com/owner/zelda-port");
    assert_eq!(payload["name"], "zelda-port");
    assert!(payload["stars"].is_null(), "the feed carries no star count");
    assert_eq!(
        payload["source_url"],
        "https://www.reddit.com/r/recomps/comments/abc/"
    );
    assert_eq!(payload["source_title"], "New zelda port");
}

#[test]
fn renditions_skip_widths_wider_than_the_source() {
    assert_eq!(planned_widths("cover", 300), vec![132, 264]);
    assert_eq!(planned_widths("screenshot", 1920), vec![480, 960, 1600]);
    assert!(
        planned_widths("og", 800).is_empty(),
        "og is 1200 wide and must not upscale"
    );
    assert_eq!(planned_widths("og", 1920), vec![1200]);
    assert!(planned_widths("unknown", 1920).is_empty());
}

#[test]
fn widths_match_the_plan_table() {
    assert_eq!(widths_for("avatar"), Some(&[40u32, 80][..]));
    assert_eq!(widths_for("video_thumb"), Some(&[320u32, 480][..]));
    assert_eq!(widths_for("bogus"), None);
}

#[test]
fn only_og_images_are_jpeg() {
    assert_eq!(extension_for("og"), "jpg");
    assert_eq!(extension_for("cover"), "avif");
    assert_eq!(extension_for("screenshot"), "avif");
}

#[test]
fn output_paths_are_kind_then_hash_then_width() {
    let path = output_path(
        Path::new("/data/media"),
        "cover",
        "0123456789abcdef",
        264,
        "avif",
    );
    assert_eq!(
        path,
        Path::new("/data/media/cover/0123456789abcdef-264.avif")
    );
}
