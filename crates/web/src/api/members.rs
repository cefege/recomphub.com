//! Viewer-facing server functions: session identity, marks, stars, wants and reports.

use dioxus::prelude::*;
use recomphub_shared::dto::{
    EditInput, EditPortForm, Marks, RepoPreview, SessionUser, StarStateDto, SubmitInput,
    SubmitResult, WantStateDto,
};

#[cfg(feature = "server")]
use recomphub_shared::dto::MediaRef;

#[cfg(feature = "server")]
use crate::api::moderation::{igdb_client, igdb_error};
#[cfg(feature = "server")]
use crate::server::session::{current_user_row, db_error, http_error, require_user, session_user};
#[cfg(feature = "server")]
use crate::server::state::pool;
#[cfg(feature = "server")]
use dioxus::server::axum::http::StatusCode;
#[cfg(feature = "server")]
use recomphub_db::PgPool;
#[cfg(feature = "server")]
use recomphub_db::games::{self, IgdbGame};
#[cfg(feature = "server")]
use recomphub_db::media::MediaRow;
#[cfg(feature = "server")]
use recomphub_db::ports::{self, AuthorInput, PortFull, PortInput, PortRow};
#[cfg(feature = "server")]
use recomphub_db::submissions::NewSubmission;
#[cfg(feature = "server")]
use recomphub_db::submissions::PortSubmission;
#[cfg(feature = "server")]
use recomphub_ingest::github::author_specs;
#[cfg(feature = "server")]
use recomphub_ingest::igdb::ensure_platforms;
#[cfg(feature = "server")]
use recomphub_shared::dto::{AiEvidenceDto, AuthorDto, AuthorForm, PortForm, RepoPreviewData};
#[cfg(feature = "server")]
use recomphub_shared::enums::SubmissionKind;
#[cfg(feature = "server")]
use serde_json::{Map, Value, json};

#[cfg(feature = "server")]
fn sql_error(e: sqlx::Error) -> ServerFnError {
    db_error(recomphub_db::DbError::Sql(e))
}

#[cfg(feature = "server")]
pub(crate) fn media_ref(m: &MediaRow) -> MediaRef {
    MediaRef {
        id: m.id,
        kind: m.kind.clone(),
        hash: m.hash.clone(),
        widths: m.widths.clone(),
        width: m.width,
        height: m.height,
        ready: m.status == "ready",
    }
}

/// The signed-in viewer, or `None` when signed out.
#[get("/api/me")]
pub async fn me() -> Result<Option<SessionUser>, ServerFnError> {
    let Some(row) = current_user_row().await? else {
        return Ok(None);
    };
    let pool = pool()?;
    let avatar = match row.avatar_media_id {
        Some(id) => recomphub_db::media::get(pool, id)
            .await
            .map_err(db_error)?
            .map(|m| media_ref(&m)),
        None => None,
    };
    Ok(Some(session_user(&row, avatar)))
}

/// Which of the given ports and games the signed-in viewer has starred or wanted.
/// A signed-out viewer has no marks, so the result is empty.
#[post("/api/marks")]
pub async fn my_marks(port_ids: Vec<i64>, game_ids: Vec<i64>) -> Result<Marks, ServerFnError> {
    let Some(row) = current_user_row().await? else {
        return Ok(Marks::default());
    };
    let pool = pool()?;
    let mut marks = Marks::default();
    if !port_ids.is_empty() {
        marks.starred = sqlx::query_scalar::<_, i64>(
            "SELECT port_id FROM port_stars WHERE user_id = $1 AND port_id = ANY($2)",
        )
        .bind(row.id)
        .bind(&port_ids)
        .fetch_all(pool)
        .await
        .map_err(sql_error)?;
    }
    if !game_ids.is_empty() {
        marks.wanted = sqlx::query_scalar::<_, i64>(
            "SELECT game_id FROM game_wants WHERE user_id = $1 AND game_id = ANY($2)",
        )
        .bind(row.id)
        .bind(&game_ids)
        .fetch_all(pool)
        .await
        .map_err(sql_error)?;
    }
    Ok(marks)
}

/// Toggle the viewer's star on a published port. The db layer writes the activity row.
#[post("/api/stars/toggle")]
pub async fn toggle_star(port_id: i64) -> Result<StarStateDto, ServerFnError> {
    let user = require_user().await?;
    let state = recomphub_db::stars::toggle_star(pool()?, port_id, user.id)
        .await
        .map_err(db_error)?;
    Ok(StarStateDto {
        starred: state.starred,
        count: state.count,
    })
}

/// Toggle the viewer's want on a portless game. The db layer writes the activity row.
#[post("/api/wants/toggle")]
pub async fn toggle_want(game_id: i64) -> Result<WantStateDto, ServerFnError> {
    let user = require_user().await?;
    let state = recomphub_db::stars::toggle_want(pool()?, game_id, user.id)
        .await
        .map_err(db_error)?;
    Ok(WantStateDto {
        wanted: state.wanted,
        count: state.count,
    })
}

/// File a problem report against a game, optionally about one of its ports.
#[post("/api/reports")]
pub async fn submit_report(
    game_id: i64,
    port_id: Option<i64>,
    text: String,
) -> Result<(), ServerFnError> {
    let user = require_user().await?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(http_error(
            StatusCode::BAD_REQUEST,
            "Describe the problem before sending.",
        ));
    }
    if text.chars().count() > 1000 {
        return Err(http_error(
            StatusCode::BAD_REQUEST,
            "Reports are limited to 1000 characters.",
        ));
    }
    let pool = pool()?;
    recomphub_db::games::get(pool, game_id)
        .await
        .map_err(db_error)?;
    if let Some(port_id) = port_id {
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM ports WHERE id = $1 AND game_id = $2)",
        )
        .bind(port_id)
        .bind(game_id)
        .fetch_one(pool)
        .await
        .map_err(sql_error)?;
        if !owned {
            return Err(http_error(
                StatusCode::NOT_FOUND,
                "That port is not listed under this game.",
            ));
        }
    }
    recomphub_db::submissions::create(
        pool,
        &NewSubmission {
            kind: SubmissionKind::Report,
            source: "user".to_string(),
            submitter_id: Some(user.id),
            payload: serde_json::json!({ "text": text }),
            target_game_id: Some(game_id),
            target_port_id: port_id,
        },
    )
    .await
    .map_err(db_error)?;
    Ok(())
}

/// Trims text and drops it when blank.
#[cfg(feature = "server")]
fn blank_to_none(text: Option<String>) -> Option<String> {
    text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// Converts the submit form into editable port input. The repo URL is normalized, and a declared
/// AI usage is recorded with source `declared`.
#[cfg(feature = "server")]
fn port_from_form(form: PortForm) -> Result<PortFull, ServerFnError> {
    let repo_url = ports::normalize_repo_url(&form.repo_url).map_err(db_error)?;
    let ai_source = (form.ai_usage != "unknown").then(|| "declared".to_string());
    Ok(PortFull {
        input: PortInput {
            name: form.name.trim().to_string(),
            slug: None,
            kind: form.kind,
            source_platform_id: form.source_platform_id,
            repo_url,
            homepage_url: blank_to_none(form.homepage_url),
            description: blank_to_none(form.description),
            import_description: None,
            requirements: blank_to_none(form.requirements),
            toolchain: blank_to_none(form.toolchain),
            license: blank_to_none(form.license),
            playability: form.playability,
            declared_platforms: form.declared_platforms,
            detected_platforms: Vec::new(),
            ai_usage: form.ai_usage,
            ai_tools: form.ai_tools,
            ai_source,
            ai_evidence: Vec::new(),
        },
        authors: form
            .authors
            .into_iter()
            .map(|a| AuthorInput {
                github_login: blank_to_none(a.github_login),
                name: a.name.trim().to_string(),
                avatar_media_id: None,
                role: a.role,
                commits: None,
            })
            .collect(),
    })
}

/// Converts stored port data into the editable form.
#[cfg(feature = "server")]
fn port_form_from(full: PortFull) -> PortForm {
    let i = full.input;
    PortForm {
        name: i.name,
        kind: i.kind,
        source_platform_id: i.source_platform_id,
        repo_url: i.repo_url,
        homepage_url: i.homepage_url,
        description: i.description,
        requirements: i.requirements,
        toolchain: i.toolchain,
        license: i.license,
        playability: i.playability,
        declared_platforms: i.declared_platforms,
        ai_usage: i.ai_usage,
        ai_tools: i.ai_tools,
        authors: full
            .authors
            .into_iter()
            .map(|a| AuthorForm {
                github_login: a.github_login,
                name: a.name,
                role: a.role,
            })
            .collect(),
    }
}

/// True when `github_login` owns the repo or is listed among the port's authors.
/// Display names never count: they are not verified against GitHub.
#[cfg(feature = "server")]
fn is_author_of(github_login: Option<&str>, port: &PortFull) -> bool {
    let Some(login) = github_login else {
        return false;
    };
    let owner = port
        .input
        .repo_url
        .trim_start_matches("https://github.com/")
        .split('/')
        .next()
        .unwrap_or("");
    owner.eq_ignore_ascii_case(login)
        || port.authors.iter().any(|a| {
            a.github_login
                .as_deref()
                .is_some_and(|l| l.eq_ignore_ascii_case(login))
        })
}

/// Resolves the game a new port belongs to: an existing game, or an IGDB game to create.
#[cfg(feature = "server")]
async fn resolve_game(
    pool: &PgPool,
    game_id: Option<i64>,
    igdb_id: Option<i64>,
) -> Result<(Option<i64>, Option<IgdbGame>), ServerFnError> {
    if let Some(id) = game_id {
        games::get(pool, id).await.map_err(db_error)?;
        return Ok((Some(id), None));
    }
    let Some(igdb_id) = igdb_id else {
        return Err(http_error(
            StatusCode::BAD_REQUEST,
            "Choose a game or search all games for one.",
        ));
    };
    let client = igdb_client()?;
    let game = client
        .fetch_game(igdb_id)
        .await
        .map_err(igdb_error)?
        .ok_or_else(|| {
            http_error(
                StatusCode::NOT_FOUND,
                format!("No game found with id {igdb_id}."),
            )
        })?;
    ensure_platforms(pool, &client, &game.platform_igdb_ids)
        .await
        .map_err(igdb_error)?;
    Ok((None, Some(game)))
}

/// Game and port slugs of a port that was published by a submission.
#[cfg(feature = "server")]
async fn slugs_for(pool: &PgPool, port: &PortRow) -> Result<(String, String), ServerFnError> {
    let game = games::get(pool, port.game_id).await.map_err(db_error)?;
    Ok((game.slug, port.slug.clone()))
}

/// Splits a normalized `https://github.com/{owner}/{repo}` URL.
#[cfg(feature = "server")]
fn split_repo(repo_url: &str) -> Result<(&str, &str), ServerFnError> {
    repo_url
        .strip_prefix("https://github.com/")
        .and_then(|path| path.split_once('/'))
        .ok_or_else(|| http_error(StatusCode::BAD_REQUEST, "Not a GitHub repository URL."))
}

/// Builds the changed-fields object for an edit. Fields left out of the input are not changed.
#[cfg(feature = "server")]
fn edit_changes(input: &EditInput) -> Map<String, Value> {
    let mut m = Map::new();
    if let Some(name) = &input.name {
        m.insert("name".into(), json!(name.trim()));
    }
    if let Some(kind) = &input.kind {
        m.insert("kind".into(), json!(kind));
    }
    for (key, value) in [
        ("homepage_url", &input.homepage_url),
        ("description", &input.description),
        ("requirements", &input.requirements),
        ("toolchain", &input.toolchain),
    ] {
        if let Some(v) = value {
            let text = v
                .clone()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty());
            m.insert(key.into(), json!(text));
        }
    }
    if let Some(p) = &input.playability {
        m.insert("playability".into(), json!(p));
    }
    if let Some(d) = &input.declared_platforms {
        m.insert("declared_platforms".into(), json!(d));
    }
    if let Some(u) = &input.ai_usage {
        let source = if u == "unknown" {
            Value::Null
        } else {
            json!("declared")
        };
        m.insert("ai_usage".into(), json!(u));
        m.insert("ai_source".into(), source);
    }
    if let Some(t) = &input.ai_tools {
        m.insert("ai_tools".into(), json!(t));
    }
    m
}

/// Looks up a GitHub repo for the submit form. A repo that is already published returns its
/// page slugs. Otherwise the repo is read from GitHub, with the viewer's authorship marked.
#[post("/api/preview_repo")]
pub async fn preview_repo(url: String) -> Result<RepoPreview, ServerFnError> {
    let viewer = current_user_row().await?;
    let pool = pool()?;
    let repo_url = ports::normalize_repo_url(&url).map_err(db_error)?;
    if let Some(row) = ports::find_by_repo_url(pool, &repo_url)
        .await
        .map_err(db_error)?
    {
        if row.status == "published" {
            let game = games::get(pool, row.game_id).await.map_err(db_error)?;
            return Ok(RepoPreview::AlreadyListed {
                game_slug: game.slug,
                port_slug: row.slug,
            });
        }
        return Err(http_error(
            StatusCode::CONFLICT,
            "This repository is listed but hidden. A moderator can restore it.",
        ));
    }
    let (owner, name) = split_repo(&repo_url)?;
    let token = crate::server::config::get().github_token.as_deref();
    let prev = recomphub_ingest::github::preview(token, owner, name)
        .await
        .map_err(|e| {
            http_error(
                StatusCode::BAD_GATEWAY,
                format!("GitHub request failed: {e}"),
            )
        })?;
    let authors: Vec<AuthorDto> = author_specs(&prev)
        .into_iter()
        .map(|a| AuthorDto {
            github_login: Some(a.login),
            name: a.name,
            role: a.role.to_string(),
            avatar: None,
            commits: a.commits,
            funding: Vec::new(),
        })
        .collect();
    let submitter_is_author = viewer.as_ref().is_some_and(|v| {
        v.github_login.as_deref().is_some_and(|login| {
            login.eq_ignore_ascii_case(&prev.owner)
                || authors.iter().any(|a| {
                    a.github_login
                        .as_deref()
                        .is_some_and(|l| l.eq_ignore_ascii_case(login))
                })
        })
    });
    let ai = &prev.ai;
    Ok(RepoPreview::Preview(Box::new(RepoPreviewData {
        repo_url: prev.repo_url.clone(),
        owner: prev.owner.clone(),
        name: prev.name.clone(),
        description: prev.description.clone(),
        homepage_url: prev.homepage_url.clone(),
        license: prev.license.clone(),
        detected_platforms: prev
            .detected_platforms
            .iter()
            .map(|p| p.as_str().to_string())
            .collect(),
        ai_usage: ai.usage.as_ref().map(|u| u.as_str().to_string()),
        ai_tools: ai.tools.clone(),
        ai_evidence: ai
            .evidence
            .iter()
            .map(|e| AiEvidenceDto {
                signal: e.signal.clone(),
                detail: e.detail.clone(),
            })
            .collect(),
        contributors: authors,
        owner_is_org: prev.owner_is_org,
        submitter_is_author,
        stars: prev.stars,
        latest_version: prev.latest_release_tag.clone(),
    })))
}

/// Submits a new port for review. Trusted submitters' ports are published at once.
#[post("/api/submit")]
pub async fn submit(input: SubmitInput) -> Result<SubmitResult, ServerFnError> {
    let viewer = require_user().await?;
    let pool = pool()?;
    let SubmitInput {
        game_id,
        igdb_id,
        port,
        ..
    } = input;
    let (game_id, new_game) = resolve_game(pool, game_id, igdb_id).await?;
    let full = port_from_form(port)?;
    let submitter_is_author = is_author_of(viewer.github_login.as_deref(), &full);
    let repo_url = full.input.repo_url.clone();
    let ps = PortSubmission {
        game_id,
        new_game,
        port: full,
    };
    let mut payload = serde_json::to_value(&ps)
        .map_err(|e| http_error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    payload["submitter_is_author"] = json!(submitter_is_author);
    let sub = recomphub_db::submissions::create(
        pool,
        &NewSubmission {
            kind: SubmissionKind::NewPort,
            source: "user".to_string(),
            submitter_id: Some(viewer.id),
            payload,
            target_game_id: game_id,
            target_port_id: None,
        },
    )
    .await
    .map_err(db_error)?;
    let published = sub.status == "auto_approved";
    let (game_slug, port_slug) = if published {
        match ports::find_by_repo_url(pool, &repo_url)
            .await
            .map_err(db_error)?
        {
            Some(row) => {
                let (g, p) = slugs_for(pool, &row).await?;
                (Some(g), Some(p))
            }
            None => (None, None),
        }
    } else {
        (None, None)
    };
    Ok(SubmitResult {
        submission_id: sub.id,
        published,
        game_slug,
        port_slug,
    })
}

/// Submits changes to an existing port. Only the fields present in the input are changed.
#[post("/api/submit/edit")]
pub async fn submit_edit(input: EditInput) -> Result<SubmitResult, ServerFnError> {
    let viewer = require_user().await?;
    let pool = pool()?;
    let current = ports::get_row(pool, input.port_id)
        .await
        .map_err(db_error)?;
    let changes = edit_changes(&input);
    if changes.is_empty() {
        return Err(http_error(StatusCode::BAD_REQUEST, "Nothing was changed."));
    }
    let sub = recomphub_db::submissions::create(
        pool,
        &NewSubmission {
            kind: SubmissionKind::EditPort,
            source: "user".to_string(),
            submitter_id: Some(viewer.id),
            payload: Value::Object(changes),
            target_game_id: Some(current.game_id),
            target_port_id: Some(current.id),
        },
    )
    .await
    .map_err(db_error)?;
    let published = sub.status == "auto_approved";
    let (game_slug, port_slug) = if published {
        let (g, p) = slugs_for(pool, &current).await?;
        (Some(g), Some(p))
    } else {
        (None, None)
    };
    Ok(SubmitResult {
        submission_id: sub.id,
        published,
        game_slug,
        port_slug,
    })
}

/// The editable form for a port, for the edit page.
#[get("/api/port_form/{port_id}")]
pub async fn port_form(port_id: i64) -> Result<EditPortForm, ServerFnError> {
    require_user().await?;
    let pool = pool()?;
    let row = ports::get_row(pool, port_id).await.map_err(db_error)?;
    let game = games::get(pool, row.game_id).await.map_err(db_error)?;
    let mut conn = pool.acquire().await.map_err(sql_error)?;
    let full = ports::load_full(&mut conn, port_id)
        .await
        .map_err(db_error)?;
    Ok(EditPortForm {
        port_id,
        game_slug: game.slug,
        game_name: game.name,
        port: port_form_from(full),
    })
}
