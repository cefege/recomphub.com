use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use recomphub_shared::{AiEvidenceDto, PortCard, TargetPlatform, VideoDto};

use crate::api;
use crate::components::icons::{AndroidIcon, AppleIcon, GlobeIcon, LinuxIcon, WindowsIcon};
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

/// Label and color classes for an AI usage value.
fn ai_style(usage: &str) -> (&'static str, &'static str) {
    match usage {
        "none" => ("Human-made", "border-success/50 bg-success/15 text-success"),
        "assisted" => (
            "AI-assisted",
            "border-warning/50 bg-warning/15 text-warning",
        ),
        "primarily_ai" => ("Primarily AI", "border-error/50 bg-error/15 text-error"),
        _ => ("AI use unknown", "border-border bg-surface-2 text-muted"),
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

#[component]
pub fn Chip(#[props(default)] class: String, children: Element) -> Element {
    rsx! {
        span { class: "inline-flex items-center gap-1 rounded-full border border-border bg-surface-2 px-2.5 py-0.5 text-xs font-semibold {class}",
            {children}
        }
    }
}

/// A titled section. Renders `section > h2`.
#[component]
pub fn Section(heading: String, #[props(default)] class: String, children: Element) -> Element {
    rsx! {
        section { class: "{class}",
            h2 { class: "mb-4 text-xl font-extrabold tracking-tight md:text-2xl", "{heading}" }
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
        span { class: "inline-flex items-center gap-1 rounded-md bg-surface-2 px-1.5 py-1 text-xs text-muted", title: "{label}",
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
        div { class: "flex items-center gap-2 text-xs text-muted", role: "meter", "aria-label": "Playability: {label}", "aria-valuemin": "0", "aria-valuemax": "4", "aria-valuenow": "{filled}",
            div { class: "flex gap-1",
                for on in segments {
                    span { class: if on { "h-1.5 w-5 rounded-full bg-success" } else { "h-1.5 w-5 rounded-full bg-surface-2" } }
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
            span { class: "inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold {cls}", "{text}" }
            if !evidence.is_empty() {
                span { class: "pointer-events-none invisible absolute bottom-full left-0 z-20 mb-2 w-64 rounded-xl border border-border bg-surface-2 p-3 text-xs text-foreground opacity-0 shadow-xl transition group-hover:visible group-hover:opacity-100 group-focus:visible group-focus:opacity-100",
                    span { class: "mb-1 block font-bold", "Evidence" }
                    for ev in evidence {
                        span { class: "block text-muted", "{ev.signal}: {ev.detail}" }
                    }
                }
            }
        }
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
                "v{v} · "
                RelativeTime { at: at }
            }
        },
        (Some(v), None) => rsx! { Chip { "v{v}" } },
        _ => rsx! { Chip { class: "text-muted", "{state_label}" } },
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
        article { class: "card flex flex-col gap-4 rounded-2xl border border-border bg-surface p-5 transition hover:-translate-y-1 hover:shadow-xl hover:shadow-accent-soft",
            header { class: "flex items-start gap-4",
                div { class: "cover-tilt relative h-20 w-16 shrink-0 overflow-hidden rounded-xl",
                    Picture {
                        media: port.cover.clone(),
                        alt: format!("{} cover art", port.game_name),
                        initials: initials,
                        sizes: "64px",
                        class: "h-full w-full object-cover",
                    }
                }
                div { class: "min-w-0 flex-1",
                    h3 { class: "text-lg font-extrabold leading-tight tracking-tight",
                        a { href: "{href}", class: "hover:text-accent-text", "{port.name}" }
                    }
                    if show_game {
                        p { class: "text-sm text-muted",
                            a { href: "{game_href}", class: "hover:underline", "{port.game_name}" }
                        }
                    }
                    div { class: "mt-2 flex flex-wrap items-center gap-2",
                        Chip { "{port.kind_label}" }
                        ReleaseChip {
                            version: port.latest_version.clone(),
                            at: port.latest_release_at,
                            state_label: port.release_state_label.clone(),
                        }
                    }
                }
            }
            div { class: "flex flex-wrap items-center gap-2",
                for t in port.targets.clone() {
                    TargetIcon { target: t }
                }
            }
            div { class: "flex flex-wrap items-center gap-3",
                AiBadge {
                    usage: port.ai_usage.clone(),
                    tools: port.ai_tools.clone(),
                    evidence: port.ai_evidence.clone(),
                }
                if !tools_text.is_empty() && port.ai_usage == "none" {
                    span { class: "sr-only", "No AI tools" }
                }
            }
            ul { class: "flex flex-wrap gap-3",
                for a in port.authors.clone() {
                    li { class: "flex items-center gap-2 text-sm",
                        match a.github_login.clone() {
                            Some(login) => rsx! {
                                a { href: "/authors/{login}", class: "flex items-center gap-2 hover:text-accent-text",
                                    Picture {
                                        media: a.avatar.clone(),
                                        alt: format!("{} avatar", a.name),
                                        initials: initials_of(&a.name),
                                        sizes: "32px",
                                        class: "h-7 w-7 rounded-full object-cover",
                                    }
                                    span { "{a.name}" }
                                }
                            },
                            None => rsx! { span { class: "text-muted", "{a.name}" } },
                        }
                    }
                }
            }
            PlayabilityMeter { playability: port.playability.clone() }
            footer { class: "mt-auto flex flex-wrap items-center gap-2",
                StarButton { port_id: port.id, count: port.star_count as i64 }
                span { class: "text-xs text-muted tabular", "★ {port.github_stars} on GitHub" }
                div { class: "ml-auto flex gap-2",
                    if let Some(url) = port.latest_release_url.clone() {
                        a { href: "{url}", target: "_blank", rel: "noopener", class: "rounded-full accent-bg px-3 py-1.5 text-xs font-bold text-white", "Download" }
                    }
                    a { href: "{port.repo_url}", target: "_blank", rel: "noopener", class: "rounded-full border border-border px-3 py-1.5 text-xs font-bold hover:bg-surface-2", "Repository" }
                }
            }
        }
    }
}

/// Trend chip from the current and previous rank: NEW, ▲n, ▼n, or a dash.
#[component]
pub fn TrendChip(rank: i64, prev_rank: Option<i64>) -> Element {
    match prev_rank {
        None => rsx! { Chip { class: "text-accent-text", "NEW" } },
        Some(p) if p > rank => rsx! { Chip { class: "text-success", "▲{p - rank}" } },
        Some(p) if p < rank => rsx! { Chip { class: "text-error", "▼{rank - p}" } },
        Some(_) => rsx! { Chip { class: "text-muted", "–" } },
    }
}

/// Grid of cards. Children are `li` elements.
#[component]
pub fn CardGrid(children: Element) -> Element {
    rsx! {
        ul { class: "grid grid-cols-2 gap-4 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-6",
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
        nav { class: "mt-8 flex items-center justify-center gap-4 text-sm", "aria-label": "Pages",
            match prev {
                Some(href) => rsx! { a { href: "{href}", rel: "prev", class: "rounded-full border border-border px-4 py-2 hover:bg-surface-2", "Previous" } },
                None => rsx! { span { class: "px-4 py-2 text-muted", "Previous" } },
            }
            span { class: "tabular text-muted", "Page {page} of {pages}" }
            match next {
                Some(href) => rsx! { a { href: "{href}", rel: "next", class: "rounded-full border border-border px-4 py-2 hover:bg-surface-2", "Next" } },
                None => rsx! { span { class: "px-4 py-2 text-muted", "Next" } },
            }
        }
    }
}

/// Cards / List toggle. `base` is the current path with its filters and no `view` parameter.
#[component]
pub fn ViewToggle(base: String, list: bool) -> Element {
    let list_href = with_query(&base, "view=list");
    let cards_cls = if list {
        "text-muted hover:text-foreground"
    } else {
        "bg-surface-2 text-foreground"
    };
    let list_cls = if list {
        "bg-surface-2 text-foreground"
    } else {
        "text-muted hover:text-foreground"
    };
    rsx! {
        nav { class: "inline-flex rounded-full border border-border p-1 text-xs font-bold", "aria-label": "View",
            a { href: "{base}", class: "rounded-full px-3 py-1.5 {cards_cls}", "Cards" }
            a { href: "{list_href}", class: "rounded-full px-3 py-1.5 {list_cls}", "List" }
        }
    }
}

/// Breadcrumb trail. Each item is `(label, href)`. The last item is the current page.
#[component]
pub fn Breadcrumb(items: Vec<(String, String)>) -> Element {
    let last = items.len().saturating_sub(1);
    rsx! {
        nav { class: "text-sm text-muted", "aria-label": "Breadcrumb",
            ol { class: "flex flex-wrap items-center gap-2",
                for (i, (label, href)) in items.into_iter().enumerate() {
                    li { class: "flex items-center gap-2",
                        if i == last {
                            span { class: "text-foreground", "aria-current": "page", "{label}" }
                        } else {
                            a { href: "{href}", class: "hover:text-foreground", "{label}" }
                            span { "›" }
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
        p { class: "rounded-2xl border border-dashed border-border p-10 text-center text-muted", "{text}" }
    }
}

/// Sign-in prompt for pages that need an account. Redirects back to the current page.
#[component]
pub fn LoginPrompt(text: String) -> Element {
    rsx! {
        div { class: "rounded-2xl border border-border bg-surface p-8 text-center",
            p { class: "mb-4 text-muted", "{text}" }
            button { r#type: "button", class: "rounded-full accent-bg px-5 py-2.5 text-sm font-bold text-white", onclick: move |_| login_redirect(),
                "Sign in with GitHub"
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
            div { role: "tablist", class: "flex flex-wrap gap-2",
                button { r#type: "button", role: "tab", "aria-selected": if active() == "all" { "true" } else { "false" },
                    class: "rounded-full border border-border px-3 py-1.5 text-xs font-bold",
                    onclick: move |_| active.set("all".to_string()),
                    "All"
                }
                for t in targets.clone() {
                    button { r#type: "button", role: "tab",
                        "aria-selected": if active() == t { "true" } else { "false" },
                        class: "rounded-full border border-border px-3 py-1.5 text-xs font-bold",
                        onclick: {
                            let t = t.clone();
                            move |_| active.set(t.clone())
                        },
                        "{target_label(&t)}"
                    }
                }
            }
            ul { class: "grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3",
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
            div { class: "overflow-hidden rounded-2xl border border-border bg-surface",
                div { class: "aspect-video w-full",
                    iframe { class: "h-full w-full", src: "{embed}", title: "{video.title}", allow: "autoplay; encrypted-media; picture-in-picture", allowfullscreen: true }
                }
                p { class: "p-3 text-sm font-semibold", "{video.title}" }
            }
        };
    }
    rsx! {
        div { class: "overflow-hidden rounded-2xl border border-border bg-surface",
            a { href: "{watch}", target: "_blank", rel: "noopener", class: "group relative block aspect-video overflow-hidden",
                onclick: move |e| { e.prevent_default(); playing.set(true); },
                Picture {
                    media: video.thumb.clone(),
                    alt: format!("Video thumbnail: {}", video.title),
                    initials: "▶".to_string(),
                    sizes: "(min-width: 1024px) 33vw, 100vw",
                    class: "h-full w-full object-cover transition group-hover:scale-105",
                }
                span { class: "absolute inset-0 flex items-center justify-center bg-black/20 text-white transition group-hover:bg-black/40",
                    span { class: "flex h-14 w-14 items-center justify-center rounded-full bg-black/60",
                        crate::components::icons::PlayIcon { class: "h-6 w-6" }
                    }
                }
            }
            div { class: "p-3",
                p { class: "line-clamp-2 text-sm font-semibold", "{video.title}" }
                if !channel.is_empty() {
                    p { class: "mt-1 text-xs text-muted", "{channel}" }
                }
            }
            if can_hide {
                div { class: "flex flex-wrap items-center gap-2 px-3 pb-3",
                    button {
                        r#type: "button",
                        class: "rounded-xl border border-error px-3 py-1 text-xs font-semibold text-error hover:bg-surface-2",
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
                        span { class: "text-xs text-error", "{hide_message}" }
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
