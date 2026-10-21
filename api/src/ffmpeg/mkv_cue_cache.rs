//! Fetch a file's trimmed cue index once, then reuse it for every range request.
//!
//! The trim itself is a pure function over the Cues element; this is the part that
//! knows how to get those bytes from a local file or from a debrid URL, and how not
//! to do it twice. Building is a ~1.7MB read plus a linear walk, so it happens once
//! per (file, policy) and every later range request is a memcpy.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use dashmap::DashMap;
use futures::{Stream, StreamExt};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::OnceCell;
use tracing::{debug, warn};

use super::mkv_cue_trim::{
    CuePatch, TrimOutcome, build_patch, cues_element_len, kept_tracks, read_layout,
};

/// Enough to cover the SeekHead and Tracks of anything mkvmerge produces.
const HEAD_BYTES: usize = 512 * 1024;
/// A cue index larger than this is not something we want to hold per file.
const MAX_CUES_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;

#[async_trait::async_trait]
pub trait ByteSource: Send + Sync {
    async fn read_at(&self, offset: u64, len: usize) -> std::io::Result<Vec<u8>>;
}

pub struct FileByteSource {
    pub path: PathBuf,
}

#[async_trait::async_trait]
impl ByteSource for FileByteSource {
    async fn read_at(&self, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
        let mut f = tokio::fs::File::open(&self.path).await?;
        f.seek(std::io::SeekFrom::Start(offset)).await?;
        let mut buf = vec![0u8; len];
        let mut read = 0usize;
        while read < len {
            match f.read(&mut buf[read..]).await? {
                0 => break,
                n => read += n,
            }
        }
        buf.truncate(read);
        Ok(buf)
    }
}

pub struct HttpByteSource {
    pub client: reqwest::Client,
    pub url: String,
}

#[async_trait::async_trait]
impl ByteSource for HttpByteSource {
    async fn read_at(&self, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let end = offset + len as u64 - 1;
        let resp = self
            .client
            .get(&self.url)
            .header(reqwest::header::RANGE, format!("bytes={offset}-{end}"))
            .send()
            .await
            .map_err(std::io::Error::other)?;
        if !resp.status().is_success() {
            return Err(std::io::Error::other(format!(
                "range request returned {}",
                resp.status()
            )));
        }
        // A server that ignores Range answers 200 with the whole file. That is the
        // wrong bytes for any offset but zero, and it will not start honouring them
        // later, so it is reported as unsupported rather than as a failed read.
        if offset > 0 && resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "upstream ignored Range",
            ));
        }

        // Taken chunk by chunk and stopped at the length asked for: an upstream that
        // ignored the Range on a head read would otherwise hand us the whole film to
        // buffer in memory.
        let mut out = Vec::with_capacity(len.min(1 << 20));
        let mut body = resp.bytes_stream();
        while out.len() < len {
            let Some(chunk) = body.next().await else {
                break;
            };
            let chunk = chunk.map_err(std::io::Error::other)?;
            let take = (len - out.len()).min(chunk.len());
            out.extend_from_slice(&chunk[..take]);
        }
        Ok(out)
    }
}

/// The session's cache file where the download has reached, the debrid host beyond it.
pub struct LocalFirstByteSource {
    pub local: Option<FileByteSource>,
    pub total: u64,
    pub remote: Option<HttpByteSource>,
}

#[async_trait::async_trait]
impl ByteSource for LocalFirstByteSource {
    async fn read_at(&self, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
        let want = len.min(self.total.saturating_sub(offset) as usize);
        if let Some(local) = &self.local
            && let Ok(bytes) = local.read_at(offset, want).await
            && bytes.len() == want
        {
            return Ok(bytes);
        }
        match &self.remote {
            Some(remote) => remote.read_at(offset, len).await,
            // A short read here is a download still in progress, not the end of the
            // file; reporting it as one would cache "not worth patching" for good.
            None => Err(std::io::Error::other("range not downloaded yet")),
        }
    }
}

/// Remembers the head read, which both locating the index and building its patch need.
pub struct HeadMemo {
    inner: Box<dyn ByteSource>,
    head: OnceCell<Vec<u8>>,
}

impl HeadMemo {
    pub fn new(inner: Box<dyn ByteSource>) -> Self {
        Self {
            inner,
            head: OnceCell::new(),
        }
    }
}

#[async_trait::async_trait]
impl ByteSource for HeadMemo {
    async fn read_at(&self, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
        if offset != 0 || len != HEAD_BYTES {
            return self.inner.read_at(offset, len).await;
        }
        self.head
            .get_or_try_init(|| self.inner.read_at(0, HEAD_BYTES))
            .await
            .cloned()
    }
}

/// Where the Cues element starts, from the head of the file alone.
pub async fn locate_cues(source: &dyn ByteSource) -> Option<u64> {
    let head = source.read_at(0, HEAD_BYTES).await.ok()?;
    read_layout(&head).map(|l| l.element_offset)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueTrimPolicy {
    /// The server is handing out extracted subtitle files, so the client will not be
    /// selecting an embedded subtitle track and none of them need an index.
    pub sidecars_available: bool,
    pub preferred_languages: Vec<String>,
}

impl CueTrimPolicy {
    fn discriminator(&self) -> String {
        format!(
            "{}|{}",
            self.sidecars_available,
            self.preferred_languages.join(",")
        )
    }
}

struct Entry {
    cell: Arc<OnceCell<Option<Arc<CuePatch>>>>,
    last_used: AtomicU64,
}

#[derive(Default)]
pub struct CueTrimCache {
    entries: DashMap<String, Entry>,
    clock: AtomicU64,
}

impl CueTrimCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The trimmed index for this file under this policy, or None when there is
    /// nothing worth patching — in which case the caller serves the file untouched.
    pub async fn get(
        &self,
        identity: &str,
        policy: &CueTrimPolicy,
        source: &dyn ByteSource,
    ) -> Option<Arc<CuePatch>> {
        let key = format!("{identity}|{}", policy.discriminator());
        let now = self.clock.fetch_add(1, Ordering::Relaxed);

        // The guard must be dropped before awaiting, or a second request for the same
        // file deadlocks on the shard lock.
        let cell = {
            let entry = self.entries.entry(key).or_insert_with(|| Entry {
                cell: Arc::new(OnceCell::new()),
                last_used: AtomicU64::new(now),
            });
            entry.last_used.store(now, Ordering::Relaxed);
            entry.cell.clone()
        };
        self.evict_if_needed();

        // A transient read failure must not be remembered: caching it would disable
        // trimming for this file until the entry is evicted.
        match cell.get_or_try_init(|| build(policy, source)).await {
            Ok(patch) => patch.clone(),
            Err(()) => None,
        }
    }

    fn evict_if_needed(&self) {
        while self.entries.len() > MAX_ENTRIES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|e| e.last_used.load(Ordering::Relaxed))
                .map(|e| e.key().clone());
            match oldest {
                Some(k) => {
                    self.entries.remove(&k);
                }
                None => break,
            }
        }
    }

    #[cfg(test)]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

/// `Err` means "ask again later"; `Ok(None)` means "this file is not worth patching".
async fn build(
    policy: &CueTrimPolicy,
    source: &dyn ByteSource,
) -> Result<Option<Arc<CuePatch>>, ()> {
    // Ok(None) from a read means the source will never serve this, so the decision
    // not to patch is worth remembering; Err means try again on the next request.
    let read = async |offset: u64, len: usize| -> Result<Option<Vec<u8>>, ()> {
        match source.read_at(offset, len).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {
                debug!(error = %e, "cue trim: source cannot serve ranges");
                Ok(None)
            }
            Err(e) => {
                debug!(error = %e, offset, "cue trim: read failed");
                Err(())
            }
        }
    };

    let Some(head) = read(0, HEAD_BYTES).await? else {
        return Ok(None);
    };
    let Some(layout) = read_layout(&head) else {
        return Ok(None);
    };
    let keep = kept_tracks(
        &layout.tracks,
        policy.sidecars_available,
        &policy.preferred_languages,
    );

    let Some(header) = read(layout.element_offset, 16).await? else {
        return Ok(None);
    };
    let Some(element_len) = cues_element_len(&header) else {
        return Ok(None);
    };
    if element_len > MAX_CUES_BYTES {
        warn!(element_len, "cue trim: index too large to hold");
        return Ok(None);
    }

    let Some(element) = read(layout.element_offset, element_len).await? else {
        return Ok(None);
    };
    if element.len() != element_len {
        return Ok(None);
    }

    Ok(match build_patch(&element, layout.element_offset, &keep) {
        TrimOutcome::Trimmed(patch) => {
            debug!(
                original_points = patch.original_points,
                kept_points = patch.kept_points,
                bytes = element_len,
                "cue trim: index trimmed"
            );
            Some(Arc::new(*patch))
        }
        TrimOutcome::NotWorthIt | TrimOutcome::Unsupported => None,
    })
}

/// Splice the patch into a byte stream that starts at `start` in the file.
pub fn patched_stream<S, E>(
    inner: S,
    start: u64,
    patch: Arc<CuePatch>,
) -> impl Stream<Item = Result<Bytes, E>> + Send
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    inner.scan(start, move |pos, item| {
        let out = item.map(|chunk| {
            let at = *pos;
            *pos += chunk.len() as u64;
            if !patch.overlaps(at, chunk.len()) {
                return chunk;
            }
            let mut buf = chunk.to_vec();
            patch.apply(at, &mut buf);
            Bytes::from(buf)
        });
        futures::future::ready(Some(out))
    })
}

/// Only Matroska has a Cues element; everything else is served untouched.
pub fn is_matroska(container_or_name: &str) -> bool {
    let s = container_or_name.to_ascii_lowercase();
    s.contains("matroska") || s.ends_with(".mkv") || s == "mkv" || s.ends_with(".mka")
}

/// The trimmed index for a file on disk, keyed so an edited file re-reads.
pub async fn patch_for_local_file(
    cache: &CueTrimCache,
    path: &std::path::Path,
    container_or_name: &str,
    policy: &CueTrimPolicy,
    enabled: bool,
) -> Option<Arc<CuePatch>> {
    if !enabled || !is_matroska(container_or_name) {
        return None;
    }
    let meta = tokio::fs::metadata(path).await.ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let identity = format!("file:{}:{}:{}", path.display(), meta.len(), mtime);
    cache
        .get(
            &identity,
            policy,
            &FileByteSource {
                path: path.to_path_buf(),
            },
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::mkv_cue_trim::fixture::{Built, build_file, many_langs};
    use std::sync::atomic::AtomicUsize;

    struct CountingSource {
        data: Vec<u8>,
        reads: AtomicUsize,
        bytes: AtomicUsize,
    }

    impl CountingSource {
        fn new(data: Vec<u8>) -> Self {
            Self {
                data,
                reads: AtomicUsize::new(0),
                bytes: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl ByteSource for CountingSource {
        async fn read_at(&self, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let start = (offset as usize).min(self.data.len());
            let end = (start + len).min(self.data.len());
            self.bytes.fetch_add(end - start, Ordering::SeqCst);
            Ok(self.data[start..end].to_vec())
        }
    }

    fn policy(sidecars: bool) -> CueTrimPolicy {
        CueTrimPolicy {
            sidecars_available: sidecars,
            preferred_languages: vec!["english".into()],
        }
    }

    fn fat_file() -> Built {
        build_file(&many_langs(), 400, None)
    }

    #[tokio::test]
    async fn a_fat_index_is_trimmed_from_a_byte_source() {
        let built = fat_file();
        let src = CountingSource::new(built.file.clone());
        let cache = CueTrimCache::new();

        let patch = cache.get("f", &policy(true), &src).await.expect("patch");
        assert_eq!(patch.offset, built.cues_offset);
        // The patch covers exactly the Cues element and nothing beyond it.
        assert_eq!(patch.region_len(), built.cues_len);
        assert!(patch.kept_points < patch.original_points);
    }

    #[tokio::test]
    async fn the_index_is_read_once_however_many_requests_arrive() {
        let built = fat_file();
        let src = CountingSource::new(built.file.clone());
        let cache = CueTrimCache::new();

        let first = cache.get("f", &policy(true), &src).await;
        let reads = src.reads.load(Ordering::SeqCst);
        assert!(first.is_some());

        for _ in 0..20 {
            assert_eq!(cache.get("f", &policy(true), &src).await, first);
        }
        assert_eq!(
            src.reads.load(Ordering::SeqCst),
            reads,
            "the cue index was re-read on a cache hit"
        );
        assert_eq!(cache.entry_count(), 1);
    }

    #[tokio::test]
    async fn concurrent_first_requests_do_not_both_build() {
        let built = fat_file();
        let src = Arc::new(CountingSource::new(built.file.clone()));
        let cache = Arc::new(CueTrimCache::new());

        let mut handles = Vec::new();
        for _ in 0..8 {
            let (cache, src) = (cache.clone(), src.clone());
            handles.push(tokio::spawn(async move {
                cache.get("f", &policy(true), src.as_ref()).await
            }));
        }
        let mut results = Vec::new();
        for h in handles {
            results.push(h.await.unwrap());
        }
        assert!(results.iter().all(|r| r.is_some()));
        assert!(results.windows(2).all(|w| w[0] == w[1]));
        // Three reads build one index: head, cue header, cue body.
        assert_eq!(src.reads.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_different_policy_gets_its_own_entry() {
        let built = fat_file();
        let src = CountingSource::new(built.file.clone());
        let cache = CueTrimCache::new();

        let with_sidecars = cache.get("f", &policy(true), &src).await.unwrap();
        let without = cache.get("f", &policy(false), &src).await.unwrap();
        assert_eq!(cache.entry_count(), 2);
        assert!(
            without.kept_points > with_sidecars.kept_points,
            "keeping the preferred language should retain more points"
        );
    }

    #[tokio::test]
    async fn a_lean_file_caches_the_decision_not_to_patch() {
        let built = build_file(&["eng"], 4, None);
        let src = CountingSource::new(built.file.clone());
        let cache = CueTrimCache::new();

        assert!(cache.get("f", &policy(true), &src).await.is_none());
        let reads = src.reads.load(Ordering::SeqCst);
        assert!(cache.get("f", &policy(true), &src).await.is_none());
        assert_eq!(
            src.reads.load(Ordering::SeqCst),
            reads,
            "a file not worth patching was re-read"
        );
    }

    #[tokio::test]
    async fn rubbish_bytes_are_not_patched() {
        let src = CountingSource::new(vec![0u8; 4096]);
        let cache = CueTrimCache::new();
        assert!(cache.get("f", &policy(true), &src).await.is_none());
    }

    #[tokio::test]
    async fn a_truncated_file_is_not_patched() {
        let built = fat_file();
        let mut truncated = built.file.clone();
        truncated.truncate(built.cues_offset as usize + 32);
        let src = CountingSource::new(truncated);
        let cache = CueTrimCache::new();
        assert!(cache.get("f", &policy(true), &src).await.is_none());
    }

    #[tokio::test]
    async fn the_cache_stays_bounded() {
        let built = fat_file();
        let src = CountingSource::new(built.file.clone());
        let cache = CueTrimCache::new();
        for i in 0..MAX_ENTRIES + 20 {
            cache.get(&format!("f{i}"), &policy(true), &src).await;
        }
        assert!(
            cache.entry_count() <= MAX_ENTRIES,
            "cache grew to {}",
            cache.entry_count()
        );
    }

    #[tokio::test]
    async fn a_file_source_reads_the_range_it_is_asked_for() {
        let dir = std::env::temp_dir().join(format!("cuetrim-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("f.bin");
        let data: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
        tokio::fs::write(&path, &data).await.unwrap();

        let src = FileByteSource { path: path.clone() };
        assert_eq!(src.read_at(0, 16).await.unwrap(), &data[..16]);
        assert_eq!(src.read_at(1234, 500).await.unwrap(), &data[1234..1734]);
        // Past the end truncates rather than erroring, which is how a partial
        // download is detected.
        assert_eq!(src.read_at(9_990, 100).await.unwrap(), &data[9_990..]);

        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[tokio::test]
    async fn a_local_file_is_only_trimmed_when_it_is_matroska_and_enabled() {
        let built = fat_file();
        let dir = std::env::temp_dir().join(format!("cuetrim-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("movie.mkv");
        tokio::fs::write(&path, &built.file).await.unwrap();
        let cache = CueTrimCache::new();

        assert!(
            patch_for_local_file(&cache, &path, "matroska,webm", &policy(true), true)
                .await
                .is_some()
        );
        assert!(
            patch_for_local_file(&cache, &path, "mp4", &policy(true), true)
                .await
                .is_none(),
            "a non-matroska container was patched"
        );
        assert!(
            patch_for_local_file(&cache, &path, "matroska", &policy(true), false)
                .await
                .is_none(),
            "trimming ran while disabled"
        );

        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[test]
    fn container_detection() {
        assert!(is_matroska("matroska,webm"));
        assert!(is_matroska("mkv"));
        assert!(is_matroska("Some.Film.2026.MKV"));
        assert!(!is_matroska("mp4"));
        assert!(!is_matroska("mov,mp4,m4a,3gp,3g2,mj2"));
        assert!(!is_matroska("film.mp4"));
    }

    #[tokio::test]
    async fn a_patched_stream_yields_the_patched_bytes_at_every_chunk_size() {
        let built = fat_file();
        let src = CountingSource::new(built.file.clone());
        let cache = CueTrimCache::new();
        let patch = cache.get("f", &policy(true), &src).await.unwrap();

        let mut expected = built.file.clone();
        patch.apply(0, &mut expected);

        for chunk in [1usize, 3, 1024, 100_000] {
            for start in [0u64, patch.offset - 5, patch.offset, patch.end() - 3] {
                let tail: Vec<Bytes> = built.file[start as usize..]
                    .chunks(chunk)
                    .map(Bytes::copy_from_slice)
                    .collect();
                let inner =
                    futures::stream::iter(tail.into_iter().map(Ok::<Bytes, std::io::Error>));
                let out: Vec<Bytes> = patched_stream(inner, start, patch.clone())
                    .map(|r| r.unwrap())
                    .collect()
                    .await;
                let joined: Vec<u8> = out.iter().flat_map(|b| b.to_vec()).collect();
                assert_eq!(
                    joined,
                    &expected[start as usize..],
                    "start {start} chunk {chunk}"
                );
            }
        }
    }
}

#[cfg(test)]
mod real_file {
    use super::CueTrimPolicy;

    fn policy(sidecars_available: bool) -> CueTrimPolicy {
        CueTrimPolicy {
            sidecars_available,
            preferred_languages: vec!["english".into()],
        }
    }

    /// Point this at an actual release to see what the trim does to it:
    /// `KOBBLER_CUE_TRIM_FILE=/path/film.mkv cargo test -p api --lib real_file -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn report() {
        let Ok(path) = std::env::var("KOBBLER_CUE_TRIM_FILE") else {
            println!("set KOBBLER_CUE_TRIM_FILE");
            return;
        };
        let path = std::path::PathBuf::from(path);
        let meta = tokio::fs::metadata(&path).await.expect("stat");
        println!("\n{} ({} bytes)", path.display(), meta.len());

        let cache = super::CueTrimCache::new();
        for (label, pol) in [
            (
                "sidecars available (drop every subtitle index)",
                policy(true),
            ),
            ("no sidecars (keep preferred language)", policy(false)),
        ] {
            let started = std::time::Instant::now();
            let patch = super::patch_for_local_file(&cache, &path, "matroska", &pol, true).await;
            let elapsed = started.elapsed();
            match patch {
                Some(p) => println!(
                    "  {label}\n    {} -> {} cue points, element {} B, {} B held ({:.1}% of points), built in {:?}",
                    p.original_points,
                    p.kept_points,
                    p.region_len(),
                    p.stored_bytes(),
                    100.0 * p.kept_points as f64 / p.original_points.max(1) as f64,
                    elapsed
                ),
                None => println!("  {label}\n    no patch"),
            }
        }
    }
}
