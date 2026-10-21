use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde_json::Value;
use uuid::Uuid;

use crate::routes::admin::debrid_open::{SessionRow, WarmGate};
use crate::routes::admin::debrid_stream::StreamResolveBody;
use crate::routes::admin::streaming::SessionResponse;
use crate::streaming::progressive::DownloadTarget;

pub(crate) const UNTOUCHED_FOR: Duration = Duration::from_secs(5 * 60);
pub(crate) const SAME_PREWARM_FOR: Duration = Duration::from_secs(10 * 60);
/// How long an ended prewarm is remembered, so a late claim learns it expired (410)
/// instead of never having existed (404).
pub(crate) const REMEMBERED_FOR: Duration = Duration::from_secs(30 * 60);
pub(crate) const PER_USER: usize = 2;

#[derive(Clone)]
pub(crate) enum Phase {
    Warm,
    Claimed(Box<SessionResponse>),
    Expired,
    Released,
}

#[derive(Clone)]
pub(crate) struct Prewarm {
    pub id: Uuid,
    pub user_id: Uuid,
    pub title: String,
    pub session_id: Uuid,
    pub release_key: Option<String>,
    pub option: Value,
    pub body: StreamResolveBody,
    pub opened: SessionResponse,
    pub row: SessionRow,
    pub remainder: Option<DownloadTarget>,
    pub gate: WarmGate,
    pub created_at: Instant,
    pub touched_at: Instant,
    pub ended_at: Option<Instant>,
    pub phase: Phase,
}

impl Prewarm {
    pub fn is_warm(&self) -> bool {
        matches!(self.phase, Phase::Warm)
    }

    pub fn untouched_too_long(&self) -> bool {
        self.is_warm() && self.touched_at.elapsed() > UNTOUCHED_FOR
    }
}

type TitleKey = (Uuid, String);

/// Prewarms by id, and the one live prewarm for each user's title.
#[derive(Default)]
pub struct PrewarmStore {
    pub(crate) entries: DashMap<Uuid, Prewarm>,
    pub(crate) titles: DashMap<TitleKey, Uuid>,
    locks: DashMap<TitleKey, Arc<tokio::sync::Mutex<()>>>,
}

impl PrewarmStore {
    pub(crate) fn title_lock(&self, user_id: Uuid, title: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks
            .entry((user_id, title.to_string()))
            .or_default()
            .clone()
    }

    pub(crate) fn for_title(&self, user_id: Uuid, title: &str) -> Option<Prewarm> {
        let id = *self.titles.get(&(user_id, title.to_string()))?;
        self.entries.get(&id).map(|e| e.clone())
    }

    pub(crate) fn get(&self, id: Uuid) -> Option<Prewarm> {
        self.entries.get(&id).map(|e| e.clone())
    }

    pub(crate) fn insert(&self, prewarm: Prewarm) {
        self.titles
            .insert((prewarm.user_id, prewarm.title.clone()), prewarm.id);
        self.entries.insert(prewarm.id, prewarm);
    }

    pub(crate) fn touch(&self, id: Uuid) {
        if let Some(mut e) = self.entries.get_mut(&id) {
            e.touched_at = Instant::now();
        }
    }

    /// Move a warm prewarm on; `false` when it had already left `Warm`.
    pub(crate) fn finish(&self, id: Uuid, phase: Phase) -> bool {
        let Some(mut e) = self.entries.get_mut(&id) else {
            return false;
        };
        if !e.is_warm() {
            return false;
        }
        e.phase = phase;
        e.ended_at = Some(Instant::now());
        let key = (e.user_id, e.title.clone());
        drop(e);
        self.titles.remove_if(&key, |_, v| *v == id);
        true
    }

    pub(crate) fn warm_for_user(&self, user_id: Uuid) -> Vec<Prewarm> {
        let mut warm: Vec<Prewarm> = self
            .entries
            .iter()
            .filter(|e| e.user_id == user_id && e.is_warm())
            .map(|e| e.clone())
            .collect();
        warm.sort_by_key(|p| p.created_at);
        warm
    }

    pub(crate) fn warm_for_release(&self, release_key: &str) -> Option<Prewarm> {
        self.entries
            .iter()
            .find(|e| e.is_warm() && e.release_key.as_deref() == Some(release_key))
            .map(|e| e.clone())
    }

    pub(crate) fn untouched(&self) -> Vec<Uuid> {
        self.entries
            .iter()
            .filter(|e| e.untouched_too_long())
            .map(|e| e.id)
            .collect()
    }

    pub(crate) fn forget_old(&self) {
        self.entries
            .retain(|_, e| e.ended_at.is_none_or(|at| at.elapsed() < REMEMBERED_FOR));
        self.locks
            .retain(|key, lock| self.titles.contains_key(key) || Arc::strong_count(lock) > 1);
    }

    /// Every prewarm's id with its session's id.
    #[doc(hidden)]
    pub fn entries_snapshot(&self) -> Vec<(Uuid, Uuid)> {
        self.entries.iter().map(|e| (e.id, e.session_id)).collect()
    }

    /// Age a prewarm as if `by` had passed since it was created and last touched.
    #[doc(hidden)]
    pub fn backdate(&self, id: Uuid, by: Duration) {
        if let Some(mut e) = self.entries.get_mut(&id) {
            e.created_at -= by;
            e.touched_at -= by;
        }
    }
}
