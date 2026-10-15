use dioxus::document::{Link, Meta, Script, Title};
use dioxus::prelude::*;
use recomphub_shared::seo::json_ld_escape;

/// Absolute site origin without a trailing slash, for example `https://recomphub.com`.
/// The app root provides it from the server (`SITE_URL`).
#[derive(Clone, Debug, PartialEq)]
pub struct SiteUrl(pub String);

/// Document head for one page: title, description, canonical, Open Graph, Twitter and JSON-LD.
#[component]
pub fn Seo(
    title: String,
    description: String,
    path: String,
    #[props(default)] noindex: bool,
    #[props(default)] og_image: Option<String>,
    #[props(default)] json_ld: Vec<String>,
) -> Element {
    let site = use_context::<SiteUrl>().0;
    let canonical = format!("{site}{path}");
    let card = if og_image.is_some() {
        "summary_large_image"
    } else {
        "summary"
    };
    rsx! {
        Title { "{title}" }
        Meta { name: "description", content: "{description}" }
        Link { rel: "canonical", href: "{canonical}" }
        Meta { property: "og:title", content: "{title}" }
        Meta { property: "og:description", content: "{description}" }
        Meta { property: "og:url", content: "{canonical}" }
        Meta { property: "og:type", content: "website" }
        if let Some(img) = og_image.clone() {
            Meta { property: "og:image", content: "{img}" }
        }
        Meta { name: "twitter:card", content: card }
        if noindex {
            Meta { name: "robots", content: "noindex,follow" }
        }
        for ld in json_ld.iter() {
            Script { r#type: "application/ld+json", "{json_ld_escape(ld)}" }
        }
    }
}
