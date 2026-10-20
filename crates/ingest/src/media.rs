//! Media pipeline: download source images, produce branded AVIF renditions, and atomically
//! replace older render versions without a broken-image interval.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use image::codecs::avif::AvifEncoder;
use image::imageops::{FilterType, overlay};
use recomphub_db::PgPool;
use recomphub_db::media::{self, MediaRow};
use recomphub_db::port_media;

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(20);
/// Largest source image accepted, in bytes.
pub const MAX_DOWNLOAD_BYTES: usize = 15 * 1024 * 1024;
const AVIF_SPEED: u8 = 6;
const AVIF_QUALITY: u8 = 55;
const OG_WIDTH: u32 = 1200;
const OG_HEIGHT: u32 = 630;
const OG_QUALITY: u8 = 82;
/// Version of the public AVIF renderer. Ready older versions remain public until v2 files exist.
pub const MEDIA_RENDER_VERSION: i32 = 2;
/// Rows fetched per `process_pending` call.
pub const BATCH_SIZE: i64 = 500;
/// Rows downloaded and encoded at the same time.
const CONCURRENCY: usize = 4;
const USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";
const WATERMARK: &[u8] = include_bytes!("../assets/recomphub-watermark.png");

/// Output widths for a media kind. `og` is handled as a fixed crop.
pub fn widths_for(kind: &str) -> Option<&'static [u32]> {
    match kind {
        "cover" => Some(&[132, 264, 528]),
        "screenshot" | "port_screenshot" => Some(&[480, 960, 1600]),
        "avatar" => Some(&[40, 80]),
        "video_thumb" => Some(&[320, 480]),
        "og" => Some(&[OG_WIDTH]),
        _ => None,
    }
}

/// Output path for one public AVIF rendition.
pub fn output_path(
    media_dir: &Path,
    kind: &str,
    public_stem: &str,
    hash: &str,
    render_version: i32,
    width: u32,
) -> PathBuf {
    media_dir
        .join(kind)
        .join(format!("{public_stem}-{hash}-v{render_version}-{width}.avif"))
}

fn legacy_output_path(media_dir: &Path, kind: &str, hash: &str, width: u32) -> PathBuf {
    media_dir.join(kind).join(format!("{hash}-{width}.avif"))
}

/// Widths to produce for a source of `source_width` pixels. Widths above the source are skipped.
pub fn planned_widths(kind: &str, source_width: u32) -> Vec<u32> {
    widths_for(kind)
        .unwrap_or(&[])
        .iter()
        .copied()
        .filter(|w| *w <= source_width)
        .collect()
}

/// Output extension for every public media kind.
pub fn extension_for(_kind: &str) -> &'static str {
    "avif"
}

/// True when `bytes` starts with a signature `image` recognizes.
pub fn looks_like_image(bytes: &[u8]) -> bool {
    image::guess_format(bytes).is_ok()
}

/// Downloads an image with a 20 s timeout and a 15 MB cap. Fails on non-2xx responses.
pub async fn download(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let mut response = http
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("GET {url} returned HTTP {status}");
    }
    if let Some(len) = response.content_length()
        && len as usize > MAX_DOWNLOAD_BYTES
    {
        bail!("{url} is {len} bytes, over the {MAX_DOWNLOAD_BYTES} byte cap");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.with_context(|| format!("reading {url}"))? {
        if body.len() + chunk.len() > MAX_DOWNLOAD_BYTES {
            bail!("{url} exceeds the {MAX_DOWNLOAD_BYTES} byte cap");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Writes `bytes` to `path` through a `.tmp` sibling and a rename. Creates parent directories.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("bin")
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

fn watermark(mut image: DynamicImage, width: u32, full: bool) -> Result<DynamicImage> {
    let source = image::load_from_memory(WATERMARK).context("decoding RecompHub watermark")?;
    let mark = if full {
        source
    } else {
        source.crop_imm(0, 0, 120.min(source.width()), source.height())
    };
    let target_width = if full {
        (width * 40 / 100).clamp(100, 260)
    } else {
        (width / 6).clamp(16, 72)
    };
    let target_height = scaled_height(mark.width(), mark.height(), target_width);
    let mark = mark.resize_exact(target_width, target_height, FilterType::Lanczos3);
    let inset = (width / 40).max(2);
    let x = i64::from(width.saturating_sub(target_width + inset));
    let y = i64::from(image.height().saturating_sub(target_height + inset));
    overlay(&mut image, &mark, x, y);
    Ok(image)
}

fn encode_avif(img: &DynamicImage, width: u32, quality: u8) -> Result<Vec<u8>> {
    let height = scaled_height(img.width(), img.height(), width);
    let resized = img.resize_exact(width, height, FilterType::Lanczos3);
    let branded = watermark(resized, width, width >= 160)?;
    let mut out = Vec::new();
    branded
        .write_with_encoder(AvifEncoder::new_with_speed_quality(&mut out, AVIF_SPEED, quality))
        .context("encoding AVIF")?;
    Ok(out)
}

fn encode_og(img: &DynamicImage) -> Result<Vec<u8>> {
    let cropped = img.resize_to_fill(OG_WIDTH, OG_HEIGHT, FilterType::Lanczos3);
    let branded = watermark(cropped, OG_WIDTH, true)?;
    let mut out = Vec::new();
    branded
        .write_with_encoder(AvifEncoder::new_with_speed_quality(
            &mut out,
            AVIF_SPEED,
            OG_QUALITY,
        ))
        .context("encoding og AVIF")?;
    Ok(out)
}

/// Height that keeps the source aspect ratio at `width`. Never returns 0.
pub fn scaled_height(src_w: u32, src_h: u32, width: u32) -> u32 {
    let h = (u64::from(src_h) * u64::from(width) + u64::from(src_w) / 2) / u64::from(src_w.max(1));
    (h as u32).max(1)
}

/// Decodes `bytes` and writes every branded AVIF rendition atomically.
pub async fn render(
    media_dir: PathBuf,
    kind: String,
    public_stem: String,
    hash: String,
    render_version: i32,
    bytes: Vec<u8>,
) -> Result<(u32, u32, Vec<u32>)> {
    tokio::task::spawn_blocking(move || -> Result<(u32, u32, Vec<u32>)> {
        let img = image::load_from_memory(&bytes).context("decoding source image")?;
        let (src_w, src_h) = (img.width(), img.height());
        if kind == "og" {
            if src_w < OG_WIDTH {
                bail!("og source is {src_w} px wide; at least {OG_WIDTH} px is required");
            }
            write_atomic(
                &output_path(&media_dir, &kind, &public_stem, &hash, render_version, OG_WIDTH),
                &encode_og(&img)?,
            )?;
            return Ok((src_w, src_h, vec![OG_WIDTH]));
        }
        let widths = planned_widths(&kind, src_w);
        if widths.is_empty() {
            bail!("no {kind} renditions fit a {src_w} px source");
        }
        for width in &widths {
            write_atomic(
                &output_path(&media_dir, &kind, &public_stem, &hash, render_version, *width),
                &encode_avif(&img, *width, AVIF_QUALITY)?,
            )?;
        }
        Ok((src_w, src_h, widths))
    })
    .await
    .context("media render task panicked")?
}

/// Where an uploaded file's bytes are kept between the upload request and the next `media-sync`
/// run, which renders it. `{hash}` is the same hash stored in the row's `source_url`.
pub fn upload_path(media_dir: &Path, hash: &str) -> PathBuf {
    media_dir.join("uploads").join(hash)
}

async fn existing_avif(dir: &Path, row: &MediaRow) -> Result<Vec<u8>> {
    let Some(width) = row.widths.iter().copied().max() else {
        bail!("no prior AVIF widths are recorded");
    };
    let width = width as u32;
    let current = output_path(
        dir,
        &row.kind,
        &row.public_stem,
        &row.hash,
        row.render_version,
        width,
    );
    match tokio::fs::read(&current).await {
        Ok(bytes) => Ok(bytes),
        Err(_) => {
            let legacy = legacy_output_path(dir, &row.kind, &row.hash, width);
            tokio::fs::read(&legacy)
                .await
                .with_context(|| format!("reading old AVIF {}", legacy.display()))
        }
    }
}

/// Deletes obsolete pre-versioned output only after its versioned replacement is ready.
async fn remove_legacy_outputs(dir: &Path, row: &MediaRow) {
    if row.render_version >= MEDIA_RENDER_VERSION {
        return;
    }
    for width in &row.widths {
        let path = legacy_output_path(dir, &row.kind, &row.hash, *width as u32);
        if let Err(error) = tokio::fs::remove_file(&path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(media_id = row.id, path = %path.display(), error = %error, "could not remove obsolete media rendition");
        }
    }
}

/// Processes one queued row. A v2 refresh first re-fetches its source; if that is unavailable it
/// decodes the largest existing local AVIF. Failed refreshes retain their old ready metadata.
pub async fn process_row(
    pool: &PgPool,
    http: &reqwest::Client,
    dir: &Path,
    row: &MediaRow,
) -> Result<bool> {
    let upload_hash = row.source_url.strip_prefix("upload:");
    let outcome = async {
        let source = match upload_hash {
            Some(hash) => {
                let path = upload_path(dir, hash);
                tokio::fs::read(&path)
                    .await
                    .with_context(|| format!("reading uploaded file {}", path.display()))
            }
            None => download(http, &row.source_url).await,
        };
        let bytes = match source {
            Ok(bytes) => bytes,
            Err(source_error) if row.status == "ready" => existing_avif(dir, row)
                .await
                .with_context(|| format!("source refresh failed: {source_error:#}"))?,
            Err(error) => return Err(error),
        };
        render(
            dir.to_path_buf(),
            row.kind.clone(),
            row.public_stem.clone(),
            row.hash.clone(),
            MEDIA_RENDER_VERSION,
            bytes,
        )
        .await
    }
    .await;
    match outcome {
        Ok((src_w, src_h, widths)) => {
            let widths: Vec<i32> = widths.into_iter().map(|width| width as i32).collect();
            media::mark_ready(
                pool,
                row.id,
                widths,
                src_w as i32,
                src_h as i32,
                MEDIA_RENDER_VERSION,
            )
            .await?;
            remove_legacy_outputs(dir, row).await;
            port_media::approve_readme_after_render(pool, row.id, src_w as i32).await?;
            if let Some(hash) = upload_hash {
                let _ = tokio::fs::remove_file(upload_path(dir, hash)).await;
            }
            Ok(true)
        }
        Err(error) => {
            let message = format!("{error:#}");
            tracing::warn!(media_id = row.id, url = %row.source_url, error = %message, "media failed");
            media::mark_failed(pool, row.id, &message).await?;
            port_media::note_readme_render_failure(pool, row.id).await?;
            Ok(false)
        }
    }
}

/// Summary of one `process_pending` run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MediaReport {
    pub processed: usize,
    pub ready: usize,
    pub failed: usize,
}

/// Processes up to `limit` pending or retryable rows, `CONCURRENCY` at a time. Failed rows are
/// retried until they reach three attempts.
pub async fn process_pending(pool: &PgPool, media_dir: &Path, limit: i64) -> Result<MediaReport> {
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .context("building the media HTTP client")?;
    let rows = media::next_pending(pool, limit.min(BATCH_SIZE), MEDIA_RENDER_VERSION).await?;
    let mut report = MediaReport::default();
    let mut tasks = tokio::task::JoinSet::new();
    let mut queue = rows.into_iter();
    loop {
        while tasks.len() < CONCURRENCY {
            let Some(row) = queue.next() else { break };
            let (pool, http, dir) = (pool.clone(), http.clone(), media_dir.to_path_buf());
            tasks.spawn(async move { process_row(&pool, &http, &dir, &row).await });
        }
        let Some(joined) = tasks.join_next().await else {
            break;
        };
        report.processed += 1;
        if joined.context("media task panicked")?? {
            report.ready += 1;
        } else {
            report.failed += 1;
        }
    }
    Ok(report)
}

/// Runs successive batches until every pending, retryable, or stale rendition has been processed.
/// Intended for an explicit one-off renderer cutover rather than scheduled syncs.
pub async fn process_all_pending(pool: &PgPool, media_dir: &Path) -> Result<MediaReport> {
    let mut total = MediaReport::default();
    loop {
        let report = process_pending(pool, media_dir, BATCH_SIZE).await?;
        total.processed += report.processed;
        total.ready += report.ready;
        total.failed += report.failed;
        if report.processed == 0 {
            let stale = media::stale_ready_count(pool, MEDIA_RENDER_VERSION).await?;
            if stale > 0 {
                bail!("{stale} public media rows could not be migrated");
            }
            return Ok(total);
        }
    }
}
