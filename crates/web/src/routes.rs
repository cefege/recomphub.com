use dioxus::prelude::*;

use crate::app::Shell;
use crate::pages::{
    About, AccountPage, AdminPage, AuthorPage, CreatorsHub, CreatorsPlatforms, CreatorsRecompile,
    CreatorsVideos, EditPortPage, GamePage, GamesIndex, Home, LibraryPage, ModerationLog,
    ModerationQueue, NewPage, NotFound, PlatformPage, PlatformTargetPage, PlatformsIndex, PortPage,
    PrivacyPage, RunsOnPage, SearchPage, StatsPage, SubmitPage, TrendingPage, WantedPage,
};

#[derive(Routable, Clone, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Shell)]
    #[route("/?:window")]
    Home { window: String },
    #[route("/trending?:window&:page&:view")]
    TrendingPage { window: String, page: String, view: String },
    #[route("/games?:platform&:runs_on&:status&:sort&:page&:view")]
    GamesIndex { platform: String, runs_on: String, status: String, sort: String, page: String, view: String },
    #[route("/library?:page")]
    LibraryPage { page: String },
    #[route("/games/:slug")]
    GamePage { slug: String },
    #[route("/games/:game_slug/:port_slug")]
    PortPage { game_slug: String, port_slug: String },
    #[route("/platforms")]
    PlatformsIndex {},
    #[route("/platforms/:slug?:page&:view")]
    PlatformPage { slug: String, page: String, view: String },
    #[route("/platforms/:slug/:target?:page&:view")]
    PlatformTargetPage { slug: String, target: String, page: String, view: String },
    #[route("/runs-on/:target?:page&:view")]
    RunsOnPage { target: String, page: String, view: String },
    #[route("/wanted?:page&:view")]
    WantedPage { page: String, view: String },
    #[route("/new?:page&:view")]
    NewPage { page: String, view: String },
    #[route("/authors/:login")]
    AuthorPage { login: String },
    #[route("/search?:q")]
    SearchPage { q: String },
    #[route("/about")]
    About {},
    #[route("/privacy")]
    PrivacyPage {},
    #[route("/submit?:game&:repo")]
    SubmitPage { game: String, repo: String },
    #[route("/submit/edit/:port_id")]
    EditPortPage { port_id: i64 },
    #[route("/account")]
    AccountPage {},
    #[route("/moderation")]
    ModerationQueue {},
    #[route("/moderation/log?:page")]
    ModerationLog { page: String },
    #[route("/admin")]
    AdminPage {},
    #[route("/creators")]
    CreatorsHub {},
    #[route("/creators/recompile?:page")]
    CreatorsRecompile { page: String },
    #[route("/creators/platforms?:missing&:page")]
    CreatorsPlatforms { missing: String, page: String },
    #[route("/creators/videos?:target&:page")]
    CreatorsVideos { target: String, page: String },
    #[route("/stats")]
    StatsPage {},
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}

#[cfg(feature = "desktop")]
#[component]
pub fn NativeWebsiteOnly(path: String, heading: String) -> Element {
    let mut error = use_signal(|| None::<String>);
    let url = format!("{}{path}", crate::desktop::runtime().origin);
    rsx! {
        main { class: "mx-auto flex min-h-[60vh] max-w-2xl flex-col items-start justify-center gap-4 p-6",
            h1 { class: "text-2xl font-semibold", "{heading}" }
            p { class: "text-muted-foreground",
                "This page is available on the RecompHub website. Open it in your system browser to continue."
            }
            button {
                r#type: "button",
                class: "rounded-md bg-primary px-4 py-2 font-medium text-primary-foreground",
                onclick: move |_| {
                    if let Err(message) = crate::desktop::external::open_external_http(&url) {
                        error.set(Some(message));
                    }
                },
                "Open on RecompHub website"
            }
            if let Some(message) = error.read().as_ref() {
                p { class: "text-sm text-destructive", "{message}" }
            }
        }
    }
}
