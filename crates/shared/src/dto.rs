//! Serde DTOs shared by the server functions and the Dioxus pages.

use crate::enums::FundingPlatform;
use crate::maintenance::MaintenanceLevel;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// A self-hosted media item. `ready` is false while the pipeline has not
/// produced files, and pages then show a placeholder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaRef {
    pub id: i64,
    pub kind: String,
    pub hash: String,
    pub public_stem: String,
    pub render_version: i32,
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

/// One donation/funding link for a GitHub author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FundingLink {
    pub platform: FundingPlatform,
    pub url: String,
}

/// A port's author row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorDto {
    pub github_login: Option<String>,
    pub name: String,
    pub role: String,
    pub avatar: Option<MediaRef>,
    pub commits: Option<i32>,
    /// Donation links for a GitHub author; empty when none are known.
    #[serde(default)]
    pub funding: Vec<FundingLink>,
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
    /// `None` until the port has been through a GitHub sync that computed maintenance signals.
    pub maintenance: Option<MaintenanceDto>,
}

/// Maintenance signals for a port. Counts are over the default branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintenanceDto {
    pub level: MaintenanceLevel,
    pub score: i32,
    pub last_commit_at: Option<DateTime<Utc>>,
    pub commits_30d: i32,
    pub commits_90d: i32,
    pub commits_365d: i32,
    pub committers_180d: i32,
    pub releases_365d: i32,
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
    /// Generated FAQ for the game. Empty when none has been generated; the page then shows the
    /// deterministic one.
    #[serde(default)]
    pub faq: Vec<FaqItem>,
}

/// One question and the answer shown under it on a game page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaqItem {
    pub question: String,
    pub answer: String,
}

/// Technique slugs a generated port description may use, with their labels.
pub const PORT_TECHNIQUES: &[(&str, &str)] = &[
    ("static_recompilation", "Static recompilation"),
    ("decompilation_port", "Decompilation port"),
    ("source_port", "Source port"),
    ("engine_reimplementation", "Engine reimplementation"),
    ("emulator_wrapper", "Emulator wrapper"),
    ("other", "Other"),
];

/// Display label for a technique slug. Unknown slugs read as "Other".
pub fn technique_label(slug: &str) -> &'static str {
    PORT_TECHNIQUES
        .iter()
        .find(|(s, _)| *s == slug)
        .map_or("Other", |(_, label)| label)
}

/// Generated description of a port, built from its GitHub repository. Shown on the port page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortAbout {
    pub summary: String,
    pub technique: String,
    pub technique_label: String,
    pub technique_note: String,
    pub features: Vec<String>,
    pub requirements: Vec<String>,
    pub limitations: Vec<String>,
    pub setup: Vec<String>,
    pub generated_at: DateTime<Utc>,
}

/// Full port page data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortDetail {
    pub port: PortCard,
    pub game: GameCard,
    pub source_platform: Option<PlatformRef>,
    pub homepage_url: Option<String>,
    pub description: Option<String>,
    pub about: Option<PortAbout>,
    pub requirements: Option<String>,
    pub toolchain: Option<String>,
    pub license: Option<String>,
    pub latest_version_label: Option<String>,
    pub last_commit_at: Option<DateTime<Utc>>,
    pub ai_source: Option<String>,
    pub other_ports: Vec<PortCard>,
    pub videos: Vec<VideoDto>,
    /// Member-contributed screenshots, approved by a moderator, oldest first.
    pub screenshots: Vec<MediaRef>,
    /// Commits per week for the last 12 weeks, oldest first. Empty until GitHub's stats are fetched.
    pub weekly_commits: Vec<i32>,
}

/// A pending member-uploaded or automated README screenshot, for moderation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingPortImageDto {
    pub id: i64,
    pub image: MediaRef,
    pub uploader_name: Option<String>,
    pub port_name: String,
    pub port_slug: String,
    pub game_slug: String,
    pub source: String,
    pub source_alt: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub decision_reason: Option<String>,
    pub media_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// A pending member-submitted or discovery-suggested gameplay video for moderation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingVideoDto {
    pub id: i64,
    pub video: VideoDto,
    pub game_name: String,
    pub game_slug: String,
    pub port_name: Option<String>,
    pub port_slug: Option<String>,
    pub source: String,
    pub decision_reason: Option<String>,
    pub created_at: DateTime<Utc>,
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
    /// Name shown for the member, whichever provider it came from.
    pub display_name: String,
    /// GitHub login, while a GitHub account is connected. Used for author links.
    pub github_login: Option<String>,
    pub role: String,
    pub avatar: Option<MediaRef>,
}

/// Display name for a stored provider key.
pub fn provider_label(provider: &str) -> &'static str {
    match provider {
        "google" => "Google",
        _ => "Unknown provider",
    }
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
    pub playability: Option<String>,
    pub declared_platforms: Option<Vec<String>>,
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
    pub submitter_name: Option<String>,
    pub submitter_approved: Option<i64>,
    pub payload_json: String,
    pub before_json: Option<String>,
    pub needs_game_match: bool,
    pub submitter_is_author: bool,
    pub created_at: DateTime<Utc>,
    pub triage: Option<crate::TriageSummary>,
    pub triage_revision: i64,
    pub triage_error: Option<String>,
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
    pub reviewer_id: Option<i64>,
    pub review_note: Option<String>,
}

/// A submitter's own submission, for the account page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MySubmissionDto {
    pub id: i64,
    pub kind: String,
    pub status: String,
    /// What it was about: a port or game name, or the start of a report's text.
    pub subject: String,
    /// Set once the port exists: after approval, or at once for a trusted submitter.
    pub game_slug: Option<String>,
    pub port_slug: Option<String>,
    /// The moderator's note, shown only once the submission has been reviewed.
    pub review_note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub reviewed_at: Option<DateTime<Utc>>,
}

/// A user row in the admin search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserAdminRow {
    pub id: i64,
    pub display_name: String,
    pub github_login: Option<String>,
    /// Sign-in providers in use (only "google").
    pub providers: Vec<String>,
    /// Emails from the sign-in providers. Admin-only; never shown publicly.
    pub emails: Vec<String>,
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

/// One AI provider in the describe fallback order. Credentials never leave the server; the
/// account label is the only identifying detail shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiProviderRow {
    pub id: i64,
    pub position: i32,
    pub kind: String,
    pub model: String,
    pub enabled: bool,
    pub account_label: String,
    pub last_error: Option<String>,
    pub last_error_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// The admin view of the provider list. `secrets_configured` is false when `SECRETS_KEY` is unset,
/// in which case providers cannot be added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiProvidersView {
    pub secrets_configured: bool,
    pub providers: Vec<AiProviderRow>,
}

/// Start of an Anthropic login: the admin opens `authorize_url`, then pastes the callback value
/// with `pending_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiOauthStart {
    pub pending_id: String,
    pub authorize_url: String,
}

/// Start of a ChatGPT device login: the admin enters `user_code` at `verify_url`, then checks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiDeviceStart {
    pub pending_id: String,
    pub user_code: String,
    pub verify_url: String,
}

/// One check of a ChatGPT device login. `provider` is set once the login completed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiDevicePoll {
    pub provider: Option<AiProviderRow>,
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
    #[serde(default)]
    pub funding: Vec<FundingLink>,
    #[serde(default)]
    pub achievements: Vec<AchievementDto>,
}

/// An achievement an author has earned, computed from their ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AchievementDto {
    pub key: String,
    pub title: String,
    pub description: String,
}

/// A platform reference used as a link target in stats lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetRefDto {
    pub slug: String,
    pub label: String,
}

/// One platform on the parity board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParityTargetDto {
    pub slug: String,
    pub label: String,
    /// Ported games with a native port on this platform.
    pub games: i64,
    /// Published ports that run on this platform.
    pub ports: i64,
    /// `games / ported_games` as a percentage.
    pub pct: f64,
    /// 1-based rank by `pct`, ties broken by label.
    pub rank: i64,
    pub pct_30d: Option<f64>,
    pub rank_30d: Option<i64>,
    /// Whether `/creators/platforms?missing=<slug>` accepts this platform.
    pub gap_link: bool,
}

/// An author ranked by the platform slots their ports added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContributorDto {
    pub login: String,
    pub name: String,
    pub count: i64,
}

/// Platform parity headline numbers for `/stats` and the home teaser.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParityDto {
    /// Covered (game, major platform) slots as a percentage of all slots.
    pub pct: f64,
    pub pct_30d: Option<f64>,
    pub slots_covered: i64,
    pub slots_total: i64,
    pub ported_games: i64,
    pub catalog_games: i64,
    /// Share of catalog games with any native port.
    pub catalog_pct: f64,
    pub catalog_pct_30d: Option<f64>,
    pub ports: i64,
    pub period_days: i64,
    pub ports_in_period: i64,
    pub slots_in_period: i64,
    /// Next parity milestone percentage above `pct`, if any.
    pub next_parity: Option<i64>,
    /// Major platforms, ranked by parity.
    pub targets: Vec<ParityTargetDto>,
    /// Authors whose ports added the most platform slots in the period.
    pub contributors: Vec<ContributorDto>,
}

/// One day of parity history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryPointDto {
    pub day: NaiveDate,
    pub pct: f64,
    pub slots: i64,
    pub ported_games: i64,
}

/// A milestone crossed on a day, for one platform or for all platforms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MilestoneDto {
    pub day: NaiveDate,
    /// Platform the milestone belongs to; `None` means all platforms.
    pub target: Option<TargetRefDto>,
    /// `"ports"` or `"slots"`.
    pub kind: String,
    pub threshold: i64,
}

/// The next milestone for one counter, with how far away it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnlockDto {
    pub target: Option<TargetRefDto>,
    /// `"ports"` or `"slots"`.
    pub kind: String,
    pub current: i64,
    pub next: i64,
    pub to_go: i64,
}

/// A ported game that is missing one or more major platforms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameGapDto {
    pub slug: String,
    pub name: String,
    pub ports: i64,
    pub missing: Vec<TargetRefDto>,
}

/// A catalog game with no native port yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnportedGameDto {
    pub slug: String,
    pub name: String,
}

/// Approximate Discord server stats from the public invite lookup (no bot token required).
/// "Approximate" per Discord's API: expect these to lag the real numbers by a few minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscordStatsDto {
    pub member_count: i64,
    pub online_count: i64,
}

/// A ported game ranked by how many major platforms it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MostCoveredDto {
    pub slug: String,
    pub name: String,
    pub ports: i64,
    pub covered: i64,
}

/// An original platform ranked by the share of its games that are ported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OriginDto {
    pub slug: String,
    pub name: String,
    pub game_count: i64,
    pub ported_count: i64,
    pub pct: f64,
}

/// An author ranked by ports or by platform slots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorRankDto {
    pub login: String,
    pub name: String,
    pub ports: i64,
    pub platforms: i64,
}

/// Everything on `/stats`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsPageDto {
    pub parity: ParityDto,
    pub history: Vec<HistoryPointDto>,
    pub milestones: Vec<MilestoneDto>,
    pub unlocks: Vec<UnlockDto>,
    pub top_authors: Vec<AuthorRankDto>,
    pub top_platform_authors: Vec<AuthorRankDto>,
    pub most_covered: Vec<MostCoveredDto>,
    pub gaps: Vec<GameGapDto>,
    pub wanted: Vec<UnportedGameDto>,
    pub origins: Vec<OriginDto>,
}

/// The best-selling entry for a game: the one with the most units sold. Shown as the strongest
/// fact on the creator lists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SalesSignal {
    pub units_sold_millions: f64,
    /// `platform`, `overall`, or `year`.
    pub scope: String,
    /// Platform abbreviation (e.g. `PS2`) when the scope is `platform`.
    pub platform: Option<String>,
    /// Calendar year when the scope is `year`.
    pub sales_year: Option<i32>,
    /// Position within its list, when the list ranks entries.
    pub rank: Option<i32>,
}

/// One game with no port, for the creators "what to recompile" list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecompileItem {
    pub game_id: i64,
    pub game_slug: String,
    pub game_name: String,
    pub release_year: Option<i32>,
    pub original_platforms: Vec<String>,
    pub want_count: i32,
    pub has_decompilation: bool,
    /// IGDB rating count (`games.popularity`).
    pub popularity: i32,
    /// Strongest best-seller entry for the game, if any.
    pub sales: Option<SalesSignal>,
}

/// A port missing major platforms, for the creators "ports missing platforms" list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortGapItem {
    pub port_id: i64,
    pub port_slug: String,
    pub port_name: String,
    pub game_slug: String,
    pub game_name: String,
    pub repo_url: String,
    pub github_stars: i32,
    /// Target slugs the port already runs on.
    pub targets: Vec<String>,
    /// Major target slugs the port lacks.
    pub missing: Vec<String>,
    /// IGDB rating count (`games.popularity`).
    pub popularity: i32,
    /// Strongest best-seller entry for the game, if any.
    pub sales: Option<SalesSignal>,
}

/// A port with few gameplay videos, for the creators "videos to make" list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoGapItem {
    pub port_id: i64,
    pub port_slug: String,
    pub port_name: String,
    pub game_slug: String,
    pub game_name: String,
    pub repo_url: String,
    pub github_stars: i32,
    /// Target slugs the port runs on.
    pub targets: Vec<String>,
    /// Visible gameplay videos counted for this view (overall, or for the chosen target).
    pub video_count: i64,
    /// IGDB rating count (`games.popularity`).
    pub popularity: i32,
    /// Strongest best-seller entry for the game, if any.
    pub sales: Option<SalesSignal>,
}

/// Top rows of each creator list for the hub page, with totals for "View all".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatorsHub {
    pub recompile: Vec<RecompileItem>,
    pub recompile_total: i64,
    pub ports: Vec<PortGapItem>,
    pub ports_total: i64,
    pub videos: Vec<VideoGapItem>,
    pub videos_total: i64,
}

/// Comment order on a port page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentSort {
    #[default]
    Newest,
    Highest,
    Lowest,
}

/// One page of a port's comments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommentQuery {
    pub port_id: i64,
    pub page: i64,
    pub sort: CommentSort,
    /// Only comments whose author gave the port this many stars.
    pub stars: Option<i16>,
    /// Moderators only: include hidden comments.
    pub include_hidden: bool,
}

/// A comment as shown on a port page. `rating` is the author's current rating of the port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommentDto {
    pub id: i64,
    pub author_id: i64,
    /// Name shown for the author.
    pub name: String,
    /// GitHub handle, when the author has linked GitHub. Used for profile links.
    pub github_login: Option<String>,
    pub avatar: Option<MediaRef>,
    pub body: String,
    /// URL slug of the platform the author tested on.
    pub tested_target: Option<String>,
    pub created_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub hidden: bool,
    pub hidden_reason: Option<String>,
    pub rating: Option<i16>,
}

/// Star ratings for a port. `distribution[0]` counts 1-star ratings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RatingSummary {
    pub count: i64,
    pub distribution: [i64; 5],
    pub average: Option<f64>,
}

impl RatingSummary {
    pub fn from_distribution(distribution: [i64; 5]) -> Self {
        let count: i64 = distribution.iter().sum();
        let total: i64 = distribution
            .iter()
            .enumerate()
            .map(|(i, n)| (i as i64 + 1) * n)
            .sum();
        let average = (count > 0).then(|| total as f64 / count as f64);
        Self {
            count,
            distribution,
            average,
        }
    }
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
