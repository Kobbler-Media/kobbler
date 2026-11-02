//! Built reader packs on disk, and their server-only sidecars in memory.
//!
//! Layout: `{storage_root}/reader_packs/{book_file_id}/{content_revision}-g{generation}/`
//! holding gzipped `manifest.json.gz`, `chapters/{n}.json.gz` and
//! `sidecars.json.gz`. A new content revision or generation is a new
//! directory, so a pack is never rewritten in place; older siblings are
//! removed after a successful build. Derived data — not backed up.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use dashmap::DashMap;
use flate2::Compression;
use flate2::write::GzEncoder;
use kob_ebook::reader_pack::{self, ChapterSidecar, READER_PACK_GENERATION};
use uuid::Uuid;

use super::file::ReaderFile;

const LOADED_CAP: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("build: {0}")]
    Build(#[from] kob_ebook::EbookError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("build task: {0}")]
    Join(#[from] tokio::task::JoinError),
}

pub struct LoadedPack {
    pub dir: PathBuf,
    pub etag: String,
    pub hrefs: Vec<String>,
    pub titles: Vec<Option<String>>,
    pub sidecars: Vec<ChapterSidecar>,
}

impl LoadedPack {
    pub fn chapter_path(&self, n: usize) -> PathBuf {
        self.dir.join("chapters").join(format!("{n}.json.gz"))
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.dir.join("manifest.json.gz")
    }

    pub fn chapter(&self, n: usize) -> Result<reader_pack::Chapter, PackError> {
        read_gz(&self.chapter_path(n))
    }
}

fn loaded() -> &'static DashMap<PathBuf, Arc<LoadedPack>> {
    static LOADED: OnceLock<DashMap<PathBuf, Arc<LoadedPack>>> = OnceLock::new();
    LOADED.get_or_init(DashMap::new)
}

fn building() -> &'static DashMap<Uuid, Arc<tokio::sync::Mutex<()>>> {
    static BUILDING: OnceLock<DashMap<Uuid, Arc<tokio::sync::Mutex<()>>>> = OnceLock::new();
    BUILDING.get_or_init(DashMap::new)
}

pub fn pack_dir(storage_root: &str, file: &ReaderFile) -> PathBuf {
    Path::new(storage_root)
        .join("reader_packs")
        .join(file.id.to_string())
        .join(format!("{}-g{READER_PACK_GENERATION}", file.revision))
}

/// The pack for `file`, building it first if this revision and generation
/// have never been built. Concurrent callers for one file share one build.
pub async fn ensure(storage_root: &str, file: &ReaderFile) -> Result<Arc<LoadedPack>, PackError> {
    let dir = pack_dir(storage_root, file);
    if let Some(pack) = loaded().get(&dir) {
        return Ok(Arc::clone(&pack));
    }
    let lock = Arc::clone(&building().entry(file.id).or_default());
    let _guard = lock.lock().await;
    if !tokio::fs::try_exists(dir.join("sidecars.json.gz")).await? {
        let source = file.path.clone();
        let target = dir.clone();
        tokio::task::spawn_blocking(move || build_into(&source, &target)).await??;
    }
    let target = dir.clone();
    let pack = Arc::new(tokio::task::spawn_blocking(move || load(&target)).await??);
    if loaded().len() >= LOADED_CAP
        && let Some(evict) = loaded().iter().next().map(|e| e.key().clone())
    {
        loaded().remove(&evict);
    }
    loaded().insert(dir, Arc::clone(&pack));
    Ok(pack)
}

/// Removes the packs of book files that no longer exist, returning how many.
pub async fn remove_orphans(pool: &sqlx::PgPool, storage_root: &str) -> Result<usize, PackError> {
    let root = Path::new(storage_root).join("reader_packs");
    let Ok(mut entries) = tokio::fs::read_dir(&root).await else {
        return Ok(0);
    };
    let mut dirs = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|n| Uuid::parse_str(n).ok())
        {
            dirs.push(id);
        }
    }
    let live = crate::repositories::book_files::existing_ids(pool, &dirs)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut removed = 0;
    for id in dirs.into_iter().filter(|id| !live.contains(id)) {
        tokio::fs::remove_dir_all(root.join(id.to_string())).await?;
        loaded().retain(|dir, _| !dir.starts_with(root.join(id.to_string())));
        removed += 1;
    }
    Ok(removed)
}

/// Whether `file` already has a pack of the current generation.
pub async fn is_built(storage_root: &str, file: &ReaderFile) -> bool {
    tokio::fs::try_exists(pack_dir(storage_root, file).join("sidecars.json.gz"))
        .await
        .unwrap_or(false)
}

fn build_into(source: &Path, dir: &Path) -> Result<(), PackError> {
    let started = std::time::Instant::now();
    let bytes = std::fs::read(source)?;
    let pack = reader_pack::build(&bytes)?;
    let parent = dir.parent().expect("pack dir has a parent");
    std::fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".staging-{}", Uuid::new_v4()));
    std::fs::create_dir_all(staging.join("chapters"))?;
    write_gz(&staging.join("manifest.json.gz"), &pack.manifest)?;
    for (n, chapter) in pack.chapters.iter().enumerate() {
        write_gz(
            &staging.join("chapters").join(format!("{n}.json.gz")),
            chapter,
        )?;
    }
    write_gz(&staging.join("sidecars.json.gz"), &pack.sidecars)?;
    match std::fs::rename(&staging, dir) {
        Ok(()) => {}
        Err(_) if dir.join("sidecars.json.gz").exists() => {
            std::fs::remove_dir_all(&staging)?;
        }
        Err(e) => return Err(e.into()),
    }
    for entry in std::fs::read_dir(parent)?.flatten() {
        if entry.path() != dir {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    tracing::info!(
        source = %source.display(),
        chapters = pack.chapters.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "built reader pack"
    );
    Ok(())
}

fn write_gz<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), PackError> {
    let mut gz = GzEncoder::new(std::fs::File::create(path)?, Compression::default());
    serde_json::to_writer(&mut gz, value)?;
    gz.finish()?.flush()?;
    Ok(())
}

fn read_gz<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, PackError> {
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(path)?);
    Ok(serde_json::from_reader(std::io::BufReader::new(gz))?)
}

fn load(dir: &Path) -> Result<LoadedPack, PackError> {
    let manifest: reader_pack::Manifest = read_gz(&dir.join("manifest.json.gz"))?;
    let sidecars: Vec<ChapterSidecar> = read_gz(&dir.join("sidecars.json.gz"))?;
    let etag = format!(
        "\"{}\"",
        dir.file_name().and_then(|n| n.to_str()).unwrap_or_default()
    );
    Ok(LoadedPack {
        dir: dir.to_path_buf(),
        etag,
        hrefs: manifest.chapters.iter().map(|c| c.href.clone()).collect(),
        titles: manifest.chapters.into_iter().map(|c| c.title).collect(),
        sidecars,
    })
}
