use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use recomphub_shared::{AiEvidenceDto, MediaRef, PortCard, TargetPlatform, VideoDto};

use crate::api;
use crate::components::icons::{AndroidIcon, AppleIcon, GlobeIcon, LinuxIcon, WindowsIcon};
#[cfg(not(feature = "desktop"))]
use crate::components::login_redirect;
#[cfg(not(feature = "desktop"))]
use crate::components::styles::BUTTON_MD;
use crate::components::styles::{
    BADGE, BADGE_DESTRUCTIVE, BADGE_OUTLINE, BADGE_SECONDARY, BADGE_SUCCESS, BADGE_WARNING, BUTTON,
    BUTTON_DESTRUCTIVE, BUTTON_OUTLINE, BUTTON_PRIMARY, BUTTON_SM, BUTTON_XS, CARD_SURFACE, H2,
    POPOVER, TAB, TAB_ACTIVE, TABS_LIST,
};
use crate::components::{Picture, RelativeTime, StarButton, ViewerStore};

#[component]
pub fn InternalLink(href: String, class: String, children: Element) -> Element {
    #[cfg(feature = "desktop")]
    {
        let navigator = use_navigator();
        let target = href.clone();
        return rsx! {
            a {
                href: "{href}",
                class: "{class}",
                onclick: move |event| {
                    event.prevent_default();
                    navigator.push(target.clone());
                },
                {children}
            }
        };
    }
    #[cfg(not(feature = "desktop"))]
    rsx! {
        a { href: "{href}", class: "{class}", {children} }
    }
}
#[component]
pub fn ExternalLink(
    url: String,
    class: String,
    #[props(default)] sponsored: bool,
    children: Element,
) -> Element {
    #[cfg(feature = "desktop")]
    let _ = sponsored;
    #[cfg(feature = "desktop")]
    let mut error = use_signal(|| None::<String>);
    #[cfg(feature = "desktop")]
    rsx! {
        span { class: "inline-flex flex-col items-start",
            button {
                r#type: "button",
                class: "{class}",
                onclick: move |_| {
                    if let Err(message) = crate::desktop::external::open_external_http(&url) {
                        error.set(Some(message));
                    }
                },
                {children}
            }
            if let Some(message) = error.read().as_ref() {
                span { class: "text-xs text-destructive", "{message}" }
            }
        }
    }
    #[cfg(not(feature = "desktop"))]
    rsx! {
        a { href: "{url}", target: "_blank", rel: if sponsored { "noopener sponsored" } else { "noopener noreferrer" }, class: "{class}", {children} }
    }
}
/// Display label for a runs-on slug (`ps-vita`, `windows`, ...).
pub fn target_label(slug: &str) -> String {
    TargetPlatform::from_slug(slug)
        .map(|t| t.label().to_string())
        .unwrap_or_else(|| slug.to_string())
}

/// Percent-encodes a value for use in a query string.
pub fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Appends an already-encoded `key=value` pair to a path, keeping any existing query string.
pub fn with_query(base: &str, pair: &str) -> String {
    if base.contains('?') {
        format!("{base}&{pair}")
    } else {
        format!("{base}?{pair}")
    }
}

/// Initials for an avatar or cover placeholder.
pub fn initials_of(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect()
}

/// Label and badge variant for an AI usage value.
fn ai_style(usage: &str) -> (&'static str, &'static str) {
    match usage {
        "none" => ("Human-made", BADGE_SUCCESS),
        "assisted" => ("AI-assisted", BADGE_WARNING),
        "primarily_ai" => ("Primarily AI", BADGE_DESTRUCTIVE),
        _ => ("AI use unknown", BADGE_OUTLINE),
    }
}

/// Full AI badge text, for example "AI-assisted · Claude".
pub fn ai_text(usage: &str, tools: &[String]) -> String {
    let (label, _) = ai_style(usage);
    if tools.is_empty() || usage == "none" || usage == "unknown" {
        label.to_string()
    } else {
        format!("{label} · {}", tools.join(", "))
    }
}

/// Renders `node` as a fragment child so the `key` on its root takes effect. Dioxus compares keys
/// only between fragment children (`for` bodies, `if` branches, `{expr}` nodes) and ignores a key on
/// a component's own root or on a nested node. Wrap a keyed node in this to remount it whenever its
/// key changes.
pub fn keyed(node: Element) -> Element {
    rsx! { {node} }
}

/// Small secondary badge. `class` adds a text colour or overrides.
#[component]
pub fn Chip(#[props(default)] class: String, children: Element) -> Element {
    rsx! {
        span { class: "{BADGE} {BADGE_SECONDARY} {class}",
            {children}
        }
    }
}

/// A titled section. Renders `section > h2`.
#[component]
pub fn Section(heading: String, #[props(default)] class: String, children: Element) -> Element {
    rsx! {
        section { class: "{class}",
            h2 { class: "mb-4 {H2}", "{heading}" }
            {children}
        }
    }
}

/// Icon for a runs-on target. Primary targets have their brand tint.
#[component]
pub fn TargetIcon(target: String) -> Element {
    let label = target_label(&target);
    let icon = match target.as_str() {
        "windows" => rsx! { WindowsIcon { class: "h-4 w-4 text-[#0078d4]" } },
        "macos" | "ios" => rsx! { AppleIcon { class: "h-4 w-4" } },
        "linux" => rsx! { LinuxIcon { class: "h-4 w-4 text-[#fcc624]" } },
        "android" => rsx! { AndroidIcon { class: "h-4 w-4 text-[#3ddc84]" } },
        "web" => rsx! { GlobeIcon { class: "h-4 w-4 text-[#22c55e]" } },
        _ => rsx! { span { class: "text-[10px] font-bold uppercase", "{label}" } },
    };
    rsx! {
        span { class: "inline-flex size-7 items-center justify-center rounded-md border text-muted-foreground", title: "{label}",
            {icon}
            span { class: "sr-only", "{label}" }
        }
    }
}

/// Four-segment playability meter.
#[component]
pub fn PlayabilityMeter(playability: String) -> Element {
    let (filled, label) = match playability.as_str() {
        "complete" => (4usize, "Complete"),
        "playable" => (3, "Playable"),
        "in_progress" => (2, "In progress"),
        _ => (0, "Unknown"),
    };
    let segments: Vec<bool> = (0..4).map(|i| i < filled).collect();
    rsx! {
        div { class: "flex items-center gap-2 text-xs text-muted-foreground", role: "meter", "aria-label": "Playability: {label}", "aria-valuemin": "0", "aria-valuemax": "4", "aria-valuenow": "{filled}",
            div { class: "flex gap-0.5",
                for on in segments {
                    span { class: if on { "h-1.5 w-4 rounded-full bg-success" } else { "h-1.5 w-4 rounded-full bg-muted" } }
                }
            }
            span { "{label}" }
        }
    }
}

/// AI usage badge with a hover and focus tooltip listing the evidence.
#[component]
pub fn AiBadge(
    usage: String,
    tools: Vec<String>,
    evidence: Vec<AiEvidenceDto>,
    #[props(default)] align_end: bool,
) -> Element {
    let (_, cls) = ai_style(&usage);
    let text = ai_text(&usage, &tools);
    let placement = if align_end { "right-0" } else { "left-0" };
    rsx! {
        span { class: "group relative inline-flex", tabindex: "0",
            span { class: "{BADGE} {cls}", "{text}" }
            if !evidence.is_empty() {
                span { class: "{POPOVER} {placement} pointer-events-none invisible absolute bottom-full mb-2 w-64 p-3 text-xs opacity-0 transition-opacity group-hover:visible group-hover:opacity-100 group-focus:visible group-focus:opacity-100",
                    span { class: "mb-1 block font-medium", "Evidence" }
                    for ev in evidence {
                        span { class: "block text-muted-foreground", "{ev.signal}: {ev.detail}" }
                    }
                }
            }
        }
    }
}

/// Display form of a release version: `v1.2` for numeric tags (`1.2` or `v1.2`), other tags
/// (`latest`, `nightly-2026-10-01`) unchanged.
pub fn version_label(v: &str) -> String {
    if v.starts_with(|c: char| c.is_ascii_digit()) {
        format!("v{v}")
    } else if let Some(rest) = v.strip_prefix('V') {
        format!("v{rest}")
    } else {
        v.to_string()
    }
}

/// Release chip: version and relative release time, or the release state label.
#[component]
pub fn ReleaseChip(
    version: Option<String>,
    at: Option<DateTime<Utc>>,
    state_label: String,
) -> Element {
    match (version, at) {
        (Some(v), Some(at)) => rsx! {
            Chip { class: "text-foreground",
                "{version_label(&v)} · "
                RelativeTime { at: at }
            }
        },
        (Some(v), None) => rsx! { Chip { "{version_label(&v)}" } },
        _ => rsx! { Chip { class: "text-muted-foreground", "{state_label}" } },
    }
}

/// Port card: used on game, author, platform and home pages.
#[component]
pub fn PortCardView(port: PortCard, #[props(default)] show_game: bool) -> Element {
    let href = crate::routes::Route::PortPage {
        game_slug: port.game_slug.clone(),
        port_slug: port.slug.clone(),
    };
    let game_href = crate::routes::Route::GamePage {
        slug: port.game_slug.clone(),
    };
    let initials = initials_of(&port.name);
    let tools_text = port.ai_tools.join(", ");
    rsx! {
        article { class: "{CARD_SURFACE} flex flex-col gap-4 p-4",
            header { class: "flex items-start gap-3",
                div { class: "relative h-20 w-15 shrink-0 overflow-hidden rounded-md bg-muted",
                    Picture {
                        media: port.cover.clone(),
                        alt: format!("{} cover art", port.game_name),
                        initials: initials,
                        sizes: "60px",
                        class: "h-full w-full object-cover",
                    }
                }
                div { class: "min-w-0 flex-1",
                    h3 { class: "font-semibold leading-snug",
                        Link { to: href.clone(), class: "hover:underline", "{port.name}" }
                    }
                    if show_game {
                        p { class: "text-sm text-muted-foreground",
                            Link { to: game_href.clone(), class: "hover:text-foreground", "{port.game_name}" }
                        }
                    }
                    div { class: "mt-2 flex flex-wrap items-center gap-1.5",
                        Chip { "{port.kind_label}" }
                        ReleaseChip {
                            version: port.latest_version.clone(),
                            at: port.latest_release_at,
                            state_label: port.release_state_label.clone(),
                        }
                        if let Some(a) = port.maintenance {
                            crate::components::MaintenanceBadge { level: a.level }
                        }
                    }
                }
            }
            div { class: "flex flex-wrap items-center gap-1.5",
                for t in port.targets.clone() {
                    TargetIcon { target: t }
                }
                AiBadge {
                    usage: port.ai_usage.clone(),
                    tools: port.ai_tools.clone(),
                    evidence: port.ai_evidence.clone(),
                }
                if !tools_text.is_empty() && port.ai_usage == "none" {
                    span { class: "sr-only", "No AI tools" }
                }
            }
            if !port.authors.is_empty() {
                ul { class: "flex flex-wrap gap-x-3 gap-y-1",
                    for a in port.authors.clone() {
                        li { class: "flex items-center gap-1.5 text-sm",
                            match a.github_login.clone() {
                                Some(login) => rsx! {
                                    Link { to: crate::routes::Route::AuthorPage { login: login.to_lowercase() }, class: "flex items-center gap-1.5 text-muted-foreground hover:text-foreground",
                                        Picture {
                                            media: a.avatar.clone(),
                                            alt: format!("{} avatar", a.name),
                                            initials: initials_of(&a.name),
                                            sizes: "24px",
                                            class: "size-5 rounded-full object-cover text-[8px]",
                                        }
                                        span { "{a.name}" }
                                    }
                                },
                                None => rsx! { span { class: "text-muted-foreground", "{a.name}" } },
                            }
                            if let Some(link) = a.funding.first() {
                                    ExternalLink {
                                        url: link.url.clone(),
                                        sponsored: true,
                                        class: "inline-flex items-center text-muted-foreground transition-colors hover:text-destructive".to_string(),
                                        crate::components::icons::HeartIcon { class: "size-3.5" }
                                    }
                            }
                        }
                    }
                }
            }
            PlayabilityMeter { playability: port.playability.clone() }
            footer { class: "mt-auto flex flex-wrap items-center gap-2 border-t pt-4",
                StarButton { port_id: port.id, count: port.star_count as i64 }
                if port.github_stars > 0 {
                    span { class: "text-xs text-muted-foreground tabular", "{port.github_stars} GitHub stars" }
                }
                div { class: "ml-auto flex gap-2",
                    ExternalLink { url: port.repo_url.clone(), class: format!("{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}"), "View source" }
                    if let Some(url) = port.latest_release_url.clone() {
                        ExternalLink { url, class: format!("{BUTTON} {BUTTON_PRIMARY} {BUTTON_SM}"), "View releases ↗" }
                    }
                }
            }
        }
    }
}

/// Trend chip from the current and previous rank: New, ▲n, ▼n, or a dash.
#[component]
pub fn TrendChip(rank: i64, prev_rank: Option<i64>) -> Element {
    match prev_rank {
        None => rsx! { span { class: "{BADGE} {BADGE_OUTLINE}", "New" } },
        Some(p) if p > rank => {
            rsx! { span { class: "{BADGE} {BADGE_SUCCESS} tabular", "▲ {p - rank}" } }
        }
        Some(p) if p < rank => {
            rsx! { span { class: "{BADGE} {BADGE_DESTRUCTIVE} tabular", "▼ {rank - p}" } }
        }
        Some(_) => rsx! { span { class: "text-xs text-muted-foreground", "–" } },
    }
}

/// Grid of cover cards. Children are `li` elements.
#[component]
pub fn CardGrid(children: Element) -> Element {
    rsx! {
        ul { class: "grid grid-cols-2 gap-x-4 gap-y-8 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-6",
            {children}
        }
    }
}

/// Horizontal scrolling rail with snap points. Children are `li` elements.
#[component]
pub fn Rail(children: Element) -> Element {
    rsx! {
        ul { class: "rail flex snap-x gap-4 overflow-x-auto pb-4",
            {children}
        }
    }
}

/// Page links. Hidden when there is only one page.
#[component]
pub fn Pager(base: String, page: i64, pages: i64) -> Element {
    if pages <= 1 {
        return rsx! {};
    }
    let prev = (page > 1).then(|| with_query(&base, &format!("page={}", page - 1)));
    let next = (page < pages).then(|| with_query(&base, &format!("page={}", page + 1)));
    rsx! {
        nav { class: "mt-10 flex items-center justify-center gap-2 text-sm", "aria-label": "Pages",
            match prev {
                Some(href) => rsx! { InternalLink { href, class: format!("{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}"), "Previous" } },
                None => rsx! { span { class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM} pointer-events-none opacity-50", "aria-disabled": "true", "Previous" } },
            }
            span { class: "px-3 tabular text-muted-foreground", "Page {page} of {pages}" }
            match next {
                Some(href) => rsx! { InternalLink { href, class: format!("{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}"), "Next" } },
                None => rsx! { span { class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM} pointer-events-none opacity-50", "aria-disabled": "true", "Next" } },
            }
        }
    }
}

/// Cards / List toggle. `base` is the current path with its filters and no `view` parameter.
#[component]
pub fn ViewToggle(base: String, list: bool) -> Element {
    let list_href = with_query(&base, "view=list");
    let (cards_cls, list_cls) = if list {
        ("", TAB_ACTIVE)
    } else {
        (TAB_ACTIVE, "")
    };
    rsx! {
        nav { class: TABS_LIST, "aria-label": "View",
            InternalLink { href: base, class: format!("{TAB} {cards_cls}"), "Cards" }
            InternalLink { href: list_href, class: format!("{TAB} {list_cls}"), "List" }
        }
    }
}

/// Breadcrumb trail. Each item is `(label, href)`. The last item is the current page.
#[component]
pub fn Breadcrumb(items: Vec<(String, String)>) -> Element {
    let last = items.len().saturating_sub(1);
    rsx! {
        nav { class: "text-sm text-muted-foreground", "aria-label": "Breadcrumb",
            ol { class: "flex flex-wrap items-center gap-2",
                for (i, (label, href)) in items.into_iter().enumerate() {
                    li { class: "flex items-center gap-2",
                        if i == last {
                            span { class: "text-foreground", "aria-current": "page", "{label}" }
                        } else {
                            InternalLink { href, class: "transition-colors hover:text-foreground".to_string(), "{label}" }
                            span { "aria-hidden": "true", "/" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn EmptyState(text: String) -> Element {
    rsx! {
        p { class: "rounded-xl border border-dashed p-10 text-center text-sm text-muted-foreground", "{text}" }
    }
}

/// Sign-in prompt for pages that need an account. Redirects back to the current page.
#[component]
pub fn LoginPrompt(text: String) -> Element {
    #[cfg(feature = "desktop")]
    return rsx! {
        div { class: "{CARD_SURFACE} flex flex-col items-center gap-4 p-8 text-center",
            p { class: "text-sm text-muted-foreground", "{text}" }
            p { class: "text-sm text-muted-foreground", "Use Continue with Google in the RecompHub desktop sign-in panel." }
        }
    };
    #[cfg(not(feature = "desktop"))]
    rsx! {
        div { class: "{CARD_SURFACE} flex flex-col items-center gap-4 p-8 text-center",
            p { class: "text-sm text-muted-foreground", "{text}" }
            button { r#type: "button", class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_MD}", onclick: move |_| login_redirect(),
                "Sign in"
            }
        }
    }
}

/// Gameplay videos, watched without leaving the page: one large player with a playlist beside
/// it. Picking a video swaps it into the player, so only one embed exists and plays at a time.
/// Platform tabs filter the playlist when the videos cover more than one target. Every control
/// is a button, so nothing navigates to YouTube, even before hydration.
#[component]
pub fn VideoPlayer(videos: Vec<VideoDto>) -> Element {
    let mut active = use_signal(|| "all".to_string());
    let mut selected = use_signal(|| None::<i64>);
    let mut playing = use_signal(|| false);
    let mut removed = use_signal(Vec::<i64>::new);
    let mut hide_error = use_signal(|| None::<String>);
    let external_error = use_signal(|| None::<String>);
    let online = crate::components::viewer::use_account_network_ready();
    let can_hide = use_context::<ViewerStore>().can_moderate() && online;
    #[cfg(feature = "desktop")]
    let native = true;
    #[cfg(not(feature = "desktop"))]
    let native = false;

    let visible: Vec<VideoDto> = videos
        .into_iter()
        .filter(|v| !v.hidden && !removed.read().contains(&v.id))
        .collect();
    let mut targets: Vec<String> = Vec::new();
    for v in &visible {
        if !targets.contains(&v.target) {
            targets.push(v.target.clone());
        }
    }
    let filter = active();
    let filtered: Vec<VideoDto> = visible
        .iter()
        .filter(|v| filter == "all" || v.target == filter)
        .cloned()
        .collect();
    let listed = if filtered.is_empty() {
        visible
    } else {
        filtered
    };
    let Some(current) = selected()
        .and_then(|id| listed.iter().find(|v| v.id == id))
        .or(listed.first())
        .cloned()
    else {
        return rsx! {};
    };
    let current_id = current.id;
    let embed = format!(
        "https://www.youtube-nocookie.com/embed/{}?autoplay=1&rel=0&playsinline=1",
        current.youtube_id
    );
    let channel = current.channel.clone().unwrap_or_default();
    let hide_message = hide_error().unwrap_or_default();
    let single = listed.len() == 1;
    let layout = if single {
        "max-w-3xl"
    } else {
        "grid gap-6 lg:grid-cols-[minmax(0,1fr)_22rem]"
    };
    rsx! {
        div { class: "flex flex-col gap-4",
            if targets.len() > 1 {
                div { role: "tablist", class: TABS_LIST,
                    for t in std::iter::once("all".to_string()).chain(targets.clone()) {
                        button { r#type: "button", role: "tab",
                            "aria-selected": if filter == t { "true" } else { "false" },
                            class: if filter == t { "{TAB} {TAB_ACTIVE}" } else { "{TAB}" },
                            onclick: {
                                let t = t.clone();
                                move |_| {
                                    active.set(t.clone());
                                    selected.set(None);
                                    playing.set(false);
                                }
                            },
                            if t == "all" { "All" } else { "{target_label(&t)}" }
                        }
                    }
                }
            }
            div { class: "{layout}",
                div { class: "min-w-0",
                    div { class: "aspect-video w-full overflow-hidden rounded-lg border bg-black",
                        if !cfg!(feature = "desktop") && playing() {
                                iframe {
                                    key: "{current.youtube_id}",
                                    class: "h-full w-full",
                                    src: "{embed}",
                                    title: "{current.title}",
                                    allow: "autoplay; encrypted-media; picture-in-picture; fullscreen",
                                    allowfullscreen: true,
                                    referrerpolicy: "strict-origin-when-cross-origin",
                                }
                        } else {
                            button { r#type: "button", class: "group relative block h-full w-full cursor-pointer",
                                "aria-label": if native { "Play video in browser" } else { "Play {current.title}" },
                                onclick: {
                                    #[cfg(feature = "desktop")]
                                    let youtube_id = current.youtube_id.clone();
                                    move |_| {
                                        #[cfg(feature = "desktop")]
                                        {
                                            let url = format!("https://www.youtube.com/watch?v={youtube_id}");
                                            if let Err(error) = crate::desktop::external::open_external_http(&url) {
                                                let mut external_error = external_error;
                                                external_error.set(Some(error));
                                            }
                                        }
                                        #[cfg(not(feature = "desktop"))]
                                        playing.set(true);
                                    }
                                },
                                Picture {
                                    media: current.thumb.clone(),
                                    alt: format!("Video thumbnail: {}", current.title),
                                    initials: "▶".to_string(),
                                    sizes: "(min-width: 1024px) 66vw, 100vw",
                                    class: "h-full w-full object-cover transition-opacity group-hover:opacity-90",
                                }
                                span { class: "absolute inset-0 flex items-center justify-center",
                                    span { class: "flex size-16 items-center justify-center rounded-full bg-black/70 text-white transition-transform group-hover:scale-105",
                                        crate::components::icons::PlayIcon { class: "size-7" }
                                    }
                                }
                                if native { span { class: "sr-only", "Play video in browser" } }
                            }
                        }
                        if let Some(error) = external_error.read().as_ref() {
                            p { class: "mt-2 text-sm text-destructive", "{error}" }
                        }
                    }
                    div { class: "mt-3 flex flex-wrap items-start justify-between gap-x-4 gap-y-2",
                        div { class: "min-w-0",
                            p { class: "font-medium", "{current.title}" }
                            if !channel.is_empty() {
                                p { class: "mt-0.5 text-sm text-muted-foreground", "{channel}" }
                            }
                        }
                        if can_hide {
                            div { class: "flex items-center gap-2",
                                if !hide_message.is_empty() {
                                    span { class: "text-xs text-destructive", "{hide_message}" }
                                }
                                button {
                                    r#type: "button",
                                    class: "{BUTTON} {BUTTON_DESTRUCTIVE} {BUTTON_XS}",
                                    onclick: move |_| {
                                        spawn(async move {
                                            if let Err(message) = crate::components::viewer::require_account_network() {
                                                hide_error.set(Some(message)); return;
                                            }
                                            match api::hide_video(current_id).await {
                                                Ok(()) => {
                                                    removed.write().push(current_id);
                                                    selected.set(None);
                                                    playing.set(false);
                                                    hide_error.set(None);
                                                }
                                                Err(e) => hide_error.set(Some(e.to_string())),
                                            }
                                        });
                                    },
                                    "Hide video"
                                }
                            }
                        }
                    }
                }
                if !single {
                    div { class: "relative min-h-0",
                        ol { class: "flex max-h-[28rem] flex-col gap-1 overflow-y-auto lg:absolute lg:inset-0 lg:max-h-none",
                            "aria-label": "Playlist",
                            for (i, v) in listed.iter().enumerate() {
                                {
                                    let id = v.id;
                                    let is_current = id == current_id;
                                    let item_channel = v.channel.clone().unwrap_or_default();
                                    rsx! {
                                        li { key: "{id}",
                                            button { r#type: "button",
                                                class: if is_current { "flex w-full cursor-pointer gap-3 rounded-md bg-muted p-2 text-left" } else { "flex w-full cursor-pointer gap-3 rounded-md p-2 text-left hover:bg-muted/60" },
                                                "aria-current": if is_current { "true" } else { "false" },
                                                onclick: move |_| {
                                                    selected.set(Some(id));
                                                    playing.set(true);
                                                },
                                                span { class: "relative aspect-video w-32 shrink-0 overflow-hidden rounded bg-muted",
                                                    Picture {
                                                        media: v.thumb.clone(),
                                                        alt: String::new(),
                                                        initials: "▶".to_string(),
                                                        sizes: "128px",
                                                        class: "h-full w-full object-cover",
                                                    }
                                                    if is_current && playing() {
                                                        span { class: "absolute inset-x-0 bottom-0 bg-black/70 px-1.5 py-0.5 text-[10px] font-medium text-white", "Now playing" }
                                                    }
                                                }
                                                span { class: "min-w-0",
                                                    span { class: "line-clamp-2 text-sm font-medium", "{v.title}" }
                                                    if !item_channel.is_empty() {
                                                        span { class: "mt-0.5 block text-xs text-muted-foreground", "{item_channel}" }
                                                    }
                                                    span { class: "sr-only", "Video {i + 1}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One slide of a [`PortHero`] gallery. `Cover` is the last-resort slide used only when a port
/// has neither a screenshot nor a video: box art is portrait and would crop badly stretched to
/// a 16:9 frame, so it renders contained instead of covered, and never appears while there is
/// real gameplay media to show.
#[derive(Clone)]
enum HeroItem {
    Screenshot(MediaRef),
    Cover(Option<MediaRef>),
    Video(VideoDto),
}

/// The port page's media gallery: member-contributed screenshots (or the game's own, when the
/// port has none yet) and gameplay videos in one Steam-style strip, opening on the first of
/// those rather than the cover. One large slide at a time with a thumbnail rail beneath it;
/// picking a video thumbnail plays it in place, so only one embed exists at once.
#[component]
pub fn PortHero(
    cover: Option<MediaRef>,
    screenshots: Vec<MediaRef>,
    videos: Vec<VideoDto>,
    alt: String,
    initials: String,
) -> Element {
    let mut selected = use_signal(|| 0usize);
    let mut playing = use_signal(|| false);
    let online = crate::components::viewer::use_account_network_ready();
    let can_hide = use_context::<ViewerStore>().can_moderate() && online;
    let mut removed = use_signal(Vec::<i64>::new);
    let mut hide_error = use_signal(|| None::<String>);
    let external_error = use_signal(|| None::<String>);

    let live_videos: Vec<VideoDto> = videos
        .into_iter()
        .filter(|v| !removed.read().contains(&v.id))
        .collect();
    let has_real_media = !screenshots.is_empty() || !live_videos.is_empty();
    let items: Vec<HeroItem> = screenshots
        .into_iter()
        .map(HeroItem::Screenshot)
        .chain(live_videos.into_iter().map(HeroItem::Video))
        .chain((!has_real_media).then_some(HeroItem::Cover(cover)))
        .collect();
    let index = selected().min(items.len() - 1);
    let current = items[index].clone();
    let single = items.len() == 1;

    rsx! {
        div { class: "flex flex-col gap-3",
            div { class: "aspect-video w-full overflow-hidden rounded-lg border bg-black",
                match &current {
                    HeroItem::Screenshot(media) => rsx! {
                        Picture {
                            media: Some(media.clone()),
                            alt: alt.clone(),
                            initials: initials.clone(),
                            sizes: "(min-width: 1024px) 66vw, 100vw",
                            eager: true,
                            class: "h-full w-full object-cover".to_string(),
                        }
                    },
                    HeroItem::Cover(media) => rsx! {
                        Picture {
                            media: media.clone(),
                            alt: alt.clone(),
                            initials: initials.clone(),
                            sizes: "(min-width: 1024px) 66vw, 100vw",
                            eager: true,
                            class: "h-full w-full object-contain".to_string(),
                        }
                    },
                    HeroItem::Video(v) => {
                        let video_id = v.id;
                        let title = v.title.clone();
                        let thumb = v.thumb.clone();
                        let embed = format!(
                            "https://www.youtube-nocookie.com/embed/{}?autoplay=1&rel=0&playsinline=1",
                            v.youtube_id
                        );
                        #[cfg(feature = "desktop")]
                        let youtube_id = v.youtube_id.clone();
                        rsx! {
                            if !cfg!(feature = "desktop") && playing() {
                                    iframe {
                                        key: "{video_id}",
                                        class: "h-full w-full",
                                        src: "{embed}",
                                        title: "{title}",
                                        allow: "autoplay; encrypted-media; picture-in-picture; fullscreen",
                                        allowfullscreen: true,
                                        referrerpolicy: "strict-origin-when-cross-origin",
                                    }
                            } else {
                                button { r#type: "button", class: "group relative block h-full w-full cursor-pointer",
                                    "aria-label": if cfg!(feature = "desktop") { "Play video in browser" } else { "Play {title}" },
                                    onclick: move |_| {
                                        #[cfg(feature = "desktop")]
                                        {
                                            let url = format!("https://www.youtube.com/watch?v={youtube_id}");
                                            if let Err(error) = crate::desktop::external::open_external_http(&url) {
                                                let mut external_error = external_error;
                                                external_error.set(Some(error));
                                            }
                                        }
                                        #[cfg(not(feature = "desktop"))]
                                        playing.set(true);
                                    },
                                    Picture {
                                        media: thumb,
                                        alt: format!("Video thumbnail: {title}"),
                                        initials: "▶".to_string(),
                                        sizes: "(min-width: 1024px) 66vw, 100vw",
                                        class: "h-full w-full object-cover transition-opacity group-hover:opacity-90".to_string(),
                                    }
                                    span { class: "absolute inset-0 flex items-center justify-center",
                                        span { class: "flex size-16 items-center justify-center rounded-full bg-black/70 text-white transition-transform group-hover:scale-105",
                                            crate::components::icons::PlayIcon { class: "size-7" }
                                        }
                                    }
                                    if cfg!(feature = "desktop") { span { class: "sr-only", "Play video in browser" } }
                                }
                            }
                            if let Some(error) = external_error.read().as_ref() {
                                p { class: "mt-2 text-sm text-destructive", "{error}" }
                            }
                        }
                    }
                }
            }
            if let HeroItem::Video(v) = &current
                && can_hide
            {
                {
                    let video_id = v.id;
                    let hide_message = hide_error().unwrap_or_default();
                    rsx! {
                        div { class: "flex items-center gap-2",
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_DESTRUCTIVE} {BUTTON_XS}",
                                onclick: move |_| {
                                    spawn(async move {
                                        if let Err(message) = crate::components::viewer::require_account_network() {
                                            hide_error.set(Some(message)); return;
                                        }
                                        match api::hide_video(video_id).await {
                                            Ok(()) => {
                                                removed.write().push(video_id);
                                                selected.set(0);
                                                playing.set(false);
                                                hide_error.set(None);
                                            }
                                            Err(e) => hide_error.set(Some(e.to_string())),
                                        }
                                    });
                                },
                                "Hide video"
                            }
                            if !hide_message.is_empty() {
                                span { class: "text-xs text-destructive", "{hide_message}" }
                            }
                        }
                    }
                }
            }
            if !single {
                ul { class: "rail flex snap-x gap-2 overflow-x-auto pb-1", "aria-label": "Gallery",
                    for (i, item) in items.iter().enumerate() {
                        li { key: "{i}", class: "w-24 shrink-0 snap-start",
                            button {
                                r#type: "button",
                                class: if i == index { "relative block aspect-video w-full overflow-hidden rounded-md border-2 border-primary" } else { "relative block aspect-video w-full overflow-hidden rounded-md border opacity-80 hover:opacity-100" },
                                "aria-current": if i == index { "true" } else { "false" },
                                onclick: move |_| {
                                    selected.set(i);
                                    playing.set(false);
                                },
                                {
                                    let (thumb_media, contain) = match item {
                                        HeroItem::Screenshot(m) => (Some(m.clone()), false),
                                        HeroItem::Cover(m) => (m.clone(), true),
                                        HeroItem::Video(v) => (v.thumb.clone(), false),
                                    };
                                    let fit = if contain { "object-contain" } else { "object-cover" };
                                    rsx! {
                                        Picture {
                                            media: thumb_media,
                                            alt: String::new(),
                                            initials: "".to_string(),
                                            sizes: "96px",
                                            class: format!("h-full w-full {fit}"),
                                        }
                                    }
                                }
                                if matches!(item, HeroItem::Video(_)) {
                                    span { class: "absolute inset-0 flex items-center justify-center bg-black/30",
                                        crate::components::icons::PlayIcon { class: "size-4 text-white" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Listing rules every project on RecompHub must follow: (rule, explanation).
pub const GROUND_RULES: [(&str, &str); 5] = [
    (
        "No game data",
        "The repository, releases and downloads must not contain or link to copyrighted game assets, ROMs, disc images, firmware, BIOS files or keys.",
    ),
    (
        "Bring your own copy",
        "Players supply their own legally obtained copy of the game. Any asset extraction runs on the player's copy.",
    ),
    (
        "Free to play",
        "No paywalled builds of someone else's game. Donations and sponsorships for your own work are fine.",
    ),
    (
        "Credit and licences",
        "Name the decompilation, recompilation and tooling projects you build on and follow their licences.",
    ),
    (
        "Honest AI disclosure",
        "State whether AI tools were used to write the project. RecompHub shows AI usage on every listing.",
    ),
];

/// The ground rules as a numbered list. Listings that break them are hidden by moderators.
#[component]
pub fn GroundRules(#[props(default)] class: String) -> Element {
    rsx! {
        section { class: "{class}", "aria-labelledby": "ground-rules",
            h2 { id: "ground-rules", class: H2, "Ground rules" }
            p { class: "mt-1 text-sm text-muted-foreground", "Every listed project must follow these. Listings that break them are hidden." }
            ol { class: "mt-4 grid gap-3 sm:grid-cols-2",
                for (i, (rule, detail)) in GROUND_RULES.iter().enumerate() {
                    li { key: "{i}", class: "{CARD_SURFACE} flex gap-3 p-4",
                        span { class: "flex size-6 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-medium tabular", "{i + 1}" }
                        div {
                            p { class: "text-sm font-medium", "{rule}" }
                            p { class: "mt-1 text-sm text-muted-foreground", "{detail}" }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    #[test]
    fn url_encode_escapes_query_delimiters() {
        assert_eq!(url_encode("mario & luigi"), "mario%20%26%20luigi");
    }

    #[test]
    fn with_query_keeps_existing_query_string() {
        assert_eq!(with_query("/trending", "page=2"), "/trending?page=2");
        assert_eq!(
            with_query("/trending?window=week", "page=2"),
            "/trending?window=week&page=2"
        );
    }

    #[test]
    fn ai_text_drops_tools_for_human_made() {
        assert_eq!(ai_text("none", &["Claude".to_string()]), "Human-made");
        assert_eq!(
            ai_text("assisted", &["Claude".to_string(), "Gemini".to_string()]),
            "AI-assisted · Claude, Gemini"
        );
    }
}
