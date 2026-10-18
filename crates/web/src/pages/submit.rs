//! The submit page (repository preview, game picker, port form) and the edit
//! page (only the changed fields are sent). Both need a signed-in viewer.

use dioxus::prelude::*;
use recomphub_shared::{
    AI_TOOLS, AiUsage, AuthorDto, AuthorForm, ChoicePlatform, EditInput, EditPortForm, GameChoice,
    GameDetail, Playability, PortForm, PortKind, RepoPreview, RepoPreviewData, SubmitInput,
    SubmitResult, TargetPlatform,
};

use crate::api;
use crate::components::styles::*;
use crate::components::{GroundRules, Seo, SignedIn, ViewerStore, ai_text};
use crate::routes::Route;

const DESCRIPTION_MAX: usize = 2000;
const REQUIREMENTS_MAX: usize = 500;
/// Checkbox tile for platform and AI tool choices.
const CHECK: &str =
    "flex items-center gap-2 rounded-md border border-input px-3 py-2 text-sm shadow-xs";
/// Selectable option tile, for game picks.
const OPTION: &str = "w-full rounded-md border bg-background px-3 py-2 text-left text-sm transition-colors hover:bg-accent hover:text-accent-foreground";

/// Form state shared by the submit and edit forms.
#[derive(Clone, PartialEq)]
struct FormState {
    name: String,
    kind: String,
    source_platform_id: Option<i32>,
    homepage_url: String,
    description: String,
    requirements: String,
    toolchain: String,
    license: String,
    playability: String,
    runs_on: Vec<String>,
    ai_usage: String,
    ai_tools: Vec<String>,
    other_tool: String,
    authors: Vec<AuthorForm>,
    submitter_is_author: bool,
}

impl Default for FormState {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: PortKind::StaticRecomp.as_str().to_string(),
            source_platform_id: None,
            homepage_url: String::new(),
            description: String::new(),
            requirements: String::new(),
            toolchain: String::new(),
            license: String::new(),
            playability: Playability::Unknown.as_str().to_string(),
            runs_on: Vec::new(),
            ai_usage: AiUsage::Unknown.as_str().to_string(),
            ai_tools: Vec::new(),
            other_tool: String::new(),
            authors: Vec::new(),
            submitter_is_author: false,
        }
    }
}

impl FormState {
    fn from_preview(d: &RepoPreviewData) -> Self {
        let (ai_tools, other_tool) = split_tools(&d.ai_tools);
        Self {
            name: d.name.clone(),
            kind: guess_kind(&d.name, &d.repo_url).to_string(),
            homepage_url: d.homepage_url.clone().unwrap_or_default(),
            description: d.description.clone().unwrap_or_default(),
            license: d.license.clone().unwrap_or_default(),
            runs_on: d.detected_platforms.clone(),
            ai_usage: d
                .ai_usage
                .clone()
                .unwrap_or_else(|| AiUsage::Unknown.as_str().to_string()),
            ai_tools,
            other_tool,
            authors: d.contributors.iter().map(author_form).collect(),
            submitter_is_author: d.submitter_is_author,
            ..Self::default()
        }
    }

    fn from_port(p: &PortForm) -> Self {
        let (ai_tools, other_tool) = split_tools(&p.ai_tools);
        Self {
            name: p.name.clone(),
            kind: p.kind.clone(),
            source_platform_id: p.source_platform_id,
            homepage_url: p.homepage_url.clone().unwrap_or_default(),
            description: p.description.clone().unwrap_or_default(),
            requirements: p.requirements.clone().unwrap_or_default(),
            toolchain: p.toolchain.clone().unwrap_or_default(),
            license: p.license.clone().unwrap_or_default(),
            playability: p.playability.clone(),
            runs_on: p.declared_platforms.clone(),
            ai_usage: p.ai_usage.clone(),
            ai_tools,
            other_tool,
            authors: p.authors.clone(),
            ..Self::default()
        }
    }
}

/// Signals for the submit flow. Signals are `Copy`, so the handle is too.
#[derive(Clone, Copy)]
struct Wiz {
    repo_url: Signal<String>,
    preview: Signal<Option<RepoPreviewData>>,
    already: Signal<Option<(String, String)>>,
    target: Signal<Option<GameChoice>>,
    hits: Signal<Vec<GameChoice>>,
    draft: Signal<FormState>,
    busy: Signal<bool>,
    error: Signal<Option<String>>,
    result: Signal<Option<SubmitResult>>,
    rules_ok: Signal<bool>,
}

/// The submit page for `/submit?:game&:repo`.
#[component]
pub fn SubmitPage(game: String, repo: String) -> Element {
    rsx! {
        Seo {
            title: "Submit a port | RecompHub".to_string(),
            description: "Submit a native port or recompilation of a game. Paste a GitHub repository and the details are filled in for you.".to_string(),
            path: "/submit".to_string(),
            noindex: true,
        }
        SignedIn {
            prompt: "Sign in to submit a port or suggest an edit.".to_string(),
            SubmitForm { game, repo }
        }
    }
}

/// The edit page for `/submit/edit/:port_id`.
#[component]
pub fn EditPortPage(port_id: i64) -> Element {
    rsx! {
        Seo {
            title: "Suggest an edit | RecompHub".to_string(),
            description: "Suggest a correction to this port's details. Moderators review every edit.".to_string(),
            path: format!("/submit/edit/{port_id}"),
            noindex: true,
        }
        SignedIn {
            prompt: "Sign in to suggest an edit.".to_string(),
            EditLoaded { port_id }
        }
    }
}

#[component]
fn SubmitForm(game: String, repo: String) -> Element {
    let mut w = Wiz {
        repo_url: use_signal(|| repo.clone()),
        preview: use_signal(|| None),
        already: use_signal(|| None),
        target: use_signal(|| None),
        hits: use_signal(Vec::new),
        draft: use_signal(FormState::default),
        busy: use_signal(|| false),
        error: use_signal(|| None),
        result: use_signal(|| None),
        rules_ok: use_signal(|| false),
    };
    let mut game_query = use_signal(String::new);

    let prefill_game = game.clone();
    use_effect(move || {
        if !prefill_game.is_empty() {
            load_target(w, prefill_game.clone());
        }
    });
    let prefill_repo = repo.clone();
    use_effect(move || {
        if !prefill_repo.is_empty() {
            start_preview(w, prefill_repo.clone());
        }
    });
    if let Some(result) = w.result.read().clone() {
        return rsx! {
            SubmitConfirmation {
                result,
                on_submit_another: move |_| reset_submit_form(w),
            }
        };
    }
    let platforms = w
        .target
        .read()
        .as_ref()
        .map(|t| t.platforms.clone())
        .unwrap_or_default();
    let target_now = w.target.read().clone();
    let preview_now = w.preview.read().clone();

    rsx! {
        div { class: "mx-auto max-w-3xl space-y-12",
            header {
                h1 { class: H1, "Submit a port" }
                p { class: "{LEAD} mt-2 max-w-2xl",
                    "Paste a GitHub repository, pick the game, and check the details. Trusted accounts publish right away."
                }
                if let Some(msg) = w.error.read().clone() {
                    p { class: "mt-6 rounded-lg border border-destructive/50 p-4 text-sm text-destructive", "{msg}" }
                }
            }
            GroundRules { class: "mb-8" }

            FormPanel { heading: "1. The repository".to_string(),
                div { class: "flex flex-col gap-2 sm:flex-row",
                    input {
                        class: INPUT,
                        r#type: "url",
                        placeholder: "https://github.com/owner/repo",
                        value: "{w.repo_url.read()}",
                        oninput: move |e| w.repo_url.set(e.value()),
                    }
                    button {
                        r#type: "button",
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                        disabled: *w.busy.read(),
                        onclick: move |_| start_preview(w, w.repo_url.read().clone()),
                        "Fetch details"
                    }
                }
                if let Some((game_slug, port_slug)) = w.already.read().clone() {
                    p { class: "text-sm text-muted-foreground",
                        "This repository is already listed. "
                        Link {
                            to: Route::PortPage { game_slug, port_slug },
                            class: TEXT_LINK,
                            "View the port"
                        }
                    }
                }
                if let Some(d) = preview_now {
                    PreviewSummary { data: d }
                }
            }

            FormPanel { heading: "2. The game".to_string(),
                if let Some(t) = target_now {
                    div { class: "flex flex-wrap items-center justify-between gap-3",
                        p { class: "font-medium",
                            "{t.name}{year_suffix(t.release_year)}{igdb_tag(&t)}"
                        }
                        button {
                            r#type: "button",
                            class: "{BUTTON} {BUTTON_GHOST} {BUTTON_SM}",
                            onclick: move |_| w.target.set(None),
                            "Change game"
                        }
                    }
                } else {
                    div { class: "grid gap-4",
                        div { class: "flex flex-col gap-2 sm:flex-row",
                            input {
                                class: INPUT,
                                r#type: "search",
                                placeholder: "Search games by title",
                                value: "{game_query.read()}",
                                oninput: move |e| game_query.set(e.value()),
                            }
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                                onclick: move |_| search_games(w, game_query.read().clone(), false),
                                "Search"
                            }
                            button {
                                r#type: "button",
                                class: "{BUTTON} {BUTTON_GHOST} {BUTTON_MD}",
                                onclick: move |_| search_games(w, game_query.read().clone(), true),
                                "Not listed? Search all games"
                            }
                        }
                        ul { class: "grid gap-2 sm:grid-cols-2",
                            for c in w.hits.read().clone() {
                                li {
                                    button {
                                        r#type: "button",
                                        class: OPTION,
                                        onclick: move |_| pick_game(w, c.clone()),
                                        "{c.name}{year_suffix(c.release_year)}{igdb_tag(&c)}"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            FormPanel { heading: "3. The port".to_string(),
                PortFields { draft: w.draft, full: true, platforms: platforms.clone() }
            }

            FormPanel { heading: "4. Submit".to_string(),
                p { class: "text-sm text-muted-foreground",
                    "Everyone else's submission goes to the moderation queue, and moderators review it before it is published."
                }
                label { class: "flex items-start gap-2 text-sm",
                    input {
                        r#type: "checkbox",
                        class: "mt-1",
                        checked: *w.rules_ok.read(),
                        onchange: move |e| w.rules_ok.set(e.checked()),
                    }
                    span { "This project follows the ground rules and ships no game data" }
                }
                button {
                    r#type: "button",
                    class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_MD} w-fit",
                    disabled: *w.busy.read() || !*w.rules_ok.read(),
                    onclick: move |_| submit_port(w),
                    if *w.busy.read() {
                        "Submitting…"
                    } else {
                        "Submit port"
                    }
                }
            }
        }
    }
}

/// The edit form for an existing port. Only changed fields are sent.
#[component]
fn EditLoaded(port_id: i64) -> Element {
    let loaded = use_loader(move || api::port_form(port_id))?;
    let data = loaded();
    rsx! { EditForm { data } }
}

#[component]
fn EditForm(data: EditPortForm) -> Element {
    let draft = use_signal(|| FormState::from_port(&data.port));
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut result = use_signal(|| None::<SubmitResult>);
    let original = data.port.clone();
    let port_id = data.port_id;
    let game_slug = data.game_slug.clone();
    let heading = format!("Edit {} for {}", data.port.name, data.game_name);

    rsx! {
        div { class: "mx-auto max-w-3xl space-y-12",
            header {
                h1 { class: H1, "{heading}" }
                p { class: "{LEAD} mt-2 max-w-2xl",
                    "Changes are checked before they go live, unless you're a moderator. A moderator usually looks within a day."
                }
                if let Some(msg) = error.read().clone() {
                    p { class: "mt-6 rounded-lg border border-destructive/50 p-4 text-sm text-destructive", "{msg}" }
                }
            }
            FormPanel { heading: "Port details".to_string(),
                PortFields { draft, full: false, platforms: Vec::new() }
                button {
                    r#type: "button",
                    class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_MD} w-fit",
                    disabled: *busy.read(),
                    onclick: move |_| {
                        let edited = match build_port(&draft.read(), &original.repo_url) {
                            Ok(p) => p,
                            Err(msg) => {
                                error.set(Some(msg));
                                return;
                            }
                        };
                        let input = edit_input(&original, &edited, port_id);
                        if !has_changes(&input) {
                            error.set(Some("Nothing has changed.".to_string()));
                            return;
                        }
                        busy.set(true);
                        error.set(None);
                        spawn(async move {
                            match api::submit_edit(input).await {
                                Ok(r) => result.set(Some(r)),
                                Err(e) => error.set(Some(e.to_string())),
                            }
                            busy.set(false);
                        });
                    },
                    "Save changes"
                }
                if let Some(r) = result.read().clone() {
                    SubmitOutcome { result: r }
                }
                if let Some(port_slug) = result.read().as_ref().and_then(|r| r.port_slug.clone()) {
                    p { class: "text-sm",
                        Link {
                            to: Route::PortPage { game_slug: game_slug.clone(), port_slug },
                            class: TEXT_LINK,
                            "Back to the port"
                        }
                    }
                }
            }
        }
    }
}

/// Fields shared by the submit and edit forms. `full` also shows the source
/// platform, license and author rows, which the edit form does not change.
#[component]
fn PortFields(draft: Signal<FormState>, full: bool, platforms: Vec<ChoicePlatform>) -> Element {
    let mut draft = draft;
    let sources: Vec<(i32, String)> = platforms
        .iter()
        .filter_map(|p| p.id.map(|id| (id, p.name.clone())))
        .collect();

    rsx! {
        div { class: "grid gap-6 md:grid-cols-2",
            Field { caption: "Port name".to_string(),
                input {
                    class: INPUT,
                    r#type: "text",
                    value: "{draft.read().name}",
                    oninput: move |e| draft.write().name = e.value(),
                }
            }
            Field { caption: "Kind".to_string(),
                select {
                    class: INPUT,
                    onchange: move |e| draft.write().kind = e.value(),
                    for k in PortKind::ALL.iter().copied() {
                        option { value: "{k.as_str()}", selected: draft.read().kind == k.as_str(), "{k.label()}" }
                    }
                }
            }
            if full {
                Field { caption: "Original platform".to_string(),
                    select {
                        class: INPUT,
                        onchange: move |e| draft.write().source_platform_id = e.value().parse::<i32>().ok(),
                        option { value: "", selected: draft.read().source_platform_id.is_none(), "Choose a platform" }
                        for (id, name) in sources.clone() {
                            option { value: "{id}", selected: draft.read().source_platform_id == Some(id), "{name}" }
                        }
                    }
                }
            }
            Field { caption: "Playability".to_string(),
                select {
                    class: INPUT,
                    onchange: move |e| draft.write().playability = e.value(),
                    for p in Playability::ALL.iter().copied() {
                        option { value: "{p.as_str()}", selected: draft.read().playability == p.as_str(), "{p.label()}" }
                    }
                }
            }
            Field { caption: "Homepage".to_string(),
                input {
                    class: INPUT,
                    r#type: "url",
                    value: "{draft.read().homepage_url}",
                    oninput: move |e| draft.write().homepage_url = e.value(),
                }
            }
            if full {
                Field { caption: "License".to_string(),
                    input {
                        class: INPUT,
                        r#type: "text",
                        value: "{draft.read().license}",
                        oninput: move |e| draft.write().license = e.value(),
                    }
                }
            }
            Field { caption: "Toolchain".to_string(),
                input {
                    class: INPUT,
                    r#type: "text",
                    value: "{draft.read().toolchain}",
                    oninput: move |e| draft.write().toolchain = e.value(),
                }
            }
        }

        div { class: "grid gap-2",
            span { class: LABEL, "Runs on" }
            div { class: "grid gap-2 sm:grid-cols-3",
                for t in TargetPlatform::ALL.iter().copied() {
                    label { class: CHECK,
                        input {
                            r#type: "checkbox",
                            checked: draft.read().runs_on.iter().any(|v| v == t.as_str()),
                            onchange: move |e| toggle(&mut draft.write().runs_on, t.as_str(), e.checked()),
                        }
                        "{t.label()}"
                    }
                }
            }
        }

        div { class: "grid gap-4",
            Field { caption: "AI use".to_string(),
                select {
                    class: INPUT,
                    onchange: move |e| draft.write().ai_usage = e.value(),
                    for u in AiUsage::ALL.iter().copied() {
                        option { value: "{u.as_str()}", selected: draft.read().ai_usage == u.as_str(), "{u.label()}" }
                    }
                }
            }
            div { class: "grid gap-2 sm:grid-cols-3",
                for tool in AI_TOOLS.iter().copied() {
                    label { class: CHECK,
                        input {
                            r#type: "checkbox",
                            checked: draft.read().ai_tools.iter().any(|v| v == tool),
                            onchange: move |e| toggle(&mut draft.write().ai_tools, tool, e.checked()),
                        }
                        "{tool}"
                    }
                }
            }
            input {
                class: INPUT,
                r#type: "text",
                placeholder: "Other AI tools, comma separated",
                value: "{draft.read().other_tool}",
                oninput: move |e| draft.write().other_tool = e.value(),
            }
        }

        div { class: "grid gap-6 md:grid-cols-2",
            Field { caption: "Requirements".to_string(),
                textarea {
                    class: TEXTAREA,
                    value: "{draft.read().requirements}",
                    oninput: move |e| draft.write().requirements = e.value(),
                }
                span { class: "block text-sm text-muted-foreground tabular", "{draft.read().requirements.chars().count()}/{REQUIREMENTS_MAX}" }
            }
            Field { caption: "Description".to_string(),
                textarea {
                    class: TEXTAREA,
                    value: "{draft.read().description}",
                    oninput: move |e| draft.write().description = e.value(),
                }
                span { class: "block text-sm text-muted-foreground tabular", "{draft.read().description.chars().count()}/{DESCRIPTION_MAX}" }
            }
        }

        if full {
            div { class: "grid gap-4",
                span { class: LABEL, "Credited authors" }
                for (i, a) in draft.read().authors.clone().into_iter().enumerate() {
                    div { class: "grid gap-2 sm:grid-cols-[1fr_1fr_auto]",
                        input {
                            class: INPUT,
                            r#type: "text",
                            placeholder: "Name",
                            value: "{a.name}",
                            oninput: move |e| {
                                if let Some(row) = draft.write().authors.get_mut(i) {
                                    row.name = e.value();
                                }
                            },
                        }
                        input {
                            class: INPUT,
                            r#type: "text",
                            placeholder: "GitHub login (optional)",
                            value: "{a.github_login.clone().unwrap_or_default()}",
                            oninput: move |e| {
                                if let Some(row) = draft.write().authors.get_mut(i) {
                                    row.github_login = opt(&e.value());
                                }
                            },
                        }
                        button {
                            r#type: "button",
                            class: "{BUTTON} {BUTTON_GHOST} {BUTTON_MD}",
                            onclick: move |_| {
                                let mut d = draft.write();
                                if i < d.authors.len() {
                                    d.authors.remove(i);
                                }
                            },
                            "Remove"
                        }
                    }
                }
                button {
                    r#type: "button",
                    class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD} w-fit",
                    onclick: move |_| draft.write().authors.push(AuthorForm {
                        github_login: None,
                        name: String::new(),
                        role: "credited".to_string(),
                    }),
                    "Add author"
                }
            }
        }
    }
}

/// A bordered form section: heading, then its fields.
#[component]
fn FormPanel(heading: String, children: Element) -> Element {
    rsx! {
        section { class: "{CARD_SURFACE} p-6",
            h2 { class: "text-base font-semibold", "{heading}" }
            div { class: "mt-4 grid gap-6", {children} }
        }
    }
}

/// A labelled field: caption, control and any helper text.
#[component]
fn Field(caption: String, children: Element) -> Element {
    rsx! {
        label { class: "grid gap-2",
            span { class: LABEL, "{caption}" }
            {children}
        }
    }
}

/// Summary of a fetched repository, shown above the port form.
#[component]
fn PreviewSummary(data: RepoPreviewData) -> Element {
    let version = data
        .latest_version
        .clone()
        .unwrap_or_else(|| "no release yet".to_string());
    let ai = data
        .ai_usage
        .as_deref()
        .map(|usage| ai_text(usage, &data.ai_tools))
        .unwrap_or_else(|| "AI use unknown".to_string());
    let platforms = if data.detected_platforms.is_empty() {
        "none detected".to_string()
    } else {
        data.detected_platforms.join(", ")
    };
    let author_note = if data.submitter_is_author {
        "You appear to maintain this repository."
    } else {
        ""
    };
    rsx! {
        div { class: "space-y-1 rounded-lg border bg-muted p-4 text-sm",
            p { class: "font-medium", "{data.owner}/{data.name}" }
            if let Some(desc) = data.description.clone() {
                p { class: "text-muted-foreground", "{desc}" }
            }
            p { class: "text-muted-foreground", "★ {data.stars} · {version}" }
            p { class: "text-muted-foreground", "Detected platforms: {platforms}" }
            p { class: "text-muted-foreground", "AI: {ai}" }
            if !author_note.is_empty() {
                p { class: "text-muted-foreground", "{author_note}" }
            }
        }
    }
}

/// Full confirmation shown after a new-port submission. Replacing the form makes a completed
/// submission unambiguous and prevents an accidental second click from creating a duplicate.
#[component]
fn SubmitConfirmation(result: SubmitResult, on_submit_another: EventHandler<()>) -> Element {
    let viewer = use_context::<ViewerStore>();
    let first_name = viewer
        .me
        .read()
        .as_ref()
        .map(|u| {
            u.display_name
                .split_whitespace()
                .next()
                .unwrap_or(&u.display_name)
                .to_string()
        })
        .unwrap_or_else(|| "there".to_string());
    let (heading, detail) = if result.published {
        (
            "Your port is live.",
            "It was published immediately and is now visible on RecompHub.",
        )
    } else {
        (
            "Submission received.",
            "Your project is in the moderation queue and will be reviewed before it is published.",
        )
    };
    rsx! {
        div { class: "mx-auto max-w-2xl space-y-8 py-8",
            header { class: "space-y-2",
                p { class: "text-sm font-medium text-success", "Thanks, {first_name}!" }
                h1 { class: H1, "{heading}" }
                p { class: LEAD, "{detail}" }
            }
            if !result.published {
                div { class: "rounded-lg border border-success/50 bg-success/10 p-5 text-sm",
                    p { class: "font-medium", "No need to submit it again — we have it." }
                    ol { class: "mt-3 list-decimal space-y-1 pl-5 text-muted-foreground",
                        li { "A moderator checks the project, game match, and submitted details." }
                        li { "If it is approved, it will appear publicly on RecompHub." }
                        li { "You can check its status anytime under My submissions." }
                    }
                }
            }
            div { class: "flex flex-wrap gap-3",
                if let (Some(game_slug), Some(port_slug)) = (result.game_slug.clone(), result.port_slug.clone()) {
                    Link {
                        to: Route::PortPage { game_slug, port_slug },
                        class: "{BUTTON} {BUTTON_PRIMARY} {BUTTON_MD}",
                        "View the port"
                    }
                }
                if !result.published {
                    Link {
                        to: Route::AccountPage {},
                        class: "{BUTTON} {BUTTON_OUTLINE} {BUTTON_MD}",
                        "My submissions"
                    }
                }
                button {
                    r#type: "button",
                    class: "{BUTTON} {BUTTON_GHOST} {BUTTON_MD}",
                    onclick: move |_| on_submit_another.call(()),
                    "Submit another port"
                }
            }
        }
    }
}

/// Result of an edit submission.
#[component]
fn SubmitOutcome(result: SubmitResult) -> Element {
    let viewer = use_context::<ViewerStore>();
    let greeting = viewer
        .me
        .read()
        .as_ref()
        .map(|u| {
            let first = u
                .display_name
                .split_whitespace()
                .next()
                .unwrap_or(&u.display_name);
            format!("Thanks, {first}! ")
        })
        .unwrap_or_default();
    let (status, detail) = if result.published {
        ("Published. It is live now.", None)
    } else {
        (
            "Got it, queued for review.",
            Some("You can check its status from your account page."),
        )
    };
    rsx! {
        div { class: "rounded-lg border border-success/50 bg-success/10 p-4 text-sm",
            p { class: "font-medium", "{greeting}{status}" }
            if let Some(detail) = detail {
                p { class: "mt-1 text-muted-foreground", "{detail}" }
            }
            div { class: "mt-2 flex flex-wrap gap-x-4 gap-y-1",
                if let (Some(game_slug), Some(port_slug)) = (result.game_slug.clone(), result.port_slug.clone()) {
                    Link {
                        to: Route::PortPage { game_slug, port_slug },
                        class: TEXT_LINK,
                        "View the port"
                    }
                }
                if !result.published {
                    Link { to: Route::AccountPage {}, class: TEXT_LINK, "My submissions" }
                }
            }
        }
    }
}

/// Clears every submission-specific signal before returning to the form. Keeping the reset local
/// avoids relying on a same-route navigation to remount the component.
fn reset_submit_form(mut w: Wiz) {
    w.repo_url.set(String::new());
    w.preview.set(None);
    w.already.set(None);
    w.target.set(None);
    w.hits.set(Vec::new());
    w.draft.set(FormState::default());
    w.busy.set(false);
    w.error.set(None);
    w.result.set(None);
    w.rules_ok.set(false);
}

fn start_preview(mut w: Wiz, url: String) {
    w.busy.set(true);
    w.error.set(None);
    w.already.set(None);
    spawn(async move {
        match api::preview_repo(url).await {
            Ok(RepoPreview::AlreadyListed {
                game_slug,
                port_slug,
            }) => {
                w.already.set(Some((game_slug, port_slug)));
            }
            Ok(RepoPreview::Preview(data)) => {
                w.draft.set(FormState::from_preview(&data));
                w.preview.set(Some(*data));
            }
            Err(e) => w.error.set(Some(e.to_string())),
        }
        w.busy.set(false);
    });
}

fn load_target(mut w: Wiz, slug: String) {
    spawn(async move {
        if let Ok(detail) = api::game_detail(slug).await {
            w.target.set(Some(choice_from_detail(&detail)));
        }
    });
}

fn search_games(mut w: Wiz, q: String, igdb: bool) {
    w.busy.set(true);
    w.error.set(None);
    spawn(async move {
        let found = if igdb {
            api::search_igdb(q).await
        } else {
            api::search_games(q).await
        };
        match found {
            Ok(list) => w.hits.set(list),
            Err(e) => w.error.set(Some(e.to_string())),
        }
        w.busy.set(false);
    });
}

fn pick_game(mut w: Wiz, choice: GameChoice) {
    w.target.set(Some(choice));
    w.hits.set(Vec::new());
}

fn submit_port(mut w: Wiz) {
    if *w.busy.read() || w.result.read().is_some() {
        return;
    }

    if !*w.rules_ok.read() {
        w.error
            .set(Some("Confirm the ground rules first.".to_string()));
        return;
    }
    let url = w.repo_url.read().trim().to_string();
    let draft = w.draft.read().clone();
    let target = match w.target.read().clone() {
        Some(t) => t,
        None => {
            w.error.set(Some("Choose the game first.".to_string()));
            return;
        }
    };
    if target.game_id.is_none() && target.igdb_id.is_none() {
        w.error.set(Some("Choose the game first.".to_string()));
        return;
    }
    let port = match build_port(&draft, &url) {
        Ok(p) => p,
        Err(msg) => {
            w.error.set(Some(msg));
            return;
        }
    };
    let input = SubmitInput {
        game_id: target.game_id,
        igdb_id: if target.game_id.is_some() {
            None
        } else {
            target.igdb_id
        },
        port,
        submitter_is_author: draft.submitter_is_author,
    };
    w.busy.set(true);
    w.error.set(None);
    spawn(async move {
        match api::submit(input).await {
            Ok(r) => w.result.set(Some(r)),
            Err(e) => w.error.set(Some(e.to_string())),
        }
        w.busy.set(false);
    });
}

fn choice_from_detail(d: &GameDetail) -> GameChoice {
    GameChoice {
        game_id: Some(d.game.id),
        igdb_id: None,
        name: d.game.name.clone(),
        release_year: d.game.release_year,
        platforms: d
            .platforms
            .iter()
            .map(|p| ChoicePlatform {
                id: Some(p.id),
                igdb_id: None,
                name: p.name.clone(),
            })
            .collect(),
    }
}

fn author_form(a: &AuthorDto) -> AuthorForm {
    AuthorForm {
        github_login: a.github_login.clone(),
        name: a.name.clone(),
        role: a.role.clone(),
    }
}

/// Validates the draft and builds the port form. Blank text becomes `None`.
fn build_port(draft: &FormState, repo_url: &str) -> Result<PortForm, String> {
    let name = draft.name.trim();
    if name.is_empty() {
        return Err("Give the port a name.".to_string());
    }
    let description = opt(&draft.description);
    if description
        .as_ref()
        .is_some_and(|d| d.chars().count() > DESCRIPTION_MAX)
    {
        return Err(format!(
            "The description can be at most {DESCRIPTION_MAX} characters."
        ));
    }
    let requirements = opt(&draft.requirements);
    if requirements
        .as_ref()
        .is_some_and(|r| r.chars().count() > REQUIREMENTS_MAX)
    {
        return Err(format!(
            "The requirements can be at most {REQUIREMENTS_MAX} characters."
        ));
    }
    let mut ai_tools = draft.ai_tools.clone();
    for tool in draft
        .other_tool
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        ai_tools.push(tool.to_string());
    }
    let authors = draft
        .authors
        .iter()
        .filter_map(|a| {
            let login = a
                .github_login
                .as_deref()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string);
            let name = opt(&a.name).or_else(|| login.clone())?;
            Some(AuthorForm {
                github_login: login,
                name,
                role: a.role.clone(),
            })
        })
        .collect();
    Ok(PortForm {
        name: name.to_string(),
        kind: draft.kind.clone(),
        source_platform_id: draft.source_platform_id,
        repo_url: repo_url.to_string(),
        homepage_url: opt(&draft.homepage_url),
        description,
        requirements,
        toolchain: opt(&draft.toolchain),
        license: opt(&draft.license),
        playability: draft.playability.clone(),
        declared_platforms: draft.runs_on.clone(),
        ai_usage: draft.ai_usage.clone(),
        ai_tools,
        authors,
    })
}

/// Builds the edit payload: only the fields that differ from the original.
fn edit_input(original: &PortForm, edited: &PortForm, port_id: i64) -> EditInput {
    fn changed<T: PartialEq + Clone>(old: &T, new: &T) -> Option<T> {
        (old != new).then(|| new.clone())
    }
    fn sorted(values: &[String]) -> Vec<String> {
        let mut out = values.to_vec();
        out.sort();
        out
    }
    let tools_changed = sorted(&original.ai_tools) != sorted(&edited.ai_tools);
    let platforms_changed =
        sorted(&original.declared_platforms) != sorted(&edited.declared_platforms);
    EditInput {
        port_id,
        name: changed(&original.name, &edited.name),
        kind: changed(&original.kind, &edited.kind),
        homepage_url: changed(&original.homepage_url, &edited.homepage_url),
        description: changed(&original.description, &edited.description),
        requirements: changed(&original.requirements, &edited.requirements),
        toolchain: changed(&original.toolchain, &edited.toolchain),
        playability: changed(&original.playability, &edited.playability),
        declared_platforms: platforms_changed.then(|| edited.declared_platforms.clone()),
        ai_usage: changed(&original.ai_usage, &edited.ai_usage),
        ai_tools: tools_changed.then(|| edited.ai_tools.clone()),
    }
}

fn has_changes(e: &EditInput) -> bool {
    e.name.is_some()
        || e.kind.is_some()
        || e.homepage_url.is_some()
        || e.description.is_some()
        || e.requirements.is_some()
        || e.toolchain.is_some()
        || e.playability.is_some()
        || e.declared_platforms.is_some()
        || e.ai_usage.is_some()
        || e.ai_tools.is_some()
}

/// Splits stored tool names into canonical checkboxes and a free-text remainder.
fn split_tools(tools: &[String]) -> (Vec<String>, String) {
    let known = tools
        .iter()
        .filter(|t| AI_TOOLS.contains(&t.as_str()))
        .cloned()
        .collect();
    let other = tools
        .iter()
        .filter(|t| !AI_TOOLS.contains(&t.as_str()))
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    (known, other)
}

/// Repositories named "recomp" are recompilations; everything else is a decompilation port.
fn guess_kind(name: &str, repo_url: &str) -> &'static str {
    if format!("{name} {repo_url}")
        .to_lowercase()
        .contains("recomp")
    {
        PortKind::StaticRecomp.as_str()
    } else {
        PortKind::DecompPort.as_str()
    }
}

fn toggle(list: &mut Vec<String>, value: &str, on: bool) {
    list.retain(|v| v.as_str() != value);
    if on {
        list.push(value.to_string());
    }
}

fn opt(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn year_suffix(year: Option<i32>) -> String {
    year.map(|y| format!(" ({y})")).unwrap_or_default()
}

fn igdb_tag(choice: &GameChoice) -> &'static str {
    if choice.game_id.is_some() {
        ""
    } else {
        " · new game"
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    fn draft_with_name(name: &str) -> FormState {
        FormState {
            name: name.to_string(),
            ..FormState::default()
        }
    }

    #[test]
    fn blank_name_is_rejected() {
        let err = build_port(&draft_with_name("   "), "https://github.com/a/b").unwrap_err();
        assert_eq!(err, "Give the port a name.");
    }

    #[test]
    fn over_long_description_is_rejected() {
        let mut draft = draft_with_name("Port");
        draft.description = "x".repeat(DESCRIPTION_MAX + 1);
        assert!(build_port(&draft, "https://github.com/a/b").is_err());
    }

    #[test]
    fn requirements_limit_is_counted_in_characters() {
        let mut draft = draft_with_name("Port");
        draft.requirements = "é".repeat(REQUIREMENTS_MAX);
        assert!(build_port(&draft, "https://github.com/a/b").is_ok());
        draft.requirements.push('é');
        assert!(build_port(&draft, "https://github.com/a/b").is_err());
    }

    #[test]
    fn free_text_tools_are_appended_and_blank_authors_dropped() {
        let mut draft = draft_with_name("Port");
        draft.ai_tools = vec!["Claude".to_string()];
        draft.other_tool = "Aider, , Zed".to_string();
        draft.authors = vec![
            AuthorForm {
                github_login: None,
                name: String::new(),
                role: "credited".to_string(),
            },
            AuthorForm {
                github_login: Some("octo".to_string()),
                name: String::new(),
                role: "credited".to_string(),
            },
        ];
        let port = build_port(&draft, "https://github.com/a/b").unwrap();
        assert_eq!(port.ai_tools, vec!["Claude", "Aider", "Zed"]);
        assert_eq!(port.authors.len(), 1);
        assert_eq!(port.authors[0].name, "octo");
    }

    #[test]
    fn edit_sends_only_changed_fields() {
        let original = PortForm {
            name: "Port".to_string(),
            kind: "static_recomp".to_string(),
            source_platform_id: None,
            repo_url: "https://github.com/a/b".to_string(),
            homepage_url: None,
            description: Some("old".to_string()),
            requirements: None,
            toolchain: None,
            license: None,
            playability: "playable".to_string(),
            declared_platforms: vec!["windows".to_string(), "linux".to_string()],
            ai_usage: "none".to_string(),
            ai_tools: vec!["Claude".to_string()],
            authors: Vec::new(),
        };
        let mut edited = original.clone();
        edited.description = Some("new".to_string());
        edited.declared_platforms = vec!["linux".to_string(), "windows".to_string()];

        let input = edit_input(&original, &edited, 7);
        assert_eq!(input.port_id, 7);
        assert_eq!(input.description, Some(Some("new".to_string())));
        assert!(input.name.is_none());
        assert!(input.ai_tools.is_none());
        assert!(input.declared_platforms.is_none());
        assert!(has_changes(&input));
    }

    #[test]
    fn unchanged_form_has_no_changes() {
        let original = build_port(&draft_with_name("Port"), "https://github.com/a/b").unwrap();
        let input = edit_input(&original, &original.clone(), 1);
        assert!(!has_changes(&input));
    }

    #[test]
    fn unknown_tools_move_to_free_text() {
        let (known, other) = split_tools(&["Claude".to_string(), "Aider".to_string()]);
        assert_eq!(known, vec!["Claude"]);
        assert_eq!(other, "Aider");
    }

    #[test]
    fn recomp_repos_guess_static_recomp() {
        assert_eq!(
            guess_kind("Zelda Recomp", "https://github.com/a/b"),
            PortKind::StaticRecomp.as_str()
        );
        assert_eq!(
            guess_kind("Doom", "https://github.com/a/doom"),
            PortKind::DecompPort.as_str()
        );
    }
}
