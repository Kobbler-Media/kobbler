use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use tokio::sync::mpsc;
use tracing::{info, warn};
use uuid::Uuid;

use crate::repositories::livetv_leases;

/// How long a viewer's lease survives a channel change so flicking back is
/// instant. Bounded by the 4-tuner cap: any waiting acquire reaps it early.
pub const LEASE_LINGER: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HolderKind {
    Viewer,
    Recording,
}

impl HolderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Recording => "recording",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "viewer" => Some(Self::Viewer),
            "recording" => Some(Self::Recording),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    #[error("all {capacity} tuners are in use ({in_use} leases held)")]
    Exhausted { in_use: u32, capacity: u32 },
}

#[derive(Debug, Clone)]
pub struct LeaseInfo {
    pub id: Uuid,
    pub tuner_id: Uuid,
    pub channel_id: Uuid,
    pub tuner_index: Option<i32>,
    pub holder_kind: HolderKind,
    pub session_id: Option<String>,
    pub recording_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub acquired_at: OffsetDateTime,
    pub expires_at: Option<OffsetDateTime>,
}

pub struct LeaseRequest {
    pub tuner_id: Uuid,
    pub channel_id: Uuid,
    /// Two holders share a physical tune only when this matches. Transcode
    /// sessions on one channel share a single HTTP connection to the device;
    /// direct-play clients each open their own, which really does cost a tuner.
    pub share_key: String,
    pub capacity: u32,
    pub holder_kind: HolderKind,
    pub session_id: Option<String>,
    pub recording_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
}

/// Shared with every holder of a lease so a preempted viewer can find out
/// without polling the registry.
#[derive(Debug, Default)]
pub struct LeaseFlags {
    evicted: AtomicBool,
    preempted_by: Mutex<Option<Uuid>>,
}

impl LeaseFlags {
    pub fn is_evicted(&self) -> bool {
        self.evicted.load(Ordering::Acquire)
    }

    pub fn preempted_by(&self) -> Option<Uuid> {
        *self.preempted_by.lock().expect("lease flags not poisoned")
    }

    fn evict(&self, by: Option<Uuid>) {
        *self.preempted_by.lock().expect("lease flags not poisoned") = by;
        self.evicted.store(true, Ordering::Release);
    }
}

/// A held slot. Dropping it decrements the holder count; the last drop starts
/// the linger window rather than releasing outright.
pub struct LeaseToken {
    pub info: LeaseInfo,
    pub flags: Arc<LeaseFlags>,
    /// True only for the holder that created the slot — the one that owes the
    /// `livetv_tuner_leases` row. Releases go through the async channel because
    /// `Drop` cannot await; the insert cannot, because the session row's foreign
    /// key needs it to already be there.
    pub fresh: bool,
    registry: Arc<TunerLeases>,
}

impl LeaseToken {
    pub fn id(&self) -> Uuid {
        self.info.id
    }

    pub fn is_evicted(&self) -> bool {
        self.flags.is_evicted()
    }
}

impl std::fmt::Debug for LeaseToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeaseToken")
            .field("info", &self.info)
            .finish()
    }
}

impl Drop for LeaseToken {
    fn drop(&mut self) {
        self.registry.release(self.info.id);
    }
}

struct Lease {
    info: LeaseInfo,
    share_key: String,
    flags: Arc<LeaseFlags>,
    holders: u32,
    linger_until: Option<Instant>,
}

#[derive(Default)]
struct Inner {
    leases: HashMap<Uuid, Lease>,
}

enum Persist {
    Released(Uuid),
    Promoted { id: Uuid, recording_id: Uuid },
    Heartbeat(Uuid),
}

/// The 4 hard slots. One Mutex over the whole map, so acquire is a genuine
/// compare-exchange rather than `provider_tracker`'s load-then-add.
pub struct TunerLeases {
    inner: Mutex<Inner>,
    persist: mpsc::UnboundedSender<Persist>,
}

impl TunerLeases {
    pub async fn persist_new(
        pool: &sqlx::PgPool,
        info: &LeaseInfo,
    ) -> Result<(), crate::repositories::RepositoryError> {
        livetv_leases::create(
            pool,
            livetv_leases::CreateLease {
                id: info.id,
                tuner_id: info.tuner_id,
                channel_id: info.channel_id,
                tuner_index: info.tuner_index,
                holder_kind: info.holder_kind.as_str().to_string(),
                // A lease is shared by every session on the same tune, so the row
                // names no single one of them.
                session_id: None,
                recording_id: info.recording_id,
                user_id: info.user_id,
                buffer_dir: None,
            },
        )
        .await
        .map(|_| ())
    }

    pub fn new(pool: sqlx::PgPool) -> Arc<Self> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let result = match msg {
                    Persist::Released(id) => livetv_leases::release(&pool, id).await.map(|_| ()),
                    Persist::Promoted { id, recording_id } => {
                        livetv_leases::promote_to_recording(&pool, id, recording_id).await
                    }
                    Persist::Heartbeat(id) => livetv_leases::touch(&pool, id).await,
                };
                if let Err(e) = result {
                    warn!(error = %e, "persisting tuner lease change failed");
                }
            }
        });
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
            persist: tx,
        })
    }

    /// Test/offline constructor: in-memory only, nothing is persisted.
    pub fn detached() -> Arc<Self> {
        let (tx, mut rx) = mpsc::unbounded_channel::<Persist>();
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
            persist: tx,
        })
    }

    pub fn acquire(self: &Arc<Self>, req: LeaseRequest) -> Result<LeaseToken, LeaseError> {
        let mut released: Vec<Uuid> = Vec::new();
        let result = {
            let mut inner = self.inner.lock().expect("lease registry not poisoned");
            let now = Instant::now();
            inner.leases.retain(|id, l| {
                let expired = l.holders == 0 && l.linger_until.is_some_and(|t| t <= now);
                if expired {
                    released.push(*id);
                }
                !expired
            });

            // Same channel on the same tuner is the same physical tune: share it.
            // A recording joining a viewer's lease upgrades the holder kind so the
            // slot stops being evictable.
            if let Some(l) = inner
                .leases
                .values_mut()
                .find(|l| l.info.tuner_id == req.tuner_id && l.share_key == req.share_key)
            {
                l.holders += 1;
                l.linger_until = None;
                l.info.expires_at = None;
                if req.holder_kind == HolderKind::Recording {
                    l.info.holder_kind = HolderKind::Recording;
                    if let Some(rid) = req.recording_id {
                        l.info.recording_id = Some(rid);
                        let _ = self.persist.send(Persist::Promoted {
                            id: l.info.id,
                            recording_id: rid,
                        });
                    }
                }
                Ok((l.info.clone(), l.flags.clone(), false))
            } else {
                let in_use = |inner: &Inner| {
                    inner
                        .leases
                        .values()
                        .filter(|l| l.info.tuner_id == req.tuner_id)
                        .count() as u32
                };

                if in_use(&inner) >= req.capacity {
                    // Somebody is waiting, so the linger window ends now.
                    let lingering: Vec<Uuid> = inner
                        .leases
                        .values()
                        .filter(|l| {
                            l.info.tuner_id == req.tuner_id
                                && l.holders == 0
                                && l.linger_until.is_some()
                        })
                        .map(|l| l.info.id)
                        .collect();
                    for id in lingering {
                        inner.leases.remove(&id);
                        released.push(id);
                    }
                }

                // A recording outranks a viewer: take the oldest viewer's slot.
                if in_use(&inner) >= req.capacity && req.holder_kind == HolderKind::Recording {
                    let victim = inner
                        .leases
                        .values()
                        .filter(|l| {
                            l.info.tuner_id == req.tuner_id
                                && l.info.holder_kind == HolderKind::Viewer
                        })
                        .min_by_key(|l| l.info.acquired_at)
                        .map(|l| l.info.id);
                    if let Some(id) = victim
                        && let Some(l) = inner.leases.remove(&id)
                    {
                        l.flags.evict(req.recording_id);
                        released.push(id);
                    }
                }

                let count = in_use(&inner);
                if count >= req.capacity {
                    Err(LeaseError::Exhausted {
                        in_use: count,
                        capacity: req.capacity,
                    })
                } else {
                    let info = LeaseInfo {
                        id: Uuid::now_v7(),
                        tuner_id: req.tuner_id,
                        channel_id: req.channel_id,
                        tuner_index: None,
                        holder_kind: req.holder_kind,
                        session_id: req.session_id.clone(),
                        recording_id: req.recording_id,
                        user_id: req.user_id,
                        acquired_at: OffsetDateTime::now_utc(),
                        expires_at: None,
                    };
                    let flags = Arc::new(LeaseFlags::default());
                    inner.leases.insert(
                        info.id,
                        Lease {
                            info: info.clone(),
                            share_key: req.share_key.clone(),
                            flags: flags.clone(),
                            holders: 1,
                            linger_until: None,
                        },
                    );
                    Ok((info, flags, true))
                }
            }
        };

        for id in released {
            let _ = self.persist.send(Persist::Released(id));
        }

        let (info, flags, fresh) = result?;
        Ok(LeaseToken {
            info,
            flags,
            fresh,
            registry: self.clone(),
        })
    }

    fn release(&self, id: Uuid) {
        let mut inner = self.inner.lock().expect("lease registry not poisoned");
        let Some(lease) = inner.leases.get_mut(&id) else {
            return;
        };
        lease.holders = lease.holders.saturating_sub(1);
        if lease.holders > 0 {
            return;
        }
        // A recording's slot is never held speculatively — it is done with it.
        if lease.info.holder_kind == HolderKind::Recording {
            inner.leases.remove(&id);
            drop(inner);
            let _ = self.persist.send(Persist::Released(id));
            return;
        }
        lease.linger_until = Some(Instant::now() + LEASE_LINGER);
        lease.info.expires_at = Some(OffsetDateTime::now_utc() + LEASE_LINGER);
    }

    /// Reap lingering leases whose window has closed. Called by the sweeper so a
    /// lease that nobody is competing for still frees its tuner.
    pub fn reap_expired(&self) -> Vec<Uuid> {
        let mut released = Vec::new();
        {
            let mut inner = self.inner.lock().expect("lease registry not poisoned");
            let now = Instant::now();
            inner.leases.retain(|id, l| {
                let expired = l.holders == 0 && l.linger_until.is_some_and(|t| t <= now);
                if expired {
                    released.push(*id);
                }
                !expired
            });
        }
        for id in &released {
            let _ = self.persist.send(Persist::Released(*id));
        }
        released
    }

    pub fn heartbeat(&self, id: Uuid) {
        let _ = self.persist.send(Persist::Heartbeat(id));
    }

    pub fn snapshot(&self) -> Vec<LeaseInfo> {
        let inner = self.inner.lock().expect("lease registry not poisoned");
        let mut items: Vec<LeaseInfo> = inner.leases.values().map(|l| l.info.clone()).collect();
        items.sort_by_key(|l| l.acquired_at);
        items
    }

    pub fn in_use(&self, tuner_id: Uuid) -> u32 {
        let inner = self.inner.lock().expect("lease registry not poisoned");
        inner
            .leases
            .values()
            .filter(|l| l.info.tuner_id == tuner_id)
            .count() as u32
    }
}

#[derive(Debug, Default)]
pub struct ReconcileReport {
    pub released_leases: u64,
    pub ended_sessions: u64,
    pub removed_buffer_dirs: usize,
    pub device_tuners_in_use: usize,
}

/// Boot-time reconcile. Nothing in the registry survives a restart, so every
/// persisted lease is an orphan; the device keeps streaming to a dead client and
/// nothing times out on its own, so say so loudly if it still reports tuners in
/// use. Buffer directories are disposable and must never accumulate.
pub async fn reconcile_on_boot(
    pool: &sqlx::PgPool,
    buffer_root: &Path,
) -> Result<ReconcileReport, crate::repositories::RepositoryError> {
    let mut report = ReconcileReport {
        released_leases: livetv_leases::release_all_active(pool).await?,
        ended_sessions: crate::repositories::livetv_sessions::end_all_active(pool).await?,
        ..Default::default()
    };

    for tuner in crate::repositories::livetv_tuners::list_all(pool).await? {
        match super::hdhomerun::status(&tuner.base_url).await {
            Ok(statuses) => {
                let busy = statuses.iter().filter(|s| s.in_use()).count();
                report.device_tuners_in_use += busy;
                if busy > 0 {
                    warn!(
                        device_id = %tuner.device_id,
                        busy,
                        "tuner still streaming after restart; it frees the slot when the dead TCP connection is reaped"
                    );
                }
            }
            Err(e) => {
                warn!(device_id = %tuner.device_id, error = %e, "tuner unreachable during boot reconcile")
            }
        }
    }

    let known = crate::repositories::livetv_sessions::all_buffer_dirs(pool).await?;
    if let Ok(mut entries) = tokio::fs::read_dir(buffer_root).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let keep = known.iter().any(|d| Path::new(d) == path);
            if !keep {
                let _ = tokio::fs::remove_dir_all(&path).await;
                report.removed_buffer_dirs += 1;
            }
        }
    }

    info!(
        released_leases = report.released_leases,
        ended_sessions = report.ended_sessions,
        removed_buffer_dirs = report.removed_buffer_dirs,
        device_tuners_in_use = report.device_tuners_in_use,
        "live TV boot reconcile complete"
    );
    Ok(report)
}

/// Buffer directories no session row and no live tune still references. `keep`
/// covers the window between creating a buffer directory and inserting its
/// session row.
pub async fn sweep_orphan_buffers(
    pool: &sqlx::PgPool,
    buffer_root: &Path,
    keep: &std::collections::HashSet<std::path::PathBuf>,
) -> usize {
    let Ok(active) = crate::repositories::livetv_sessions::active_buffer_dirs(pool).await else {
        return 0;
    };
    let Ok(retained) = crate::repositories::livetv_sessions::recorded_buffer_dirs(pool).await
    else {
        return 0;
    };
    let mut removed = 0;
    if let Ok(mut entries) = tokio::fs::read_dir(buffer_root).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let referenced = keep.contains(&path)
                || active
                    .iter()
                    .chain(retained.iter())
                    .any(|d| Path::new(d) == path);
            if !referenced {
                let _ = tokio::fs::remove_dir_all(&path).await;
                removed += 1;
            }
        }
    }
    removed
}
