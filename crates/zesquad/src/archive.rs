//! Keeps every auction dump as Battle.net sent it, for uses nobody has thought of yet. Each new
//! dump is compressed into a file in a local folder; with a bucket configured, the files are
//! uploaded and then removed, so the folder only holds what hasn't been uploaded yet. Without
//! one, the folder is the archive.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Context as _;
use zesquad_core::Region;
use zesquad_core::archive::dump_key;

use crate::s3;

pub use crate::s3::Config as BucketConfig;

/// zstd level. On a dump-shaped 24 MB of JSON: 13% smaller than gzip in a quarter of a
/// second; level 19 would save another fifth but take 20 seconds.
const LEVEL: i32 = 9;
/// Files being written end in this, so a crash mid-write never leaves one that looks done.
const PARTIAL: &str = ".partial";

/// Where dumps are kept.
#[derive(Debug, Clone)]
pub struct Config {
    /// The local folder: everything, or only what waits for upload when `bucket` is set.
    pub dir: PathBuf,
    pub bucket: Option<BucketConfig>,
}

pub struct Archive {
    dir: PathBuf,
    bucket: Option<s3::Bucket>,
}

impl Archive {
    /// # Errors
    ///
    /// When the bucket's URL or the HTTP client is broken.
    pub fn new(config: Config) -> anyhow::Result<Self> {
        Ok(Self {
            dir: config.dir,
            bucket: config.bucket.map(s3::Bucket::new).transpose()?,
        })
    }

    /// Compresses a dump into the local folder. `last_modified` is Battle.net's header, which
    /// names the file (when it's missing or odd, `fetched_at` does). Returns the key.
    ///
    /// # Errors
    ///
    /// When the file can't be written.
    pub async fn save(
        &self,
        region: Region,
        house: u32,
        commodities: bool,
        last_modified: Option<&str>,
        fetched_at: i64,
        body: Arc<str>,
    ) -> anyhow::Result<String> {
        let at = last_modified
            .and_then(|m| httpdate::parse_http_date(m).ok())
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_secs()).ok())
            .unwrap_or(fetched_at);
        let key = dump_key(region, house, commodities, at);
        let path = self.dir.join(&key);
        tokio::task::spawn_blocking(move || write_compressed(&path, &body))
            .await?
            .with_context(|| format!("archiving {key}"))?;
        Ok(key)
    }

    /// Uploads the saved dumps, oldest first, removing each once it's in the bucket. Stops at
    /// the first failure; the rest wait for the next call. Returns how many were uploaded.
    ///
    /// # Errors
    ///
    /// When an upload fails or the folder can't be read.
    pub async fn upload(&self) -> anyhow::Result<usize> {
        let Some(bucket) = &self.bucket else {
            return Ok(0);
        };
        let dir = self.dir.clone();
        let keys = tokio::task::spawn_blocking(move || saved(&dir)).await??;
        for (done, key) in keys.iter().enumerate() {
            let path = self.dir.join(key);
            let body = tokio::fs::read(&path)
                .await
                .with_context(|| format!("reading {}", path.display()))?;
            if let Err(e) = bucket.put(key, "application/zstd", body).await {
                return Err(e.context(format!("{done} of {} archived dumps uploaded", keys.len())));
            }
            tokio::fs::remove_file(&path)
                .await
                .with_context(|| format!("removing {}", path.display()))?;
            // The day's folder, once it's empty; fails harmlessly while it isn't.
            if let Some(day) = path.parent() {
                let _ = tokio::fs::remove_dir(day).await;
            }
        }
        Ok(keys.len())
    }
}

fn write_compressed(path: &Path, body: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let compressed = zstd::bulk::compress(body.as_bytes(), LEVEL)?;
    let mut partial = path.as_os_str().to_owned();
    partial.push(PARTIAL);
    std::fs::write(&partial, compressed)?;
    std::fs::rename(&partial, path)?;
    Ok(())
}

/// The keys of the finished files under `dir`, sorted (so oldest first within a house).
fn saved(dir: &Path) -> anyhow::Result<Vec<String>> {
    fn walk(dir: &Path, prefix: &str, keys: &mut Vec<String>) -> std::io::Result<()> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let key = format!("{prefix}{name}");
            if entry.file_type()?.is_dir() {
                walk(&entry.path(), &format!("{key}/"), keys)?;
            } else if name.ends_with(".json.zst") {
                keys.push(key);
            }
        }
        Ok(())
    }
    let mut keys = Vec::new();
    walk(dir, "", &mut keys).with_context(|| format!("reading {}", dir.display()))?;
    keys.sort();
    Ok(keys)
}
