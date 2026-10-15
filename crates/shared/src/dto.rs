//! Serde DTOs shared by the server functions and the Dioxus pages.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// A self-hosted media item. `ready` is false while the pipeline has not
/// produced files, and pages then show a placeholder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaRef {
    pub id: i64,
    pub kind: String,
    pub hash: String,
    pub widths: Vec<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub ready: bool,
}

/// Compact game data for cards and rails.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameCard {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub release_year: Option<i32>,
    pub cover: Option<MediaRef>,
    pub port_count: i32,
    pub want_count: i32,
    pub targets: Vec<String>,
    pub best_playability: Option<String>,
    pub popularity: i32,
}

/// A game in a ranking window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankedGame {
    pub game: GameCard,
    pub rank: i64,
    pub prev_rank: Option<i64>,
    pub score: f64,
    pub hits: i64,
    pub viewer_minutes: i64,
}

/// An original platform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlatformRef {
    pub id: i32,
    pub slug: Option<String>,
    pub name: String,
    pub abbreviation: Option<String>,
    pub family: Option<String>,
    pub game_count: i64,
    pub ported_count: i64,
}

/// A port's author row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorDto {
    pub github_login: Option<String>,
    pub name: String,
    pub role: String,
    pub avatar: Option<MediaRef>,
    pub commits: Option<i32>,
}

/// One AI evidence line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiEvidenceDto {
    pub signal: String,
    pub detail: String,
}

/// A port as shown in a card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortCard {
    pub id: i64,
    pub slug: String,
    pub game_id: i64,
    pub game_slug: String,
    pub game_name: String,
    pub name: String,
    pub kind: String,
    pub kind_label: String,
    pub targets: Vec<String>,
    pub authors: Vec<AuthorDto>,
    pub ai_usage: String,
    pub ai_tools: Vec<String>,
    pub ai_evidence: Vec<AiEvidenceDto>,
    pub playability: String,
    pub latest_version: Option<String>,
    pub latest_release_url: Option<String>,
    pub latest_release_at: Option<DateTime<Utc>>,
    pub release_state: String,
    pub release_state_label: String,
    pub github_stars: i32,
    pub star_count: i32,
    pub repo_url: String,
    pub cover: Option<MediaRef>,
}

/// A gameplay video.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoDto {
    pub id: i64,
    pub youtube_id: String,
    pub title: String,
    pub channel: Option<String>,
    pub target: String,
    pub thumb: Option<MediaRef>,
    pub hidden: bool,
}

/// Full game page data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameDetail {
    pub game: GameCard,
    pub summary: Option<String>,
    pub alt_names: Vec<String>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub genres: Vec<String>,
    pub first_release_date: Option<NaiveDate>,
    pub platforms: Vec<PlatformRef>,
    pub screenshots: Vec<MediaRef>,
    pub og: Option<MediaRef>,
    pub ports: Vec<PortCard>,
    pub decompilations: Vec<PortCard>,
    pub videos: Vec<VideoDto>,
    pub related: Vec<GameCard>,
    pub total_github_stars: i64,
    pub live_viewers: u32,
}

/// Full port page data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortDetail {
    pub port: PortCard,
    pub game: GameCard,
    pub source_platform: Option<PlatformRef>,
    pub homepage_url: Option<String>,
    pub description: Option<String>,
    pub requirements: Option<String>,
    pub toolchain: Option<String>,
    pub license: Option<String>,
    pub latest_version_label: Option<String>,
    pub last_commit_at: Option<DateTime<Utc>>,
    pub ai_source: Option<String>,
    pub other_ports: Vec<PortCard>,
    pub videos: Vec<VideoDto>,
}

/// One page of a listing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: i64,
    pub pages: i64,
    pub total: i64,
}

/// Session user as seen by the browser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionUser {
    pub id: i64,
    pub login: String,
    pub name: Option<String>,
    pub role: String,
    pub avatar: Option<MediaRef>,
}

/// The viewer's own stars and wants for a set of ids.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Marks {
    pub starred: Vec<i64>,
    pub wanted: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StarStateDto {
    pub starred: bool,
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WantStateDto {
    pub wanted: bool,
    pub count: i64,
}

/// Search-as-you-type result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuggestItem {
    pub label: String,
    pub sublabel: String,
    pub href: String,
    pub cover: Option<MediaRef>,
}

/// Search page result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub games: Vec<GameCard>,
    pub ports: Vec<PortCard>,
}

/// Outcome of previewing a GitHub repo for submission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RepoPreview {
    AlreadyListed {
        game_slug: String,
        port_slug: String,
    },
    Preview(Box<RepoPreviewData>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoPreviewData {
    pub repo_url: String,
    pub owner: String,
    pub name: String,
    pub description: Option<String>,
    pub homepage_url: Option<String>,
    pub license: Option<String>,
    pub detected_platforms: Vec<String>,
    pub ai_usage: Option<String>,
    pub ai_tools: Vec<String>,
    pub ai_evidence: Vec<AiEvidenceDto>,
    pub contributors: Vec<AuthorDto>,
    pub owner_is_org: bool,
    pub submitter_is_author: bool,
    pub stars: i32,
    pub latest_version: Option<String>,
}

/// One game the user can pick for a submission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameChoice {
    pub game_id: Option<i64>,
    pub igdb_id: Option<i64>,
    pub name: String,
    pub release_year: Option<i32>,
    pub platforms: Vec<ChoicePlatform>,
}

/// A platform offered for a game choice. `id` is the local `platforms.id`
/// and is None for IGDB-only choices, which have no local row yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoicePlatform {
    pub id: Option<i32>,
    pub igdb_id: Option<i64>,
    pub name: String,
}

/// Moderator choices applied when approving a submission. Each field is
/// optional: `game_id` or `igdb_id` picks the game for a candidate or an
/// unmatched import, and `kind` overrides the submission's kind.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReviewOverrides {
    pub game_id: Option<i64>,
    pub igdb_id: Option<i64>,
    pub kind: Option<String>,
}

/// A port form, as typed by a user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortForm {
    pub name: String,
    pub kind: String,
    pub source_platform_id: Option<i32>,
    pub repo_url: String,
    pub homepage_url: Option<String>,
    pub description: Option<String>,
    pub requirements: Option<String>,
    pub toolchain: Option<String>,
    pub license: Option<String>,
    pub playability: String,
    pub declared_platforms: Vec<String>,
    pub ai_usage: String,
    pub ai_tools: Vec<String>,
    pub authors: Vec<AuthorForm>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorForm {
    pub github_login: Option<String>,
    pub name: String,
    pub role: String,
}

/// A submission from the submit page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmitInput {
    /// Existing game, or `None` when `igdb_id` is set and the game is new.
    pub game_id: Option<i64>,
    pub igdb_id: Option<i64>,
    pub port: PortForm,
    pub submitter_is_author: bool,
}

/// Result of a submission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmitResult {
    pub submission_id: i64,
    pub published: bool,
    pub game_slug: Option<String>,
    pub port_slug: Option<String>,
}

/// The current values of a port, loaded into the edit form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditPortForm {
    pub port_id: i64,
    pub game_slug: String,
    pub game_name: String,
    pub port: PortForm,
}

/// An edit to an existing port. Only changed fields are set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditInput {
    pub port_id: i64,
    pub name: Option<String>,
    pub kind: Option<String>,
    #[serde(
        default,
        with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub homepage_url: Option<Option<String>>,
    #[serde(
        default,
        with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<Option<String>>,
    #[serde(
        default,
        with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub requirements: Option<Option<String>>,
    #[serde(
        default,
        with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub toolchain: Option<Option<String>>,
    pub playability: Option<String>,
    pub declared_platforms: Option<Vec<String>>,
    pub ai_usage: Option<String>,
    pub ai_tools: Option<Vec<String>>,
}

/// Serde adapter for `Option<Option<String>>` fields. An absent key is `None`
/// (leave the field unchanged), `null` is `Some(None)` (clear it), and a string
/// is `Some(Some(value))`.
mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<Option<String>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(inner) => inner.serialize(s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Option<String>>, D::Error> {
        Option::<String>::deserialize(d).map(Some)
    }
}

/// A moderation queue row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: i64,
    pub kind: String,
    pub status: String,
    pub source: String,
    pub submitter_login: Option<String>,
    pub submitter_approved: Option<i64>,
    pub payload_json: String,
    pub before_json: Option<String>,
    pub needs_game_match: bool,
    pub submitter_is_author: bool,
    pub created_at: DateTime<Utc>,
}

/// A row of the moderation log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogItem {
    pub id: i64,
    pub kind: String,
    pub status: String,
    pub source: String,
    pub target_port_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub reviewed_at: Option<DateTime<Utc>>,
}

/// A user row in the admin search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserAdminRow {
    pub id: i64,
    pub login: String,
    pub name: Option<String>,
    pub role: String,
    pub banned: bool,
    pub approved_count: i32,
}

/// One setting row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingRow {
    pub key: String,
    pub value: i64,
}

/// Home page data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HomeData {
    pub trending: Vec<RankedGame>,
    pub window_has_scores: bool,
    pub just_released: Vec<PortCard>,
    pub activity: Vec<crate::live::ActivityItem>,
    pub wanted: Vec<GameCard>,
    pub platforms: Vec<PlatformRef>,
    pub runs_on: Vec<RunsOnTile>,
    pub stats: SiteStatsDto,
    pub live_top: Vec<crate::live::LiveGame>,
    pub online: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunsOnTile {
    pub target: String,
    pub label: String,
    pub port_count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteStatsDto {
    pub games_with_port: i64,
    pub ports: i64,
    pub platforms_with_ports: i64,
}

/// Author profile for `/authors/:login`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorProfile {
    pub login: String,
    pub name: String,
    pub avatar: Option<MediaRef>,
    pub ports: Vec<PortCard>,
}

#[cfg(test)]
mod tests {
    use super::EditInput;

    fn base() -> EditInput {
        EditInput {
            port_id: 7,
            name: None,
            kind: None,
            homepage_url: None,
            description: None,
            requirements: None,
            toolchain: None,
            playability: None,
            declared_platforms: None,
            ai_usage: None,
            ai_tools: None,
        }
    }

    #[test]
    fn absent_field_stays_unchanged_on_the_wire() {
        let json = serde_json::to_string(&base()).unwrap();
        let back: EditInput = serde_json::from_str(&json).unwrap();
        assert_eq!(back.description, None);
        assert!(!json.contains("description"));
    }

    #[test]
    fn null_clears_and_value_sets_the_field() {
        let cleared: EditInput =
            serde_json::from_str(r#"{"port_id":7,"description":null}"#).unwrap();
        assert_eq!(cleared.description, Some(None));

        let set: EditInput =
            serde_json::from_str(r#"{"port_id":7,"description":"Fixed text"}"#).unwrap();
        assert_eq!(set.description, Some(Some("Fixed text".to_string())));
    }

    #[test]
    fn all_three_states_survive_a_round_trip() {
        let mut input = base();
        input.homepage_url = Some(None);
        input.requirements = Some(Some("4 GB RAM".to_string()));
        let json = serde_json::to_string(&input).unwrap();
        let back: EditInput = serde_json::from_str(&json).unwrap();
        assert_eq!(back.homepage_url, Some(None));
        assert_eq!(back.requirements, Some(Some("4 GB RAM".to_string())));
        assert_eq!(back.toolchain, None);
    }
}
