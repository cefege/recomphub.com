use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Error returned when a stored or submitted string is not a known enum value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownValue {
    pub kind: &'static str,
    pub value: String,
}

impl fmt::Display for UnknownValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown {} value: {:?}", self.kind, self.value)
    }
}

impl std::error::Error for UnknownValue {}

macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident, $kind:literal { $($variant:ident => ($value:literal, $label:literal)),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// Stored and URL representation.
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $value),+
                }
            }

            /// Human-readable label.
            pub fn label(self) -> &'static str {
                match self {
                    $($name::$variant => $label),+
                }
            }
        }

        impl FromStr for $name {
            type Err = UnknownValue;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($value => Ok($name::$variant),)+
                    other => Err(UnknownValue { kind: $kind, value: other.to_string() }),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum!(
    /// A platform a port can run on. Stored as `text` with a CHECK constraint.
    TargetPlatform, "target platform" {
        Windows => ("windows", "Windows"),
        MacOs => ("macos", "macOS"),
        Linux => ("linux", "Linux"),
        Web => ("web", "Web browser"),
        Android => ("android", "Android"),
        Ios => ("ios", "iOS"),
        Switch => ("switch", "Nintendo Switch (homebrew)"),
        PsVita => ("ps_vita", "PS Vita"),
        Psp => ("psp", "PSP"),
        N3ds => ("n3ds", "Nintendo 3DS"),
        WiiU => ("wii_u", "Wii U"),
    }
);

impl TargetPlatform {
    /// URL slug used in `/runs-on/{slug}` and `/platforms/{slug}/{target}`.
    pub fn slug(self) -> &'static str {
        match self {
            TargetPlatform::Windows => "windows",
            TargetPlatform::MacOs => "macos",
            TargetPlatform::Linux => "linux",
            TargetPlatform::Web => "web",
            TargetPlatform::Android => "android",
            TargetPlatform::Ios => "ios",
            TargetPlatform::Switch => "switch",
            TargetPlatform::PsVita => "ps-vita",
            TargetPlatform::Psp => "psp",
            TargetPlatform::N3ds => "3ds",
            TargetPlatform::WiiU => "wii-u",
        }
    }

    /// Parse a URL slug from `slug()`.
    pub fn from_slug(s: &str) -> Option<Self> {
        TargetPlatform::ALL.iter().copied().find(|t| t.slug() == s)
    }

    /// Primary targets always appear as columns or badges.
    pub fn is_primary(self) -> bool {
        matches!(
            self,
            TargetPlatform::Windows
                | TargetPlatform::MacOs
                | TargetPlatform::Linux
                | TargetPlatform::Web
        )
    }

    /// Targets that get YouTube searches in the youtube-sync worker.
    pub fn youtube_searchable(self) -> bool {
        matches!(
            self,
            TargetPlatform::Windows
                | TargetPlatform::MacOs
                | TargetPlatform::Linux
                | TargetPlatform::Web
                | TargetPlatform::Android
                | TargetPlatform::Ios
        )
    }
}

string_enum!(
    /// What kind of project a port is.
    PortKind, "port kind" {
        StaticRecomp => ("static_recomp", "Static recompilation"),
        DecompPort => ("decomp_port", "Decompilation port"),
        SourcePort => ("source_port", "Source port"),
        Reimplementation => ("reimplementation", "Engine reimplementation"),
        FanRemake => ("fan_remake", "Fan remake"),
        NativeLayer => ("native_layer", "Native compatibility layer"),
        Decompilation => ("decompilation", "Decompilation (no playable port)"),
    }
);

impl PortKind {
    /// Every kind except `decompilation` counts as a port.
    pub fn is_port(self) -> bool {
        self != PortKind::Decompilation
    }
}

string_enum!(
    /// How complete a port is.
    Playability, "playability" {
        Complete => ("complete", "Complete"),
        Playable => ("playable", "Playable"),
        InProgress => ("in_progress", "In progress"),
        Unknown => ("unknown", "Unknown"),
    }
);

impl Playability {
    /// Sort rank: complete first.
    pub fn rank(self) -> u8 {
        match self {
            Playability::Complete => 0,
            Playability::Playable => 1,
            Playability::InProgress => 2,
            Playability::Unknown => 3,
        }
    }
}

string_enum!(
    /// A donation or funding platform linked from a GitHub author's profile (FUNDING.yml keys,
    /// GitHub Sponsors, or a PayPal link in `custom`). Stored as `text` with a CHECK constraint.
    FundingPlatform, "funding platform" {
        Github => ("github", "GitHub Sponsors"),
        Patreon => ("patreon", "Patreon"),
        OpenCollective => ("open_collective", "Open Collective"),
        KoFi => ("ko_fi", "Ko-fi"),
        Tidelift => ("tidelift", "Tidelift"),
        CommunityBridge => ("community_bridge", "Community Bridge"),
        Liberapay => ("liberapay", "Liberapay"),
        Issuehunt => ("issuehunt", "IssueHunt"),
        Polar => ("polar", "Polar"),
        BuyMeACoffee => ("buy_me_a_coffee", "Buy Me a Coffee"),
        ThanksDev => ("thanks_dev", "thanks.dev"),
        Paypal => ("paypal", "PayPal"),
        Boosty => ("boosty", "Boosty"),
        Fanbox => ("fanbox", "pixiv Fanbox"),
        Custom => ("custom", "Website"),
    }
);

string_enum!(
    /// AI usage of a port's project.
    AiUsage, "ai usage" {
        None => ("none", "Human-made"),
        Assisted => ("assisted", "AI-assisted"),
        PrimarilyAi => ("primarily_ai", "Primarily AI"),
        Unknown => ("unknown", "AI use unknown"),
    }
);

string_enum!(
    /// Where an AI usage value came from.
    AiSource, "ai source" {
        Declared => ("declared", "Declared"),
        Detected => ("detected", "Detected"),
        Imported => ("imported", "Imported"),
    }
);

string_enum!(
    /// Account role.
    Role, "role" {
        User => ("user", "User"),
        Moderator => ("moderator", "Moderator"),
        Admin => ("admin", "Admin"),
    }
);

impl Role {
    /// Moderators and admins can review submissions.
    pub fn is_moderator(self) -> bool {
        matches!(self, Role::Moderator | Role::Admin)
    }

    /// Admins manage users, roles and settings.
    pub fn is_admin(self) -> bool {
        matches!(self, Role::Admin)
    }
}

string_enum!(
    /// Kind of submission.
    SubmissionKind, "submission kind" {
        NewPort => ("new_port", "New port"),
        EditPort => ("edit_port", "Edit port"),
        NewGame => ("new_game", "New game"),
        Report => ("report", "Report"),
        Candidate => ("candidate", "Candidate"),
    }
);

string_enum!(
    /// Submission lifecycle state.
    SubmissionStatus, "submission status" {
        Pending => ("pending", "Pending"),
        Approved => ("approved", "Approved"),
        AutoApproved => ("auto_approved", "Auto-approved"),
        Rejected => ("rejected", "Rejected"),
        Reverted => ("reverted", "Reverted"),
    }
);

string_enum!(
    /// Time window for hit-based popularity.
    HitWindow, "hit window" {
        Today => ("today", "Today"),
        Week => ("week", "This week"),
        Month => ("month", "This month"),
        Year => ("year", "This year"),
    }
);

impl HitWindow {
    /// Length of the window in days. Matches `game_score_ranking.window_days`.
    pub fn days(self) -> i32 {
        match self {
            HitWindow::Today => 1,
            HitWindow::Week => 7,
            HitWindow::Month => 30,
            HitWindow::Year => 365,
        }
    }

    /// Default window used when the query value is missing.
    pub fn default_window() -> Self {
        HitWindow::Week
    }
}

impl Default for HitWindow {
    fn default() -> Self {
        HitWindow::default_window()
    }
}

/// Canonical AI tool names stored in `ports.ai_tools`.
pub const AI_TOOLS: &[&str] = &[
    "Claude",
    "ChatGPT/Codex",
    "GitHub Copilot",
    "Cursor",
    "Gemini",
    "Qwen",
    "DeepSeek",
    "Windsurf",
    "Cline",
    "Other",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_round_trips_through_storage_value() {
        for t in TargetPlatform::ALL {
            assert_eq!(t.as_str().parse::<TargetPlatform>().unwrap(), *t);
        }
    }

    #[test]
    fn target_slugs_round_trip() {
        for t in TargetPlatform::ALL {
            assert_eq!(TargetPlatform::from_slug(t.slug()), Some(*t));
        }
        assert_eq!(TargetPlatform::from_slug("ps_vita"), None);
    }

    #[test]
    fn unknown_value_is_rejected() {
        let err = "ps5".parse::<TargetPlatform>().unwrap_err();
        assert_eq!(err.kind, "target platform");
        assert_eq!(err.value, "ps5");
    }

    #[test]
    fn only_decompilation_is_not_a_port() {
        assert!(PortKind::StaticRecomp.is_port());
        assert!(!PortKind::Decompilation.is_port());
    }

    #[test]
    fn hit_window_days_and_default() {
        assert_eq!(HitWindow::default(), HitWindow::Week);
        assert_eq!(HitWindow::Today.days(), 1);
        assert_eq!(HitWindow::Year.days(), 365);
    }

    #[test]
    fn primary_targets_are_the_four_listed() {
        let primary: Vec<_> = TargetPlatform::ALL
            .iter()
            .filter(|t| t.is_primary())
            .collect();
        assert_eq!(primary.len(), 4);
    }
}
