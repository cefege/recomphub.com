pub mod cards;
pub mod comments;
pub mod discord;
pub mod funding;
pub mod icons;
pub mod library;
pub mod listing;
pub mod live;
pub mod maintenance;
pub mod picture;
pub mod report;
pub mod seo;
pub mod shop;
pub mod stats;
pub mod styles;
pub mod time;
pub mod ui;
pub mod viewer;

pub use cards::GameCardView;
pub use comments::PortComments;
pub use discord::{DiscordCount, DiscordLink};
pub use funding::FundingLinks;
pub use library::{FavoriteButton, LibraryIdentityRetry, LibraryStore, StorageState};
#[cfg(not(feature = "desktop"))]
pub use library::{prune_missing_guest, reset_device_favorites};
pub use listing::{
    GameListTable, PRIMARY_TARGETS, PortTable, RankingTable, WINDOWS, WatchVisible, chip_class,
    parse_page, query_string,
};
#[cfg(not(feature = "desktop"))]
pub use live::LiveSocket;
pub use live::{LiveCount, LiveStore, OnlineCount, use_live_watch};
pub use maintenance::{MaintenanceBadge, MaintenancePanel};
pub use picture::Picture;
pub use report::ReportDialog;
pub use seo::{Seo, SiteUrl};
pub use stats::{AuthorAchievements, ParityMeter, PlatformCoverage};
pub use time::RelativeTime;
pub use ui::{
    AiBadge, Breadcrumb, CardGrid, Chip, EmptyState, ExternalLink, GroundRules, InternalLink,
    LoginPrompt, Pager, PlayabilityMeter, PortCardView, PortHero, Rail, ReleaseChip, Section,
    TargetIcon, TrendChip, VideoPlayer, ViewToggle, ai_text, initials_of, keyed, target_label,
    url_encode, with_query,
};
pub use viewer::{
    Burst, RoleGate, SignedIn, StarButton, ViewerStore, WantButton, login_redirect, retry_identity,
    set_me, set_me_error, use_marks,
};
