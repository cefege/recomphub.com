use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use recomphub_shared::{AiEvidenceDto, PortCard, TargetPlatform, VideoDto};

use crate::api;
use crate::components::icons::{AndroidIcon, AppleIcon, GlobeIcon, LinuxIcon, WindowsIcon};
use crate::components::styles::{
    BADGE, BADGE_DESTRUCTIVE, BADGE_OUTLINE, BADGE_SECONDARY, BADGE_SUCCESS, BADGE_WARNING, BUTTON,
    BUTTON_DESTRUCTIVE, BUTTON_MD, BUTTON_OUTLINE, BUTTON_PRIMARY, BUTTON_SM, BUTTON_XS,
    CARD_SURFACE, H2, POPOVER, TAB, TAB_ACTIVE, TABS_LIST,
};
use crate::components::{Picture, RelativeTime, StarButton, ViewerStore, login_redirect};

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
pub fn AiBadge(usage: String, tools: Vec<String>, evidence: Vec<AiEvidenceDto>) -> Element {
    let (_, cls) = ai_style(&usage);
    let text = ai_text(&usage, &tools);
    rsx! {
        span { class: "group relative inline-flex", tabindex: "0",
            span { class: "{BADGE} {cls}", "{text}" }
            if !evidence.is_empty() {
                span { class: "{POPOVER} pointer-events-none invisible absolute bottom-full left-0 mb-2 w-64 p-3 text-xs opacity-0 transition-opacity group-hover:visible group-hover:opacity-100 group-focus:visible group-focus:opacity-100",
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
    let href = format!("/games/{}/{}", port.game_slug, port.slug);
    let game_href = format!("/games/{}", port.game_slug);
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
                        a { href: "{href}", class: "hover:underline", "{port.name}" }
                    }
                    if show_game {
                        p { class: "text-sm text-muted-foreground",
                            a { href: "{game_href}", class: "hover:text-foreground", "{port.game_name}" }
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
                                    a { href: "/authors/{login}", class: "flex items-center gap-1.5 text-muted-foreground hover:text-foreground",
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
                                a {
                                    href: "{link.url}",
                                    target: "_blank",
                                    rel: "noopener sponsored",
                                    title: "Support {a.name} on {link.platform.label()}",
                                    "aria-label": "Support {a.name} on {link.platform.label()}",
                                    class: "inline-flex items-center text-muted-foreground transition-colors hover:text-destructive",
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
                    a { href: "{port.repo_url}", target: "_blank", rel: "noopener", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}", "Source" }
                    if let Some(url) = port.latest_release_url.clone() {
                        a { href: "{url}", target: "_blank", rel: "noopener", class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_SM}", "Download" }
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
                Some(href) => rsx! { a { href: "{href}", rel: "prev", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}", "Previous" } },
                None => rsx! { span { class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM} pointer-events-none opacity-50", "aria-disabled": "true", "Previous" } },
            }
            span { class: "px-3 tabular text-muted-foreground", "Page {page} of {pages}" }
            match next {
                Some(href) => rsx! { a { href: "{href}", rel: "next", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}", "Next" } },
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
            a { href: "{base}", class: "{TAB} {cards_cls}", "aria-current": if !list { "page" } else { "false" }, "Cards" }
            a { href: "{list_href}", class: "{TAB} {list_cls}", "aria-current": if list { "page" } else { "false" }, "List" }
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
                            a { href: "{href}", class: "transition-colors hover:text-foreground", "{label}" }
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
    rsx! {
        div { class: "{CARD_SURFACE} flex flex-col items-center gap-4 p-8 text-center",
            p { class: "text-sm text-muted-foreground", "{text}" }
            button { r#type: "button", class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_MD}", onclick: move |_| login_redirect(),
                "Sign in"
            }
        }
    }
}

/// Gameplay video tabs. "All" shows every video. Other tabs filter by target.
/// Clicking a thumbnail swaps in the nocookie iframe.
#[component]
pub fn VideoTabs(videos: Vec<VideoDto>) -> Element {
    let mut active = use_signal(|| "all".to_string());
    let visible: Vec<VideoDto> = videos.into_iter().filter(|v| !v.hidden).collect();
    if visible.is_empty() {
        return rsx! {};
    }
    let mut targets: Vec<String> = Vec::new();
    for v in &visible {
        if !targets.contains(&v.target) {
            targets.push(v.target.clone());
        }
    }
    rsx! {
        div { class: "flex flex-col gap-4",
            if targets.len() > 1 {
                div { role: "tablist", class: TABS_LIST,
                    button { r#type: "button", role: "tab", "aria-selected": if active() == "all" { "true" } else { "false" },
                        class: if active() == "all" { "{TAB} {TAB_ACTIVE}" } else { "{TAB}" },
                        onclick: move |_| active.set("all".to_string()),
                        "All"
                    }
                    for t in targets.clone() {
                        button { r#type: "button", role: "tab",
                            "aria-selected": if active() == t { "true" } else { "false" },
                            class: if active() == t { "{TAB} {TAB_ACTIVE}" } else { "{TAB}" },
                            onclick: {
                                let t = t.clone();
                                move |_| active.set(t.clone())
                            },
                            "{target_label(&t)}"
                        }
                    }
                }
            }
            ul { class: "grid grid-cols-1 gap-x-4 gap-y-6 sm:grid-cols-2 lg:grid-cols-3",
                for v in visible {
                    {
                        let id = v.id;
                        let target = v.target.clone();
                        rsx! {
                            li { key: "{id}", hidden: active() != "all" && active() != target, "data-target": "{target}",
                                VideoFacade { video: v }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One video. Renders a link to YouTube until clicked, then the nocookie embed.
#[component]
pub fn VideoFacade(video: VideoDto) -> Element {
    let mut playing = use_signal(|| false);
    let mut gone = use_signal(|| false);
    let mut hide_error = use_signal(|| None::<String>);
    let can_hide = use_context::<ViewerStore>().can_moderate();
    let hide_message = hide_error().unwrap_or_default();
    let video_id = video.id;
    let watch = format!("https://www.youtube.com/watch?v={}", video.youtube_id);
    let embed = format!(
        "https://www.youtube-nocookie.com/embed/{}?autoplay=1",
        video.youtube_id
    );
    let channel = video.channel.clone().unwrap_or_default();
    if gone() {
        return rsx! {};
    }
    if playing() {
        return rsx! {
            div { class: "flex flex-col gap-2",
                div { class: "aspect-video w-full overflow-hidden rounded-lg bg-muted",
                    iframe { class: "h-full w-full", src: "{embed}", title: "{video.title}", allow: "autoplay; encrypted-media; picture-in-picture", allowfullscreen: true }
                }
                p { class: "line-clamp-2 text-sm font-medium", "{video.title}" }
            }
        };
    }
    rsx! {
        div { class: "flex flex-col gap-2",
            a { href: "{watch}", target: "_blank", rel: "noopener", class: "group relative block aspect-video overflow-hidden rounded-lg bg-muted",
                onclick: move |e| { e.prevent_default(); playing.set(true); },
                Picture {
                    media: video.thumb.clone(),
                    alt: format!("Video thumbnail: {}", video.title),
                    initials: "▶".to_string(),
                    sizes: "(min-width: 1024px) 33vw, 100vw",
                    class: "h-full w-full object-cover transition-opacity group-hover:opacity-90",
                }
                span { class: "absolute inset-0 flex items-center justify-center",
                    span { class: "flex size-12 items-center justify-center rounded-full bg-black/70 text-white",
                        crate::components::icons::PlayIcon { class: "size-5" }
                    }
                }
            }
            div {
                p { class: "line-clamp-2 text-sm font-medium", "{video.title}" }
                if !channel.is_empty() {
                    p { class: "mt-0.5 text-xs text-muted-foreground", "{channel}" }
                }
            }
            if can_hide {
                div { class: "flex flex-wrap items-center gap-2",
                    button {
                        r#type: "button",
                        class: "{BUTTON} {BUTTON_DESTRUCTIVE} {BUTTON_XS}",
                        onclick: move |_| {
                            spawn(async move {
                                match api::hide_video(video_id).await {
                                    Ok(()) => gone.set(true),
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
