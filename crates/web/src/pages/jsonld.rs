//! JSON-LD for game and port pages. Port ratings come from RecompHub users only, never from GitHub stars; `aggregateRating` is emitted once a port has at least three ratings.

use recomphub_shared::dto::RatingSummary;
use recomphub_shared::{GameDetail, PortCard, PortDetail};
use serde_json::{Value, json};

use super::games::game_crumbs;
use crate::components::picture::avif_url;
use crate::components::ui::target_label;

const CONTEXT: &str = "https://schema.org";

fn breadcrumbs(site: &str, items: &[(String, String)]) -> Value {
    let elements: Vec<Value> = items
        .iter()
        .enumerate()
        .map(|(index, (name, path))| {
            json!({
                "@type": "ListItem",
                "position": index + 1,
                "name": name,
                "item": format!("{site}{path}"),
            })
        })
        .collect();
    json!({
        "@context": CONTEXT,
        "@type": "BreadcrumbList",
        "itemListElement": elements,
    })
}

fn software_application(site: &str, port: &PortCard) -> Value {
    let operating_system: Vec<String> = port.targets.iter().map(|t| target_label(t)).collect();
    let mut app = json!({
        "@type": "SoftwareApplication",
        "name": port.name,
        "applicationCategory": "GameApplication",
        "url": format!("{site}/games/{}/{}", port.game_slug, port.slug),
        "sameAs": port.repo_url,
    });
    if !operating_system.is_empty() {
        app["operatingSystem"] = json!(operating_system.join(", "));
    }
    let authors: Vec<Value> = port
        .authors
        .iter()
        .map(|author| json!({"@type": "Person", "name": author.name}))
        .collect();
    if !authors.is_empty() {
        app["author"] = Value::Array(authors);
    }
    app
}

/// `VideoGame`, `BreadcrumbList`, `FAQPage` and, when there are ports, an `ItemList` of
/// `SoftwareApplication`s.
pub fn game_json_ld(site: &str, detail: &GameDetail) -> Vec<String> {
    let game = &detail.game;
    let platforms: Vec<String> = detail.platforms.iter().map(|p| p.name.clone()).collect();
    let mut video = json!({
        "@context": CONTEXT,
        "@type": "VideoGame",
        "name": game.name,
        "url": format!("{site}/games/{}", game.slug),
        "gamePlatform": platforms,
    });
    if let Some(summary) = &detail.summary {
        video["description"] = json!(summary);
    }
    if let Some(date) = detail.first_release_date {
        video["datePublished"] = json!(date.to_string());
    }
    if let Some(cover) = game.cover.as_ref().filter(|m| m.ready)
        && let Some(width) = cover.widths.iter().max()
    {
        video["image"] = json!(format!("{site}{}", avif_url(cover, *width)));
    }
    if let Some(developer) = &detail.developer {
        video["author"] = json!({"@type": "Organization", "name": developer});
    }
    if let Some(publisher) = &detail.publisher {
        video["publisher"] = json!({"@type": "Organization", "name": publisher});
    }

    let mut out = vec![
        video.to_string(),
        breadcrumbs(site, &game_crumbs(detail)).to_string(),
    ];
    if !detail.ports.is_empty() {
        let items: Vec<Value> = detail
            .ports
            .iter()
            .enumerate()
            .map(|(index, port)| {
                json!({
                    "@type": "ListItem",
                    "position": index + 1,
                    "item": software_application(site, port),
                })
            })
            .collect();
        out.push(
            json!({
                "@context": CONTEXT,
                "@type": "ItemList",
                "itemListElement": items,
            })
            .to_string(),
        );
    }
    out.push(faq_page(&super::faq::game_faq(detail)).to_string());
    out
}

/// `FAQPage` with one `Question`/`Answer` pair per entry, matching the visible FAQ.
fn faq_page(faq: &[super::faq::Faq]) -> Value {
    let entities: Vec<Value> = faq
        .iter()
        .map(|f| {
            json!({
                "@type": "Question",
                "name": f.question,
                "acceptedAnswer": {"@type": "Answer", "text": f.answer},
            })
        })
        .collect();
    json!({
        "@context": CONTEXT,
        "@type": "FAQPage",
        "mainEntity": entities,
    })
}

/// `SoftwareApplication`, `SoftwareSourceCode` and `BreadcrumbList` for a port page.
pub fn port_json_ld(
    site: &str,
    detail: &PortDetail,
    crumbs: &[(String, String)],
    ratings: &RatingSummary,
) -> Vec<String> {
    let port = &detail.port;
    let mut app = software_application(site, port);
    app["@context"] = json!(CONTEXT);
    if let Some(version) = &port.latest_version {
        app["softwareVersion"] = json!(version);
    }
    if let Some(modified) = port.latest_release_at.or(detail.last_commit_at) {
        app["dateModified"] = json!(modified.to_rfc3339());
    }
    let mut source = json!({
        "@context": CONTEXT,
        "@type": "SoftwareSourceCode",
        "name": port.name,
        "codeRepository": port.repo_url,
    });
    if let Some(license) = &detail.license {
        source["license"] = json!(license);
    }
    if let (true, Some(average)) = (ratings.count >= 3, ratings.average) {
        app["aggregateRating"] = json!({
            "@type": "AggregateRating",
            "ratingValue": (average * 10.0).round() / 10.0,
            "ratingCount": ratings.count,
            "bestRating": 5,
            "worstRating": 1,
        });
    }
    vec![
        app.to_string(),
        source.to_string(),
        breadcrumbs(site, crumbs).to_string(),
    ]
}
