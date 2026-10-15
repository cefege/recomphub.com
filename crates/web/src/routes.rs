use dioxus::prelude::*;

use crate::app::Shell;
use crate::pages::{
    About, AccountPage, AdminPage, AuthorPage, EditPortPage, GamePage, GamesIndex, Home,
    ModerationLog, ModerationQueue, NewPage, NotFound, PlatformPage, PlatformTargetPage,
    PlatformsIndex, PortPage, RunsOnPage, SearchPage, SubmitPage, TrendingPage, WantedPage,
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
    #[route("/:..segments")]
    NotFound { segments: Vec<String> },
}
