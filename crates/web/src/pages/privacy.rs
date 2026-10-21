//! `/privacy`: what RecompHub stores about visitors and members. Linked from the footer and
//! from the Google sign-in consent screen.

use dioxus::prelude::*;

use crate::components::styles::{BUTTON, BUTTON_MD, BUTTON_OUTLINE, H1, LEAD};
use crate::components::{Section, Seo};

const ANALYTICS_SETTINGS: &str = r#"
if (!window.zaraz?.consent?.APIReady) return false;
window.zaraz.consent.modal = true;
return true;
"#;

const ADVERTISING_SETTINGS: &str = r#"
var consent = window.googlefc;
if (typeof consent?.showRevocationMessage !== 'function' || !consent.callbackQueue) return false;
consent.callbackQueue.push({
    CONSENT_API_READY: function() { consent.showRevocationMessage(); }
});
return true;
"#;

async fn open_settings(
    script: &'static str,
    mut status: Signal<Option<String>>,
    unavailable: &'static str,
) {
    status.set(None);
    match document::eval(script).join::<bool>().await {
        Ok(true) => {}
        Ok(false) => status.set(Some(unavailable.to_string())),
        Err(_) => status.set(Some(
            "Could not open cookie settings. Try again after reloading the page.".to_string(),
        )),
    }
}
#[component]
pub fn PrivacyPage() -> Element {
    let choices_status = use_signal(|| None::<String>);
    rsx! {
        Seo {
            title: "Privacy | RecompHub".to_string(),
            description: "How RecompHub, Google Analytics and advertising partners use data, how to manage cookie choices, and how to delete your account.".to_string(),
            path: "/privacy".to_string(),
        }
        div { class: "mx-auto max-w-3xl space-y-12 leading-relaxed",
            header { class: "space-y-2",
                h1 { class: H1, "Privacy" }
                p { class: "{LEAD} max-w-2xl", "How RecompHub and its service providers use data when you visit, sign in, or see advertising." }
            }
            Section { heading: "When you visit".to_string(),
                ul { class: "list-disc space-y-2 pl-6",
                    li { "Page visits are counted for popularity rankings. The ranking system stores a one-way hash of your IP address and browser name, salted with a value that changes every day. It does not store the raw IP address for these rankings. Hosting and other service providers may process IP addresses to deliver and protect the site." }
                    li {
                        "If you accept analytics in the Cloudflare Zaraz cookie banner, Google Analytics counts page views and referrers. Declining keeps this analytics collection off. See "
                        a { href: "https://policies.google.com/privacy", class: "underline underline-offset-4", "Google's privacy policy" }
                        " and "
                        a { href: "https://policies.google.com/technologies/partner-sites", class: "underline underline-offset-4", "how Google uses data from partner sites" }
                        ". Analytics permission is separate from advertising permission."
                    }
                    li { "Your light or dark theme choice is kept in your browser's local storage." }
                    li { "Gameplay videos load from youtube-nocookie.com only after you press play. YouTube's own privacy policy applies to the player." }
                    li { "If you save games while signed out, their game ids are stored only in this browser's local storage. They are not installed games, do not contain game files or saves, and can be cleared from Your library. Browser storage may be unavailable or cleared by browser settings." }
                }
            }
            Section { heading: "Advertising".to_string(),
                p { "RecompHub uses Google AdSense. Third-party vendors, including Google, may use cookies or similar technologies to serve ads based on your prior visits to this site or other websites. Google's advertising cookies allow Google and its partners to personalize ads based on those visits." }
                p { "Ad services may process information such as your IP address, browser or device information, and interactions with ads to deliver, measure and protect advertising. The advertising consent message, where available, identifies participating vendors and the purposes for which they request permission." }
                p {
                    "You can manage Google's ad personalization in "
                    a { href: "https://www.google.com/settings/ads", class: "underline underline-offset-4", "Google Ads Settings" }
                    ". You can also opt out of personalized advertising from participating third-party vendors at "
                    a { href: "https://optout.aboutads.info/", class: "underline underline-offset-4", "AboutAds" }
                    ". These choices do not necessarily remove ads or prevent all advertising-related data use."
                }
            }
            div { id: "privacy-choices", class: "scroll-mt-20",
                Section { heading: "Your cookie and advertising choices".to_string(),
                    p { "Analytics and advertising have separate consent controls. For visitors covered by European consent requirements, advertising consent must be handled by a Google-certified consent manager; accepting analytics does not grant advertising consent." }
                    div { class: "flex flex-wrap gap-3",
                        button {
                            r#type: "button",
                            class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                            onclick: move |_| {
                                spawn(open_settings(
                                    ANALYTICS_SETTINGS,
                                    choices_status,
                                    "Analytics cookie settings are not available. The consent service may be blocked or still loading.",
                                ));
                            },
                            "Analytics cookie settings"
                        }
                        button {
                            r#type: "button",
                            class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                            onclick: move |_| {
                                spawn(open_settings(
                                    ADVERTISING_SETTINGS,
                                    choices_status,
                                    "Google's advertising consent settings are not available in this browser or region. You can also use Google Ads Settings above.",
                                ));
                            },
                            "Advertising cookie settings"
                        }
                    }
                    if let Some(message) = choices_status() {
                        p { role: "status", class: "text-sm text-muted-foreground", "{message}" }
                    }
                    p { "You can revisit these controls using Cookie settings in the footer. Consent controls may be unavailable if their service is blocked, still loading, or not applicable in your region. You can also manage cookies through your browser settings." }
                }
            }
            Section { heading: "When you sign in".to_string(),
                p { "You sign in with Google. From Google RecompHub receives and stores:" }
                ul { class: "list-disc space-y-2 pl-6",
                    li { "the account's stable id, used to recognise you next time;" }
                    li { "your name and profile picture, shown next to your comments, ratings and submissions;" }
                    li { "your email address, shown only to the site admin and used to recognise the admin account. It is never shown publicly." }
                }
                p { "A sign-in cookie keeps you signed in for up to 30 days. RecompHub does not post anything or read anything else from your Google account." }
                p { "Saved games are private account bookmarks. When you sign in, valid device-only saved games are additively copied into your account; existing account saved games are never replaced. Signing out does not copy account saved games back to device storage." }
                p { "Stars, wants, ratings, comments and submissions you make are stored with your account. Comments, ratings and submissions are public; stars and wants are counted without showing who made them." }
            }
            Section { heading: "Deleting your data".to_string(),
                p {
                    "To have your account and everything attached to it, including private saved games, deleted, open an issue at "
                    a { href: "https://github.com/cefege/recomphub.com/issues", class: "underline underline-offset-4", "github.com/cefege/recomphub.com" }
                    " or email mateiasmihaiandrei@gmail.com from the address you signed in with. Deleting an account or a catalog game removes its related saved-game bookmarks."
                }
            }
        }
    }
}
