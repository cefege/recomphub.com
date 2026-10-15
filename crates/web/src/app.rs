use dioxus::document::{Link, Script, Stylesheet};
use dioxus::fullstack::FullstackContext;
use dioxus::fullstack::http::StatusCode;
use dioxus::prelude::*;

use crate::api;
use crate::components::{
    LiveSocket, LiveStore, OnlineCount, Picture, SiteUrl, ViewerStore, login_redirect, set_me,
};
use crate::pages::RouteError;
use crate::routes::Route;

/// Unhashed so `tailwind.css` can name it: dx serves it at `/assets/inter-latin-wght.woff2`.
const FONT_WOFF2: Asset = asset!(
    "/assets/fonts/inter-latin-wght.woff2",
    AssetOptions::builder().with_hash_suffix(false)
);

/// Root component. Routing and the shared shell live here.
#[component]
pub fn App() -> Element {
    rsx! {
        // Set the theme before first paint, from localStorage (default dark).
        Script {
            "(function(){{var t=localStorage.getItem('theme')||'dark';document.documentElement.dataset.theme=t;}})();"
        }
        Stylesheet { href: asset!("/assets/tailwind.css") }
        Link {
            rel: "preload",
            r#as: "font",
            r#type: "font/woff2",
            crossorigin: "anonymous",
            href: "{FONT_WOFF2}",
        }
        Router::<Route> {}
    }
}

/// Shared chrome for every page: header, search entry, nav, footer.
/// Loads the site origin once through `api::site_url`, then provides `SiteUrl`, `LiveStore`
/// and `ViewerStore`. The socket and the viewer session start only after hydration.
#[component]
pub fn Shell() -> Element {
    let site = use_loader(api::site_url)?;
    use_context_provider(|| SiteUrl(site()));
    let _live = LiveStore::provide();
    let viewer = ViewerStore::provide();
    let mut hydrated = use_signal(|| false);

    use_effect(move || {
        hydrated.set(true);
        document::eval(
            "document.addEventListener('keydown',function(e){\
             if(e.key!=='/'||e.metaKey||e.ctrlKey||e.altKey)return;\
             var t=document.activeElement&&document.activeElement.tagName;\
             if(t==='INPUT'||t==='TEXTAREA'||t==='SELECT')return;\
             var q=document.querySelector('input[name=q]');\
             if(q){e.preventDefault();q.focus();}});",
        );
        spawn(async move {
            let me = api::me().await.ok().flatten();
            set_me(viewer, me);
        });
    });

    let me = viewer.me.read().clone();
    rsx! {
        if hydrated() {
            LiveSocket {}
        }
        div { class: "min-h-screen bg-background text-foreground",
            header { class: "sticky top-0 z-40 border-b border-border/60 bg-background/70 backdrop-blur-xl",
                div { class: "mx-auto flex max-w-7xl items-center gap-4 px-4 py-3",
                    a { href: "/", class: "text-xl font-black tracking-tight bg-gradient-to-r from-[#8b5cf6] to-[#22d3ee] bg-clip-text text-transparent", "RecompHub" }
                    nav { class: "hidden gap-4 text-sm font-semibold text-muted md:flex",
                        a { href: "/trending", class: "hover:text-foreground", "Trending" }
                        a { href: "/games", class: "hover:text-foreground", "Games" }
                        a { href: "/platforms", class: "hover:text-foreground", "Platforms" }
                        a { href: "/wanted", class: "hover:text-foreground", "Wanted" }
                        a { href: "/submit", class: "hover:text-foreground", "Submit" }
                    }
                    form { action: "/search", method: "get", class: "ml-auto hidden flex-1 max-w-sm md:block",
                        input {
                            r#type: "search",
                            name: "q",
                            placeholder: "Search games and ports ( / )",
                            class: "w-full rounded-xl border border-border bg-surface px-3 py-2 text-sm",
                        }
                    }
                    div { class: "ml-auto flex items-center gap-3 md:ml-0",
                        OnlineCount {}
                        button {
                            r#type: "button",
                            class: "rounded-full border border-border bg-surface px-3 py-1 text-sm",
                            title: "Toggle light and dark theme",
                            onclick: move |_| {
                                document::eval(
                                    "var d=document.documentElement;var t=d.dataset.theme==='light'?'dark':'light';d.dataset.theme=t;localStorage.setItem('theme',t);",
                                );
                            },
                            "Theme"
                        }
                        if let Some(me) = me {
                            details { class: "relative",
                                summary { class: "flex cursor-pointer list-none items-center gap-2",
                                    Picture {
                                        media: me.avatar.clone(),
                                        alt: me.login.clone(),
                                        initials: me.login.chars().take(2).collect::<String>(),
                                        sizes: "32px",
                                        class: "h-8 w-8 rounded-full",
                                    }
                                    span { class: "hidden text-sm font-semibold sm:inline", "{me.login}" }
                                }
                                div { class: "absolute right-0 mt-2 flex w-48 flex-col gap-1 rounded-xl border border-border bg-surface p-2 shadow-lg",
                                    a { href: "/account", class: "rounded-lg px-3 py-2 text-sm hover:bg-surface-2", "Account" }
                                    if me.role == "moderator" || me.role == "admin" {
                                        a { href: "/moderation", class: "rounded-lg px-3 py-2 text-sm hover:bg-surface-2", "Moderation" }
                                    }
                                    if me.role == "admin" {
                                        a { href: "/admin", class: "rounded-lg px-3 py-2 text-sm hover:bg-surface-2", "Admin" }
                                    }
                                    form { method: "post", action: "/auth/logout",
                                        button { r#type: "submit", class: "w-full rounded-lg px-3 py-2 text-left text-sm hover:bg-surface-2", "Sign out" }
                                    }
                                }
                            }
                        } else {
                            button {
                                r#type: "button",
                                class: "rounded-full bg-surface-2 px-3 py-1 text-sm font-semibold hover:text-accent",
                                onclick: move |_| login_redirect(),
                                "Sign in"
                            }
                        }
                    }
                }
            }
            main { class: "mx-auto max-w-7xl px-4 py-8",
                ErrorBoundary {
                    handle_error: |errors: ErrorContext| {
                        let status = errors
                            .error()
                            .map(|err| FullstackContext::commit_error_status(err).status)
                            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                        rsx! { RouteError { status: status.as_u16() } }
                    },
                    Outlet::<Route> {}
                }
            }
            footer { class: "mt-16 border-t border-border/60 py-8 text-center text-sm text-muted",
                div { class: "flex flex-wrap justify-center gap-4",
                    a { href: "/about", class: "hover:text-foreground", "About" }
                    a { href: "/feed.xml", class: "hover:text-foreground", "RSS" }
                }
                p { class: "mt-3", "Seed data: Recompendium (MIT)" }
            }
        }
    }
}
