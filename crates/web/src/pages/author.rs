use dioxus::prelude::*;
use recomphub_shared::seo::{author_title, meta_description};

use crate::api;
use crate::components::icons::GithubIcon;
use crate::components::listing::WatchVisible;
use crate::components::{Breadcrumb, Picture, Section, Seo, initials_of, keyed};
use crate::pages::browse::PortGrid;

/// `/authors/:login`: the GitHub-linked author profile and their published ports.
#[component]
pub fn AuthorPage(login: String) -> Element {
    keyed(rsx! { AuthorInner { key: "{login}", login } })
}

#[component]
fn AuthorInner(login: String) -> Element {
    let loaded = use_loader({
        let login = login.clone();
        move || api::author(login.clone())
    })?;
    let profile = loaded();

    let name = if profile.name.trim().is_empty() {
        profile.login.clone()
    } else {
        profile.name.clone()
    };
    let path = format!("/authors/{}", profile.login);
    let ports = profile.ports.clone();

    let title = author_title(&name, &profile.login);
    let description = meta_description(&format!(
        "{} maintains {} game port(s) and recompilations on RecompHub. See the platforms, releases and AI usage of each project.",
        name,
        ports.len()
    ));
    let initials = initials_of(&name);
    let visible: Vec<i64> = ports.iter().map(|p| p.game_id).collect();

    rsx! {
        Seo { title, description, path: path.clone(), noindex: false }
        WatchVisible { ids: visible }
        div { class: "mx-auto max-w-7xl px-4 py-8",
            Breadcrumb { items: vec![
                ("Home".to_string(), "/".to_string()),
                (name.clone(), path.clone()),
            ] }
            header { class: "mb-10 flex flex-wrap items-center gap-6",
                div { class: "h-24 w-24 overflow-hidden rounded-full border border-border bg-surface-2",
                    Picture {
                        media: profile.avatar.clone(),
                        alt: format!("Avatar of {}", profile.login),
                        initials,
                        sizes: "96px".to_string(),
                        eager: true,
                    }
                }
                div { class: "min-w-0 flex-1",
                    h1 { class: "text-3xl font-black tracking-tight md:text-4xl", "{name}" }
                    p { class: "mt-1 text-muted", "@{profile.login}" }
                    a {
                        href: "https://github.com/{profile.login}",
                        rel: "noopener",
                        class: "mt-3 inline-flex items-center gap-2 text-sm font-semibold accent-text",
                        GithubIcon { class: "h-4 w-4" }
                        "GitHub profile"
                    }
                }
            }
            Section { heading: format!("Ports by {name}"),
                PortGrid { ports, list: false }
            }
        }
    }
}
