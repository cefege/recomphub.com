use dioxus::prelude::*;
use recomphub_shared::dto::RatingSummary;
use recomphub_shared::seo::{
    game_with_ports_description, game_with_ports_title, game_without_port_description,
    game_without_port_title, meta_description,
};
use recomphub_shared::{GameCard, GameDetail, Playability, PortCard, PortDetail, TargetPlatform};

use super::jsonld;
use crate::api;
use crate::api::games::GamesQuery;
use crate::components::cards::GameCardView;
use crate::components::comments::PortComments;
use crate::components::listing::{GameListTable, PRIMARY_TARGETS, parse_page, query_string};
use crate::components::live::{LiveCount, LiveStore, use_live_watch};
use crate::components::picture::Picture;
use crate::components::report::ReportDialog;
use crate::components::seo::{Seo, SiteUrl};
use crate::components::styles::{
    BADGE, BADGE_SECONDARY, BUTTON, BUTTON_MD, BUTTON_OUTLINE, CARD_SURFACE, H1, H2, LEAD, TAB,
    TAB_ACTIVE, TABS_LIST,
};
use crate::components::ui::{
    Breadcrumb, CardGrid, Chip, EmptyState, Pager, PortCardView, Rail, Section, TargetIcon,
    VideoPlayer, ViewToggle, initials_of, keyed, target_label,
};
use crate::components::viewer::{WantButton, use_marks};
use crate::pages::PortMainContent;
use crate::pages::faq::game_faq;

/// The `/games` filter state, parsed from the query string.
#[derive(Clone, PartialEq)]
pub(crate) struct GamesParams {
    platform: String,
    runs_on: String,
    status: String,
    sort: String,
    page: i64,
    list: bool,
}

impl GamesParams {
    fn parse(
        platform: &str,
        runs_on: &str,
        status: &str,
        sort: &str,
        page: &str,
        view: &str,
    ) -> Self {
        Self {
            platform: platform.trim().to_string(),
            runs_on: runs_on.trim().to_string(),
            status: status.trim().to_string(),
            sort: sort.trim().to_string(),
            page: parse_page(page),
            list: view == "list",
        }
    }

    fn filtered(&self) -> bool {
        !(self.platform.is_empty()
            && self.runs_on.is_empty()
            && self.status.is_empty()
            && self.sort.is_empty())
    }

    fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.platform, self.runs_on, self.status, self.sort, self.page, self.list
        )
    }

    /// `/games` with every filter kept. `page` 1 and the default view are left out of the URL.
    fn href(&self, page: i64, list: bool) -> String {
        let view = if list {
            "list".to_string()
        } else {
            String::new()
        };
        let page = if page > 1 {
            page.to_string()
        } else {
            String::new()
        };
        format!(
            "/games{}",
            query_string(&[
                ("platform", self.platform.clone()),
                ("runs_on", self.runs_on.clone()),
                ("status", self.status.clone()),
                ("sort", self.sort.clone()),
                ("view", view),
                ("page", page),
            ])
        )
    }
}

fn option_of(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

/// `1 game` or `3 games`.
fn counted(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Classes for one link in a `TABS_LIST`: `TAB`, plus `TAB_ACTIVE` when selected.
fn tab_class(on: bool) -> String {
    if on {
        format!("{TAB} {TAB_ACTIVE}")
    } else {
        TAB.to_string()
    }
}

/// `/games` with filters, sorting, paging and the Cards / List view.
#[component]
pub fn GamesIndex(
    platform: String,
    runs_on: String,
    status: String,
    sort: String,
    page: String,
    view: String,
) -> Element {
    let params = GamesParams::parse(&platform, &runs_on, &status, &sort, &page, &view);
    let key = params.key();
    keyed(rsx! { GamesIndexInner { key: "{key}", params } })
}

#[component]
fn GamesIndexInner(params: GamesParams) -> Element {
    let query = GamesQuery {
        platform: option_of(&params.platform),
        runs_on: option_of(&params.runs_on),
        status: option_of(&params.status),
        sort: option_of(&params.sort),
        page: params.page,
    };
    let data = use_loader(move || api::games_index(query.clone()))?;
    let result = data();

    let filtered = params.filtered();
    let noindex = filtered || params.list;
    let canonical = if filtered {
        "/games".to_string()
    } else {
        params.href(params.page, false)
    };
    let title = "Games: Native Ports & Recompilations | RecompHub".to_string();
    let description = meta_description(
        "Every game with a native port or recompilation, and the games still waiting for one. Filter by original platform or by where they run.",
    );

    let sort_links: Vec<(String, &str, bool)> = [("", "Most popular"), ("name", "A to Z")]
        .iter()
        .map(|(value, label)| {
            let link = GamesParams {
                sort: value.to_string(),
                page: 1,
                ..params.clone()
            };
            let on = if value.is_empty() {
                params.sort.is_empty()
            } else {
                params.sort == *value
            };
            (link.href(1, params.list), *label, on)
        })
        .collect();
    let status_links: Vec<(String, &str, bool)> =
        [("", "All"), ("ported", "With a port"), ("wanted", "Wanted")]
            .iter()
            .map(|(value, label)| {
                let link = GamesParams {
                    status: value.to_string(),
                    page: 1,
                    ..params.clone()
                };
                let on = if value.is_empty() {
                    params.status.is_empty()
                } else {
                    params.status == *value
                };
                (link.href(1, params.list), *label, on)
            })
            .collect();
    let toggle_base = params.href(1, false);
    let pager_base = params.href(1, params.list);
    let list = params.list;
    let games_text = counted(result.total, "game", "games");

    rsx! {
        Seo { title, description, path: canonical, noindex }
        section {
            div { class: "mb-8",
                h1 { class: H1, "Games" }
                if result.total > 0 {
                    p { class: "mt-2 text-sm tabular text-muted-foreground", "{games_text}" }
                }
            }
            div { class: "mb-6 flex flex-wrap items-center gap-3",
                nav { class: TABS_LIST, "aria-label": "Status",
                    for (href, label, on) in status_links {
                        a { href: "{href}", class: tab_class(on), "{label}" }
                    }
                }
                nav { class: TABS_LIST, "aria-label": "Sort",
                    for (href, label, on) in sort_links {
                        a { href: "{href}", class: tab_class(on), "{label}" }
                    }
                }
                ViewToggle { base: toggle_base, list }
            }
            if result.items.is_empty() {
                EmptyState { text: "No games match these filters.".to_string() }
            } else if list {
                GameListTable { games: result.items.clone() }
            } else {
                CardGrid {
                    for game in result.items.clone() {
                        {
                            let key = game.id.to_string();
                            rsx! { li { key: "{key}", GameCardView { game } } }
                        }
                    }
                }
            }
            Pager { base: pager_base, page: result.page, pages: result.pages }
        }
    }
}

/// `/games/:slug`. Remounts when the slug changes so its loader runs again.
#[component]
pub fn GamePage(slug: String) -> Element {
    keyed(rsx! { GamePageInner { key: "{slug}", slug } })
}

#[component]
fn GamePageInner(slug: String) -> Element {
    let lookup = slug.clone();
    let data = use_loader(move || api::game_detail(lookup.clone()))?;
    let detail = data();
    rsx! { GameBody { detail } }
}

/// Breadcrumb trail for a game: Home, its first platform, then the game.
pub(crate) fn game_crumbs(detail: &GameDetail) -> Vec<(String, String)> {
    let mut crumbs = vec![("Home".to_string(), "/".to_string())];
    if let Some(platform) = detail.platforms.first()
        && let Some(slug) = platform.slug.as_ref()
    {
        crumbs.push(("Platforms".to_string(), "/platforms".to_string()));
        crumbs.push((platform.name.clone(), format!("/platforms/{slug}")));
    }
    crumbs.push((
        detail.game.name.clone(),
        format!("/games/{}", detail.game.slug),
    ));
    crumbs
}

fn is_primary(target: &str) -> bool {
    target
        .parse::<TargetPlatform>()
        .map(|t| t.is_primary())
        .unwrap_or(false)
}

fn playability_rank(value: &str) -> u8 {
    value.parse::<Playability>().map(|p| p.rank()).unwrap_or(3)
}

fn sorted_ports(list: &[PortCard]) -> Vec<PortCard> {
    let mut out = list.to_vec();
    out.sort_by(|a, b| {
        playability_rank(&a.playability)
            .cmp(&playability_rank(&b.playability))
            .then(b.github_stars.cmp(&a.github_stars))
    });
    out
}

fn tint_class(target: &str, on: bool) -> String {
    let class = match (target, on) {
        ("windows", true) => "border-windows/60 bg-windows/15 text-foreground",
        ("linux", true) => "border-linux/60 bg-linux/15 text-foreground",
        ("web", true) => "border-web/60 bg-web/15 text-foreground",
        ("macos", true) => "border-foreground/40 bg-foreground/10 text-foreground",
        _ => "border-border text-muted-foreground line-through opacity-60",
    };
    class.to_string()
}

fn blurb(game: &GameCard, platform: &str, ports: &[PortCard], primary: &[String]) -> String {
    let year = game
        .release_year
        .map(|y| format!(", {y}"))
        .unwrap_or_default();
    let mut text = format!(
        "{} ({platform}{year}) has {}.",
        game.name,
        counted(ports.len() as i64, "native port", "native ports")
    );
    if let Some(top) = ports.iter().max_by_key(|p| p.github_stars) {
        text.push_str(&format!(
            " The most popular is {} with {}.",
            top.name,
            counted(i64::from(top.github_stars), "GitHub star", "GitHub stars")
        ));
    }
    if !primary.is_empty() {
        text.push_str(&format!(
            " You can play it natively on {}.",
            primary.join(", ")
        ));
    }
    text
}

#[component]
fn GameBody(detail: GameDetail) -> Element {
    let site = use_context::<SiteUrl>().0;
    let live = use_context::<LiveStore>();
    let game = detail.game.clone();
    let game_id = game.id;
    let mut ports = use_signal(|| detail.ports.clone());
    let mut want_total = use_signal(|| i64::from(game.want_count));
    let mut lightbox = use_signal(|| None::<usize>);
    let mut last_seen = use_signal(|| {
        live.activity
            .peek()
            .front()
            .map(|item| item.id)
            .unwrap_or(0)
    });

    let port_ids: Vec<i64> = detail
        .ports
        .iter()
        .chain(detail.decompilations.iter())
        .map(|p| p.id)
        .collect();
    use_marks(port_ids, vec![game_id]);
    let mut visible: Vec<i64> = detail.related.iter().map(|g| g.id).collect();
    visible.push(game_id);
    use_live_watch(Some(game_id), visible);

    let refresh_ports = move || {
        spawn(async move {
            if let Ok(list) = api::game_ports(game_id).await {
                ports.set(list);
            }
        });
    };

    use_effect(move || {
        spawn(async move {
            let _ = api::record_hit(game_id).await;
        });
    });

    use_effect(move || {
        let front = live.activity.read().front().cloned();
        let Some(item) = front else {
            return;
        };
        if item.id <= *last_seen.peek() {
            return;
        }
        last_seen.set(item.id);
        if item.game_id != game_id {
            return;
        }
        match item.kind.as_str() {
            "starred" | "port_added" | "release" => refresh_ports(),
            "wanted" => {
                let next = *want_total.peek() + 1;
                want_total.set(next);
            }
            _ => {}
        }
    });

    let list = ports();
    let has_ports = !list.is_empty();
    // Display-only: not stored anywhere, re-decided from the live port count on every load, so
    // the game page automatically goes back to a card list the moment a second port is added.
    let is_solo_port = list.len() == 1;
    let want = want_total();
    let platform_name = detail
        .platforms
        .first()
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let labels_all: Vec<String> = game.targets.iter().map(|t| target_label(t)).collect();
    let primary: Vec<String> = game
        .targets
        .iter()
        .filter(|t| is_primary(t))
        .map(|t| target_label(t))
        .take(3)
        .collect();
    let primary_refs: Vec<&str> = primary.iter().map(String::as_str).collect();
    let all_refs: Vec<&str> = labels_all.iter().map(String::as_str).collect();

    let (title, description) = if has_ports {
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        (
            game_with_ports_title(&game.name, &primary_refs),
            meta_description(&game_with_ports_description(
                &game.name,
                &platform_name,
                game.release_year,
                &names,
                &all_refs,
            )),
        )
    } else {
        (
            game_without_port_title(&game.name),
            meta_description(&game_without_port_description(
                &game.name,
                &platform_name,
                game.release_year,
                want,
            )),
        )
    };
    let og_image = detail
        .og
        .as_ref()
        .filter(|m| m.ready && m.widths.contains(&1200))
        .map(|m| format!("{site}/media/og/{}-1200.jpg", m.hash));
    let json_ld = jsonld::game_json_ld(&site, &detail);
    let path = format!("/games/{}", game.slug);
    let crumbs = game_crumbs(&detail);
    let initials = initials_of(&game.name);
    let shots = detail.screenshots.clone();
    let year_text = game.release_year.map(|y| y.to_string()).unwrap_or_default();
    let developer = detail.developer.clone().unwrap_or_default();
    let summary = detail.summary.clone();
    let stars_total = detail.total_github_stars;
    let stars_text = counted(
        stars_total,
        "GitHub star across ports",
        "GitHub stars across ports",
    );
    let blurb_text = if has_ports {
        blurb(&game, &platform_name, &list, &primary)
    } else {
        String::new()
    };

    let want_text = if want == 1 {
        "1 person wants a port".to_string()
    } else {
        format!("{want} people want a port")
    };
    let target_chips: Vec<(&str, String, bool)> = PRIMARY_TARGETS
        .iter()
        .map(|t| {
            (
                *t,
                target_label(t),
                game.targets.iter().any(|x| x.as_str() == *t),
            )
        })
        .collect();
    let faqs = game_faq(&detail);
    let decomps: Vec<(String, String, String)> = detail
        .decompilations
        .iter()
        .map(|p| {
            (
                format!("/games/{}/{}", game.slug, p.slug),
                p.name.clone(),
                p.kind_label.clone(),
            )
        })
        .collect();
    let submit_href = format!("/submit?game={}", game.slug);
    let related = detail.related.clone();
    let videos = detail.videos.clone();
    let sorted = sorted_ports(&list);
    let shot_alt = game.name.clone();
    let lightbox_alt = game.name.clone();
    let lightbox_shot = lightbox().and_then(|i| shots.get(i).cloned());

    rsx! {
        Seo { title, description, path, og_image, json_ld }
        section { class: "grid gap-8 md:grid-cols-[260px_1fr]",
            div { class: "mx-auto w-48 md:w-full",
                div { class: "aspect-[3/4] overflow-hidden rounded-lg border",
                    Picture {
                        media: game.cover.clone(),
                        alt: format!("Cover art for {}", game.name),
                        initials: initials.clone(),
                        sizes: "(min-width: 768px) 260px, 60vw".to_string(),
                        eager: true,
                        class: "h-full w-full object-cover".to_string(),
                    }
                }
            }
            div { class: "min-w-0",
                Breadcrumb { items: crumbs }
                h1 { class: "mt-4 {H1}", "{game.name}: native ports & recompilations" }
                div { class: "mt-4 flex flex-wrap items-center gap-2",
                    for platform in detail.platforms.iter() {
                        if let Some(slug) = platform.slug.as_ref() {
                            Chip { class: "hover:bg-accent".to_string(),
                                a { href: "/platforms/{slug}", "{platform.name}" }
                            }
                        }
                    }
                    if !year_text.is_empty() {
                        span { class: "text-sm tabular text-muted-foreground", "{year_text}" }
                    }
                    if !developer.is_empty() {
                        span { class: "text-sm text-muted-foreground", "by {developer}" }
                    }
                }
                div { class: "mt-4 flex flex-wrap gap-2",
                    for (target, label, on) in target_chips.clone() {
                        Chip { class: tint_class(target, on),
                            TargetIcon { target: target.to_string() }
                            span { class: "font-semibold", "{label}" }
                            if on {
                                span { class: "text-success", "✓" }
                            }
                        }
                    }
                }
                div { class: "mt-4 flex flex-wrap items-center gap-4",
                    LiveCount { game_id, portless: !has_ports }
                    if stars_total > 0 {
                        span { class: "text-sm tabular text-muted-foreground", "{stars_text}" }
                    }
                }
                if has_ports {
                    p { class: "mt-4 max-w-2xl {LEAD}", "{blurb_text}" }
                }
            }
        }

        if is_solo_port {
            div { class: "mt-12",
                GameSolePort { port: list[0].clone() }
            }
        } else if has_ports {
            Section { heading: "Native ports".to_string(), class: "mt-12".to_string(),
                ul { class: "grid gap-4 md:grid-cols-2",
                    for port in sorted {
                        {
                            let key = format!("{}-{}", port.id, port.star_count);
                            rsx! { li { key: "{key}", PortCardView { port } } }
                        }
                    }
                }
            }
        } else {
            div { class: "{CARD_SURFACE} mt-12 p-8 text-center",
                h2 { class: H2, "No native port yet" }
                p { class: "mt-2 text-sm text-muted-foreground", "Know of a recompilation or native port? Submit it and it joins the catalog." }
                div { class: "mt-6 flex flex-wrap items-center justify-center gap-3",
                    if want > 0 {
                        {keyed(rsx! { span { key: "{want}", class: "{BADGE} {BADGE_SECONDARY} tabular animate-bump", "{want_text}" } })}
                    }
                    {keyed(rsx! { WantButton { key: "{game_id}-{want}", game_id, count: want } })}
                }
                a { href: "{submit_href}", class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD} mt-4", "Know of one? Submit it" }
            }
        }

        if !decomps.is_empty() {
            Section { heading: "Decompilation projects".to_string(), class: "mt-12".to_string(),
                ul { class: "grid gap-4 md:grid-cols-2",
                    for (href, name, kind) in decomps {
                        li { key: "{href}",
                            a { href: "{href}", class: "flex items-center justify-between gap-3 {CARD_SURFACE} p-4 transition-colors hover:bg-accent",
                                span { class: "font-medium", "{name}" }
                                Chip { class: "text-xs".to_string(), "{kind}" }
                            }
                        }
                    }
                }
            }
        }

        if !videos.is_empty() && !is_solo_port {
            Section { heading: "Gameplay videos".to_string(), class: "mt-12".to_string(),
                VideoPlayer { videos }
            }
        }

        if !shots.is_empty() && !is_solo_port {
            Section { heading: "Screenshots".to_string(), class: "mt-12".to_string(),
                Rail {
                    for (index, shot) in shots.iter().cloned().enumerate() {
                        li { key: "{index}", class: "w-72 shrink-0 snap-start md:w-96",
                            button {
                                r#type: "button",
                                class: "block w-full overflow-hidden rounded-lg border transition-opacity hover:opacity-90",
                                onclick: move |_| lightbox.set(Some(index)),
                                Picture {
                                    media: Some(shot),
                                    alt: format!("{} screenshot {}", shot_alt, index + 1),
                                    initials: initials.clone(),
                                    sizes: "384px".to_string(),
                                    class: "aspect-video w-full object-cover".to_string(),
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(shot) = lightbox_shot {
            div {
                class: "fixed inset-0 z-50 flex cursor-zoom-out items-center justify-center bg-black/85 p-4",
                "role": "dialog",
                "aria-modal": "true",
                onclick: move |_| lightbox.set(None),
                Picture {
                    media: Some(shot),
                    alt: format!("{} screenshot", lightbox_alt),
                    initials: initials.clone(),
                    sizes: "100vw".to_string(),
                    eager: true,
                    class: "max-h-full max-w-6xl rounded-lg".to_string(),
                }
            }
        }

        if let Some(text) = summary {
            Section { heading: "About".to_string(), class: "mt-12".to_string(),
                p { class: "mt-2 max-w-3xl leading-relaxed", "{text}" }
            }
        }

        if !faqs.is_empty() {
            Section { heading: "Frequently asked questions".to_string(), class: "mt-12".to_string(),
                dl {
                    for (index, item) in faqs.iter().enumerate() {
                        div { key: "{index}", class: "border-b py-4",
                            dt { class: "text-sm font-medium", "{item.question}" }
                            dd { class: "mt-2 text-sm leading-relaxed text-muted-foreground", "{item.answer}" }
                        }
                    }
                }
            }
        }

        if !related.is_empty() {
            Section { heading: "Related games".to_string(), class: "mt-12".to_string(),
                Rail {
                    for game in related {
                        {
                            let key = game.id.to_string();
                            rsx! { li { key: "{key}", class: "w-40 shrink-0 snap-start sm:w-48", GameCardView { game } } }
                        }
                    }
                }
            }
        }

        if is_solo_port {
            GameSolePortComments { port_id: list[0].id, port_name: list[0].name.clone(), game_id }
        } else {
            div { class: "mt-12 flex justify-end",
                ReportDialog { game_id }
            }
        }
    }
}

/// Fetches and renders one port's full detail inline on its game page, used in place of the
/// "Native ports" list when a game has exactly one published native port. Nothing is stored for
/// this: `GameBody` decides fresh from the live port count every time the page loads, so it
/// reverts to the card list on its own the moment a second port is added. A failed fetch (a
/// race with the port being hidden or renamed, a timeout) falls back to the plain card instead
/// of failing the whole game page: before this merge, a solo-port game always rendered.
#[component]
fn GameSolePort(port: PortCard) -> Element {
    let (game_slug, port_slug) = (port.game_slug.clone(), port.slug.clone());
    let data = use_loader(move || {
        let game_slug = game_slug.clone();
        let port_slug = port_slug.clone();
        async move {
            Ok::<Option<PortDetail>, ServerFnError>(
                api::port_detail(game_slug, port_slug).await.ok(),
            )
        }
    })?;
    match data() {
        Some(detail) => {
            let port_id = detail.port.id;
            let ratings_data = use_loader(move || async move {
                Ok::<RatingSummary, ServerFnError>(
                    api::port_ratings(port_id).await.unwrap_or_default(),
                )
            })?;
            rsx! { PortMainContent { detail, ratings: ratings_data() } }
        }
        None => rsx! {
            ul { class: "grid gap-4 md:grid-cols-2",
                li { PortCardView { port } }
            }
        },
    }
}

/// The merged game page's comments and report dialog for its sole port, rendered once at the
/// foot of the page instead of inside `GameSolePort`, which only has the port's facts. Ratings
/// are non-essential: a failed fetch falls back to an empty summary instead of failing the page.
#[component]
fn GameSolePortComments(port_id: i64, port_name: String, game_id: i64) -> Element {
    let ratings_data = use_loader(move || async move {
        Ok::<RatingSummary, ServerFnError>(api::port_ratings(port_id).await.unwrap_or_default())
    })?;
    rsx! {
        div { class: "mt-12",
            PortComments { key: "{port_id}", port_id, port_name: port_name.clone(), ratings: ratings_data() }
        }
        div { class: "mt-12 flex justify-end",
            ReportDialog { game_id, port_id }
        }
    }
}
