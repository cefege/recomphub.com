//! For creators: a prioritised to-do list for recompilers, porters and video creators. The hub
//! previews the top of each list. Each list page is paginated and filterable.

use dioxus::prelude::*;
use recomphub_shared::dto::{PortGapItem, RecompileItem, SalesSignal, VideoGapItem};
use recomphub_shared::seo::meta_description;

use crate::api;
use crate::components::GroundRules;
use crate::components::listing::parse_page;
use crate::components::styles::{
    BUTTON, BUTTON_GHOST, BUTTON_OUTLINE, BUTTON_SM, H1, H2, LEAD, TAB, TAB_ACTIVE, TABLE,
    TABLE_HEAD_ROW, TABLE_ROW, TABLE_TD, TABLE_TH, TABLE_TH_NUM, TABLE_WRAP, TABS_LIST,
};
use crate::components::{
    Breadcrumb, EmptyState, ExternalLink, Pager, Seo, keyed, target_label, with_query,
};
use crate::routes::Route;

/// Video filters. Keep in step with `TargetPlatform::youtube_searchable`.
const VIDEO_FILTERS: [&str; 6] = ["windows", "macos", "linux", "web", "android", "ios"];

/// Count with its noun, pluralised: `1 game`, `3 games`.
fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Labels for target slugs, joined for display.
fn labels(slugs: &[String]) -> String {
    slugs
        .iter()
        .map(|s| target_label(s))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Count with thousands separators: `4,812`.
fn thousands(n: i64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Best-sale fact: `21.4M sold (best-selling PS2 games #3)`.
fn sales_fact(s: &SalesSignal) -> String {
    let units = format!("{:.1}M", s.units_sold_millions);
    let list = match (s.scope.as_str(), s.platform.as_deref(), s.sales_year) {
        ("platform", Some(platform), _) => format!("best-selling {platform} games"),
        ("year", _, Some(year)) => format!("best-selling games of {year}"),
        _ => "best-selling games overall".to_string(),
    };
    match s.rank {
        Some(rank) => format!("{units} sold ({list} #{rank})"),
        None => format!("{units} sold ({list})"),
    }
}

/// Rating fact for a game with no sales signal. Few ratings read as a count, not as high regard.
fn rating_fact(popularity: i32) -> Option<String> {
    match popularity {
        p if p <= 0 => None,
        p if p >= 100 => Some(format!(
            "Highly rated on IGDB ({} ratings)",
            thousands(i64::from(p))
        )),
        p => Some(format!(
            "{} on IGDB",
            plural(i64::from(p), "rating", "ratings")
        )),
    }
}

/// A game's strongest fact: its best sale, else its IGDB ratings.
fn importance_fact(sales: Option<&SalesSignal>, popularity: i32) -> Option<String> {
    match sales {
        Some(sale) => Some(sales_fact(sale)),
        None => rating_fact(popularity),
    }
}

/// Facts joined with a middle dot, in the order given. Empty when there are none.
fn join_facts(facts: &[Option<String>]) -> String {
    facts
        .iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Why a game ranks: its strongest fact, then wants, then an existing decompilation.
fn recompile_why(row: &RecompileItem) -> String {
    let wants = (row.want_count > 0).then(|| {
        plural(
            i64::from(row.want_count),
            "person wants it",
            "people want it",
        )
    });
    let decomp = row
        .has_decompilation
        .then(|| "decompilation exists".to_string());
    let why = join_facts(&[
        importance_fact(row.sales.as_ref(), row.popularity),
        wants,
        decomp,
    ]);
    if why.is_empty() {
        "No sales or rating data yet".to_string()
    } else {
        why
    }
}

/// Why a port ranks: the game's strongest fact, then what it runs on, then what it lacks.
fn gap_why(row: &PortGapItem) -> String {
    let runs = if row.targets.len() == 1 && row.targets[0] == "windows" {
        "Windows only".to_string()
    } else {
        format!("Runs on {}", labels(&row.targets))
    };
    join_facts(&[
        importance_fact(row.sales.as_ref(), row.popularity),
        Some(runs),
        Some(format!("missing {}", labels(&row.missing))),
    ])
}

/// Why a port needs videos: the game's strongest fact, then the video gap, optionally per target.
fn video_why(row: &VideoGapItem, target: Option<&str>) -> String {
    let gap = match target {
        None if row.video_count == 0 => "No gameplay videos yet".to_string(),
        None => format!(
            "Only {}",
            plural(row.video_count, "gameplay video", "gameplay videos")
        ),
        Some(t) if row.video_count == 0 => format!("No {} gameplay videos yet", target_label(t)),
        Some(t) => format!(
            "Only {} for {}",
            plural(row.video_count, "gameplay video", "gameplay videos"),
            target_label(t)
        ),
    };
    join_facts(&[
        importance_fact(row.sales.as_ref(), row.popularity),
        Some(gap),
    ])
}

/// Original platforms under a game: the first three, then how many are left.
fn platforms_line(platforms: &[String]) -> String {
    const SHOWN: usize = 3;
    if platforms.len() <= SHOWN {
        platforms.join(", ")
    } else {
        format!(
            "{} +{} more",
            platforms[..SHOWN].join(", "),
            platforms.len() - SHOWN
        )
    }
}

/// The hub: top rows of all three lists, each with a link to its full list.
#[component]
pub fn CreatorsHub() -> Element {
    let loaded = use_loader(api::creators_hub)?;
    let data = loaded();
    let description = meta_description(
        "Games to recompile, ports missing major platforms, and ports that need gameplay videos. A prioritised to-do list for recompilers, porters and video creators.",
    );

    rsx! {
        Seo { title: "For creators: what to work on next".to_string(), description, path: "/creators".to_string() }
        div { class: "space-y-12",
            header { class: "space-y-4",
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    ("For creators".to_string(), "/creators".to_string()),
                ] }
                h1 { class: "mt-4 {H1}", "What to work on next" }
                p { class: "mt-2 max-w-2xl {LEAD}",
                    "For recompilers, porters and video creators. Each list is ordered by what is most likely to pay off next. Start with the top rows, then open the full list."
                }
            }

            section { "aria-labelledby": "recompile",
                div { class: "mb-4 flex items-end justify-between gap-4",
                    div { class: "space-y-1",
                        h2 { id: "recompile", class: H2, "Recompile next" }
                        p { class: "text-sm text-muted-foreground", "Games people want that have no port yet." }
                    }
                    if data.recompile_total > data.recompile.len() as i64 {
                        Link {
                            to: Route::CreatorsRecompile { page: String::new() },
                            class: "{BUTTON} {BUTTON_GHOST} {BUTTON_SM}",
                            "View all {plural(data.recompile_total, \"game\", \"games\")}"
                        }
                    }
                }
                if data.recompile.is_empty() {
                    EmptyState { text: "Every listed game has a port.".to_string() }
                } else {
                    RecompileTable { rows: data.recompile.clone(), start: 0 }
                }
            }

            section { "aria-labelledby": "missing-platforms",
                div { class: "mb-4 flex items-end justify-between gap-4",
                    div { class: "space-y-1",
                        h2 { id: "missing-platforms", class: H2, "Ports missing platforms" }
                        p { class: "text-sm text-muted-foreground", "Ports that do not yet run on a major platform." }
                    }
                    if data.ports_total > data.ports.len() as i64 {
                        Link {
                            to: Route::CreatorsPlatforms { missing: String::new(), page: String::new() },
                            class: "{BUTTON} {BUTTON_GHOST} {BUTTON_SM}",
                            "View all {plural(data.ports_total, \"port\", \"ports\")}"
                        }
                    }
                }
                if data.ports.is_empty() {
                    EmptyState { text: "No ports are missing a major platform.".to_string() }
                } else {
                    PortGapTable { rows: data.ports.clone(), start: 0 }
                }
            }

            section { "aria-labelledby": "videos",
                div { class: "mb-4 flex items-end justify-between gap-4",
                    div { class: "space-y-1",
                        h2 { id: "videos", class: H2, "Videos to make" }
                        p { class: "text-sm text-muted-foreground", "Ports with few or no gameplay videos." }
                    }
                    if data.videos_total > data.videos.len() as i64 {
                        Link {
                            to: Route::CreatorsVideos { target: String::new(), page: String::new() },
                            class: "{BUTTON} {BUTTON_GHOST} {BUTTON_SM}",
                            "View all {plural(data.videos_total, \"port\", \"ports\")}"
                        }
                    }
                }
                if data.videos.is_empty() {
                    EmptyState { text: "Every listed port has gameplay videos.".to_string() }
                } else {
                    VideoGapTable { rows: data.videos.clone(), start: 0, target: None }
                }
            }

            GroundRules { class: "mt-12" }
        }
    }
}

/// `/creators/recompile`: every game with no port, most wanted first.
#[component]
pub fn CreatorsRecompile(page: String) -> Element {
    let page = parse_page(&page);
    keyed(rsx! { RecompileInner { key: "{page}", page } })
}

#[component]
fn RecompileInner(page: i64) -> Element {
    let loaded = use_loader(move || api::creators_recompile(page))?;
    let data = loaded();
    let path = "/creators/recompile".to_string();
    let start = (data.page - 1) * 50;

    rsx! {
        Seo {
            title: "Games to recompile next".to_string(),
            description: meta_description("Games people want that have no native port or recompilation yet, most wanted first."),
            path: path.clone(),
        }
        div { class: "space-y-8",
            header { class: "space-y-4",
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    ("For creators".to_string(), "/creators".to_string()),
                    ("Recompile next".to_string(), path.clone()),
                ] }
                h1 { class: "mt-4 {H1}", "Games to recompile next" }
                p { class: "mt-2 max-w-2xl {LEAD}",
                    "Games people want that have no port yet, most wanted first. Game importance breaks ties."
                }
            }
            if data.items.is_empty() {
                EmptyState { text: "Every listed game has a port.".to_string() }
            } else {
                RecompileTable { rows: data.items.clone(), start }
            }
            Pager { base: path, page: data.page, pages: data.pages }
        }
    }
}

/// `/creators/platforms?missing=`: ports missing a major platform, optionally for one target.
#[component]
pub fn CreatorsPlatforms(missing: String, page: String) -> Element {
    let page = parse_page(&page);
    keyed(rsx! { PlatformsInner { key: "{missing}-{page}", missing, page } })
}

#[component]
fn PlatformsInner(missing: String, page: i64) -> Element {
    let loaded = use_loader({
        let missing = missing.clone();
        move || api::creators_port_gaps(missing.clone(), page)
    })?;
    let data = loaded();
    let path = "/creators/platforms".to_string();
    let base = if missing.is_empty() {
        path.clone()
    } else {
        with_query(&path, &format!("missing={missing}"))
    };
    let start = (data.page - 1) * 50;
    let heading = if missing.is_empty() {
        "Ports missing platforms".to_string()
    } else {
        format!("Ports missing {}", target_label(&missing))
    };

    rsx! {
        Seo {
            title: heading.clone(),
            description: meta_description("Native ports and recompilations that do not yet run on Windows, macOS, Linux, the web, Android or iOS."),
            path: path.clone(),
            noindex: !missing.is_empty(),
        }
        div { class: "space-y-8",
            header { class: "space-y-4",
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    ("For creators".to_string(), "/creators".to_string()),
                    ("Ports missing platforms".to_string(), path.clone()),
                ] }
                h1 { class: "mt-4 {H1}", "{heading}" }
                p { class: "mt-2 max-w-2xl {LEAD}",
                    "Ports ranked by how many major platforms they lack, weighted by game importance. Filter by the platform you want to add."
                }
            }
            nav { class: TABS_LIST, "aria-label": "Filter by missing platform",
                Link {
                    to: Route::CreatorsPlatforms { missing: String::new(), page: String::new() },
                    class: if missing.is_empty() { "{TAB} {TAB_ACTIVE}" } else { "{TAB}" },
                    "All"
                }
                for slug in recomphub_shared::MAJOR_TARGETS {
                    Link {
                        key: "{slug}",
                        to: Route::CreatorsPlatforms { missing: slug.to_string(), page: String::new() },
                        class: if missing == slug { "{TAB} {TAB_ACTIVE}" } else { "{TAB}" },
                        "{target_label(slug)}"
                    }
                }
            }
            if data.items.is_empty() {
                EmptyState { text: "No ports match this filter.".to_string() }
            } else {
                PortGapTable { rows: data.items.clone(), start }
            }
            Pager { base, page: data.page, pages: data.pages }
        }
    }
}

/// `/creators/videos?target=`: ports with fewer than two gameplay videos, overall or per target.
#[component]
pub fn CreatorsVideos(target: String, page: String) -> Element {
    let page = parse_page(&page);
    keyed(rsx! { VideosInner { key: "{target}-{page}", target, page } })
}

#[component]
fn VideosInner(target: String, page: i64) -> Element {
    let loaded = use_loader({
        let target = target.clone();
        move || api::creators_video_gaps(target.clone(), page)
    })?;
    let data = loaded();
    let path = "/creators/videos".to_string();
    let base = if target.is_empty() {
        path.clone()
    } else {
        with_query(&path, &format!("target={target}"))
    };
    let start = (data.page - 1) * 50;
    let filter = (!target.is_empty()).then(|| target.clone());
    let heading = match &filter {
        Some(t) => format!("Ports that need {} videos", target_label(t)),
        None => "Ports that need videos".to_string(),
    };

    rsx! {
        Seo {
            title: heading.clone(),
            description: meta_description("Native ports and recompilations with few or no gameplay videos, overall or for one platform."),
            path: path.clone(),
            noindex: !target.is_empty(),
        }
        div { class: "space-y-8",
            header { class: "space-y-4",
                Breadcrumb { items: vec![
                    ("Home".to_string(), "/".to_string()),
                    ("For creators".to_string(), "/creators".to_string()),
                    ("Videos to make".to_string(), path.clone()),
                ] }
                h1 { class: "mt-4 {H1}", "{heading}" }
                p { class: "mt-2 max-w-2xl {LEAD}",
                    "Ports with fewer than two gameplay videos, ranked by game importance, stars and latest release. Filter by platform to find the gaps for one target."
                }
            }
            nav { class: TABS_LIST, "aria-label": "Filter by platform",
                Link {
                    to: Route::CreatorsVideos { target: String::new(), page: String::new() },
                    class: if target.is_empty() { "{TAB} {TAB_ACTIVE}" } else { "{TAB}" },
                    "All"
                }
                for slug in VIDEO_FILTERS {
                    Link {
                        key: "{slug}",
                        to: Route::CreatorsVideos { target: slug.to_string(), page: String::new() },
                        class: if target == slug { "{TAB} {TAB_ACTIVE}" } else { "{TAB}" },
                        "{target_label(slug)}"
                    }
                }
            }
            if data.items.is_empty() {
                EmptyState { text: "No ports match this filter.".to_string() }
            } else {
                VideoGapTable { rows: data.items.clone(), start, target: filter.clone() }
            }
            Pager { base, page: data.page, pages: data.pages }
        }
    }
}

/// Recompile to-do rows: rank, game, why it ranks, and the next step.
#[component]
fn RecompileTable(rows: Vec<RecompileItem>, start: i64) -> Element {
    rsx! {
        div { class: TABLE_WRAP,
            table { class: TABLE,
                thead { class: TABLE_HEAD_ROW,
                    tr {
                        th { class: "{TABLE_TH_NUM} w-12", scope: "col", "#" }
                        th { class: TABLE_TH, scope: "col", "Game" }
                        th { class: TABLE_TH, scope: "col", "Why it ranks high" }
                        th { class: TABLE_TH_NUM, scope: "col", "Next step" }
                    }
                }
                tbody {
                    for (i, row) in rows.iter().enumerate() {
                        {
                            let rank = start + i as i64 + 1;
                            let year = row.release_year.map(|y| y.to_string()).unwrap_or_default();
                            let platforms = platforms_line(&row.original_platforms);
                            let meta = [year, platforms]
                                .into_iter()
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>()
                                .join(" · ");
                            let why = recompile_why(row);
                            rsx! {
                                tr { key: "{row.game_id}", class: TABLE_ROW,
                                    td { class: "{TABLE_TD} w-12 text-right tabular text-muted-foreground", "{rank}" }
                                    td { class: TABLE_TD,
                                        Link {
                                            to: Route::GamePage { slug: row.game_slug.clone() },
                                            class: "font-medium hover:underline",
                                            "{row.game_name}"
                                        }
                                        if !meta.is_empty() {
                                            p { class: "text-xs text-muted-foreground", "{meta}" }
                                        }
                                    }
                                    td { class: "{TABLE_TD} text-sm", "{why}" }
                                    td { class: "{TABLE_TD} text-right",
                                        Link {
                                            to: Route::SubmitPage { game: row.game_slug.clone(), repo: String::new() },
                                            class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                                            "Submit a port"
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

/// Missing-platform to-do rows: rank, port, why it ranks, and the repo to add a build to.
#[component]
fn PortGapTable(rows: Vec<PortGapItem>, start: i64) -> Element {
    rsx! {
        div { class: TABLE_WRAP,
            table { class: TABLE,
                thead { class: TABLE_HEAD_ROW,
                    tr {
                        th { class: "{TABLE_TH_NUM} w-12", scope: "col", "#" }
                        th { class: TABLE_TH, scope: "col", "Port" }
                        th { class: TABLE_TH, scope: "col", "Why it ranks high" }
                        th { class: TABLE_TH_NUM, scope: "col", "Next step" }
                    }
                }
                tbody {
                    for (i, row) in rows.iter().enumerate() {
                        {
                            let rank = start + i as i64 + 1;
                            let why = gap_why(row);
                            rsx! {
                                tr { key: "{row.port_id}", class: TABLE_ROW,
                                    td { class: "{TABLE_TD} w-12 text-right tabular text-muted-foreground", "{rank}" }
                                    td { class: TABLE_TD,
                                        Link {
                                            to: Route::PortPage { game_slug: row.game_slug.clone(), port_slug: row.port_slug.clone() },
                                            class: "font-medium hover:underline",
                                            "{row.port_name}"
                                        }
                                        p { class: "text-xs text-muted-foreground", "{row.game_name}" }
                                    }
                                    td { class: "{TABLE_TD} text-sm", "{why}" }
                                    td { class: "{TABLE_TD} text-right",
                                        ExternalLink {
                                            url: row.repo_url.clone(),
                                            class: format!("{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}"),
                                            "Open repo"
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

/// Video to-do rows: rank, port, why it ranks, and the port page to watch and link from.
#[component]
fn VideoGapTable(rows: Vec<VideoGapItem>, start: i64, target: Option<String>) -> Element {
    rsx! {
        div { class: TABLE_WRAP,
            table { class: TABLE,
                thead { class: TABLE_HEAD_ROW,
                    tr {
                        th { class: "{TABLE_TH_NUM} w-12", scope: "col", "#" }
                        th { class: TABLE_TH, scope: "col", "Port" }
                        th { class: TABLE_TH, scope: "col", "Why it ranks high" }
                        th { class: TABLE_TH_NUM, scope: "col", "Next step" }
                    }
                }
                tbody {
                    for (i, row) in rows.iter().enumerate() {
                        {
                            let rank = start + i as i64 + 1;
                            let why = video_why(row, target.as_deref());
                            rsx! {
                                tr { key: "{row.port_id}", class: TABLE_ROW,
                                    td { class: "{TABLE_TD} w-12 text-right tabular text-muted-foreground", "{rank}" }
                                    td { class: TABLE_TD,
                                        Link {
                                            to: Route::PortPage { game_slug: row.game_slug.clone(), port_slug: row.port_slug.clone() },
                                            class: "font-medium hover:underline",
                                            "{row.port_name}"
                                        }
                                        p { class: "text-xs text-muted-foreground", "{row.game_name}" }
                                    }
                                    td { class: "{TABLE_TD} text-sm", "{why}" }
                                    td { class: "{TABLE_TD} text-right",
                                        Link {
                                            to: Route::PortPage { game_slug: row.game_slug.clone(), port_slug: row.port_slug.clone() },
                                            class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                                            "Open port page"
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
