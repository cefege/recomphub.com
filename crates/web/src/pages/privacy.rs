//! `/privacy`: what RecompHub stores about visitors and members. Linked from the footer and
//! from the Google sign-in consent screen.

use dioxus::prelude::*;

use crate::components::styles::{H1, LEAD};
use crate::components::{Section, Seo};

#[component]
pub fn PrivacyPage() -> Element {
    rsx! {
        Seo {
            title: "Privacy | RecompHub".to_string(),
            description: "What RecompHub stores when you visit or sign in, and how to have it deleted.".to_string(),
            path: "/privacy".to_string(),
        }
        div { class: "mx-auto max-w-3xl space-y-12 leading-relaxed",
            header { class: "space-y-2",
                h1 { class: H1, "Privacy" }
                p { class: "{LEAD} max-w-2xl", "RecompHub collects as little as it can. There are no ads, no third-party trackers and nothing is sold." }
            }
            Section { heading: "When you visit".to_string(),
                ul { class: "list-disc space-y-2 pl-6",
                    li { "Page visits are counted for popularity rankings. A visit is stored as a one-way hash of your IP address and browser name, salted with a value that changes every day, so the same visitor cannot be followed from one day to the next. The IP address itself is not stored." }
                    li { "Your light or dark theme choice is kept in your browser's local storage." }
                    li { "Gameplay videos load from youtube-nocookie.com only after you press play. YouTube's own privacy policy applies to the player." }
                }
            }
            Section { heading: "When you sign in".to_string(),
                p { "You can sign in with Google (or GitHub, when offered). From the provider RecompHub receives and stores:" }
                ul { class: "list-disc space-y-2 pl-6",
                    li { "the account's stable id, used to recognise you next time;" }
                    li { "your name and profile picture, shown next to your comments, ratings and submissions;" }
                    li { "your email address, used only to recognise the site admin. It is never shown to anyone." }
                }
                p { "A sign-in cookie keeps you signed in for up to 30 days. RecompHub does not post anything or read anything else from your Google or GitHub account." }
                p { "Stars, wants, ratings, comments and submissions you make are stored with your account. Comments, ratings and submissions are public; stars and wants are counted without showing who made them." }
            }
            Section { heading: "Deleting your data".to_string(),
                p {
                    "To have your account and everything attached to it deleted, open an issue at "
                    a { href: "https://github.com/cefege/recomphub.com/issues", class: "underline underline-offset-4", "github.com/cefege/recomphub.com" }
                    " or email mateiasmihaiandrei@gmail.com from the address you signed in with."
                }
            }
        }
    }
}
