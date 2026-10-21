//! Media pipeline: download source images, produce branded AVIF renditions, and atomically
//! replace older render versions without a broken-image interval.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use image::codecs::avif::AvifEncoder;
use image::imageops::{FilterType, overlay};
use rand::random;
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

/// Writes `bytes` to a unique temporary sibling before atomically replacing `path`.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("bin");
    let filename = path.file_name().and_then(|name| name.to_str()).unwrap_or("media");
    let tmp = path.with_file_name(format!(".{filename}-{:016x}.{extension}.tmp", random::<u64>()));
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

async fn read_rendition(path: &Path) -> Result<Vec<u8>> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("reading media rendition {}", path.display()))?;
    image::load_from_memory(&bytes)
        .with_context(|| format!("decoding media rendition {}", path.display()))?;
    Ok(bytes)
}

/// Returns a usable prior AVIF/JPEG rendition, trying all recorded widths from largest first.
async fn existing_rendition(dir: &Path, row: &MediaRow) -> Result<Vec<u8>> {
    let mut widths = row.widths.clone();
    widths.sort_unstable_by(|a, b| b.cmp(a));
    let mut errors = Vec::new();
    for width in widths {
        if width <= 0 {
            continue;
        }
        let width = width as u32;
        for path in [
            output_path(
                dir,
                &row.kind,
                &row.public_stem,
                &row.hash,
                row.render_version,
                width,
            ),
            legacy_output_path(dir, &row.kind, &row.hash, width),
        ] {
            match read_rendition(&path).await {
                Ok(bytes) => return Ok(bytes),
                Err(error) => errors.push(format!("{error:#}")),
            }
        }
    }
    if row.kind == "og" {
        let jpeg = dir.join("og").join(format!("{}-1200.jpg", row.hash));
        match read_rendition(&jpeg).await {
            Ok(bytes) => return Ok(bytes),
            Err(error) => errors.push(format!("{error:#}")),
        }
    }
    bail!("no usable prior rendition: {}", errors.join("; "))
}

/// True only when every recorded current rendition exists and decodes.
async fn has_current_renditions(dir: &Path, row: &MediaRow) -> bool {
    if row.widths.is_empty() || row.widths.iter().any(|width| *width <= 0) {
        return false;
    }
    for width in &row.widths {
        let path = output_path(
            dir,
            &row.kind,
            &row.public_stem,
            &row.hash,
            MEDIA_RENDER_VERSION,
            *width as u32,
        );
        if read_rendition(&path).await.is_err() {
            return false;
        }
    }
    true
}

/// Deletes only the obsolete pre-v2 files after complete v2 output has been recorded.
async fn remove_legacy_outputs(dir: &Path, row: &MediaRow) {
    if row.render_version >= MEDIA_RENDER_VERSION {
        return;
    }
    for width in &row.widths {
        if *width <= 0 {
            continue;
        }
        for path in [
            legacy_output_path(dir, &row.kind, &row.hash, *width as u32),
            output_path(
                dir,
                &row.kind,
                &row.public_stem,
                &row.hash,
                row.render_version,
                *width as u32,
            ),
        ] {
            if let Err(error) = tokio::fs::remove_file(&path).await
                && error.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(media_id = row.id, path = %path.display(), error = %error, "could not remove obsolete media rendition");
            }
        }
    }
    if row.kind == "og" {
        let jpeg = dir.join("og").join(format!("{}-1200.jpg", row.hash));
        if let Err(error) = tokio::fs::remove_file(&jpeg).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(media_id = row.id, path = %jpeg.display(), error = %error, "could not remove obsolete OG JPEG");
        }
    }
}

/// Processes one queued row. A current ready rendition is merely published when all of its
/// recorded files verify; older or incomplete output is rendered before any filename is assigned.
pub async fn process_row(
    pool: &PgPool,
    http: &reqwest::Client,
    dir: &Path,
    row: &MediaRow,
) -> Result<bool> {
    let upload_hash = row.source_url.strip_prefix("upload:");
    let outcome = async {
        if row.status == "ready"
            && row.render_version >= MEDIA_RENDER_VERSION
            && has_current_renditions(dir, row).await
        {
            let (base, preferred_number) = media::public_filename_input(pool, row).await?;
            media::assign_public_filename(pool, row.id, &base, preferred_number).await?;
            return Ok(());
        }
        let source = async {
            match upload_hash {
                Some(hash) => {
                    let path = upload_path(dir, hash);
                    tokio::fs::read(&path)
                        .await
                        .with_context(|| format!("reading uploaded file {}", path.display()))
                }
                None => download(http, &row.source_url).await,
            }
        };
        let bytes = if row.status == "ready" && row.render_version < MEDIA_RENDER_VERSION {
            match existing_rendition(dir, row).await {
                Ok(bytes) => bytes,
                Err(existing_error) => source
                    .await
                    .with_context(|| format!("reading prior rendition failed: {existing_error:#}"))?,
            }
        } else {
            match source.await {
                Ok(bytes) => bytes,
                Err(source_error) if row.status == "ready" => existing_rendition(dir, row)
                    .await
                    .with_context(|| format!("source refresh failed: {source_error:#}"))?,
                Err(error) => return Err(error),
            }
        };
        let (src_w, src_h, widths) = render(
            dir.to_path_buf(),
            row.kind.clone(),
            row.public_stem.clone(),
            row.hash.clone(),
            MEDIA_RENDER_VERSION,
            bytes,
        )
        .await?;
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
        let (base, preferred_number) = media::public_filename_input(pool, row).await?;
        media::assign_public_filename(pool, row.id, &base, preferred_number).await?;
        remove_legacy_outputs(dir, row).await;
        port_media::approve_readme_after_render(pool, row.id, src_w as i32).await?;
        if let Some(hash) = upload_hash {
            let _ = tokio::fs::remove_file(upload_path(dir, hash)).await;
        }
        Ok(())
    }
    .await;
    match outcome {
        Ok(()) => Ok(true),
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

/// Processes one advisory-locked media row. Keeping the claiming connection alive protects the
/// internal rendition paths for the entire render; a crashed process releases its lock with the
/// database session.
async fn process_claimed_one(
    pool: &PgPool,
    http: &reqwest::Client,
    media_dir: &Path,
) -> Result<Option<bool>> {
    let mut connection = pool.acquire().await.context("acquiring media claim connection")?;
    let Some(row) = media::claim_next(&mut connection, MEDIA_RENDER_VERSION).await? else {
        return Ok(None);
    };
    let outcome = process_row(pool, http, media_dir, &row).await;
    let released = media::release_claim(&mut connection, row.id).await;
    released.context("releasing media claim")?;
    Ok(Some(outcome?))
}

/// Continuously processes durable media work. It is safe for a Kubernetes Deployment: an
/// advisory lock prevents duplicate writers and PostgreSQL releases abandoned claims on restart.
pub async fn process_forever(pool: &PgPool, media_dir: &Path) -> Result<()> {
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .context("building the media HTTP client")?;
    loop {
        match process_claimed_one(pool, &http, media_dir).await? {
            Some(true) => tracing::info!("media worker processed one rendition"),
            Some(false) => tracing::warn!("media worker recorded one rendition failure"),
            None => tokio::time::sleep(Duration::from_secs(5)).await,
        }
    }
}

/// Runs the explicit cutover with one renderer at a time. Supplying IDs repairs precisely those
/// rows, including already-current rows whose files failed an external audit.
pub async fn process_all_pending(
    pool: &PgPool,
    media_dir: &Path,
    ids: &[i64],
) -> Result<MediaReport> {
    let http = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .context("building the media HTTP client")?;
    if !ids.is_empty() {
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let rows = media::get_many(pool, ids.clone()).await?;
        let missing = ids.len().saturating_sub(rows.len());
        let mut report = MediaReport::default();
        for row in rows {
            report.processed += 1;
            if process_row(pool, &http, media_dir, &row).await? {
                report.ready += 1;
            } else {
                report.failed += 1;
            }
        }
        if missing > 0 || report.failed > 0 {
            bail!(
                "focused media repair failed: missing_ids={missing}, failed_rows={}",
                report.failed
            );
        }
        return Ok(report);
    }

    let mut total = MediaReport::default();
    media::reset_stale_attempts(pool, MEDIA_RENDER_VERSION).await?;
    loop {
        let rows = media::next_pending(pool, BATCH_SIZE, MEDIA_RENDER_VERSION).await?;
        if rows.is_empty() {
            let unresolved = media::stale_ready_count(pool, MEDIA_RENDER_VERSION).await?;
            if unresolved > 0 {
                bail!("{unresolved} ready media rows could not be migrated");
            }
            return Ok(total);
        }
        let mut batch = MediaReport::default();
        for row in rows {
            batch.processed += 1;
            if process_row(pool, &http, media_dir, &row).await? {
                batch.ready += 1;
            } else {
                batch.failed += 1;
            }
        }
        total.processed += batch.processed;
        total.ready += batch.ready;
        total.failed += batch.failed;
        let unresolved = media::stale_ready_count(pool, MEDIA_RENDER_VERSION).await?;
        tracing::info!(
            processed = batch.processed,
            ready = batch.ready,
            failed = batch.failed,
            remaining_unresolved_ready = unresolved,
            "media migration batch finished"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    #[test]
    fn rendered_avif_can_be_decoded_for_a_version_refresh() {
        let image = DynamicImage::new_rgba8(320, 180);
        let bytes = encode_avif(&image, 320, AVIF_QUALITY).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!(decoded.dimensions(), (320, 180));
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn refreshes_saved_og_and_preserves_failed_or_current_renditions(
        pool: PgPool,
    ) -> anyhow::Result<()> {
        use image::{GenericImageView, ImageFormat};
        use std::io::Cursor;
        use std::time::{SystemTime, UNIX_EPOCH};

        let dir = std::env::temp_dir().join(format!(
            "recomphub-media-test-{}",
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        ));
        std::fs::create_dir_all(&dir)?;
        let http = reqwest::Client::new();
        let mut jpeg = Cursor::new(Vec::new());
        DynamicImage::new_rgba8(1600, 900).write_to(&mut jpeg, ImageFormat::Jpeg)?;

        let og_id = media::enqueue(&pool, "og", "https://127.0.0.1:1/og.jpg", "fixture").await?;
        let og = media::get(&pool, og_id).await?.expect("queued OG media");
        let old_jpeg = dir.join("og").join(format!("{}-1200.jpg", og.hash));
        std::fs::create_dir_all(old_jpeg.parent().expect("OG parent"))?;
        std::fs::write(&old_jpeg, jpeg.into_inner())?;
        media::mark_ready(&pool, og.id, vec![1200], 1200, 630, MEDIA_RENDER_VERSION - 1).await?;
        let old_og = media::get(&pool, og.id).await?.expect("ready OG media");
        assert!(process_row(&pool, &http, &dir, &old_og).await?);
        let refreshed_og = media::get(&pool, og.id).await?.expect("refreshed OG media");
        assert_eq!(refreshed_og.render_version, MEDIA_RENDER_VERSION);
        assert!(refreshed_og.public_filename.is_some());
        let og_path = output_path(
            &dir,
            &refreshed_og.kind,
            &refreshed_og.public_stem,
            &refreshed_og.hash,
            MEDIA_RENDER_VERSION,
            1200,
        );
        assert_eq!(image::open(og_path)?.dimensions(), (1200, 630));

        let failed_id =
            media::enqueue(&pool, "screenshot", "https://127.0.0.1:1/failed.png", "failed").await?;
        let failed = media::get(&pool, failed_id).await?.expect("queued failed fixture");
        media::mark_ready(
            &pool,
            failed.id,
            vec![960],
            960,
            540,
            MEDIA_RENDER_VERSION - 1,
        )
        .await?;
        let failed_path = output_path(
            &dir,
            &failed.kind,
            &failed.public_stem,
            &failed.hash,
            MEDIA_RENDER_VERSION - 1,
            960,
        );
        std::fs::create_dir_all(failed_path.parent().expect("failed parent"))?;
        let original = b"old-but-corrupt-rendition".to_vec();
        std::fs::write(&failed_path, &original)?;
        let failed_before = media::get(&pool, failed.id).await?.expect("ready failed fixture");
        assert!(!process_row(&pool, &http, &dir, &failed_before).await?);
        let failed_after = media::get(&pool, failed.id).await?.expect("failed refresh fixture");
        assert_eq!(failed_after.status, "ready");
        assert_eq!(failed_after.render_version, MEDIA_RENDER_VERSION - 1);
        assert_eq!(failed_after.widths, vec![960]);
        assert_eq!(std::fs::read(&failed_path)?, original);

        let current_id =
            media::enqueue(&pool, "screenshot", "https://127.0.0.1:1/current.png", "current").await?;
        let current = media::get(&pool, current_id).await?.expect("queued current fixture");
        let mut png = Cursor::new(Vec::new());
        DynamicImage::new_rgba8(960, 540).write_to(&mut png, ImageFormat::Png)?;
        let (width, height, widths) = render(
            dir.clone(),
            current.kind.clone(),
            current.public_stem.clone(),
            current.hash.clone(),
            MEDIA_RENDER_VERSION,
            png.into_inner(),
        )
        .await?;
        media::mark_ready(
            &pool,
            current.id,
            widths.into_iter().map(|value| value as i32).collect(),
            width as i32,
            height as i32,
            MEDIA_RENDER_VERSION,
        )
        .await?;
        let current_path = output_path(
            &dir,
            &current.kind,
            &current.public_stem,
            &current.hash,
            MEDIA_RENDER_VERSION,
            960,
        );
        let bytes_before = std::fs::read(&current_path)?;
        let current_before = media::get(&pool, current.id).await?.expect("ready current fixture");
        assert!(process_row(&pool, &http, &dir, &current_before).await?);
        let current_after = media::get(&pool, current.id).await?.expect("named current fixture");
        assert!(current_after.public_filename.is_some());
        assert_eq!(std::fs::read(current_path)?, bytes_before);

        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
