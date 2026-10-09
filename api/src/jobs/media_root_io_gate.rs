//! Per-media-root semaphores that gate heavy NAS IO across handlers.
//!
//! Each media root has a configurable `max_concurrent_io`. Jobs that stream
//! bytes from media files (thumbnails, skip_detect, media_extract) must hold
//! a permit before starting their ffmpeg work. This prevents the SMB credit
//! window collapse measured on slow NAS hardware on 2026-04-13, where a
//! single backfill thread would thrash a QNAP TS-469's read credits and
//! starve every other process on the box.
//!
//! The gate is keyed by `media_root_id`. The first acquire for a root lazily
//! reads its cap from the DB. Cap changes via the admin UI call `reload` to
//! swap in a fresh semaphore — in-flight permits keep running on the old one
//! until they drop, which is fine (eventual consistency, no atomic hand-off
//! required).

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};
use uuid::Uuid;

use crate::repositories::media_roots;

#[derive(Clone)]
pub struct MediaRootIoGate {
    pool: PgPool,
    semaphores: Arc<RwLock<HashMap<Uuid, Arc<Semaphore>>>>,
}

impl MediaRootIoGate {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            semaphores: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Acquire a permit for the given media root. Held until the returned
    /// permit is dropped. Lazy-initialises the semaphore from DB on first use.
    pub async fn acquire(&self, media_root_id: Uuid) -> OwnedSemaphorePermit {
        // Fast path: read lock, check map.
        {
            let map = self.semaphores.read().await;
            if let Some(sem) = map.get(&media_root_id) {
                return sem
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("media root io semaphore not closed");
            }
        }
        // Slow path: load cap from DB, take write lock, double-check, insert.
        let cap = media_roots::get_max_concurrent_io(&self.pool, media_root_id)
            .await
            .unwrap_or(1)
            .max(1) as usize;
        let sem = {
            let mut map = self.semaphores.write().await;
            map.entry(media_root_id)
                .or_insert_with(|| Arc::new(Semaphore::new(cap)))
                .clone()
        };
        sem.acquire_owned()
            .await
            .expect("media root io semaphore not closed")
    }

    /// Acquire permits for several media roots at once, returned in ascending
    /// id order to prevent deadlocks when two jobs touch the same pair of
    /// roots from different directions.
    pub async fn acquire_many(&self, mut ids: Vec<Uuid>) -> Vec<OwnedSemaphorePermit> {
        ids.sort();
        ids.dedup();
        let mut permits = Vec::with_capacity(ids.len());
        for id in ids {
            permits.push(self.acquire(id).await);
        }
        permits
    }

    /// Reload the cap for one media root from DB and recreate its semaphore.
    /// Called by the media root edit endpoint after a successful update so
    /// the new cap takes effect without an api restart.
    ///
    /// Existing in-flight permits keep running on the OLD semaphore. New
    /// acquires use the new one.
    pub async fn reload(&self, media_root_id: Uuid) {
        let cap = media_roots::get_max_concurrent_io(&self.pool, media_root_id)
            .await
            .unwrap_or(1)
            .max(1) as usize;
        let mut map = self.semaphores.write().await;
        map.insert(media_root_id, Arc::new(Semaphore::new(cap)));
    }
}
