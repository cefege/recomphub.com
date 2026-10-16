//! Port video discovery with yt-dlp (`worker port-videos`).
//!
//! Two sources feed the `videos` table, both attributed to the port:
//! 1. YouTube videos linked from the port's README. `github-sync` stores the IDs in
//!    `port_readme_videos`. They are trusted as gameplay and skip the title filter.
//! 2. yt-dlp searches per port, filtered by `judge`.
//!
//! Each run resolves a bounded number of README refs, then searches a bounded number of
//! ports, stalest first, with a pause before every yt-dlp call. A failed search is logged,
//! counted, and recorded on the port. The port still rotates to the back of the queue.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use recomphub_db::PgPool;
use recomphub_db::media;
use recomphub_db::port_videos as store;
use recomphub_db::port_videos::SearchPort;
use recomphub_db::videos::{self, NewVideo};
use recomphub_shared::{TargetPlatform, normalize_title};

use crate::github::parse_repo;
use crate::video_links::is_youtube_id;
use crate::youtube::thumb_url;
use crate::ytdlp::{VideoMeta, YtDlp};

/// README refs that fail this many times are no longer retried.
pub const MAX_README_ATTEMPTS: i32 = 5;
const README_PER_RUN: i64 = 100;
const PORTS_PER_RUN: i64 = 20;
const SEARCH_RESULTS: u32 = 10;
/// Most new videos stored for one port in one run.
const KEEP_PER_PORT: usize = 5;
/// Pause before each yt-dlp call, so a run stays well under YouTube's rate limits.
const REQUEST_GAP: Duration = Duration::from_secs(5);
/// Search hits shorter than this are clips, not playthroughs.
const MIN_DURATION_SECS: u32 = 120;
/// Stop the run after this many yt-dlp search failures in a row. Blocks such as bot checks and
/// rate limits hit every port, so the remaining ports stay due for the next run.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;
/// Title words that mark a trailer, announcement, or short. Matched on whole words.
const EXCLUDED_TITLE_WORDS: &[&str] = &[
    "trailer",
    "teaser",
    "announcement",
    "announce",
    "reveal",
    "promo",
    "live",
    "livestream",
    "shorts",
];
/// Title words that show the video is about a port or recompilation, not the original game.
const PORT_TITLE_WORDS: &[&str] = &[
    "recomp",
    "recompiled",
    "recompilation",
    "port",
    "ported",
    "porting",
    "decomp",
    "decompiled",
];

/// Names a port is known by. `game` holds the game's name and alternate names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortNames {
    pub port: String,
    pub repo: Option<String>,
    pub game: Vec<String>,
}

/// Outcome of `judge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Keep,
    Reject(&'static str),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PortVideoReport {
    pub readme_stored: u32,
    pub readme_failed: u32,
    pub ports_searched: u32,
    pub search_failed: u32,
    pub stored: u32,
    pub rejected: u32,
}

/// True when the port is named exactly like its game, e.g. "Perfect Dark".
fn port_shares_game_name(names: &PortNames) -> bool {
    let port = normalize_title(&names.port);
    names.game.iter().any(|g| normalize_title(g) == port)
}

/// Search queries for one port, most specific first, with duplicates removed after
/// normalization. A port named like its game gets port-specific queries first, because plain
/// queries mostly return original-game videos.
pub fn search_queries(names: &PortNames) -> Vec<String> {
    let mut candidates = Vec::new();
    if port_shares_game_name(names) {
        candidates.push(format!("{} pc port", names.port));
        candidates.push(format!("{} decomp", names.port));
    }
    candidates.push(format!("{} gameplay", names.port));
    candidates.push(names.port.clone());
    if let Some(repo) = &names.repo {
        candidates.push(format!("{repo} gameplay"));
    }
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|q| seen.insert(normalize_title(q)))
        .collect()
}

/// Decides whether a search hit is gameplay of this port. README videos skip this check.
///
/// A hit is kept when its title names the port. A port named exactly like its game also needs a
/// port or recompilation word. It is also kept when the title names the game together with a
/// port or recompilation word. Clips under two minutes, trailers, and anything already stored
/// are rejected.
pub fn judge(hit: &VideoMeta, names: &PortNames, known: &HashSet<String>) -> Verdict {
    if known.contains(&hit.youtube_id) {
        return Verdict::Reject("already stored");
    }
    if hit.duration_secs.is_some_and(|d| d < MIN_DURATION_SECS) {
        return Verdict::Reject("shorter than two minutes");
    }
    let title = normalize_title(&hit.title);
    let words: Vec<&str> = title.split_whitespace().collect();
    if words.iter().any(|w| EXCLUDED_TITLE_WORDS.contains(w)) {
        return Verdict::Reject("trailer, teaser, announcement, short, or livestream");
    }
    let padded = format!(" {title} ");
    let mentions = |name: &str| {
        let n = normalize_title(name);
        !n.is_empty() && padded.contains(&format!(" {n} "))
    };
    let has_port_word = words.iter().any(|w| PORT_TITLE_WORDS.contains(w));
    // A port named exactly like its game (e.g. "Perfect Dark") also needs a port word, or
    // original-game videos match. Repo names are not used here: generic repo names such as
    // "Shipwright" match unrelated titles.
    let port_is_game = port_shares_game_name(names);
    if mentions(&names.port) && (has_port_word || !port_is_game) {
        return Verdict::Keep;
    }
    if has_port_word && names.game.iter().any(|g| mentions(g)) {
        return Verdict::Keep;
    }
    Verdict::Reject("no port name, or no game name with a port word")
}

/// Target for videos of a port: its platform when exactly one is declared, otherwise `any`.
pub fn target_for(platforms: &[String]) -> String {
    match platforms {
        [only] if only.parse::<TargetPlatform>().is_ok() => only.clone(),
        _ => "any".to_string(),
    }
}

fn names_for(p: &SearchPort) -> PortNames {
    let repo = parse_repo(&p.repo_url).ok().map(|(_, repo)| repo);
    let mut game = vec![p.game_name.clone()];
    game.extend(p.game_alt_names.iter().cloned());
    PortNames {
        port: p.port_name.clone(),
        repo,
        game,
    }
}

async fn store_video(
    pool: &PgPool,
    game_id: i64,
    port_id: i64,
    target: &str,
    meta: &VideoMeta,
) -> Result<()> {
    let thumb = media::enqueue(pool, "video_thumb", &thumb_url(&meta.youtube_id)).await?;
    videos::upsert_video(
        pool,
        &NewVideo {
            game_id,
            port_id: Some(port_id),
            target: target.to_string(),
            youtube_id: meta.youtube_id.clone(),
            title: meta.title.clone(),
            channel: meta.channel.clone(),
            published_at: meta.published_at,
            thumb_media_id: Some(thumb),
        },
    )
    .await?;
    Ok(())
}

/// Runs one pass of README resolution and port searches. Never aborts on a single
/// failed video or search; only database errors stop the run.
pub async fn run_port_video_sync(pool: &PgPool, yt: &YtDlp) -> Result<PortVideoReport> {
    let mut report = PortVideoReport::default();
    let mut consecutive_failures = 0u32;

    let refs = store::due_readme_refs(pool, MAX_README_ATTEMPTS, README_PER_RUN).await?;
    for r in &refs {
        tokio::time::sleep(REQUEST_GAP).await;
        if !is_youtube_id(&r.youtube_id) {
            continue;
        }
        match yt.video(&r.youtube_id).await {
            Ok(meta) => {
                let target = target_for(&r.platforms);
                store_video(pool, r.game_id, r.port_id, &target, &meta).await?;
                store::mark_readme_resolved(pool, r.port_id, &r.youtube_id).await?;
                report.readme_stored += 1;
            }
            Err(e) => {
                let error = format!("{e:#}");
                tracing::warn!(port_id = r.port_id, video = %r.youtube_id, error = %error, "README video not resolved");
                store::mark_readme_failed(pool, r.port_id, &r.youtube_id, &error).await?;
                report.readme_failed += 1;
            }
        }
    }

    let ports = store::ports_due_for_search(pool, PORTS_PER_RUN).await?;
    for p in &ports {
        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            tracing::warn!("stopping port video searches after repeated yt-dlp failures");
            break;
        }
        let names = names_for(p);
        let target = target_for(&p.platforms);
        let mut seen = store::known_youtube_ids(pool, p.game_id).await?;
        let mut kept = 0usize;
        let mut last_error: Option<String> = None;
        let mut kept_channels: HashSet<String> = HashSet::new();

        'queries: for query in search_queries(&names) {
            tokio::time::sleep(REQUEST_GAP).await;
            let hits = match yt.search(&query, SEARCH_RESULTS).await {
                Ok(hits) => {
                    consecutive_failures = 0;
                    hits
                }
                Err(e) => {
                    let error = format!("{e:#}");
                    tracing::warn!(port_id = p.port_id, query = %query, error = %error, "port video search failed");
                    report.search_failed += 1;
                    consecutive_failures += 1;
                    last_error = Some(error);
                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        break 'queries;
                    }
                    continue;
                }
            };
            for hit in hits {
                match judge(&hit, &names, &seen) {
                    Verdict::Keep
                        if hit
                            .channel
                            .as_ref()
                            .is_some_and(|c| kept_channels.contains(c)) =>
                    {
                        report.rejected += 1;
                        tracing::debug!(port_id = p.port_id, video = %hit.youtube_id, "channel already kept for this port");
                    }
                    Verdict::Keep => {
                        store_video(pool, p.game_id, p.port_id, &target, &hit).await?;
                        seen.insert(hit.youtube_id.clone());
                        if let Some(channel) = &hit.channel {
                            kept_channels.insert(channel.clone());
                        }
                        report.stored += 1;
                        kept += 1;
                        if kept >= KEEP_PER_PORT {
                            break 'queries;
                        }
                    }
                    Verdict::Reject(reason) => {
                        report.rejected += 1;
                        tracing::debug!(port_id = p.port_id, video = %hit.youtube_id, title = %hit.title, reason, "search hit rejected");
                    }
                }
            }
        }

        store::mark_search_run(pool, p.port_id, last_error.as_deref(), Utc::now()).await?;
        report.ports_searched += 1;
        tracing::info!(port_id = p.port_id, port = %p.port_name, kept, "port searched");
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn names() -> PortNames {
        PortNames {
            port: "Ship of Harkinian".into(),
            repo: Some("Shipwright".into()),
            game: vec!["The Legend of Zelda: Ocarina of Time".into()],
        }
    }

    fn hit(id: &str, title: &str, duration: Option<u32>) -> VideoMeta {
        VideoMeta {
            youtube_id: id.into(),
            title: title.into(),
            channel: None,
            published_at: Some(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()),
            duration_secs: duration,
        }
    }

    #[test]
    fn queries_cover_port_name_and_repo_without_duplicates() {
        assert_eq!(
            search_queries(&names()),
            vec![
                "Ship of Harkinian gameplay",
                "Ship of Harkinian",
                "Shipwright gameplay"
            ]
        );
        let same = PortNames {
            port: "Zelda64Recomp".into(),
            repo: Some("zelda64recomp".into()),
            game: vec![],
        };
        assert_eq!(
            search_queries(&same),
            vec!["Zelda64Recomp gameplay", "Zelda64Recomp"]
        );
    }

    #[test]
    fn same_name_port_queries_lead_with_port_words() {
        let names = PortNames {
            port: "Perfect Dark".into(),
            repo: Some("perfect_dark".into()),
            game: vec!["Perfect Dark".into()],
        };
        assert_eq!(
            search_queries(&names),
            vec![
                "Perfect Dark pc port",
                "Perfect Dark decomp",
                "Perfect Dark gameplay",
                "Perfect Dark" // "perfect_dark gameplay" normalizes to the same query as "Perfect Dark gameplay".
            ]
        );
    }

    #[test]
    fn livestreams_are_rejected() {
        let names = PortNames {
            port: "Zelda64Recomp".into(),
            repo: None,
            game: vec!["The Legend of Zelda: Majora's Mask".into()],
        };
        let live = judge(
            &hit(
                "LLLLLLLLLLL",
                "The Legend of Zelda: Majora's Mask LIVE | Zelda64Recomp Gameplay!",
                Some(7261),
            ),
            &names,
            &HashSet::new(),
        );
        assert!(matches!(live, Verdict::Reject(_)));
        let vod = judge(
            &hit(
                "MMMMMMMMMMM",
                "Zelda64Recomp Gameplay | Majora's Mask on PC",
                Some(1800),
            ),
            &names,
            &HashSet::new(),
        );
        assert_eq!(vod, Verdict::Keep);
    }

    #[test]
    fn port_name_in_title_is_kept() {
        let v = judge(
            &hit(
                "AAAAAAAAAAA",
                "Ship of Harkinian - Full Playthrough",
                Some(7200),
            ),
            &names(),
            &HashSet::new(),
        );
        assert_eq!(v, Verdict::Keep);
    }

    #[test]
    fn repo_name_alone_is_not_enough() {
        let v = judge(
            &hit("AAAAAAAAAAA", "Shipwright Dungeon Speedrun", Some(900)),
            &names(),
            &HashSet::new(),
        );
        assert!(matches!(v, Verdict::Reject(_)));
    }

    #[test]
    fn same_name_port_needs_a_port_word() {
        let names = PortNames {
            port: "Perfect Dark".into(),
            repo: Some("perfect_dark".into()),
            game: vec!["Perfect Dark".into()],
        };
        let original = judge(
            &hit("DDDDDDDDDDD", "Perfect Dark - N64 Gameplay", Some(900)),
            &names,
            &HashSet::new(),
        );
        assert!(matches!(original, Verdict::Reject(_)));
        let port = judge(
            &hit("EEEEEEEEEEE", "Perfect Dark PC port gameplay", Some(900)),
            &names,
            &HashSet::new(),
        );
        assert_eq!(port, Verdict::Keep);
    }

    #[test]
    fn game_name_needs_a_port_word_to_be_kept() {
        let known = HashSet::new();
        let with_word = hit(
            "BBBBBBBBBBB",
            "The Legend of Zelda: Ocarina of Time PC port gameplay",
            Some(1800),
        );
        assert_eq!(judge(&with_word, &names(), &known), Verdict::Keep);
        let without_word = hit(
            "CCCCCCCCCCC",
            "The Legend of Zelda: Ocarina of Time N64 walkthrough",
            Some(1800),
        );
        assert!(matches!(
            judge(&without_word, &names(), &known),
            Verdict::Reject(_)
        ));
    }

    #[test]
    fn trailers_teasers_and_clips_are_rejected() {
        let known = HashSet::new();
        assert!(matches!(
            judge(
                &hit("DDDDDDDDDDD", "Ship of Harkinian Trailer", Some(90)),
                &names(),
                &known
            ),
            Verdict::Reject(_)
        ));
        assert!(matches!(
            judge(
                &hit("EEEEEEEEEEE", "Ship of Harkinian gameplay", Some(45)),
                &names(),
                &known
            ),
            Verdict::Reject(_)
        ));
        assert!(matches!(
            judge(
                &hit("FFFFFFFFFFF", "Ship of Harkinian teaser", Some(600)),
                &names(),
                &known
            ),
            Verdict::Reject(_)
        ));
    }

    #[test]
    fn unrelated_titles_are_rejected() {
        assert!(matches!(
            judge(
                &hit("GGGGGGGGGGG", "Cats napping for six hours", Some(21600)),
                &names(),
                &HashSet::new()
            ),
            Verdict::Reject(_)
        ));
    }

    #[test]
    fn already_stored_videos_are_rejected() {
        let known: HashSet<String> = ["HHHHHHHHHHH".to_string()].into_iter().collect();
        assert!(matches!(
            judge(
                &hit("HHHHHHHHHHH", "Ship of Harkinian gameplay", Some(900)),
                &names(),
                &known
            ),
            Verdict::Reject("already stored")
        ));
    }

    #[test]
    fn unknown_duration_is_not_a_reason_to_reject() {
        assert_eq!(
            judge(
                &hit("IIIIIIIIIII", "Ship of Harkinian gameplay", None),
                &names(),
                &HashSet::new()
            ),
            Verdict::Keep
        );
    }

    #[test]
    fn target_is_the_single_declared_platform_or_any() {
        assert_eq!(target_for(&["windows".into()]), "windows");
        assert_eq!(target_for(&["windows".into(), "linux".into()]), "any");
        assert_eq!(target_for(&[]), "any");
        assert_eq!(target_for(&["not-a-platform".into()]), "any");
    }
}
