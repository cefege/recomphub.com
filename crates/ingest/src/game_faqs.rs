//! Generated FAQs for game pages. `describe-games` builds one facts object per game from the
//! database, asks the enabled AI providers for a short FAQ written only from those facts, and
//! keeps the answer only if it passes [`validate`]. Anything else leaves the deterministic FAQ
//! on the page.

use std::collections::HashMap;
use std::collections::HashSet;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Datelike, Utc};
use recomphub_db::ai_providers::{self as ai, EnabledProvider};
use recomphub_db::comments;
use recomphub_db::game_faqs::{self, FaqAttempt, NewGameFaq};
use recomphub_db::games::{self, GameRow, PlatformRow};
use recomphub_db::port_descriptions;
use recomphub_db::ports::{self, PortRow};
use recomphub_db::secrets::Sealer;
use recomphub_shared::dto::FaqItem;
use recomphub_shared::{MaintenanceLevel, PortKind, RatingSummary, TargetPlatform};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::describe::{BLOCKED_TERMS, digest};
use crate::llm;

/// Generation calls per run. Games needing a FAQ wait for later runs.
pub const RUN_LIMIT: usize = 60;
/// A game whose last attempt failed on the same facts is not retried for this long.
const RETRY_AFTER: chrono::Duration = chrono::Duration::days(3);
/// Pause between generation calls, to stay within provider rate limits.
const CALL_SPACING: Duration = Duration::from_secs(1);
const MAX_TOKENS: u32 = 1536;
/// Bumping this regenerates every FAQ, for example after a change to the facts layout.
pub const PROMPT_VERSION: u32 = 1;
const TOOL_NAME: &str = "record_faq";
const TOOL_DESCRIPTION: &str = "Record the FAQ for one game as questions and answers.";

const MIN_ITEMS: usize = 3;
const MAX_ITEMS: usize = 7;
const QUESTION_MIN: usize = 10;
const QUESTION_MAX: usize = 120;
const ANSWER_MIN: usize = 20;
const ANSWER_MAX: usize = 400;
const MAX_SENTENCES: usize = 3;

/// Extra lowercase phrases a FAQ answer may not contain, beyond the ROM-site list.
const EXTRA_BLOCKED: &[&str] = &[
    "torrent", "pirat", "warez", "free rom", "rom site", "cracked",
];

/// Capitalised words that are fine to use even though the facts do not name them.
const STOPLIST: &[&str] = &[
    "I",
    "PC",
    "Mac",
    "macOS",
    "Windows",
    "Linux",
    "Android",
    "iOS",
    "iPhone",
    "iPad",
    "Web",
    "Switch",
    "Nintendo",
    "PlayStation",
    "Xbox",
    "Sony",
    "Microsoft",
    "Sega",
    "Wii",
    "PSP",
    "Vita",
    "DS",
    "3DS",
    "Steam",
    "Deck",
    "GOG",
    "GitHub",
    "Wine",
    "Proton",
    "CrossOver",
    "RecompHub",
    "ROM",
    "ROMs",
    "FAQ",
    "API",
    "Yes",
    "No",
    "Not",
    "The",
    "It",
    "This",
    "That",
    "If",
    "Do",
    "Does",
    "Is",
    "Are",
    "Can",
    "How",
    "What",
    "Which",
    "Who",
    "Where",
    "When",
    "You",
    "Your",
    "They",
    "There",
    "Some",
    "Most",
    "Many",
    "Each",
    "Both",
    "Also",
    "Start",
    "Just",
    "Only",
    "Still",
    "One",
    "Two",
    "Three",
    "Four",
    "Five",
];

const SYSTEM_PROMPT: &str = "\
You write the FAQ for one game page on RecompHub, a directory of fan projects that run classic \
games on modern systems: recompilations, source ports and decompilation ports. The readers are \
players deciding whether they can play this game and which version to use. You get the game's \
facts as one JSON object. Write 3 to 7 questions and answers a curious player would actually ask.

Rules:
- Use only the facts. Do not add anything from memory. Do not guess versions, dates, platforms, \
ratings or features the facts do not state. Skip a question the facts cannot answer properly.
- Write like a knowledgeable friend, not a form. Plain conversational English. Vary sentence \
shapes and openings. Never begin an answer with Yes, No, Not yet or Not natively.
- Each answer is 1 to 3 sentences. Do not put statistics in parentheses. Use a number only when \
it helps a reader decide, such as a rating, a release version or how many people asked for a port.
- Name projects exactly as the facts name them. Do not write URLs, and do not name any website, \
store or ROM source.
- Do not explain how to get game files, ROMs or keys. If the game's data comes up, say the player \
needs their own copy.
- No marketing words, no superlatives, no comparisons of projects with each other.
- Each question ends with a question mark, is under 120 characters, and no two questions are alike.

Good, for a made-up game \"Example Quest\" with one port \"Questkit\" (Windows, release v1.2, last \
commit 2024-03):
{\"question\": \"Is Questkit still being worked on?\", \"answer\": \"It is. The latest release is \
v1.2, and the code was last changed in March 2024.\"}
Bad:
{\"question\": \"Is there a Questkit?\", \"answer\": \"Yes. Example Quest runs natively on Windows \
through Questkit, a static recompilation (playable, latest release v1.2).\"}

Call the record_faq tool with your answer and nothing else.";

/// Facts about one game, as the model sees them. Built only from stored rows.
#[derive(Debug, Serialize)]
pub struct Facts {
    pub game: GameFacts,
    pub original_platforms: Vec<OriginalPlatform>,
    pub ports: Vec<ProjectFacts>,
    pub decompilations: Vec<ProjectFacts>,
}

#[derive(Debug, Serialize)]
pub struct GameFacts {
    pub name: String,
    pub alt_names: Vec<String>,
    pub release_year: Option<i32>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    /// Players who asked for a port of this game.
    pub wanted_by: i32,
}

#[derive(Debug, Serialize)]
pub struct OriginalPlatform {
    pub name: String,
    /// The player-facing platform the original release runs natively on, if RecompHub has one.
    pub native_target: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct ProjectFacts {
    pub name: String,
    pub kind: String,
    pub targets: Vec<&'static str>,
    pub playability: &'static str,
    pub release: Option<ReleaseFacts>,
    pub last_commit: Option<String>,
    pub activity: Option<&'static str>,
    pub github_stars: i32,
    pub rating: Option<RatingFacts>,
    pub authors: Vec<String>,
    pub summary: Option<String>,
    pub technique: Option<&'static str>,
    pub requirements: Vec<String>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ReleaseFacts {
    pub version: String,
    pub released: Option<String>,
    pub prerelease: bool,
}

#[derive(Debug, Serialize)]
pub struct RatingFacts {
    pub votes: i64,
    /// Average out of 5, to one decimal place.
    pub average: f64,
}

/// Outcome of one `describe-games` run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GameFaqReport {
    pub up_to_date: usize,
    pub generated: usize,
    pub failed: usize,
    pub held: usize,
    pub deferred: usize,
}

/// The tool input as the model returned it, before validation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FaqOutput {
    pub faq: Vec<QaOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct QaOutput {
    pub question: String,
    pub answer: String,
}

/// JSON Schema for the tool input. [`validate`] enforces the same limits again.
pub fn output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "faq": {
                "type": "array",
                "minItems": MIN_ITEMS,
                "maxItems": MAX_ITEMS,
                "items": {
                    "type": "object",
                    "properties": {
                        "question": {"type": "string", "maxLength": QUESTION_MAX},
                        "answer": {"type": "string", "maxLength": ANSWER_MAX},
                    },
                    "required": ["question", "answer"],
                    "additionalProperties": false,
                },
            },
        },
        "required": ["faq"],
        "additionalProperties": false,
    })
}

/// Hash of everything a FAQ is built from: the prompt, its version, and the facts. A changed
/// hash means the stored FAQ is stale.
pub fn facts_hash(facts_json: &str) -> String {
    digest(&[&PROMPT_VERSION.to_string(), SYSTEM_PROMPT, facts_json])
}

fn month(t: DateTime<Utc>) -> String {
    t.format("%Y-%m").to_string()
}

fn playability_phrase(p: &str) -> &'static str {
    match p {
        "complete" => "fully playable",
        "playable" => "playable",
        "in_progress" => "still in progress",
        _ => "not yet rated",
    }
}

fn original_platform(p: &PlatformRow) -> OriginalPlatform {
    OriginalPlatform {
        name: p.name.clone(),
        native_target: p
            .slug
            .as_deref()
            .and_then(TargetPlatform::from_platform_slug)
            .map(TargetPlatform::label),
    }
}

async fn project(pool: &PgPool, p: &PortRow) -> Result<ProjectFacts> {
    let authors = ports::authors(pool, p.id)
        .await
        .context("loading authors")?
        .into_iter()
        .filter_map(|a| {
            let name = a.name.trim();
            if !name.is_empty() {
                Some(name.to_string())
            } else {
                a.github_login
            }
        })
        .collect();
    let distribution = comments::rating_distribution(pool, p.id)
        .await
        .context("loading ratings")?;
    let summary = RatingSummary::from_distribution(distribution);
    let rating = (summary.count > 0).then(|| RatingFacts {
        votes: summary.count,
        average: ((summary.average.unwrap_or_default() * 10.0).round()) / 10.0,
    });
    let description = port_descriptions::get(pool, p.id)
        .await
        .context("loading port description")?;

    Ok(ProjectFacts {
        name: p.name.clone(),
        kind: PortKind::from_str(&p.kind)
            .map_or_else(|_| p.kind.clone(), |k| k.label().to_string()),
        targets: p
            .platforms()
            .into_iter()
            .map(TargetPlatform::label)
            .collect(),
        playability: playability_phrase(&p.playability),
        release: p.latest_version.as_ref().map(|version| ReleaseFacts {
            version: version.clone(),
            released: p.latest_release_at.map(month),
            prerelease: p.latest_is_prerelease == Some(true),
        }),
        last_commit: p.last_commit_at.map(month),
        activity: p
            .maintenance_level
            .as_deref()
            .and_then(MaintenanceLevel::from_key)
            .map(MaintenanceLevel::label),
        github_stars: p.github_stars,
        rating,
        authors,
        summary: description.as_ref().map(|d| d.summary.clone()),
        technique: description
            .as_ref()
            .map(|d| recomphub_shared::technique_label(&d.technique)),
        requirements: description
            .as_ref()
            .map(|d| d.requirements.clone())
            .unwrap_or_default(),
        limitations: description
            .as_ref()
            .map(|d| d.limitations.clone())
            .unwrap_or_default(),
    })
}

/// Everything the model may use for one game. Ported games come first in the output.
pub async fn facts(pool: &PgPool, game: &GameRow) -> Result<Facts> {
    let platforms = games::platforms_for_game(pool, game.id)
        .await
        .context("loading platforms")?;
    let ported = ports::list_for_game(pool, game.id)
        .await
        .context("loading ports")?;
    let decomps = ports::list_decompilations(pool, game.id)
        .await
        .context("loading decompilations")?;
    let mut ports_out = Vec::with_capacity(ported.len());
    for p in &ported {
        ports_out.push(project(pool, p).await?);
    }
    let mut decomp_out = Vec::with_capacity(decomps.len());
    for p in &decomps {
        decomp_out.push(project(pool, p).await?);
    }
    Ok(Facts {
        game: GameFacts {
            name: game.name.clone(),
            alt_names: game.alt_names.clone(),
            release_year: game.first_release_date.map(|d| d.year()),
            developer: game.developer.clone(),
            publisher: game.publisher.clone(),
            wanted_by: game.want_count,
        },
        original_platforms: platforms.iter().map(original_platform).collect(),
        ports: ports_out,
        decompilations: decomp_out,
    })
}

/// Splits text into runs of digits and dots, trimmed of stray dots, for number checks.
fn number_tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map(|t| t.trim_matches('.'))
        .filter(|t| t.chars().any(|c| c.is_ascii_digit()))
}

/// Checks the model's FAQ against the schema, the house rules and the facts. Any failure makes
/// the whole FAQ unusable; the caller then keeps the deterministic one.
pub fn validate(out: FaqOutput, facts_json: &str) -> Result<Vec<FaqItem>> {
    if !(MIN_ITEMS..=MAX_ITEMS).contains(&out.faq.len()) {
        bail!(
            "FAQ has {} items, expected {MIN_ITEMS} to {MAX_ITEMS}",
            out.faq.len()
        );
    }
    let fact_numbers: HashSet<&str> = number_tokens(facts_json).collect();
    let mut questions = HashSet::new();
    let mut openings = HashSet::new();
    let mut items = Vec::with_capacity(out.faq.len());

    for qa in out.faq {
        let question = qa.question.trim().to_string();
        let answer = qa.answer.trim().to_string();
        single_line(&question, "question")?;
        single_line(&answer, "answer")?;
        let q_len = question.chars().count();
        if !(QUESTION_MIN..=QUESTION_MAX).contains(&q_len) {
            bail!("question is {q_len} characters, expected {QUESTION_MIN} to {QUESTION_MAX}");
        }
        if !question.ends_with('?') {
            bail!("question does not end with a question mark: {question:?}");
        }
        let a_len = answer.chars().count();
        if !(ANSWER_MIN..=ANSWER_MAX).contains(&a_len) {
            bail!("answer is {a_len} characters, expected {ANSWER_MIN} to {ANSWER_MAX}");
        }
        let sentences = sentence_count(&answer);
        if sentences > MAX_SENTENCES {
            bail!("answer has {sentences} sentences, expected at most {MAX_SENTENCES}");
        }

        let combined = format!("{question} {answer}");
        let lower = combined.to_lowercase();
        if ["http", "www.", "://"].iter().any(|m| lower.contains(m)) {
            bail!("text contains a URL");
        }
        if let Some(term) = BLOCKED_TERMS
            .iter()
            .chain(EXTRA_BLOCKED)
            .find(|term| lower.contains(*term))
        {
            bail!("text mentions a ROM site or piracy ({term:?})");
        }

        let first = answer
            .split_whitespace()
            .next()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .unwrap_or_default();
        if matches!(first.as_str(), "yes" | "no" | "not") {
            bail!("answer opens with {first:?}");
        }
        let opening: String = answer
            .split_whitespace()
            .take(3)
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .collect::<Vec<_>>()
            .join(" ");
        if opening.split(' ').count() == 3 && !openings.insert(opening.clone()) {
            bail!("two answers open with {opening:?}");
        }
        if !questions.insert(question.to_lowercase()) {
            bail!("duplicate question {question:?}");
        }

        for name in capitalised_mid_sentence(&combined) {
            if !STOPLIST.contains(&name.as_str()) && !facts_json.contains(name.as_str()) {
                bail!("text names {name:?}, which the facts do not mention");
            }
        }
        for number in number_tokens(&combined) {
            if !fact_numbers.contains(number) {
                bail!("text states {number:?}, which the facts do not contain");
            }
        }

        items.push(FaqItem { question, answer });
    }

    Ok(items)
}

fn single_line(text: &str, field: &str) -> Result<()> {
    if text.contains(['\n', '\r']) {
        bail!("{field} spans several lines");
    }
    Ok(())
}

/// Sentence endings: `.`, `!` or `?` followed by whitespace or the end of the text. A dot inside
/// a version such as `v1.2` does not count.
fn sentence_count(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    chars
        .iter()
        .enumerate()
        .filter(|(i, c)| {
            matches!(c, '.' | '!' | '?') && chars.get(i + 1).is_none_or(|next| next.is_whitespace())
        })
        .count()
}

/// Words that start with a capital letter and could be invented project names, with punctuation
/// and possessives removed. Sentence-initial words count only when they look like names (a
/// capital, digit or hyphen after the first letter), so ordinary openers like "Nobody" pass.
fn capitalised_mid_sentence(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut sentence_start = true;
    for raw in text.split_whitespace() {
        let word = raw
            .trim_matches(|c: char| !c.is_alphanumeric())
            .trim_end_matches("'s")
            .trim_end_matches("’s");
        if word.is_empty() {
            continue;
        }
        let starts_upper = word.chars().next().is_some_and(char::is_uppercase);
        let name_like = word
            .chars()
            .skip(1)
            .any(|c| c.is_uppercase() || c.is_ascii_digit())
            || word.contains('-');
        if starts_upper && (!sentence_start || name_like) {
            out.push(word.to_string());
        }
        sentence_start = raw.ends_with(['.', '!', '?']);
    }
    out
}

/// Generates FAQs for games whose facts changed since their stored FAQ. At most `limit` calls
/// per run. Ported games come first, then by importance. Each game tries the enabled AI
/// providers in position order and keeps the first FAQ that validates.
pub async fn describe_games(
    pool: &PgPool,
    http: &reqwest::Client,
    sealer: &Sealer,
    limit: usize,
) -> Result<GameFaqReport> {
    let mut report = GameFaqReport::default();
    let pruned = game_faqs::prune_without_ports(pool)
        .await
        .context("removing FAQs for games without projects")?;
    if pruned > 0 {
        tracing::info!(pruned, "removed FAQs for games without projects");
    }
    let providers = ai::list_enabled(pool)
        .await
        .context("listing enabled AI providers")?;
    if providers.is_empty() {
        tracing::warn!("no enabled AI providers; describe-games has nothing to call");
        return Ok(report);
    }
    let stored = game_faqs::stored_hashes(pool)
        .await
        .context("loading stored FAQ hashes")?;
    let attempts: HashMap<i64, FaqAttempt> = game_faqs::attempts(pool)
        .await
        .context("loading FAQ attempts")?
        .into_iter()
        .map(|a| (a.game_id, a))
        .collect();
    let schema = output_schema();
    let now = Utc::now();
    let mut calls = 0usize;

    for id in game_faqs::candidate_ids(pool)
        .await
        .context("listing FAQ candidates")?
    {
        let game = games::get(pool, id).await.context("loading game")?;
        let facts = facts(pool, &game).await?;
        let facts_json = serde_json::to_string(&facts)?;
        let hash = facts_hash(&facts_json);
        if stored.get(&id).map(String::as_str) == Some(hash.as_str()) {
            report.up_to_date += 1;
            continue;
        }
        if let Some(a) = attempts.get(&id)
            && a.source_hash == hash
            && now - a.attempted_at < RETRY_AFTER
        {
            report.held += 1;
            continue;
        }
        if calls >= limit {
            report.deferred += 1;
            continue;
        }
        if calls > 0 {
            tokio::time::sleep(CALL_SPACING).await;
        }
        calls += 1;

        let user = user_message(&facts_json);
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
            async |p: &EnabledProvider| -> Result<Vec<FaqItem>> {
                let credential = llm::oauth::ensure_fresh(pool, http, sealer, p).await?;
                let reply = llm::complete(http, &p.kind, &p.model, &credential, &request).await?;
                let out: FaqOutput = serde_json::from_value(reply)
                    .context("model output does not match the schema")?;
                validate(out, &facts_json)
            },
        )
        .await;

        for (index, message) in &chain.failures {
            let provider = &providers[*index];
            tracing::warn!(game_id = id, provider = %provider.kind, error = %message, "AI provider failed");
            ai::record_failure(pool, provider.id, message).await?;
        }
        match chain.answer {
            Some((index, items)) => {
                let provider = &providers[index];
                ai::record_success(pool, provider.id).await?;
                game_faqs::save(
                    pool,
                    &NewGameFaq {
                        game_id: id,
                        items: &items,
                        source_hash: &hash,
                        provider: &provider.kind,
                        model: &provider.model,
                    },
                )
                .await?;
                report.generated += 1;
            }
            None => {
                let message = chain
                    .failures
                    .iter()
                    .map(|(index, error)| format!("{}: {error}", providers[*index].kind))
                    .collect::<Vec<_>>()
                    .join("; ");
                game_faqs::record_failure(pool, id, &hash, &message).await?;
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

fn user_message(facts_json: &str) -> String {
    format!("Write the FAQ for this game from its facts. Use only these facts.\n\n{facts_json}")
}

/// What a dry run prints: the prompt and schema a real run would send, for one game.
pub struct Preview {
    pub game_id: i64,
    pub system: &'static str,
    pub user: String,
    pub schema: Value,
}

/// The prompt for the named game, or for the most important game that has projects.
pub async fn preview(pool: &PgPool, slug: Option<&str>) -> Result<Option<Preview>> {
    let id = match slug {
        Some(slug) => {
            let game = games::get_by_slug(pool, slug)
                .await
                .context("loading game")?
                .with_context(|| format!("no game with slug {slug:?}"))?;
            game.id
        }
        None => match game_faqs::candidate_ids(pool).await?.first() {
            Some(id) => *id,
            None => return Ok(None),
        },
    };
    let game = games::get(pool, id).await.context("loading game")?;
    let facts = facts(pool, &game).await?;
    let facts_json = serde_json::to_string_pretty(&facts)?;
    Ok(Some(Preview {
        game_id: id,
        system: SYSTEM_PROMPT,
        user: user_message(&facts_json),
        schema: output_schema(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qa(question: &str, answer: &str) -> QaOutput {
        QaOutput {
            question: question.into(),
            answer: answer.into(),
        }
    }

    const FACTS: &str = r#"{"game":{"name":"Example Quest","release_year":2005},"ports":[{"name":"Questkit","release":{"version":"v1.2"},"github_stars":312}]}"#;

    fn good() -> FaqOutput {
        FaqOutput {
            faq: vec![
                qa(
                    "Can I play Example Quest on Windows?",
                    "Questkit covers Windows and is playable today. Builds come from the project's release page.",
                ),
                qa(
                    "Is Questkit still being worked on?",
                    "It is. The latest release is v1.2, and the code keeps moving.",
                ),
                qa(
                    "Do I need my own copy?",
                    "You do. The port reads the art and levels from a copy you already own.",
                ),
            ],
        }
    }

    #[test]
    fn accepts_a_grounded_faq() {
        let items = validate(good(), FACTS).unwrap();
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn rejects_robotic_openers() {
        let mut out = good();
        out.faq[0].answer = "Yes. Questkit covers Windows and is playable today.".into();
        assert!(validate(out, FACTS).is_err());
    }

    #[test]
    fn rejects_duplicate_openings() {
        let mut out = good();
        out.faq[1].answer = "Questkit covers Windows, and it keeps moving forward.".into();
        out.faq[0].answer = "Questkit covers Windows and is playable today.".into();
        assert!(validate(out, FACTS).is_err());
    }

    #[test]
    fn rejects_names_missing_from_facts() {
        let mut out = good();
        out.faq[0].answer = "NFSMW-Recompiled covers Windows and is playable today.".into();
        assert!(validate(out, FACTS).is_err());
    }

    #[test]
    fn rejects_numbers_missing_from_facts() {
        let mut out = good();
        out.faq[1].answer = "It is. The latest release is v9.9, and the code keeps moving.".into();
        assert!(validate(out, FACTS).is_err());
    }

    #[test]
    fn rejects_urls_and_rom_terms() {
        let mut out = good();
        out.faq[2].answer = "Grab it from https://example.com for the data.".into();
        assert!(validate(out, FACTS).is_err());
        let mut out = good();
        out.faq[2].answer = "You can find a free ROM of it online to use with the port.".into();
        assert!(validate(out, FACTS).is_err());
    }

    #[test]
    fn rejects_too_few_or_long_answers() {
        let mut out = good();
        out.faq.truncate(2);
        assert!(validate(out, FACTS).is_err());
        let mut out = good();
        out.faq[0].answer = "Short.".into();
        assert!(validate(out, FACTS).is_err());
    }

    #[test]
    fn version_dots_do_not_count_as_sentences() {
        assert_eq!(sentence_count("The release is v1.2 and it works."), 1);
        assert_eq!(sentence_count("One. Two. Three."), 3);
    }

    #[test]
    fn facts_hash_follows_the_facts() {
        assert_eq!(facts_hash(FACTS), facts_hash(FACTS));
        assert_ne!(facts_hash(FACTS), facts_hash("{}"));
    }
}
