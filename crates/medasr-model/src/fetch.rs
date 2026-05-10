//! Resumable model download from Hugging Face Hub.
//!
//! Per the personal/research distribution posture: rustls + native trust
//! store, no SPKI pinning (LFS files redirect to a CDN whose pin set we
//! cannot maintain). The compile-time SHA-256 manifest is the integrity
//! guarantee for the bytes that land on disk.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, RANGE, USER_AGENT};
use thiserror::Error;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::manifest::{ManifestFile, HF_REPO, HF_REVISION};

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("server returned status {0} for {1}")]
    Status(u16, String),
    #[error("cancelled")]
    Cancelled,
    #[error("disk full or write failure")]
    Write,
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// Download a single manifest file into `dest_dir` with resume support.
/// Atomically renames `<file>.part` to `<file>` on success.
pub async fn fetch_file<F>(
    client: &reqwest::Client,
    file: &ManifestFile,
    dest_dir: &Path,
    cancel: &CancellationToken,
    mut on_progress: F,
) -> Result<PathBuf, FetchError>
where
    F: FnMut(Progress) + Send,
{
    let url = format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        HF_REPO, HF_REVISION, file.path
    );
    let final_path = dest_dir.join(file.path);
    let part_path = dest_dir.join(format!("{}.part", file.path));

    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let existing_bytes = match tokio::fs::metadata(&part_path).await {
        Ok(m) => m.len(),
        Err(_) => 0,
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(concat!("medasr/", env!("CARGO_PKG_VERSION"))),
    );
    if existing_bytes > 0 {
        let range = format!("bytes={}-", existing_bytes);
        headers.insert(RANGE, HeaderValue::from_str(&range).expect("ascii"));
        info!("resuming {} from byte {}", file.path, existing_bytes);
    }

    let resp = client
        .get(&url)
        .headers(headers)
        .timeout(Duration::from_secs(300))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() && status.as_u16() != 206 {
        return Err(FetchError::Status(status.as_u16(), file.path.into()));
    }
    let total_size = resp.content_length().map(|cl| cl + existing_bytes);

    let mut file_handle = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&part_path)
        .await?;

    let mut stream = resp.bytes_stream();
    let mut downloaded = existing_bytes;
    while let Some(chunk) = stream.next().await {
        if cancel.is_cancelled() {
            return Err(FetchError::Cancelled);
        }
        let chunk = chunk?;
        file_handle
            .write_all(&chunk)
            .await
            .map_err(|_| FetchError::Write)?;
        downloaded += chunk.len() as u64;
        on_progress(Progress {
            downloaded,
            total: total_size,
        });
    }
    file_handle.flush().await?;
    drop(file_handle);

    tokio::fs::rename(&part_path, &final_path).await?;
    info!("downloaded {} ({} bytes)", file.path, downloaded);
    Ok(final_path)
}

/// Build a default reqwest client with sane timeouts and a typed
/// User-Agent. Reused across files so connection pooling kicks in.
pub fn default_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .pool_idle_timeout(Some(Duration::from_secs(30)))
        .build()
        .unwrap_or_else(|e| {
            warn!("reqwest builder failed ({e}); falling back to defaults");
            reqwest::Client::new()
        })
}
