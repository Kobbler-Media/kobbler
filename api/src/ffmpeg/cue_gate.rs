//! One playback session's view of the cue trim.
//!
//! The trimmed index is spliced into whatever bytes the player reads, so every response
//! in a session has to agree on it: an index that is patched in one read and original
//! in the next is a corrupt index. The gate decides once per session and remembers,
//! including when the answer is "not patched" because the upstream was slow or failed.
//!
//! Nothing outside the Cues element is touched, so the decision is only waited on by a
//! chunk that reaches the element; everything before it streams immediately.

use std::sync::Arc;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use tokio::sync::OnceCell;

use super::mkv_cue_cache::{ByteSource, CueTrimCache, CueTrimPolicy, HeadMemo, locate_cues};
use super::mkv_cue_trim::CuePatch;

pub struct CueGate {
    build: Option<Build>,
    cues_at: OnceCell<Option<u64>>,
    patch: OnceCell<Option<Arc<CuePatch>>>,
}

struct Build {
    cache: Arc<CueTrimCache>,
    identity: String,
    policy: CueTrimPolicy,
    source: HeadMemo,
}

impl CueGate {
    pub fn off() -> Arc<Self> {
        Arc::new(Self {
            build: None,
            cues_at: OnceCell::new_with(Some(None)),
            patch: OnceCell::new_with(Some(None)),
        })
    }

    pub fn new(
        cache: Arc<CueTrimCache>,
        identity: String,
        policy: CueTrimPolicy,
        source: Box<dyn ByteSource>,
    ) -> Arc<Self> {
        Arc::new(Self {
            build: Some(Build {
                cache,
                identity,
                policy,
                source: HeadMemo::new(source),
            }),
            cues_at: OnceCell::new(),
            patch: OnceCell::new(),
        })
    }

    /// Settle the session's decision ahead of the read that needs it.
    pub async fn warm(&self) {
        if let Some(at) = self.cues_at().await {
            self.patch_for(at, 1).await;
        }
    }

    async fn cues_at(&self) -> Option<u64> {
        let Some(build) = &self.build else {
            return None;
        };
        *self
            .cues_at
            .get_or_init(|| locate_cues(&build.source))
            .await
    }

    /// The patch to splice into `len` bytes read at `start`, or None to pass them
    /// through untouched.
    pub async fn patch_for(&self, start: u64, len: u64) -> Option<Arc<CuePatch>> {
        if let Some(decided) = self.patch.get() {
            return decided.clone();
        }
        let build = self.build.as_ref()?;
        let Some(cues_at) = self.cues_at().await else {
            let _ = self.patch.set(None);
            return self.patch.get().cloned().flatten();
        };
        if start.saturating_add(len) <= cues_at {
            return None;
        }
        self.patch
            .get_or_init(|| {
                build
                    .cache
                    .get(&build.identity, &build.policy, &build.source)
            })
            .await
            .clone()
    }

    /// A range lying wholly inside the trimmed index, answered without reading it.
    /// Only once the index has been located — a read that cannot know yet goes to the
    /// source and is patched on the way through instead.
    pub async fn whole_range(&self, start: u64, len: u64) -> Option<Vec<u8>> {
        let cues_at = (*self.cues_at.get()?)?;
        if start.saturating_add(len) <= cues_at {
            return None;
        }
        self.patch_for(start, len)
            .await?
            .whole_range(start, len as usize)
    }
}

/// Splice the session's patch into a byte stream that starts at `start` in the file.
pub fn gated<S, E>(
    inner: S,
    start: u64,
    gate: Arc<CueGate>,
) -> impl Stream<Item = Result<Bytes, E>> + Send
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    inner
        .scan(start, |pos, item| {
            let at = *pos;
            if let Ok(chunk) = &item {
                *pos += chunk.len() as u64;
            }
            futures::future::ready(Some((at, item)))
        })
        .then(move |(at, item)| {
            let gate = gate.clone();
            async move {
                let chunk = item?;
                match gate.patch_for(at, chunk.len() as u64).await {
                    Some(p) if p.overlaps(at, chunk.len()) => {
                        let mut buf = chunk.to_vec();
                        p.apply(at, &mut buf);
                        Ok(Bytes::from(buf))
                    }
                    _ => Ok(chunk),
                }
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::mkv_cue_trim::fixture::{build_file, many_langs};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Source {
        data: Vec<u8>,
        fail: AtomicBool,
        reads: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ByteSource for Arc<Source> {
        async fn read_at(&self, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err(std::io::Error::other("upstream down"));
            }
            let start = (offset as usize).min(self.data.len());
            let end = (start + len).min(self.data.len());
            Ok(self.data[start..end].to_vec())
        }
    }

    fn policy() -> CueTrimPolicy {
        CueTrimPolicy {
            sidecars_available: true,
            preferred_languages: vec!["english".into()],
        }
    }

    fn gate_over(data: Vec<u8>) -> (Arc<CueGate>, Arc<Source>) {
        let src = Arc::new(Source {
            data,
            fail: AtomicBool::new(false),
            reads: AtomicUsize::new(0),
        });
        let gate = CueGate::new(
            Arc::new(CueTrimCache::new()),
            "id".into(),
            policy(),
            Box::new(src.clone()),
        );
        (gate, src)
    }

    async fn collect(gate: Arc<CueGate>, data: &[u8], start: usize, chunk: usize) -> Vec<u8> {
        let chunks: Vec<_> = data[start..]
            .chunks(chunk)
            .map(|c| Ok::<_, std::io::Error>(Bytes::copy_from_slice(c)))
            .collect();
        gated(futures::stream::iter(chunks), start as u64, gate)
            .map(|r| r.unwrap().to_vec())
            .concat()
            .await
    }

    #[tokio::test]
    async fn bytes_before_the_index_never_build_the_patch() {
        let built = build_file(&many_langs(), 400, None);
        let (gate, src) = gate_over(built.file.clone());
        let head = built.cues_offset as usize;
        let out = collect(gate.clone(), &built.file[..head], 0, 4096).await;
        assert_eq!(out, &built.file[..head]);
        assert_eq!(
            src.reads.load(Ordering::SeqCst),
            1,
            "only the head should have been read to find the index"
        );
        assert!(gate.patch.get().is_none(), "the patch was decided early");
    }

    #[tokio::test]
    async fn the_index_is_patched_and_stays_patched() {
        let built = build_file(&many_langs(), 400, None);
        let (gate, src) = gate_over(built.file.clone());
        let first = collect(gate.clone(), &built.file, 0, 7000).await;
        assert_ne!(first, built.file, "the index was not trimmed");
        assert_eq!(first.len(), built.file.len());

        src.fail.store(true, Ordering::SeqCst);
        let again = collect(gate, &built.file, 0, 1 << 20).await;
        assert_eq!(first, again);
    }

    #[tokio::test]
    async fn a_failed_build_is_remembered_as_unpatched_for_the_session() {
        let built = build_file(&many_langs(), 400, None);
        let (gate, src) = gate_over(built.file.clone());
        src.fail.store(true, Ordering::SeqCst);
        let first = collect(gate.clone(), &built.file, 0, 1 << 20).await;
        assert_eq!(first, built.file);

        src.fail.store(false, Ordering::SeqCst);
        let again = collect(gate, &built.file, 0, 1 << 20).await;
        assert_eq!(again, built.file, "the session switched to a patched index");
    }

    #[tokio::test]
    async fn an_off_gate_passes_everything() {
        let built = build_file(&many_langs(), 400, None);
        let out = collect(CueGate::off(), &built.file, 0, 999).await;
        assert_eq!(out, built.file);
    }
}
