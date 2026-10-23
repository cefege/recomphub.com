mod about;
#[cfg(not(feature = "desktop"))]
mod account;
#[cfg(not(feature = "desktop"))]
mod admin;
#[cfg(not(feature = "desktop"))]
mod admin_ai_providers;
mod author;
mod browse;
mod creators;
pub(crate) mod faq;
mod games;
mod home;
mod jsonld;
pub(crate) mod library;
#[cfg(not(feature = "desktop"))]
mod moderation;
mod not_found;
mod platforms;
mod port;
#[cfg(not(feature = "desktop"))]
mod privacy;
mod search;
mod stats;
#[cfg(not(feature = "desktop"))]
mod submit;
mod trending;
#[cfg(feature = "desktop")]
mod website_only;

pub use about::About;
#[cfg(not(feature = "desktop"))]
pub use account::AccountPage;
#[cfg(not(feature = "desktop"))]
pub use admin::AdminPage;
pub use author::AuthorPage;
pub use browse::{NewPage, RunsOnPage, WantedPage};
pub use creators::{CreatorsHub, CreatorsPlatforms, CreatorsRecompile, CreatorsVideos};
pub use games::{GamePage, GamesIndex};
pub use home::Home;
pub use library::LibraryPage;
#[cfg(not(feature = "desktop"))]
pub use moderation::{ModerationLog, ModerationQueue};
pub use not_found::{NotFound, RouteError};
pub use platforms::{PlatformPage, PlatformTargetPage, PlatformsIndex};
pub(crate) use port::PortMainContent;
pub use port::PortPage;
#[cfg(not(feature = "desktop"))]
pub use privacy::PrivacyPage;
pub use search::SearchPage;
pub use stats::StatsPage;
#[cfg(not(feature = "desktop"))]
pub use submit::{EditPortPage, SubmitPage};
pub use trending::TrendingPage;
#[cfg(feature = "desktop")]
pub use website_only::{
    AccountPage, AdminPage, EditPortPage, ModerationLog, ModerationQueue, PrivacyPage, SubmitPage,
};
