use dioxus::document::{Link, Script, Stylesheet};
use dioxus::fullstack::FullstackContext;
use dioxus::fullstack::http::StatusCode;
use dioxus::prelude::*;

use crate::api;
use crate::components::styles::{
    BADGE, BADGE_PRIMARY, BUTTON, BUTTON_DISCORD, BUTTON_GHOST, BUTTON_ICON, BUTTON_MD,
    BUTTON_OUTLINE, BUTTON_SM, CARD_SURFACE, INPUT, MENU_ITEM, POPOVER,
};
use crate::components::{
    DiscordCount, DiscordLink, LiveSocket, LiveStore, OnlineCount, Picture, SiteUrl, ViewerStore,
    initials_of, login_redirect, set_me,
};
use crate::pages::RouteError;
use crate::routes::Route;

/// Unhashed so `tailwind.css` can name it: dx serves it at `/assets/inter-latin-wght.woff2`.
const FONT_WOFF2: Asset = asset!(
    "/assets/fonts/inter-latin-wght.woff2",
    AssetOptions::builder().with_hash_suffix(false)
);

const RECOMPHUB_LOGO: Asset = asset!(
    "/assets/recomphub-logo.svg",
    AssetOptions::builder().with_hash_suffix(false)
);

const RECOMPHUB_FAVICON: Asset = asset!(
    "/assets/recomphub-logo.png",
    AssetOptions::builder().with_hash_suffix(false)
);

/// Shared invite code, used only to look up the public member/online count (any valid invite to
/// the guild works for that). Every on-page CTA below links through its own placement invite
/// instead, so Discord's per-invite use count (Server Settings -> Invites) shows which spot
/// actually converts. Minted with `cargo run -p recomphub-worker --bin discord-setup --
/// --placement-invites`; re-running that flag mints fresh codes, so don't re-run it casually.
pub(crate) const DISCORD_INVITE_CODE: &str = "XJWPAQHEzr";
pub(crate) const DISCORD_INVITE_HEADER: &str = "https://discord.gg/by63B5vnsX";
pub(crate) const DISCORD_INVITE_FOOTER: &str = "https://discord.gg/BCReT4WnUZ";
pub(crate) const DISCORD_INVITE_HOME: &str = "https://discord.gg/K8CvdQDdwV";
pub(crate) const DISCORD_INVITE_LISTING: &str = "https://discord.gg/6pJaWpBtWh";

const GOOGLE_ANALYTICS_MEASUREMENT_ID: &str = "G-EZ96BDJE3J";
const GOOGLE_ANALYTICS_BOOTSTRAP: &str = r#"
window['ga-disable-G-EZ96BDJE3J'] = true;
window.dataLayer = window.dataLayer || [];
function gtag(){dataLayer.push(arguments);}
gtag('js', new Date());
gtag('consent', 'default', {
    analytics_storage: 'denied',
    ad_storage: 'denied',
    ad_user_data: 'denied',
    ad_personalization: 'denied'
});
(function() {
    var analyticsPurpose = 'pUSs';
    var enabled = false;
    function syncAnalyticsConsent() {
        var choices = window.zaraz?.consent?.getAll?.();
        var allowed = choices?.[analyticsPurpose] === true;
        window['ga-disable-G-EZ96BDJE3J'] = !allowed;
        gtag('consent', 'update', {
            analytics_storage: allowed ? 'granted' : 'denied'
        });
        if (allowed && !enabled) {
            enabled = true;
            gtag('config', 'G-EZ96BDJE3J');
        } else if (!allowed) {
            enabled = false;
        }
    }
    if (window.zaraz?.consent?.APIReady) {
        syncAnalyticsConsent();
    } else {
        document.addEventListener('zarazConsentAPIReady', syncAnalyticsConsent);
    }
    document.addEventListener('zarazConsentChoicesUpdated', syncAnalyticsConsent);
}());
"#;

/// Root component. Routing and the shared shell live here.
#[component]
pub fn App() -> Element {
    rsx! {
        // Set the theme before first paint, from localStorage (default dark).
        Script {
            "(function(){{var t=localStorage.getItem('theme')||'dark';document.documentElement.dataset.theme=t;}})();"
        }
        Script { "{GOOGLE_ANALYTICS_BOOTSTRAP}" }
        Script {
            src: "https://www.googletagmanager.com/gtag/js?id={GOOGLE_ANALYTICS_MEASUREMENT_ID}",
            r#async: true,
        }
        Script {
            src: "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js?client=ca-pub-6004542868270159",
            r#async: true,
            crossorigin: "anonymous",
        }
        Stylesheet { href: asset!("/assets/tailwind.css") }
        Link {
            rel: "preload",
            r#as: "font",
            r#type: "font/woff2",
            crossorigin: "anonymous",
            href: "{FONT_WOFF2}",
        }
        Link { rel: "icon", r#type: "image/png", href: "{RECOMPHUB_FAVICON}" }
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
                            DiscordLink {
                                href: DISCORD_INVITE_HEADER,
                                label: "Join Discord",
                                class: MENU_ITEM.to_string(),
                            }
                        }
                    }
                    a { href: "/", class: "flex items-center gap-2 text-base font-semibold tracking-tight",
                        img { src: "{RECOMPHUB_LOGO}", alt: "", class: "size-7 rounded-md" }
                        span { "RecompHub" }
                    }
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
                        DiscordLink {
                            href: DISCORD_INVITE_HEADER,
                            label: "Join Discord",
                            class: "{BUTTON} {BUTTON_DISCORD} {BUTTON_SM} hidden sm:inline-flex",
                        }
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
                                        a { href: "/moderation", class: "{MENU_ITEM} flex items-center justify-between gap-2",
                                            "Moderation"
                                            PendingBadge {}
                                        }
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
                            // A real link, so it works before hydration; once hydrated it also
                            // carries the current page as `next`.
                            a {
                                href: "/auth/google",
                                class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                                onclick: move |e| {
                                    e.prevent_default();
                                    login_redirect();
                                },
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
                div { class: "mx-auto max-w-7xl px-4 py-8 sm:px-6 lg:px-8",
                    div { class: "{CARD_SURFACE} flex flex-col items-start gap-4 p-6 sm:flex-row sm:items-center sm:justify-between",
                        div {
                            h2 { class: "text-base font-semibold tracking-tight", "Join the RecompHub Discord" }
                            p { class: "mt-1 text-sm text-muted-foreground",
                                "Share a build, ask for help porting something, or hear about new releases first."
                            }
                        }
                        div { class: "flex shrink-0 flex-wrap items-center gap-3",
                            DiscordCount {}
                            DiscordLink {
                                href: DISCORD_INVITE_FOOTER,
                                label: "Join Discord",
                                class: "{BUTTON} {BUTTON_DISCORD} {BUTTON_MD}",
                            }
                        }
                    }
                    div { class: "mt-6 flex flex-col gap-4 text-sm text-muted-foreground sm:flex-row sm:items-center sm:justify-between",
                        p { "RecompHub · seed data from Recompendium (MIT)" }
                        nav { class: "flex flex-wrap gap-x-5 gap-y-2", "aria-label": "Footer",
                            a { href: "/about", class: "transition-colors hover:text-foreground", "About" }
                            a { href: "/about#ground-rules", class: "transition-colors hover:text-foreground", "Ground rules" }
                            a { href: "/submit", class: "transition-colors hover:text-foreground", "Submit a port" }
                            a { href: "/feed.xml", class: "transition-colors hover:text-foreground", "RSS" }
                            a { href: "/privacy", class: "transition-colors hover:text-foreground", "Privacy" }
                            a { href: "/privacy#privacy-choices", class: "transition-colors hover:text-foreground", "Cookie settings" }
                        }
                        a { href: "/feed.xml", class: "transition-colors hover:text-foreground", "RSS" }
                        a { href: "/privacy", class: "transition-colors hover:text-foreground", "Privacy" }
                        a { href: "/privacy#privacy-choices", class: "transition-colors hover:text-foreground", "Cookie settings" }
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

/// Count of pending member submissions, screenshots and videos, shown next to the Moderation
/// link. Loads once per mount and is silent on any error or an empty count, so the menu never
/// looks broken.
#[component]
fn PendingBadge() -> Element {
    let count = use_resource(|| async move {
        let subs = api::pending_submission_count().await?;
        let media = api::pending_media_count().await?;
        Ok::<i64, ServerFnError>(subs + media)
    });
    match &*count.read() {
        Some(Ok(n)) if *n > 0 => rsx! {
            span { class: "{BADGE} {BADGE_PRIMARY}", "{n}" }
        },
        _ => rsx! {},
    }
}

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
