//! Generated port descriptions. github-sync stores the repository text it already fetches
//! ([`record_sources`]). The worker's `describe-ports` run turns each port's stored text into a
//! short description through the admin's AI providers, tried in position order, and stores the
//! first answer that validates.
//!
//! The model sees only the repository text and the port's facts. [`validate`] enforces the shape of
//! the answer and rejects anything that names a ROM download site.

use std::fmt::Write as _;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use recomphub_db::ai_providers::{self as ai, EnabledProvider};
use recomphub_db::port_descriptions::{
    self as store, Candidate, NewPortDescription, SourceSnapshot,
};
use recomphub_db::secrets::Sealer;
use recomphub_shared::dto::PORT_TECHNIQUES;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::llm;

/// Ports generated per run. Later daily runs work through the rest of the queue.
pub const RUN_LIMIT: usize = 200;
/// Bumped when the prompt or the output schema changes, so every description regenerates.
pub const PROMPT_VERSION: u32 = 1;

const TOOL_NAME: &str = "record_description";
const TOOL_DESCRIPTION: &str = "Record the generated description of this port.";
const MAX_TOKENS: u32 = 1024;
const README_CHARS: usize = 12_000;
const RELEASE_CHARS: usize = 3_000;
const CALL_SPACING: Duration = Duration::from_secs(1);

const SUMMARY_MIN: usize = 20;
const SUMMARY_MAX: usize = 400;
const NOTE_MAX: usize = 240;
const ITEM_MAX: usize = 200;
const MAX_FEATURES: usize = 8;
const MAX_REQUIREMENTS: usize = 8;
const MAX_LIMITATIONS: usize = 6;
const MAX_SETUP: usize = 5;

/// Lowercase phrases that mark output naming a ROM download site or a request for one.
const BLOCKED_TERMS: &[&str] = &[
    "romsmania",
    "emuparadise",
    "coolrom",
    "wowroms",
    "romhustler",
    "rom download",
    "download rom",
    "roms download",
    "download roms",
];

const SYSTEM_PROMPT: &str = "\
You write short, factual descriptions of open-source projects that port classic games to modern \
systems. You get one project's facts and the text of its GitHub repository: description, topics, \
README and latest release notes. Describe what the project is and what it does for a player.

Rules:
- Use only the text you are given and the facts block. Do not add anything from memory. Do not \
guess versions, dates, platforms or features the text does not state.
- No marketing language, no superlatives, no comparisons with other ports.
- Do not explain how to obtain game data. If the player needs the original game's files, say they \
must use their own copy. Never name or link a website that hosts ROMs or pirated games.
- Leave out anything the text does not support. An empty list is correct when the text says nothing.

Fields:
- summary: one or two plain sentences in the present tense, 20 to 400 characters, on one line.
- technique: how the port was made. static_recompilation: the game binary is translated to native \
code. decompilation_port: the original source is reconstructed by decompilation, then ported. \
source_port: an engine's source is released or ported to run on new systems. \
engine_reimplementation: the engine is rewritten from scratch. emulator_wrapper: the original binary \
runs inside an emulator frontend. other: none of these fit.
- technique_note: one sentence, at most 240 characters, saying how this project applies that technique.
- features: up to 8 short phrases naming features or improvements the project states.
- requirements: up to 8 short phrases for what a player needs, such as the original game's files or \
a dependency.
- limitations: up to 6 short phrases for known problems or missing features the project states.
- setup: up to 5 short imperative steps taken from the README. Empty if the README gives no setup steps.

Call the record_description tool with your answer and nothing else.";

/// Repository text for one port, as the GitHub scan fetched it. Raw; cleaned by [`snapshot`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawSource {
    pub topics: Vec<String>,
    pub readme: Option<String>,
    pub release: RawRelease,
}

/// Latest release name and notes, as the GitHub scan fetched them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawRelease {
    pub name: Option<String>,
    pub body: Option<String>,
}

static HTML_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").expect("valid regex"));
static MARKDOWN_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").expect("valid regex"));
static MARKDOWN_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").expect("valid regex"));
static HTML_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"</?[A-Za-z][^>]*>").expect("valid regex"));
static BLANK_RUNS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").expect("valid regex"));

/// Reduces README or release markdown to plain prose for the model. Drops comments, images,
/// badges and HTML tags, keeps link text, and caps the result at `max_chars` characters.
pub fn clean_markdown(raw: &str, max_chars: usize) -> String {
    let text = HTML_COMMENT.replace_all(raw, "");
    let text = MARKDOWN_IMAGE.replace_all(&text, "");
    let text = MARKDOWN_LINK.replace_all(&text, "$1");
    let text = HTML_TAG.replace_all(&text, "");
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let joined = lines.join("\n");
    let text = BLANK_RUNS.replace_all(&joined, "\n\n");
    text.trim()
        .chars()
        .take(max_chars)
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn nonempty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Cleaned, capped text ready to store, with its content hash.
pub fn snapshot(description: Option<&str>, source: &RawSource) -> SourceSnapshot {
    let repo_description = nonempty(description);
    let topics: Vec<String> = source
        .topics
        .iter()
        .filter_map(|t| nonempty(Some(t.as_str())))
        .collect();
    let readme = source
        .readme
        .as_deref()
        .map(|r| clean_markdown(r, README_CHARS))
        .filter(|s| !s.is_empty());
    let release_name = nonempty(source.release.name.as_deref());
    let release_notes = source
        .release
        .body
        .as_deref()
        .map(|b| clean_markdown(b, RELEASE_CHARS))
        .filter(|s| !s.is_empty());
    let content_hash = digest(&[
        repo_description.as_deref().unwrap_or_default(),
        &topics.join("\n"),
        readme.as_deref().unwrap_or_default(),
        release_name.as_deref().unwrap_or_default(),
        release_notes.as_deref().unwrap_or_default(),
    ]);
    SourceSnapshot {
        repo_description,
        topics,
        readme,
        release_name,
        release_notes,
        content_hash,
    }
}

/// Length-prefixed SHA-256 over the parts, so different part lists never hash alike.
fn digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Hash of everything a description is built from: the stored text, the port's facts, and the
/// prompt version. A changed hash means the description is stale.
pub fn source_hash(c: &Candidate) -> String {
    let version = PROMPT_VERSION.to_string();
    let targets = c.game_targets.join(",");
    digest(&[
        &version,
        &c.content_hash,
        &c.port_name,
        &c.kind,
        &c.game_name,
        &targets,
        c.platform_name.as_deref().unwrap_or_default(),
        c.homepage_url.as_deref().unwrap_or_default(),
        c.license.as_deref().unwrap_or_default(),
    ])
}

/// Stores the cleaned repository text for a port. Called by github-sync after each scan.
pub async fn record_sources(
    pool: &PgPool,
    port_id: i64,
    description: Option<&str>,
    source: &RawSource,
) -> Result<()> {
    let snapshot = snapshot(description, source);
    store::upsert_sources(pool, port_id, &snapshot)
        .await
        .context("saving description sources")
}

fn has_text(s: &SourceSnapshot) -> bool {
    s.repo_description.is_some()
        || !s.topics.is_empty()
        || s.readme.is_some()
        || s.release_name.is_some()
        || s.release_notes.is_some()
}

/// The user turn: the port's facts, then the cleaned repository text.
pub fn user_message(c: &Candidate, s: &SourceSnapshot) -> String {
    let mut out = String::new();
    let targets = if c.game_targets.is_empty() {
        "unknown".to_string()
    } else {
        c.game_targets.join(", ")
    };
    let _ = writeln!(out, "## Facts");
    let _ = writeln!(out, "- Port: {}", c.port_name);
    let _ = writeln!(out, "- Game: {}", c.game_name);
    let _ = writeln!(out, "- Port kind: {}", c.kind);
    let _ = writeln!(
        out,
        "- Original platform: {}",
        c.platform_name.as_deref().unwrap_or("unknown")
    );
    let _ = writeln!(out, "- Targets: {targets}");
    let _ = writeln!(
        out,
        "- Homepage: {}",
        c.homepage_url.as_deref().unwrap_or("none")
    );
    let _ = writeln!(
        out,
        "- License: {}",
        c.license.as_deref().unwrap_or("unknown")
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "## Repository");
    let _ = writeln!(
        out,
        "Description: {}",
        s.repo_description.as_deref().unwrap_or("(none)")
    );
    let topics = if s.topics.is_empty() {
        "(none)".to_string()
    } else {
        s.topics.join(", ")
    };
    let _ = writeln!(out, "Topics: {topics}");
    let _ = writeln!(out);
    let _ = writeln!(out, "## README");
    let _ = writeln!(out, "{}", s.readme.as_deref().unwrap_or("(none)"));
    let _ = writeln!(out);
    let _ = writeln!(out, "## Latest release");
    let _ = writeln!(
        out,
        "Name: {}",
        s.release_name.as_deref().unwrap_or("(none)")
    );
    let _ = writeln!(
        out,
        "{}",
        s.release_notes.as_deref().unwrap_or("(no release notes)")
    );
    out
}

/// JSON Schema for the record the model fills in. The length limits here are advisory for the
/// provider; [`validate`] enforces them again.
pub fn output_schema() -> Value {
    let techniques: Vec<&str> = PORT_TECHNIQUES.iter().map(|(slug, _)| *slug).collect();
    let list = |max: usize| {
        json!({
            "type": "array",
            "maxItems": max,
            "items": {"type": "string", "maxLength": ITEM_MAX},
        })
    };
    json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string", "minLength": SUMMARY_MIN, "maxLength": SUMMARY_MAX},
            "technique": {"type": "string", "enum": techniques},
            "technique_note": {"type": "string", "maxLength": NOTE_MAX},
            "features": list(MAX_FEATURES),
            "requirements": list(MAX_REQUIREMENTS),
            "limitations": list(MAX_LIMITATIONS),
            "setup": list(MAX_SETUP),
        },
        "required": [
            "summary", "technique", "technique_note", "features",
            "requirements", "limitations", "setup"
        ],
        "additionalProperties": false,
    })
}

/// The tool input as the model returned it, before validation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DescriptionOutput {
    pub summary: String,
    pub technique: String,
    pub technique_note: String,
    pub features: Vec<String>,
    pub requirements: Vec<String>,
    pub limitations: Vec<String>,
    pub setup: Vec<String>,
}

fn single_line(text: &str, field: &str, min: usize, max: usize) -> Result<String> {
    let trimmed = text.trim();
    if trimmed.contains(['\n', '\r']) {
        bail!("{field} spans several lines");
    }
    let len = trimmed.chars().count();
    if len < min || len > max {
        bail!("{field} is {len} characters, expected {min} to {max}");
    }
    Ok(trimmed.to_string())
}

/// Trims each item, drops empty ones, keeps at most `max`, and rejects overlong items.
fn clean_list(field: &str, items: Vec<String>, max: usize) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for item in items {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push(single_line(trimmed, field, 1, ITEM_MAX)?);
        if out.len() == max {
            break;
        }
    }
    Ok(out)
}

/// Checks the model's answer against the schema and the ROM rule, and builds the row to store.
pub fn validate(
    out: DescriptionOutput,
    port_id: i64,
    provider: &str,
    model: &str,
    source_hash: &str,
) -> Result<NewPortDescription> {
    let summary = single_line(&out.summary, "summary", SUMMARY_MIN, SUMMARY_MAX)?;
    let technique = out.technique.trim().to_string();
    if !PORT_TECHNIQUES.iter().any(|(slug, _)| *slug == technique) {
        bail!("unknown technique {technique:?}");
    }
    let technique_note = single_line(&out.technique_note, "technique_note", 1, NOTE_MAX)?;
    let features = clean_list("features", out.features, MAX_FEATURES)?;
    let requirements = clean_list("requirements", out.requirements, MAX_REQUIREMENTS)?;
    let limitations = clean_list("limitations", out.limitations, MAX_LIMITATIONS)?;
    let setup = clean_list("setup", out.setup, MAX_SETUP)?;

    let all_text = std::iter::once(summary.as_str())
        .chain(std::iter::once(technique_note.as_str()))
        .chain(features.iter().map(String::as_str))
        .chain(requirements.iter().map(String::as_str))
        .chain(limitations.iter().map(String::as_str))
        .chain(setup.iter().map(String::as_str));
    for text in all_text {
        let lower = text.to_lowercase();
        if let Some(term) = BLOCKED_TERMS.iter().find(|term| lower.contains(*term)) {
            bail!("output mentions a ROM download site or request ({term:?})");
        }
    }

    Ok(NewPortDescription {
        port_id,
        summary,
        technique,
        technique_note,
        features,
        requirements,
        limitations,
        setup,
        provider: provider.to_string(),
        model: model.to_string(),
        source_hash: source_hash.to_string(),
    })
}

/// Outcome of one `describe-ports` run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DescribeReport {
    pub up_to_date: usize,
    pub generated: usize,
    pub failed: usize,
    pub without_text: usize,
    pub deferred: usize,
}

/// Generates descriptions for ports whose stored text or facts changed since their last one.
/// At most `limit` ports per run. Least recently attempted ports go first. Each port tries the
/// enabled AI providers in position order and keeps the first valid description.
pub async fn describe_ports(
    pool: &PgPool,
    http: &reqwest::Client,
    sealer: &Sealer,
    limit: usize,
) -> Result<DescribeReport> {
    let mut report = DescribeReport::default();
    let providers = ai::list_enabled(pool)
        .await
        .context("listing enabled AI providers")?;
    if providers.is_empty() {
        tracing::warn!("no enabled AI providers; describe-ports has nothing to call");
        return Ok(report);
    }
    let schema = output_schema();
    let mut calls = 0usize;
    for c in store::candidates(pool)
        .await
        .context("listing port description candidates")?
    {
        if c.source_hash.as_deref() == Some(source_hash(&c).as_str()) {
            report.up_to_date += 1;
            continue;
        }
        if calls >= limit {
            report.deferred += 1;
            continue;
        }
        let Some(snapshot) = store::get_sources(pool, c.port_id).await? else {
            continue;
        };
        if !has_text(&snapshot) {
            store::record_attempt(pool, c.port_id, Some("no repository text to describe")).await?;
            report.without_text += 1;
            continue;
        }
        if calls > 0 {
            tokio::time::sleep(CALL_SPACING).await;
        }
        calls += 1;

        let hash = source_hash(&c);
        let user = user_message(&c, &snapshot);
        let request = llm::Request {
            system: SYSTEM_PROMPT,
            user: &user,
            tool_name: TOOL_NAME,
            tool_description: TOOL_DESCRIPTION,
            schema: &schema,
            max_tokens: MAX_TOKENS,
        };
        let chain = llm::run_chain(
            &providers,
            async |p: &EnabledProvider| -> Result<NewPortDescription> {
                let credential = llm::oauth::ensure_fresh(pool, http, sealer, p).await?;
                let reply = llm::complete(http, &p.kind, &p.model, &credential, &request).await?;
                let out: DescriptionOutput = serde_json::from_value(reply)
                    .context("model output does not match the schema")?;
                validate(out, c.port_id, &p.kind, &p.model, &hash)
            },
        )
        .await;

        for (index, message) in &chain.failures {
            let provider = &providers[*index];
            tracing::warn!(port_id = c.port_id, provider = %provider.kind, error = %message, "AI provider failed");
            ai::record_failure(pool, provider.id, message).await?;
        }
        match chain.answer {
            Some((index, description)) => {
                ai::record_success(pool, providers[index].id).await?;
                store::save(pool, &description).await?;
                store::record_attempt(pool, c.port_id, None).await?;
                report.generated += 1;
            }
            None => {
                let message = chain
                    .failures
                    .iter()
                    .map(|(index, error)| format!("{}: {error}", providers[*index].kind))
                    .collect::<Vec<_>>()
                    .join("; ");
                store::record_attempt(pool, c.port_id, Some(message.as_str())).await?;
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

/// The system prompt and user message the next run would send for the first port that needs a
/// description, with its port id. Nothing is called or written.
pub async fn preview_prompt(pool: &PgPool) -> Result<Option<(i64, String, String)>> {
    for c in store::candidates(pool).await? {
        if c.source_hash.as_deref() == Some(source_hash(&c).as_str()) {
            continue;
        }
        let Some(snapshot) = store::get_sources(pool, c.port_id).await? else {
            continue;
        };
        if has_text(&snapshot) {
            let user = user_message(&c, &snapshot);
            return Ok(Some((c.port_id, SYSTEM_PROMPT.to_string(), user)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> Candidate {
        Candidate {
            port_id: 7,
            content_hash: "abc".into(),
            port_name: "Sonic Port".into(),
            kind: "source_port".into(),
            game_name: "Sonic".into(),
            game_targets: vec!["linux".into(), "windows".into()],
            platform_name: Some("Mega Drive".into()),
            source_hash: None,
            describe_attempted_at: None,
            homepage_url: None,
            license: Some("MIT".into()),
        }
    }

    fn good_output() -> DescriptionOutput {
        DescriptionOutput {
            summary: "A source port of the original game that runs natively on modern systems."
                .into(),
            technique: "source_port".into(),
            technique_note: "The engine source is compiled for each target platform.".into(),
            features: vec!["Widescreen support".into(), "  ".into()],
            requirements: vec!["A copy of the original game's data files".into()],
            limitations: vec![],
            setup: vec!["Copy your own game data next to the executable.".into()],
        }
    }

    #[test]
    fn clean_markdown_drops_comments_images_badges_and_tags() {
        let raw = "<!-- hidden -->\n# Title\n[![Build](https://x/badge.svg)](https://x/ci)\n\
                   ![shot](img.png)\n<p align=\"center\">Hi</p>\nSee [the docs](https://d.example).\n\n\n\nEnd";
        let cleaned = clean_markdown(raw, 1000);
        assert_eq!(cleaned, "# Title\n\nHi\nSee the docs.\n\nEnd");
    }

    #[test]
    fn clean_markdown_caps_on_character_boundary() {
        let raw = "é".repeat(20);
        assert_eq!(clean_markdown(&raw, 5), "ééééé");
    }

    #[test]
    fn source_hash_tracks_text_and_facts() {
        let base = candidate();
        let hash = source_hash(&base);
        assert_eq!(hash, source_hash(&candidate()));

        let mut retitled = candidate();
        retitled.port_name = "Other".into();
        assert_ne!(hash, source_hash(&retitled));

        let mut new_text = candidate();
        new_text.content_hash = "def".into();
        assert_ne!(hash, source_hash(&new_text));
    }

    #[test]
    fn snapshot_without_text_has_no_text() {
        let s = snapshot(Some("   "), &RawSource::default());
        assert!(!has_text(&s));
        let with = snapshot(Some("A port"), &RawSource::default());
        assert!(has_text(&with));
    }

    #[test]
    fn validate_accepts_well_formed_output() {
        let row = validate(
            good_output(),
            7,
            "anthropic_api",
            "claude-haiku-5-5",
            "hash",
        )
        .expect("valid");
        assert_eq!(row.features, vec!["Widescreen support".to_string()]);
        assert_eq!(row.technique, "source_port");
        assert!(row.limitations.is_empty());
        assert_eq!(row.provider, "anthropic_api");
    }

    #[test]
    fn validate_rejects_rom_sites() {
        let mut out = good_output();
        out.setup = vec!["Download the ROM from RomsMania.".into()];
        let err = validate(out, 7, "anthropic_api", "m", "h").unwrap_err();
        assert!(format!("{err:#}").contains("ROM download site"));
    }

    #[test]
    fn validate_rejects_bad_shape() {
        let mut short = good_output();
        short.summary = "Too short".into();
        assert!(validate(short, 7, "anthropic_api", "m", "h").is_err());

        let mut multi = good_output();
        multi.summary = "A port\nof a game that runs on modern systems.".into();
        assert!(validate(multi, 7, "anthropic_api", "m", "h").is_err());

        let mut unknown = good_output();
        unknown.technique = "magic".into();
        assert!(validate(unknown, 7, "anthropic_api", "m", "h").is_err());
    }

    #[test]
    fn output_schema_requires_every_field_and_keeps_the_technique_list() {
        let schema = output_schema();
        assert_eq!(schema["required"].as_array().map(Vec::len), Some(7));
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["properties"]["technique"]["enum"]
                .as_array()
                .map(Vec::len),
            Some(PORT_TECHNIQUES.len())
        );
    }
}
