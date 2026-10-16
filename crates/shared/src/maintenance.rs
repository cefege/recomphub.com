//! Maintenance level of a project, derived from its commit and release history.
//!
//! Inputs are whole-day ages and window counts taken at sync time, so both functions are pure
//! and can be tested without a clock. The level is stored on the port and shown as a badge.

use serde::{Deserialize, Serialize};

/// Commits in the last 30 days that make a project Active.
pub const ACTIVE_COMMITS_30D: u32 = 8;
/// A release this recent (in days) makes a project Active.
pub const ACTIVE_RELEASE_DAYS: i64 = 30;
/// Commits in the last 90 days that make a project Maintained.
pub const MAINTAINED_COMMITS_90D: u32 = 5;
/// A release this recent (in days) makes a project Maintained.
pub const MAINTAINED_RELEASE_DAYS: i64 = 90;

/// How actively a project is maintained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceLevel {
    Active,
    Maintained,
    Slowing,
    Dormant,
    Archived,
}

impl MaintenanceLevel {
    /// Stable storage and wire key; matches the serde name.
    pub fn key(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Maintained => "maintained",
            Self::Slowing => "slowing",
            Self::Dormant => "dormant",
            Self::Archived => "archived",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "active" => Some(Self::Active),
            "maintained" => Some(Self::Maintained),
            "slowing" => Some(Self::Slowing),
            "dormant" => Some(Self::Dormant),
            "archived" => Some(Self::Archived),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Maintained => "Maintained",
            Self::Slowing => "Slowing",
            Self::Dormant => "Dormant",
            Self::Archived => "Archived",
        }
    }

    /// One sentence on what the level means; shown as the badge tooltip and in the panel.
    pub fn description(self) -> &'static str {
        match self {
            Self::Active => "Committed to in the last 30 days, or released in that time.",
            Self::Maintained => "Committed to in the last 90 days, or released in that time.",
            Self::Slowing => "Some commits in the last year, but below the Maintained bar.",
            Self::Dormant => "No commits in the last year.",
            Self::Archived => "Archived on GitHub. No further changes are expected.",
        }
    }
}

/// Signals behind a level. Day ages are whole days before the sync; `None` means never.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaintenanceInput {
    pub archived: bool,
    pub days_since_commit: Option<i64>,
    pub commits_30d: u32,
    pub commits_90d: u32,
    pub commits_365d: u32,
    pub committers_180d: u32,
    pub days_since_release: Option<i64>,
}

fn released_within(days_since_release: Option<i64>, window: i64) -> bool {
    days_since_release.is_some_and(|d| d <= window)
}

/// Level from the window counts. Rules are checked in order:
/// archived; Active; Maintained; no commit in a year is Dormant; anything else is Slowing.
pub fn maintenance_level(input: MaintenanceInput) -> MaintenanceLevel {
    if input.archived {
        MaintenanceLevel::Archived
    } else if input.commits_30d >= ACTIVE_COMMITS_30D
        || released_within(input.days_since_release, ACTIVE_RELEASE_DAYS)
    {
        MaintenanceLevel::Active
    } else if input.commits_90d >= MAINTAINED_COMMITS_90D
        || released_within(input.days_since_release, MAINTAINED_RELEASE_DAYS)
    {
        MaintenanceLevel::Maintained
    } else if input.commits_365d == 0 {
        MaintenanceLevel::Dormant
    } else {
        MaintenanceLevel::Slowing
    }
}

/// Points for how recent the last commit is: full up to 30 days, falling to zero at 365.
const RECENCY_MAX: i64 = 50;
/// Points for commit volume over 90 days, one per commit up to the cap.
const VOLUME_CAP: u32 = 30;
/// Points for contributors over 180 days, two per author up to five authors.
const PEOPLE_CAP: u32 = 5;

/// Score from 0 to 100 used to order projects by how actively they are kept up. Archived is 0.
///
/// Recency up to 50, commit volume up to 30, a recent release up to 10 and contributors up to 10.
pub fn maintenance_score(input: MaintenanceInput) -> u8 {
    if input.archived {
        return 0;
    }
    let recency = match input.days_since_commit {
        None => 0,
        Some(d) if d <= 30 => RECENCY_MAX,
        Some(d) if d >= 365 => 0,
        Some(d) => RECENCY_MAX * (365 - d) / 335,
    };
    let volume = i64::from(input.commits_90d.min(VOLUME_CAP));
    let release = match input.days_since_release {
        Some(d) if d <= 90 => 10,
        Some(d) if d <= 365 => 5,
        _ => 0,
    };
    let people = i64::from(input.committers_180d.min(PEOPLE_CAP)) * 2;
    u8::try_from((recency + volume + release + people).min(100)).unwrap_or(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> MaintenanceInput {
        MaintenanceInput {
            days_since_commit: Some(0),
            ..MaintenanceInput::default()
        }
    }

    #[test]
    fn archived_wins_over_any_activity() {
        let busy = MaintenanceInput {
            archived: true,
            commits_30d: 100,
            commits_90d: 100,
            commits_365d: 100,
            days_since_release: Some(0),
            ..input()
        };
        assert_eq!(maintenance_level(busy), MaintenanceLevel::Archived);
        assert_eq!(maintenance_score(busy), 0);
    }

    #[test]
    fn active_by_commits_needs_eight_in_thirty_days() {
        let seven = MaintenanceInput {
            commits_30d: 7,
            commits_90d: 7,
            ..input()
        };
        assert_eq!(maintenance_level(seven), MaintenanceLevel::Maintained);
        let eight = MaintenanceInput {
            commits_30d: 8,
            commits_90d: 8,
            ..input()
        };
        assert_eq!(maintenance_level(eight), MaintenanceLevel::Active);
    }

    #[test]
    fn active_by_release_within_thirty_days() {
        let at_30 = MaintenanceInput {
            days_since_release: Some(30),
            ..input()
        };
        assert_eq!(maintenance_level(at_30), MaintenanceLevel::Active);
        let at_31 = MaintenanceInput {
            days_since_release: Some(31),
            commits_365d: 1,
            ..input()
        };
        assert_eq!(maintenance_level(at_31), MaintenanceLevel::Maintained);
    }

    #[test]
    fn maintained_by_commits_needs_five_in_ninety_days() {
        let four = MaintenanceInput {
            commits_90d: 4,
            commits_365d: 4,
            ..input()
        };
        assert_eq!(maintenance_level(four), MaintenanceLevel::Slowing);
        let five = MaintenanceInput {
            commits_90d: 5,
            commits_365d: 5,
            ..input()
        };
        assert_eq!(maintenance_level(five), MaintenanceLevel::Maintained);
    }

    #[test]
    fn maintained_by_release_within_ninety_days() {
        let at_90 = MaintenanceInput {
            days_since_release: Some(90),
            commits_365d: 1,
            ..input()
        };
        assert_eq!(maintenance_level(at_90), MaintenanceLevel::Maintained);
        let at_91 = MaintenanceInput {
            days_since_release: Some(91),
            commits_365d: 1,
            ..input()
        };
        assert_eq!(maintenance_level(at_91), MaintenanceLevel::Slowing);
    }

    #[test]
    fn slowing_needs_one_commit_in_a_year() {
        let one = MaintenanceInput {
            days_since_commit: Some(300),
            commits_365d: 1,
            ..input()
        };
        assert_eq!(maintenance_level(one), MaintenanceLevel::Slowing);
    }

    #[test]
    fn dormant_when_no_commit_in_a_year() {
        let none = MaintenanceInput {
            days_since_commit: Some(400),
            ..input()
        };
        assert_eq!(maintenance_level(none), MaintenanceLevel::Dormant);
        assert_eq!(
            maintenance_level(MaintenanceInput::default()),
            MaintenanceLevel::Dormant
        );
    }

    #[test]
    fn keys_round_trip() {
        for level in [
            MaintenanceLevel::Active,
            MaintenanceLevel::Maintained,
            MaintenanceLevel::Slowing,
            MaintenanceLevel::Dormant,
            MaintenanceLevel::Archived,
        ] {
            assert_eq!(MaintenanceLevel::from_key(level.key()), Some(level));
        }
        assert_eq!(MaintenanceLevel::from_key("bogus"), None);
    }

    #[test]
    fn recency_steps_from_full_to_zero() {
        let score = |days: i64| {
            maintenance_score(MaintenanceInput {
                days_since_commit: Some(days),
                ..MaintenanceInput::default()
            })
        };
        assert_eq!(score(0), 50);
        assert_eq!(score(30), 50);
        assert_eq!(score(31), 49);
        assert_eq!(score(364), 0);
        assert_eq!(score(365), 0);
        assert_eq!(score(400), 0);
    }

    #[test]
    fn volume_is_capped_at_thirty() {
        let score = |commits: u32| {
            maintenance_score(MaintenanceInput {
                commits_90d: commits,
                ..MaintenanceInput::default()
            })
        };
        assert_eq!(score(29), 29);
        assert_eq!(score(30), 30);
        assert_eq!(score(31), 30);
    }

    #[test]
    fn release_points_step_at_ninety_and_365_days() {
        let score = |days: i64| {
            maintenance_score(MaintenanceInput {
                days_since_release: Some(days),
                ..MaintenanceInput::default()
            })
        };
        assert_eq!(score(90), 10);
        assert_eq!(score(91), 5);
        assert_eq!(score(365), 5);
        assert_eq!(score(366), 0);
    }

    #[test]
    fn people_points_cap_at_five_authors() {
        let score = |people: u32| {
            maintenance_score(MaintenanceInput {
                committers_180d: people,
                ..MaintenanceInput::default()
            })
        };
        assert_eq!(score(2), 4);
        assert_eq!(score(5), 10);
        assert_eq!(score(6), 10);
    }

    #[test]
    fn maximum_score_is_one_hundred() {
        let max = MaintenanceInput {
            days_since_commit: Some(0),
            commits_30d: 30,
            commits_90d: 30,
            commits_365d: 30,
            committers_180d: 5,
            days_since_release: Some(0),
            ..MaintenanceInput::default()
        };
        assert_eq!(maintenance_score(max), 100);
    }
}
