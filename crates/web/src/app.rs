#[cfg(not(feature = "desktop"))]
use dioxus::document::Script;
use dioxus::document::{Link, Stylesheet};
use dioxus::fullstack::FullstackContext;
use dioxus::fullstack::http::StatusCode;
use dioxus::prelude::*;

#[cfg(not(feature = "desktop"))]
use crate::api;
#[cfg(not(feature = "desktop"))]
use crate::components::styles::{
    BADGE, BADGE_PRIMARY, BUTTON, BUTTON_DISCORD, BUTTON_GHOST, BUTTON_ICON, BUTTON_MD,
    BUTTON_OUTLINE, BUTTON_PRIMARY, BUTTON_SM, CARD_SURFACE, INPUT, MENU_ITEM, POPOVER,
};
#[cfg(not(feature = "desktop"))]
use crate::components::{
    DiscordCount, DiscordLink, LiveSocket, Picture, initials_of, login_redirect, set_me,
    set_me_error,
};
use crate::components::{LibraryStore, LiveStore, SiteUrl, ViewerStore};
use crate::pages::RouteError;
use crate::routes::Route;

/// Unhashed so `tailwind.css` can name it: dx serves it at `/assets/inter-latin-wght.woff2`.
const FONT_WOFF2: Asset = asset!(
    "/assets/fonts/inter-latin-wght.woff2",
    AssetOptions::builder().with_hash_suffix(false)
);

#[cfg(not(feature = "desktop"))]
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
#[cfg(feature = "server")]
pub(crate) const DISCORD_INVITE_CODE: &str = "XJWPAQHEzr";
#[cfg(not(feature = "desktop"))]
pub(crate) const DISCORD_INVITE_HEADER: &str = "https://discord.gg/by63B5vnsX";
#[cfg(not(feature = "desktop"))]
pub(crate) const DISCORD_INVITE_FOOTER: &str = "https://discord.gg/BCReT4WnUZ";
pub(crate) const DISCORD_INVITE_HOME: &str = "https://discord.gg/K8CvdQDdwV";
pub(crate) const DISCORD_INVITE_LISTING: &str = "https://discord.gg/6pJaWpBtWh";

#[cfg(not(feature = "desktop"))]
const GOOGLE_ANALYTICS_MEASUREMENT_ID: &str = "G-EZ96BDJE3J";
#[cfg(not(feature = "desktop"))]
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
    #[cfg(feature = "desktop")]
    {
        use_context_provider(|| {
            std::rc::Rc::new(dioxus::history::MemoryHistory::with_initial_path(
                "/library",
            )) as std::rc::Rc<dyn dioxus::history::History>
        });
        return rsx! {
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
        };
    }

    #[cfg(not(feature = "desktop"))]
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
#[cfg(not(feature = "desktop"))]
#[component]
pub fn Shell() -> Element {
    let site = use_loader(api::site_url)?;
    use_context_provider(|| SiteUrl(site()));
    let _live = LiveStore::provide();
    let viewer = ViewerStore::provide();
    let _library = LibraryStore::provide(viewer);
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
    });
    use_effect(move || {
        let _retry = *viewer.retry.read();
        spawn(async move {
            match api::me().await {
                Ok(me) => set_me(viewer, me),
                Err(_) => set_me_error(viewer),
            }
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
                div { class: "mx-auto flex h-14 max-w-7xl items-center gap-3 px-4 sm:px-6 lg:px-8",
                    details { class: "relative lg:hidden",
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
                            for (href, label) in PRIMARY_NAV {
                                a { key: "{href}", href: "{href}", class: MENU_ITEM, "{label}" }
                            }
                            a { href: "/submit", class: MENU_ITEM, "Submit a port" }
                            div { class: "mx-2 my-1 h-px bg-border" }
                            p { class: "px-2 py-1 text-xs font-medium uppercase tracking-wide text-muted-foreground", "More" }
                            for (href, label) in MORE_NAV {
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
                    nav { class: "hidden items-center gap-5 text-sm lg:flex", "aria-label": "Main",
                        for (href, label) in PRIMARY_NAV {
                            a { key: "{href}", href: "{href}", class: "text-muted-foreground transition-colors hover:text-foreground", "{label}" }
                        }
                        details { class: "relative",
                            summary { class: "cursor-pointer list-none text-muted-foreground transition-colors hover:text-foreground", "More" }
                            div { class: "{POPOVER} absolute left-0 mt-2 flex w-44 flex-col",
                                for (href, label) in MORE_NAV {
                                    a { key: "{href}", href: "{href}", class: MENU_ITEM, "{label}" }
                                }
                            }
                        }
                    }
                    div { class: "ml-auto flex items-center gap-2",
                        a { href: "/submit", class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_SM} hidden sm:inline-flex", "Submit a port" }
                        form { action: "/search", method: "get", role: "search", class: "hidden lg:block",
                            input {
                                r#type: "search",
                                name: "q",
                                "aria-label": "Search games and ports",
                                placeholder: "Search games and ports…",
                                class: "{INPUT} h-8 w-32 xl:w-40",
                            }
                        }
                        a { href: "/search", class: "{BUTTON} {BUTTON_GHOST} {BUTTON_ICON} lg:hidden", "aria-label": "Search",
                            SearchIcon { class: "size-4" }
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
                                    a { href: "/library", class: MENU_ITEM, "Your library" }
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
                    }
                }
            }
        }
    }
}

/// Native chrome avoids remote scripts, sockets, and a suspending site-origin loader.
#[cfg(feature = "desktop")]
#[component]
pub fn Shell() -> Element {
    let runtime = crate::desktop::runtime();
    use_context_provider(|| runtime.clone());
    use_context_provider(|| SiteUrl(runtime.origin.clone()));
    let viewer = ViewerStore::provide();
    let _auth = crate::desktop::auth::provide(viewer);

    rsx! {
        div { "data-theme": "dark", class: "h-screen overflow-hidden bg-background text-foreground",
            NativeContent {}
        }
    }
}

/// Mount shared content and device-local library independently of account state.
#[cfg(feature = "desktop")]
#[component]
fn NativeContent() -> Element {
    let viewer = use_context::<ViewerStore>();
    let _live = LiveStore::provide();
    let _library = LibraryStore::provide(viewer);
    let mut retry = use_signal(|| 0u64);
    let mut ready = use_signal(|| None::<Result<(), String>>);
    use_effect(move || {
        let _ = retry();
        ready.set(None);
        spawn(async move {
            ready.set(Some(crate::desktop::runtime().library().await.map(|_| ())));
        });
    });
    rsx! {
        if let Some(Err(message)) = ready() {
            div { class: "fixed right-4 top-16 z-50 max-w-sm rounded-lg border border-destructive bg-card p-3 text-sm shadow-lg", role: "alert",
                p { "{message}" }
                button { r#type: "button", class: "mt-2 underline",
                    onclick: move |_| { retry += 1; },
                    "Retry opening library"
                }
            }
        }
        NativeRoutes {}
    }
}

#[cfg(feature = "desktop")]
#[component]
fn NativeRoutes() -> Element {
    let _native = crate::desktop::library::NativeLibraryStore::provide();
    rsx! {
        ErrorBoundary {
            handle_error: |errors: ErrorContext| {
                let status = errors.error()
                    .map(|err| FullstackContext::commit_error_status(err).status)
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                rsx! { RouteError { status: status.as_u16() } }
            },
            crate::desktop::workspace::DesktopWorkspace {
                Outlet::<Route> {}
            }
        }
    }
}

/// Primary navigation: the three most common catalog destinations.
#[cfg(not(feature = "desktop"))]
const PRIMARY_NAV: [(&str, &str); 3] = [
    ("/games", "Browse"),
    ("/trending", "Trending"),
    ("/library", "Library"),
];

/// Secondary destinations, available without competing with the primary catalog flow.
#[cfg(not(feature = "desktop"))]
const MORE_NAV: [(&str, &str); 4] = [
    ("/platforms", "Platforms"),
    ("/wanted", "Wanted"),
    ("/stats", "Stats"),
    ("/creators", "Creators"),
];

/// Count of pending member submissions, screenshots and videos, shown next to the Moderation
/// link. Loads once per mount and is silent on any error or an empty count, so the menu never
/// looks broken.
#[cfg(not(feature = "desktop"))]
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
