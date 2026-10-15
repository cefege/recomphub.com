//! Media pipeline: download source images, resize them, and write AVIF files under `MEDIA_DIR`.
//!
//! Output layout: `{MEDIA_DIR}/{kind}/{hash}-{width}.avif`, plus `{hash}-1200.jpg` for `og`.
//! Each file is written to a `.tmp` sibling first and then renamed into place.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use image::codecs::avif::AvifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use recomphub_db::PgPool;
use recomphub_db::media::{self, MediaRow};

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(20);
/// Largest source image accepted, in bytes.
pub const MAX_DOWNLOAD_BYTES: usize = 15 * 1024 * 1024;
const AVIF_SPEED: u8 = 6;
const AVIF_QUALITY: u8 = 55;
const OG_WIDTH: u32 = 1200;
const OG_HEIGHT: u32 = 630;
const OG_QUALITY: u8 = 82;
/// Rows fetched per `process_pending` call.
pub const BATCH_SIZE: i64 = 500;
/// Rows downloaded and encoded at the same time. Encoding is single-threaded per image, so this
/// is roughly the number of cores one run uses.
const CONCURRENCY: usize = 4;
const USER_AGENT: &str = "recomphub/1.0 (+https://recomphub.com)";

/// Output widths for a media kind (plan §3). `og` is handled separately.
pub fn widths_for(kind: &str) -> Option<&'static [u32]> {
    match kind {
        "cover" => Some(&[132, 264, 528]),
        "screenshot" => Some(&[480, 960, 1600]),
        "avatar" => Some(&[40, 80]),
        "video_thumb" => Some(&[320, 480]),
        "og" => Some(&[OG_WIDTH]),
        _ => None,
    }
}

/// Output path for one rendition: `{dir}/{kind}/{hash}-{width}.{ext}`.
pub fn output_path(media_dir: &Path, kind: &str, hash: &str, width: u32, ext: &str) -> PathBuf {
    media_dir.join(kind).join(format!("{hash}-{width}.{ext}"))
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

/// Output extension for a media kind.
pub fn extension_for(kind: &str) -> &'static str {
    if kind == "og" { "jpg" } else { "avif" }
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
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("reading {url}"))?
    {
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

/// Encodes an image as AVIF at `width` pixels wide, keeping the aspect ratio.
fn encode_avif(img: &DynamicImage, width: u32) -> Result<Vec<u8>> {
    let height = scaled_height(img.width(), img.height(), width);
    let resized = img.resize_exact(width, height, FilterType::Lanczos3);
    let mut out = Vec::new();
    resized
        .write_with_encoder(AvifEncoder::new_with_speed_quality(
            &mut out,
            AVIF_SPEED,
            AVIF_QUALITY,
        ))
        .context("encoding AVIF")?;
    Ok(out)
}

/// Encodes the 1200x630 centre-cropped JPEG used for social link previews.
fn encode_og(img: &DynamicImage) -> Result<Vec<u8>> {
    let cropped = img.resize_to_fill(OG_WIDTH, OG_HEIGHT, FilterType::Lanczos3);
    let mut out = Vec::new();
    cropped
        .write_with_encoder(JpegEncoder::new_with_quality(&mut out, OG_QUALITY))
        .context("encoding og JPEG")?;
    Ok(out)
}

/// Height that keeps the source aspect ratio at `width`. Never returns 0.
pub fn scaled_height(src_w: u32, src_h: u32, width: u32) -> u32 {
    let h = (u64::from(src_h) * u64::from(width) + u64::from(src_w) / 2) / u64::from(src_w.max(1));
    (h as u32).max(1)
}

/// Decodes `bytes` and writes every rendition for `kind` under `media_dir`. Returns the source
/// dimensions and the widths written.
///
/// Decoding and encoding are CPU-bound, so they run on a blocking thread.
pub async fn render(
    media_dir: PathBuf,
    kind: String,
    hash: String,
    bytes: Vec<u8>,
) -> Result<(u32, u32, Vec<u32>)> {
    tokio::task::spawn_blocking(move || -> Result<(u32, u32, Vec<u32>)> {
        let img = image::load_from_memory(&bytes).context("decoding source image")?;
        let (src_w, src_h) = (img.width(), img.height());
        if kind == "og" {
            if src_w < OG_WIDTH {
                bail!("og source is {src_w} px wide; at least {OG_WIDTH} px is required");
            }
            let encoded = encode_og(&img)?;
            write_atomic(
                &output_path(&media_dir, &kind, &hash, OG_WIDTH, "jpg"),
                &encoded,
            )?;
            return Ok((src_w, src_h, vec![OG_WIDTH]));
        }
        let widths = planned_widths(&kind, src_w);
        if widths.is_empty() {
            bail!("no {kind} renditions fit a {src_w} px source");
        }
        for width in &widths {
            let encoded = encode_avif(&img, *width)?;
            write_atomic(
                &output_path(&media_dir, &kind, &hash, *width, "avif"),
                &encoded,
            )?;
        }
        Ok((src_w, src_h, widths))
    })
    .await
    .context("media render task panicked")?
}

/// Processes one pending row: download, render, then mark ready or failed.
/// Returns `Ok(true)` when the row became ready. Failures are recorded on the row and return
/// `Ok(false)`.
pub async fn process_row(
    pool: &PgPool,
    http: &reqwest::Client,
    dir: &Path,
    row: &MediaRow,
) -> Result<bool> {
    let outcome = async {
        let bytes = download(http, &row.source_url).await?;
        render(dir.to_path_buf(), row.kind.clone(), row.hash.clone(), bytes).await
    }
    .await;
    match outcome {
        Ok((src_w, src_h, widths)) => {
            let widths: Vec<i32> = widths.into_iter().map(|w| w as i32).collect();
            media::mark_ready(pool, row.id, widths, src_w as i32, src_h as i32).await?;
            Ok(true)
        }
        Err(err) => {
            let message = format!("{err:#}");
            tracing::warn!(media_id = row.id, url = %row.source_url, error = %message, "media failed");
            media::mark_failed(pool, row.id, &message).await?;
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
    let rows = media::next_pending(pool, limit.min(BATCH_SIZE)).await?;
    let mut report = MediaReport::default();
    let mut tasks = tokio::task::JoinSet::new();
    let mut queue = rows.into_iter();
    loop {
        while tasks.len() < CONCURRENCY {
            let Some(row) = queue.next() else { break };
            let (pool, http, dir) = (pool.clone(), http.clone(), media_dir.to_path_buf());
            tasks.spawn(async move { process_row(&pool, &http, &dir, &row).await });
        }
        let Some(joined) = tasks.join_next().await else { break };
        report.processed += 1;
        if joined.context("media task panicked")?? {
            report.ready += 1;
        } else {
            report.failed += 1;
        }
    }
    Ok(report)
}
