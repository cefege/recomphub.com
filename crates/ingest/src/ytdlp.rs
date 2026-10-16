//! yt-dlp wrapper: flat YouTube search and single-video metadata. Parsing is pure;
//! the subprocess is only started by `YtDlp`.
//!
//! Configuration (read by `YtDlp::from_env`):
//! - `YTDLP_BIN`: binary to run, default `yt-dlp`.
//! - `YTDLP_COOKIES`: optional Netscape cookies file. It is copied to a private temp file
//!   for the lifetime of the process, so the mounted secret is never written to. Cookie
//!   contents are never logged.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::Deserialize;

use crate::video_links::is_youtube_id;

const DEFAULT_BIN: &str = "yt-dlp";
/// One yt-dlp call (a search or a single video) may take this long before it is killed.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// Error text kept for logs and the `last_error` columns.
const MAX_ERROR_CHARS: usize = 300;

/// Metadata for one YouTube video, from a search hit or a video lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoMeta {
    pub youtube_id: String,
    pub title: String,
    pub channel: Option<String>,
    /// Absent in flat search output; present in single-video output.
    pub published_at: Option<DateTime<Utc>>,
    pub duration_secs: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    id: Option<String>,
    title: Option<String>,
    channel: Option<String>,
    uploader: Option<String>,
    duration: Option<f64>,
    timestamp: Option<i64>,
    upload_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Playlist {
    #[serde(default)]
    entries: Vec<Option<Entry>>,
}

fn meta_from_entry(e: Entry) -> Option<VideoMeta> {
    let youtube_id = e.id.filter(|id| is_youtube_id(id))?;
    let published_at = e
        .timestamp
        .and_then(|ts| Utc.timestamp_opt(ts, 0).single())
        .or_else(|| {
            e.upload_date
                .as_deref()
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y%m%d").ok())
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|dt| dt.and_utc())
        });
    Some(VideoMeta {
        youtube_id,
        title: e.title.unwrap_or_default().trim().to_string(),
        channel: e
            .channel
            .or(e.uploader)
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty()),
        published_at,
        duration_secs: e
            .duration
            .filter(|d| d.is_finite() && *d >= 0.0)
            .map(|d| d.round() as u32),
    })
}

/// Parses `yt-dlp -J --flat-playlist ytsearchN:...` output. Entries without a valid video
/// ID are skipped.
pub fn parse_search(json: &str) -> Result<Vec<VideoMeta>> {
    let playlist: Playlist = serde_json::from_str(json).context("search output is not JSON")?;
    Ok(playlist
        .entries
        .into_iter()
        .flatten()
        .filter_map(meta_from_entry)
        .collect())
}

/// Parses `yt-dlp -J` output for one video.
pub fn parse_video(json: &str) -> Result<VideoMeta> {
    let entry: Entry = serde_json::from_str(json).context("video output is not JSON")?;
    meta_from_entry(entry).ok_or_else(|| anyhow!("video output has no valid video ID"))
}

/// Private copy of the cookies file, deleted when dropped.
struct TempCookies(PathBuf);

impl TempCookies {
    fn copy_from(src: &Path) -> std::io::Result<Self> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let dst = std::env::temp_dir().join(format!(
            "recomphub-ytdlp-cookies-{}.txt",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&dst);
        let bytes = std::fs::read(src)?;
        // Created private from the start; only this process needs to read it.
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&dst)?;
        out.write_all(&bytes)?;
        Ok(Self(dst))
    }
}

impl Drop for TempCookies {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub struct YtDlp {
    bin: PathBuf,
    cookies: Option<TempCookies>,
}

fn truncate(s: &str) -> String {
    s.chars().take(MAX_ERROR_CHARS).collect()
}

impl YtDlp {
    /// Reads `YTDLP_BIN` and `YTDLP_COOKIES` and checks that the binary runs.
    /// A missing or unreadable cookie file is a warning, not an error.
    pub async fn from_env() -> Result<Self> {
        let bin = std::env::var("YTDLP_BIN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_BIN.to_string());
        let cookies = match std::env::var_os("YTDLP_COOKIES").filter(|v| !v.is_empty()) {
            None => None,
            Some(src) => match TempCookies::copy_from(Path::new(&src)) {
                Ok(copy) => Some(copy),
                Err(e) => {
                    tracing::warn!(kind = ?e.kind(), "YTDLP_COOKIES could not be read; continuing without cookies");
                    None
                }
            },
        };
        let yt = Self {
            bin: PathBuf::from(bin),
            cookies,
        };
        let version = yt
            .run(&["--version"])
            .await
            .context("yt-dlp is not runnable")?;
        tracing::info!(version = %version.trim(), cookies = yt.cookies.is_some(), "yt-dlp ready");
        Ok(yt)
    }

    /// Top `limit` YouTube results for `query`, in relevance order.
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<VideoMeta>> {
        let term = format!("ytsearch{limit}:{query}");
        let limit_arg = limit.to_string();
        let json = self
            .run(&[
                "--flat-playlist",
                "-J",
                "--playlist-end",
                limit_arg.as_str(),
                term.as_str(),
            ])
            .await?;
        parse_search(&json)
    }

    /// Metadata for one video. Fails for unavailable, private, or removed videos.
    pub async fn video(&self, youtube_id: &str) -> Result<VideoMeta> {
        if !is_youtube_id(youtube_id) {
            bail!("not a YouTube video ID");
        }
        let url = format!("https://www.youtube.com/watch?v={youtube_id}");
        let json = self.run(&["--no-playlist", "-J", url.as_str()]).await?;
        parse_video(&json)
    }

    async fn run(&self, args: &[&str]) -> Result<String> {
        let mut cmd = tokio::process::Command::new(&self.bin);
        cmd.args([
            "--ignore-config",
            "--no-cache-dir",
            "--no-update",
            "--no-warnings",
            "--skip-download",
        ]);
        if let Some(cookies) = &self.cookies {
            cmd.arg("--cookies").arg(&cookies.0);
        }
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let child = cmd.spawn().context("starting yt-dlp")?;
        let out = tokio::time::timeout(CALL_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| anyhow!("yt-dlp timed out after {}s", CALL_TIMEOUT.as_secs()))?
            .context("waiting for yt-dlp")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            bail!("yt-dlp exited with {}: {}", out.status, truncate(first));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = include_str!("../tests/fixtures/ytdlp_search_zelda64recomp.json");
    const VIDEO: &str = include_str!("../tests/fixtures/ytdlp_video_info.json");

    #[test]
    fn search_output_yields_video_metadata() {
        let hits = parse_search(SEARCH).unwrap();
        assert_eq!(hits.len(), 8);
        assert_eq!(hits[0].youtube_id, "ywWwUuWRgsM");
        assert_eq!(
            hits[0].title,
            "Recompilation: An Incredible New Way to Keep N64 Games Alive"
        );
        assert_eq!(hits[0].channel.as_deref(), Some("Nerrel"));
        assert_eq!(hits[0].duration_secs, Some(673));
        assert_eq!(hits[0].published_at, None);
    }

    #[test]
    fn search_output_without_entries_is_empty_not_an_error() {
        assert!(parse_search(r#"{"_type":"playlist"}"#).unwrap().is_empty());
        assert!(
            parse_search(r#"{"entries":[null,{"id":"short"}]}"#)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn non_json_output_is_an_error() {
        assert!(parse_search("ERROR: something").is_err());
        assert!(parse_video("").is_err());
    }

    #[test]
    fn video_output_yields_metadata_with_timestamp() {
        let v = parse_video(VIDEO).unwrap();
        assert_eq!(v.youtube_id, "ytptWDMw7z0");
        assert_eq!(v.title, "Zelda64Recomp - Returning to Link Ultrawide");
        assert_eq!(v.channel.as_deref(), Some("Philmania"));
        assert_eq!(v.duration_secs, Some(688));
        assert_eq!(v.published_at.map(|d| d.timestamp()), Some(1716084055));
    }

    #[test]
    fn upload_date_is_used_when_timestamp_is_missing() {
        let v = parse_video(
            r#"{"id":"ytptWDMw7z0","title":"t","upload_date":"20240519","duration":12.6}"#,
        )
        .unwrap();
        assert_eq!(
            v.published_at.map(|d| d.format("%Y-%m-%d").to_string()),
            Some("2024-05-19".to_string())
        );
        assert_eq!(v.duration_secs, Some(13));
        assert_eq!(v.channel, None);
    }

    #[test]
    fn invalid_ids_are_rejected() {
        assert!(parse_video(r#"{"id":"../../etc","title":"x"}"#).is_err());
        assert!(parse_video(r#"{"title":"no id"}"#).is_err());
    }
}
