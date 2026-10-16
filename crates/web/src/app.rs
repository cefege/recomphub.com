use dioxus::document::{Link, Script, Stylesheet};
use dioxus::fullstack::FullstackContext;
use dioxus::fullstack::http::StatusCode;
use dioxus::prelude::*;

use crate::api;
use crate::components::styles::{
    BUTTON, BUTTON_GHOST, BUTTON_ICON, BUTTON_OUTLINE, BUTTON_SM, INPUT, MENU_ITEM, POPOVER,
};
use crate::components::{
    LiveSocket, LiveStore, OnlineCount, Picture, SiteUrl, ViewerStore, initials_of, login_redirect,
    set_me,
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
        div { class: "flex min-h-screen flex-col bg-background text-foreground",
            a { href: "#main", class: "sr-only focus:not-sr-only focus:absolute focus:left-4 focus:top-3 focus:z-50 {BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}", "Skip to content" }
            header { class: "sticky top-0 z-40 w-full border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/80",
                div { class: "mx-auto flex h-14 max-w-7xl items-center gap-6 px-4 sm:px-6 lg:px-8",
                    details { class: "relative md:hidden",
                        summary { class: "{BUTTON} {BUTTON_GHOST} {BUTTON_ICON} list-none", "aria-label": "Menu",
                            MenuIcon { class: "size-5" }
                        }
                        nav { class: "{POPOVER} absolute left-0 mt-2 flex w-56 flex-col", "aria-label": "Main",
                            form { action: "/search", method: "get", role: "search", class: "p-1",
                                input {
                                    r#type: "search",
                                    name: "q",
                                    "aria-label": "Search games and ports",
                                    placeholder: "Search games and ports…",
                                    class: "{INPUT} h-8 w-full",
                                }
                            }
                            for (href, label) in NAV {
                                a { key: "{href}", href: "{href}", class: MENU_ITEM, "{label}" }
                            }
                        }
                    }
                    a { href: "/", class: "text-base font-semibold tracking-tight", "RecompHub" }
                    nav { class: "hidden items-center gap-5 text-sm md:flex", "aria-label": "Main",
                        for (href, label) in NAV {
                            a { key: "{href}", href: "{href}", class: "text-muted-foreground transition-colors hover:text-foreground", "{label}" }
                        }
                    }
                    div { class: "ml-auto flex items-center gap-2",
                        form { action: "/search", method: "get", role: "search", class: "hidden lg:block",
                            input {
                                r#type: "search",
                                name: "q",
                                "aria-label": "Search games and ports",
                                placeholder: "Search games and ports…",
                                class: "{INPUT} h-8 w-44 xl:w-72",
                            }
                        }
                        a { href: "/search", class: "{BUTTON} {BUTTON_GHOST} {BUTTON_ICON} lg:hidden", "aria-label": "Search",
                            SearchIcon { class: "size-4" }
                        }
                        OnlineCount {}
                        button {
                            r#type: "button",
                            class: "{BUTTON} {BUTTON_GHOST} {BUTTON_ICON}",
                            title: "Toggle light and dark theme",
                            "aria-label": "Toggle light and dark theme",
                            onclick: move |_| {
                                document::eval(
                                    "var d=document.documentElement;var t=d.dataset.theme==='light'?'dark':'light';d.dataset.theme=t;localStorage.setItem('theme',t);",
                                );
                            },
                            ContrastIcon { class: "size-4" }
                        }
                        if let Some(me) = me {
                            details { class: "relative",
                                summary { class: "flex cursor-pointer list-none items-center gap-2 rounded-full", "aria-label": "Account menu",
                                    Picture {
                                        media: me.avatar.clone(),
                                        alt: me.display_name.clone(),
                                        initials: initials_of(&me.display_name),
                                        sizes: "32px",
                                        class: "size-8 rounded-full text-xs",
                                    }
                                }
                                div { class: "{POPOVER} absolute right-0 mt-2 flex w-48 flex-col",
                                    p { class: "truncate px-2 py-1.5 text-sm font-medium", "{me.display_name}" }
                                    div { class: "-mx-1 my-1 h-px bg-border" }
                                    a { href: "/account", class: MENU_ITEM, "Account" }
                                    if me.role == "moderator" || me.role == "admin" {
                                        a { href: "/moderation", class: MENU_ITEM, "Moderation" }
                                    }
                                    if me.role == "admin" {
                                        a { href: "/admin", class: MENU_ITEM, "Admin" }
                                    }
                                    form { method: "post", action: "/auth/logout",
                                        button { r#type: "submit", class: MENU_ITEM, "Sign out" }
                                    }
                                }
                            }
                        } else {
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                                onclick: move |_| login_redirect(),
                                "Sign in"
                            }
                        }
                    }
                }
            }
            main { id: "main", class: "mx-auto w-full max-w-7xl flex-1 px-4 py-8 sm:px-6 lg:px-8",
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
            footer { class: "border-t",
                div { class: "mx-auto flex max-w-7xl flex-col gap-4 px-4 py-8 text-sm text-muted-foreground sm:flex-row sm:items-center sm:justify-between sm:px-6 lg:px-8",
                    p { "RecompHub · seed data from Recompendium (MIT)" }
                    nav { class: "flex flex-wrap gap-x-5 gap-y-2", "aria-label": "Footer",
                        a { href: "/about", class: "transition-colors hover:text-foreground", "About" }
                        a { href: "/about#ground-rules", class: "transition-colors hover:text-foreground", "Ground rules" }
                        a { href: "/submit", class: "transition-colors hover:text-foreground", "Submit a port" }
                        a { href: "/feed.xml", class: "transition-colors hover:text-foreground", "RSS" }
                    }
                }
            }
        }
    }
}

/// Main navigation: (href, label).
const NAV: [(&str, &str); 7] = [
    ("/trending", "Trending"),
    ("/games", "Games"),
    ("/platforms", "Platforms"),
    ("/stats", "Stats"),
    ("/wanted", "Wanted"),
    ("/creators", "For creators"),
    ("/submit", "Submit"),
];

/// Lucide "menu" icon.
#[component]
fn MenuIcon(#[props(default)] class: String) -> Element {
    rsx! {
        svg { class: "{class}", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            path { d: "M4 6h16M4 12h16M4 18h16" }
        }
    }
}

/// Lucide "search" icon.
#[component]
fn SearchIcon(#[props(default)] class: String) -> Element {
    rsx! {
        svg { class: "{class}", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            circle { cx: "11", cy: "11", r: "8" }
            path { d: "M21 21l-4.35-4.35" }
        }
    }
}

/// Lucide "contrast" icon, for the theme toggle.
#[component]
fn ContrastIcon(#[props(default)] class: String) -> Element {
    rsx! {
        svg { class: "{class}", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
            circle { cx: "12", cy: "12", r: "10" }
            path { d: "M12 18a6 6 0 0 0 0-12v12z" }
        }
    }
}
