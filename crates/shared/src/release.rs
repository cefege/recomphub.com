use serde::{Deserialize, Serialize};

/// Computed release state of a port. Never edited by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseState {
    Archived,
    Inactive,
    Released,
    Prerelease,
    Unreleased,
}

impl ReleaseState {
    pub fn label(self) -> &'static str {
        match self {
            ReleaseState::Archived => "Archived",
            ReleaseState::Inactive => "Inactive",
            ReleaseState::Released => "Released",
            ReleaseState::Prerelease => "Prerelease",
            ReleaseState::Unreleased => "Unreleased",
        }
    }
}

/// Inputs to `release_state`, all taken from GitHub-derived columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseInput {
    pub archived: bool,
    /// Days since the repo was last pushed.
    pub days_since_push: i64,
    /// `None` when the repo has no releases.
    pub latest_is_prerelease: Option<bool>,
}

/// Apply the rules in order: archived, inactive (pushed more than 365 days ago), released,
/// prerelease, then unreleased.
pub fn release_state(input: ReleaseInput) -> ReleaseState {
    if input.archived {
        ReleaseState::Archived
    } else if input.days_since_push > 365 {
        ReleaseState::Inactive
    } else if input.latest_is_prerelease == Some(false) {
        ReleaseState::Released
    } else if input.latest_is_prerelease == Some(true) {
        ReleaseState::Prerelease
    } else {
        ReleaseState::Unreleased
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(archived: bool, days: i64, pre: Option<bool>) -> ReleaseInput {
        ReleaseInput {
            archived,
            days_since_push: days,
            latest_is_prerelease: pre,
        }
    }

    #[test]
    fn archived_beats_inactive_and_released() {
        assert_eq!(
            release_state(input(true, 1000, Some(false))),
            ReleaseState::Archived
        );
    }

    #[test]
    fn inactive_beats_released() {
        assert_eq!(
            release_state(input(false, 366, Some(false))),
            ReleaseState::Inactive
        );
        assert_eq!(
            release_state(input(false, 365, Some(false))),
            ReleaseState::Released
        );
    }

    #[test]
    fn released_beats_prerelease_flag_order() {
        assert_eq!(
            release_state(input(false, 1, Some(false))),
            ReleaseState::Released
        );
        assert_eq!(
            release_state(input(false, 1, Some(true))),
            ReleaseState::Prerelease
        );
    }

    #[test]
    fn no_release_is_unreleased() {
        assert_eq!(
            release_state(input(false, 1, None)),
            ReleaseState::Unreleased
        );
    }
}
