//! The AI providers section of the admin page. The describe job runs the enabled providers in this
//! order and falls back to the next one when a call fails. Credentials stay on the server; the page
//! shows model names, account labels and recent errors only.

use dioxus::prelude::*;
use recomphub_shared::dto::{AiDeviceStart, AiOauthStart, AiProviderRow, AiProvidersView};

use crate::api;
use crate::components::styles::*;

const KIND_ANTHROPIC_API: &str = "anthropic_api";
const KIND_OPENAI_API: &str = "openai_api";
const KIND_ANTHROPIC_OAUTH: &str = "anthropic_oauth";
const KIND_OPENAI_CODEX_OAUTH: &str = "openai_codex_oauth";

/// Kinds in the order the add form offers them.
const KINDS: [&str; 4] = [
    KIND_ANTHROPIC_API,
    KIND_OPENAI_API,
    KIND_ANTHROPIC_OAUTH,
    KIND_OPENAI_CODEX_OAUTH,
];

const SUBSCRIPTION_WARNING: &str = "Subscription logins (Claude Pro or Max, ChatGPT Plus or Pro) are meant for \
     interactive use. Running them in background jobs may breach the provider's terms and could get the account \
     suspended. API keys are the supported choice for jobs.";

type Status = Option<Result<String, String>>;

fn kind_label(kind: &str) -> &'static str {
    match kind {
        KIND_ANTHROPIC_API => "Anthropic API key",
        KIND_OPENAI_API => "OpenAI API key",
        KIND_ANTHROPIC_OAUTH => "Claude Pro or Max subscription",
        KIND_OPENAI_CODEX_OAUTH => "ChatGPT Plus or Pro subscription",
        _ => "Unknown provider",
    }
}

fn is_subscription(kind: &str) -> bool {
    kind == KIND_ANTHROPIC_OAUTH || kind == KIND_OPENAI_CODEX_OAUTH
}

/// Starting model for each kind. Admins can change it after adding.
fn default_model(kind: &str) -> &'static str {
    match kind {
        KIND_OPENAI_API => "gpt-6-luna",
        KIND_OPENAI_CODEX_OAUTH => "gpt-5.6-luna",
        _ => "claude-haiku-5-5",
    }
}

/// Shows the outcome of a list action and reloads the list when it succeeded.
fn settle(
    outcome: Result<(), ServerFnError>,
    mut status: Signal<Status>,
    done: &str,
    on_changed: EventHandler<()>,
) {
    match outcome {
        Ok(()) => {
            status.set(Some(Ok(done.to_string())));
            on_changed.call(());
        }
        Err(e) => status.set(Some(Err(e.to_string()))),
    }
}

#[component]
pub fn AiProvidersSection() -> Element {
    let mut view = use_signal(|| None::<Result<AiProvidersView, String>>);
    let mut reload = use_signal(|| 0u32);
    use_effect(move || {
        let _ = reload();
        spawn(async move {
            let result = api::admin_ai_providers().await.map_err(|e| e.to_string());
            view.set(Some(result));
        });
    });
    let on_changed = move |_: ()| reload.set(reload() + 1);

    let state = view.read().clone();
    let body = match state {
        None => rsx! {
            div { class: "skeleton h-40 rounded-xl", "aria-hidden": "true" }
        },
        Some(Err(message)) => rsx! {
            p { class: "text-sm text-destructive", "Could not load AI providers: {message}" }
        },
        Some(Ok(data)) => rsx! {
            if !data.secrets_configured {
                p { class: "rounded-lg border bg-destructive/10 p-4 text-sm text-destructive",
                    "Provider keys are stored encrypted, and the server's SECRETS_KEY is not set. Providers cannot be added until it is."
                }
            }
            ProviderList { providers: data.providers, on_changed }
            AddProvider { secrets_configured: data.secrets_configured, on_changed }
        },
    };

    rsx! {
        section { aria_labelledby: "admin-ai-providers",
            h2 { id: "admin-ai-providers", class: H2, "AI providers" }
            p { class: "mt-2 text-sm text-muted-foreground",
                "The describe job runs the enabled providers in this order. If one fails, the next is tried. Model and order changes apply to the next run."
            }
            div { class: "mt-4 grid gap-4", {body} }
        }
    }
}

#[component]
fn ProviderList(providers: Vec<AiProviderRow>, on_changed: EventHandler<()>) -> Element {
    if providers.is_empty() {
        return rsx! {
            p { class: "text-sm text-muted-foreground", "No providers yet. The describe job does nothing until one is added." }
        };
    }
    let last = providers.len() - 1;
    rsx! {
        ol { class: "grid gap-4",
            for (index, row) in providers.into_iter().enumerate() {
                li { key: "{row.id}",
                    ProviderItem {
                        row,
                        number: index + 1,
                        first: index == 0,
                        last: index == last,
                        on_changed,
                    }
                }
            }
        }
    }
}

#[component]
fn ProviderItem(
    row: AiProviderRow,
    number: usize,
    first: bool,
    last: bool,
    on_changed: EventHandler<()>,
) -> Element {
    let id = row.id;
    let enabled = row.enabled;
    let mut model = use_signal(|| row.model.clone());
    let mut status = use_signal(|| None::<Result<String, String>>);

    let save = move |next: bool| {
        let value = model().trim().to_string();
        spawn(async move {
            let outcome = api::update_ai_provider(id, value, next).await;
            settle(outcome, status, "Saved.", on_changed);
        });
    };
    let reorder = move |up: bool| {
        spawn(async move {
            let outcome = api::move_ai_provider(id, up).await;
            settle(outcome, status, "Order updated.", on_changed);
        });
    };
    let test = move || {
        spawn(async move {
            match api::test_ai_provider(id).await {
                Ok(message) => status.set(Some(Ok(message))),
                Err(e) => status.set(Some(Err(e.to_string()))),
            }
            on_changed.call(());
        });
    };
    let delete = move || {
        spawn(async move {
            let outcome = api::delete_ai_provider(id).await;
            settle(outcome, status, "Deleted.", on_changed);
        });
    };

    let model_now = model();
    let status_now = status();
    let subscription = is_subscription(&row.kind);
    let kind = kind_label(&row.kind);
    let used_text = row
        .last_used_at
        .map(|t| format!("Last used {}", t.format("%-d %b %Y %H:%M UTC")));
    let error_text = row
        .last_error
        .clone()
        .map(|message| match row.last_error_at {
            Some(t) => format!("Last error {} UTC: {message}", t.format("%-d %b %Y %H:%M")),
            None => format!("Last error: {message}"),
        });

    rsx! {
        div { class: "{CARD_SURFACE} grid gap-4 p-4",
            div { class: "flex flex-wrap items-start justify-between gap-2",
                div { class: "grid gap-1",
                    p { class: "text-sm font-medium",
                        span { class: "tabular mr-2 text-muted-foreground", "{number}." }
                        "{kind}"
                    }
                    p { class: "text-sm text-muted-foreground", "{row.account_label}" }
                }
                div { class: "flex flex-wrap items-center gap-2",
                    if subscription {
                        span { class: "{BADGE} {BADGE_WARNING}", "Subscription" }
                    }
                    if enabled {
                        span { class: "{BADGE} {BADGE_SUCCESS}", "Enabled" }
                    } else {
                        span { class: "{BADGE} {BADGE_OUTLINE}", "Disabled" }
                    }
                }
            }
            div { class: "grid gap-2 sm:grid-cols-[1fr_auto] sm:items-end",
                label { class: "grid gap-2",
                    span { class: LABEL, "Model" }
                    input {
                        r#type: "text",
                        class: INPUT,
                        value: "{model_now}",
                        oninput: move |e| model.set(e.value()),
                    }
                }
                button {
                    r#type: "button",
                    class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                    onclick: move |_| save(enabled),
                    "Save model"
                }
            }
            div { class: "flex flex-wrap items-center gap-2",
                button {
                    r#type: "button",
                    class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                    disabled: first,
                    onclick: move |_| reorder(true),
                    "Move up"
                }
                button {
                    r#type: "button",
                    class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                    disabled: last,
                    onclick: move |_| reorder(false),
                    "Move down"
                }
                button {
                    r#type: "button",
                    class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                    onclick: move |_| test(),
                    "Test"
                }
                button {
                    r#type: "button",
                    class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM}",
                    onclick: move |_| save(!enabled),
                    if enabled { "Disable" } else { "Enable" }
                }
                button {
                    r#type: "button",
                    class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_SM} text-destructive",
                    onclick: move |_| delete(),
                    "Delete"
                }
            }
            if let Some(text) = used_text {
                p { class: "tabular text-xs text-muted-foreground", "{text}" }
            }
            if let Some(text) = error_text {
                p { class: "text-sm text-destructive", "{text}" }
            }
            match status_now {
                None => rsx! {},
                Some(Ok(message)) => rsx! {
                    p { class: "text-sm text-success", "{message}" }
                },
                Some(Err(message)) => rsx! {
                    p { class: "text-sm text-destructive", "{message}" }
                },
            }
        }
    }
}

#[component]
fn AddProvider(secrets_configured: bool, on_changed: EventHandler<()>) -> Element {
    let mut kind = use_signal(|| KIND_ANTHROPIC_API.to_string());
    let mut model = use_signal(|| default_model(KIND_ANTHROPIC_API).to_string());
    let mut api_key = use_signal(String::new);
    let mut pasted = use_signal(String::new);
    let mut paste_login = use_signal(|| None::<AiOauthStart>);
    let mut device_login = use_signal(|| None::<AiDeviceStart>);
    let mut status = use_signal(|| None::<Result<String, String>>);

    let mut choose = move |value: String| {
        model.set(default_model(&value).to_string());
        kind.set(value);
        paste_login.set(None);
        device_login.set(None);
        status.set(None);
    };

    let add_key = move || {
        let kind_now = kind();
        let model_now = model();
        let key = api_key();
        spawn(async move {
            match api::add_ai_api_key(kind_now, model_now, key).await {
                Ok(_) => {
                    api_key.set(String::new());
                    status.set(Some(Ok("Provider added.".to_string())));
                    on_changed.call(());
                }
                Err(e) => status.set(Some(Err(e.to_string()))),
            }
        });
    };

    let start_anthropic = move || {
        let model_now = model();
        spawn(async move {
            match api::start_ai_oauth(model_now).await {
                Ok(login) => {
                    paste_login.set(Some(login));
                    pasted.set(String::new());
                    status.set(None);
                }
                Err(e) => status.set(Some(Err(e.to_string()))),
            }
        });
    };

    let finish_anthropic = move || {
        let Some(login) = paste_login() else {
            return;
        };
        let value = pasted();
        spawn(async move {
            match api::finish_ai_oauth(login.pending_id, value).await {
                Ok(_) => {
                    paste_login.set(None);
                    pasted.set(String::new());
                    status.set(Some(Ok("Claude login added.".to_string())));
                    on_changed.call(());
                }
                Err(e) => status.set(Some(Err(e.to_string()))),
            }
        });
    };

    let start_device = move || {
        let model_now = model();
        spawn(async move {
            match api::start_ai_device(model_now).await {
                Ok(login) => {
                    device_login.set(Some(login));
                    status.set(None);
                }
                Err(e) => status.set(Some(Err(e.to_string()))),
            }
        });
    };

    let check_device = move || {
        let Some(login) = device_login() else {
            return;
        };
        spawn(async move {
            match api::poll_ai_device(login.pending_id).await {
                Ok(poll) if poll.provider.is_some() => {
                    device_login.set(None);
                    status.set(Some(Ok("ChatGPT login added.".to_string())));
                    on_changed.call(());
                }
                Ok(_) => status.set(Some(Err(
                    "Not approved yet. Enter the code on the ChatGPT page, then check again."
                        .to_string(),
                ))),
                Err(e) => status.set(Some(Err(e.to_string()))),
            }
        });
    };

    let kind_now = kind();
    let model_now = model();
    let key_now = api_key();
    let pasted_now = pasted();
    let paste_now = paste_login();
    let device_now = device_login();
    let status_now = status();
    let api_kind = kind_now == KIND_ANTHROPIC_API || kind_now == KIND_OPENAI_API;

    rsx! {
        section { class: "{CARD_SURFACE} grid gap-4 p-4", aria_labelledby: "admin-ai-add",
            h3 { id: "admin-ai-add", class: "text-base font-semibold", "Add a provider" }
            p { class: "rounded-lg border bg-warning/10 p-4 text-sm", "{SUBSCRIPTION_WARNING}" }
            div { class: "grid gap-2 sm:grid-cols-2",
                label { class: "grid gap-2",
                    span { class: LABEL, "Provider" }
                    select {
                        class: INPUT,
                        onchange: move |e| choose(e.value()),
                        for option in KINDS {
                            option { value: "{option}", selected: option == kind_now, "{kind_label(option)}" }
                        }
                    }
                }
                label { class: "grid gap-2",
                    span { class: LABEL, "Model" }
                    input {
                        r#type: "text",
                        class: INPUT,
                        value: "{model_now}",
                        oninput: move |e| model.set(e.value()),
                    }
                }
            }
            if api_kind {
                div { class: "grid gap-2 sm:grid-cols-[1fr_auto] sm:items-end",
                    label { class: "grid gap-2",
                        span { class: LABEL, "API key" }
                        input {
                            r#type: "password",
                            autocomplete: "off",
                            class: INPUT,
                            value: "{key_now}",
                            oninput: move |e| api_key.set(e.value()),
                        }
                    }
                    button {
                        r#type: "button",
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                        disabled: !secrets_configured,
                        onclick: move |_| add_key(),
                        "Add key"
                    }
                }
            } else if kind_now == KIND_ANTHROPIC_OAUTH {
                div { class: "grid gap-3",
                    match paste_now {
                        None => rsx! {
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD} justify-self-start",
                                disabled: !secrets_configured,
                                onclick: move |_| start_anthropic(),
                                "Connect Claude"
                            }
                        },
                        Some(login) => rsx! {
                            p { class: "text-sm text-muted-foreground",
                                "Open the page and sign in with the Claude account. It ends on a localhost page that won't load (or shows a code): copy the full address from the browser's address bar, or the code, and paste it here."
                            }
                            a {
                                href: "{login.authorize_url}",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                class: TEXT_LINK,
                                "Open the Claude login page"
                            }
                            div { class: "grid gap-2 sm:grid-cols-[1fr_auto] sm:items-end",
                                label { class: "grid gap-2",
                                    span { class: LABEL, "Pasted callback" }
                                    input {
                                        r#type: "text",
                                        autocomplete: "off",
                                        class: INPUT,
                                        placeholder: "Callback URL, or code#state",
                                        value: "{pasted_now}",
                                        oninput: move |e| pasted.set(e.value()),
                                    }
                                }
                                button {
                                    r#type: "button",
                                    class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                                    onclick: move |_| finish_anthropic(),
                                    "Finish login"
                                }
                            }
                        },
                    }
                }
            } else {
                div { class: "grid gap-3",
                    match device_now {
                        None => rsx! {
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD} justify-self-start",
                                disabled: !secrets_configured,
                                onclick: move |_| start_device(),
                                "Connect ChatGPT"
                            }
                        },
                        Some(login) => rsx! {
                            p { class: "text-sm text-muted-foreground",
                                "Open the ChatGPT device page, enter this code, then check here."
                            }
                            p { class: "font-mono text-2xl tabular", "{login.user_code}" }
                            a {
                                href: "{login.verify_url}",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                class: TEXT_LINK,
                                "Open the ChatGPT device page"
                            }
                            div { class: "flex flex-wrap gap-2",
                                button {
                                    r#type: "button",
                                    class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                                    onclick: move |_| check_device(),
                                    "Check login"
                                }
                                button {
                                    r#type: "button",
                                    class: "{BUTTON} {BUTTON_GHOST} {BUTTON_MD}",
                                    onclick: move |_| device_login.set(None),
                                    "Cancel"
                                }
                            }
                        },
                    }
                }
            }
            match status_now {
                None => rsx! {},
                Some(Ok(message)) => rsx! {
                    p { class: "text-sm text-success", "{message}" }
                },
                Some(Err(message)) => rsx! {
                    p { class: "text-sm text-destructive", "{message}" }
                },
            }
        }
    }
}
