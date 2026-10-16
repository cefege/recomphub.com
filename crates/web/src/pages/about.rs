use dioxus::prelude::*;
use recomphub_shared::seo::meta_description;

use crate::components::styles::{H1, LEAD};
use crate::components::{GroundRules, Section, Seo};

/// `/about`: how the score, live counts, stars, AI detection and data sources work.
#[component]
pub fn About() -> Element {
    rsx! {
        Seo {
            title: "About RecompHub".to_string(),
            description: meta_description(
                "How RecompHub ranks game recompilations and native ports, what live counts and stars mean, how AI use is detected, and where the data comes from.",
            ),
            path: "/about",
        }
        div { class: "mx-auto max-w-3xl space-y-12 leading-relaxed",
            header { class: "space-y-2",
                h1 { class: H1, "About RecompHub" }
                p { class: "{LEAD} max-w-2xl",
                    "RecompHub is a database of game recompilations and native ports across consoles, PCs, handhelds, mobile and the web. Every game has its own page listing its known ports, the platforms each one runs on, its authors, any AI tools used to make it, GitHub stars, release status and gameplay videos."
                }
            }
            Section { heading: "How the popularity score works".to_string(),
                p {
                    "Each visitor counts once per game per day, and only visits that run JavaScript are counted, so search crawlers do not inflate the numbers. Each counted visit adds one hit. Live viewing adds more: every "
                    "five minutes a visitor spends on a game page adds one more point (the divisor is set by the site's settings). A visitor who stays 30 minutes adds six points, so live attention counts for more than a drive-by visit."
                }
                p { "The score is the total for the selected window: today, this week, this month or this year." }
            }
            Section { heading: "Live counts and the Live now list".to_string(),
                p {
                    "\"here now\" is the number of distinct visitors who have a game page open right now. Several tabs from one visitor count once. \"waiting right now\" appears on games with no port yet, and it means people are looking for one. The number in the header is the total online across the whole site."
                }
            }
            Section { heading: "Stars, wants and activity".to_string(),
                ul { class: "list-disc space-y-2 pl-6",
                    li { "GitHub stars are the star count of the port's repository on GitHub." }
                    li { "RecompHub stars are signed-in users starring a port. A star is a bookmark, not a rating." }
                    li { "Wants are signed-in users asking for a port of a game that has none yet." }
                    li { "The activity feed shows recent stars, wants, new ports and releases. It does not show who did them." }
                }
            }
            Section { heading: "How AI use is detected".to_string(),
                p {
                    "Each port shows whether AI tools were used to make it: human-made, AI-assisted, primarily AI, or unknown. A port's own declaration always wins. When there is no declaration, RecompHub checks public signals in the repository and records the evidence next to the badge:"
                }
                ul { class: "mt-4 list-disc space-y-2 pl-6",
                    li { "Root files: CLAUDE.md or .claude (Claude), GEMINI.md (Gemini), .cursorrules or .cursor (Cursor), .windsurfrules (Windsurf), .clinerules (Cline), AGENTS.md (Other)." }
                    li { ".github/copilot-instructions.md (GitHub Copilot)." }
                    li { "Commit messages with a Claude Code co-author or generated-with line (Claude), and commits by the Copilot, Codex, Cursor or Jules agents." }
                    li { "README text naming Claude, ChatGPT, Codex, Copilot, Gemini, Qwen or DeepSeek, or describing the project as AI-assisted, AI-generated, made with AI or vibe-coded." }
                    li { "README text stating that the project was made without AI or LLMs, or by humans, counts as human-made." }
                }
                p { class: "mt-4",
                    "Detection never marks a port as primarily AI. Only a declaration by the port's maintainers or a moderator can do that."
                }
            }
            Section { heading: "Release status".to_string(),
                p {
                    "Release status is computed from the GitHub repository: archived, inactive (no push for a year), released, prerelease, or unreleased. Nobody edits it by hand."
                }
            }
            Section { heading: "Data sources and attributions".to_string(),
                ul { class: "list-disc space-y-2 pl-6",
                    li { "Seed data: Recompendium (MIT licensed), PortsDR, and GitHub search and r/recomps for new candidates." }
                    li { "Platform icons: Simple Icons (CC0)." }
                    li { "Gameplay videos are YouTube search results, shown with YouTube's privacy-enhanced embeds." }
                }
            }
            Section { heading: "What RecompHub does not host".to_string(),
                p {
                    "RecompHub never hosts or links to game ROMs, disc images or other copyrighted game files. Download buttons link only to the ports' own GitHub release pages. Moderators reject any submission that links to ROMs."
                }
            }
            GroundRules { class: "mt-12" }
        }
    }
}
