//! Per-file serialization for the ingest pipeline.
//!
//! `media_probe` runs at concurrency 16 and `media_analyze` at 2, and neither
//! had any guard against two jobs working the same file at once. When that
//! happens both write the same cache directory and both bump
//! `media_cache_version`, so the sidecars can interleave and the winner is
//! whichever finished last. Rare — 7 overlapping pairs in 26,937 recorded runs
//! — but silent, and the failure it produces looks exactly like the corruption
//! we already spent two months not noticing.
//!
//! Queue, do not drop. A second request for a file is usually a *later* view of
//! it: the download finished, the file changed, and the new pass is the one
//! that gets the cache right. Dropping duplicates would discard exactly the
//! re-examination that saves a file analyzed while it was still being written.
//! So callers wait their turn and then run.
//!
//! In-process only, which is sufficient because every handler runs inside the
//! one API process. It is not a distributed lock and must not be relied on as
//! one if handlers ever move out of process.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use uuid::Uuid;

/// Live locks, keyed by file id. Entries are dropped once nothing holds or
/// awaits them, so the map tracks in-flight work rather than every file ever
/// seen.
static FILE_LOCKS: std::sync::LazyLock<Mutex<HashMap<Uuid, Arc<AsyncMutex<()>>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Held for as long as the caller is working on the file.
pub struct FileGuard {
    file_id: Uuid,
    _guard: OwnedMutexGuard<()>,
}

impl Drop for FileGuard {
    fn drop(&mut self) {
        let mut map = match FILE_LOCKS.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        // Two strong refs means this map entry and our guard: nobody else is
        // waiting, so the entry can go. Any higher count means a waiter still
        // needs it.
        if let Some(arc) = map.get(&self.file_id)
            && Arc::strong_count(arc) <= 2
        {
            map.remove(&self.file_id);
        }
    }
}

/// Wait until no other task is working on `file_id`, then take the lock.
///
/// The returned guard releases on drop, including on early return or panic.
pub async fn lock_file(file_id: Uuid) -> FileGuard {
    let arc = {
        let mut map = match FILE_LOCKS.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        map.entry(file_id)
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    };
    let guard = arc.lock_owned().await;
    FileGuard {
        file_id,
        _guard: guard,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn second_caller_waits_for_the_first() {
        let id = Uuid::new_v4();
        let concurrent = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let concurrent = concurrent.clone();
            let peak = peak.clone();
            handles.push(tokio::spawn(async move {
                let _g = lock_file(id).await;
                let now = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::task::yield_now().await;
                concurrent.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1, "only one holder at a time");
    }

    #[tokio::test]
    async fn different_files_do_not_block_each_other() {
        let a = lock_file(Uuid::new_v4()).await;
        // Would deadlock if the lock were global rather than per file.
        let b = lock_file(Uuid::new_v4()).await;
        drop((a, b));
    }

    #[tokio::test]
    async fn entries_are_released_when_idle() {
        let id = Uuid::new_v4();
        {
            let _g = lock_file(id).await;
        }
        let map = FILE_LOCKS.lock().unwrap();
        assert!(!map.contains_key(&id), "idle entry should be cleaned up");
    }
}
