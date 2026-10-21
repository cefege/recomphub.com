use crate::{Playability, PortKind, TargetPlatform};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const TRIAGE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageCategory {
    GameProject,
    Tooling,
    Reference,
    Unrelated,
    Uncertain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageConfidence {
    Strong,
    Limited,
    Insufficient,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkEvidence {
    Implementation,
    DocumentedProgress,
    IntentOnly,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageSource {
    Description,
    Readme,
    RootEntries,
    RecentCommits,
    ReleaseNotes,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageWarningCode {
    ClaimedLeakedSource,
    PartialImplementation,
    ConflictingClaims,
    MultiGameRepository,
    MissingRepository,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageCitation {
    pub claim: String,
    pub source: TriageSource,
    pub quote: String,
    #[serde(default)]
    pub source_url: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageAssessment {
    pub category: TriageCategory,
    pub confidence: TriageConfidence,
    pub work_evidence: WorkEvidence,
    pub summary: String,
    pub game_titles: Vec<String>,
    pub kind: Option<PortKind>,
    pub playability: Playability,
    pub targets: Vec<TargetPlatform>,
    pub warnings: Vec<TriageWarningCode>,
    pub citations: Vec<TriageCitation>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageGameMatch {
    pub game_id: i64,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageSummary {
    pub version: u32,
    pub input_hash: String,
    pub checked_at: DateTime<Utc>,
    pub assessment: TriageAssessment,
    pub game_matches: Vec<TriageGameMatch>,
    pub archived: Option<bool>,
    pub last_commit_at: Option<DateTime<Utc>>,
    pub release_url: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueLane {
    Members,
    Found,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageStateFilter {
    All,
    Untriaged,
    Classified,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModerationQueueQuery {
    pub lane: QueueLane,
    pub page: i64,
    pub category: Option<TriageCategory>,
    pub confidence: Option<TriageConfidence>,
    pub target: Option<TargetPlatform>,
    pub playability: Option<Playability>,
    pub flagged_only: bool,
    pub triage_state: TriageStateFilter,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModerationQueueCounts {
    pub members: i64,
    pub found: i64,
    pub game_project: i64,
    pub tooling: i64,
    pub reference: i64,
    pub unrelated: i64,
    pub uncertain: i64,
    pub untriaged: i64,
    pub failed: i64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModerationQueuePage {
    pub rows: crate::dto::Page<crate::dto::QueueItem>,
    pub counts: ModerationQueueCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchReviewAction {
    Approve,
    Reject,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchReviewItem {
    pub item_id: i64,
    pub expected_triage_revision: i64,
    pub overrides: crate::dto::ReviewOverrides,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchReviewRequest {
    pub action: BatchReviewAction,
    pub items: Vec<BatchReviewItem>,
    pub note: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchReviewStatus {
    Approved,
    Rejected,
    Conflict,
    Error,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchReviewOutcome {
    pub item_id: i64,
    pub status: BatchReviewStatus,
    pub error: Option<String>,
}
