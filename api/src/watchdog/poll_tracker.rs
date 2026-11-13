use std::panic::Location;
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::runtime::TaskMeta;
use tokio::task;

#[derive(Debug, Clone)]
pub struct LongPoll {
    pub worker: usize,
    pub running_for: Duration,
    pub spawned_at: &'static Location<'static>,
    pub job: Option<String>,
    pub blocked_in: Option<String>,
}

thread_local! {
    static THREAD_ID: i32 = unsafe { libc::gettid() };
}

/// What each worker is polling right now. A worker blocked inside a task looks exactly like
/// a parked one from outside the process, so this is the only way to name the code doing it.
pub struct PollTracker {
    epoch: Instant,
    slots: Box<[Slot]>,
}

struct Slot {
    // Nanoseconds since `epoch`, plus one so zero can mean "not polling".
    started: AtomicU64,
    spawned_at: AtomicPtr<Location<'static>>,
    task: Mutex<Option<task::Id>>,
    thread: AtomicI32,
}

impl PollTracker {
    pub fn new(workers: usize) -> Arc<Self> {
        Arc::new(Self {
            epoch: Instant::now(),
            slots: (0..workers)
                .map(|_| Slot {
                    started: AtomicU64::new(0),
                    spawned_at: AtomicPtr::new(std::ptr::null_mut()),
                    task: Mutex::new(None),
                    thread: AtomicI32::new(0),
                })
                .collect(),
        })
    }

    pub(super) fn before_poll(&self, meta: &TaskMeta<'_>) {
        let Some(slot) = self.current_slot() else {
            return;
        };
        let location: &'static Location<'static> = meta.spawned_at();
        slot.spawned_at
            .store(std::ptr::from_ref(location).cast_mut(), Ordering::Relaxed);
        if let Ok(mut task) = slot.task.lock() {
            *task = Some(meta.id());
        }
        slot.thread.store(THREAD_ID.with(|t| *t), Ordering::Relaxed);
        slot.started.store(self.now(), Ordering::Release);
    }

    pub(super) fn after_poll(&self) {
        if let Some(slot) = self.current_slot() {
            slot.started.store(0, Ordering::Release);
        }
    }

    pub fn long_polls(&self, min: Duration) -> Vec<LongPoll> {
        let now = self.now();
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(worker, slot)| {
                let started = slot.started.load(Ordering::Acquire);
                let location = slot.spawned_at.load(Ordering::Relaxed);
                let task = slot.task.lock().ok().and_then(|t| *t);
                let thread = slot.thread.load(Ordering::Relaxed);
                // A poll that ended and a new one that began between the loads would pair
                // one task's start with another's location.
                if started == 0
                    || location.is_null()
                    || slot.started.load(Ordering::Acquire) != started
                {
                    return None;
                }
                let running_for = Duration::from_nanos(now.saturating_sub(started));
                (running_for >= min).then(|| LongPoll {
                    worker,
                    running_for,
                    spawned_at: unsafe { &*location },
                    job: task
                        .and_then(nats_jobs::running::running_job)
                        .map(|j| format!("{} {}", j.job_type, j.job_id)),
                    blocked_in: super::blocked::describe(thread),
                })
            })
            .collect()
    }

    fn current_slot(&self) -> Option<&Slot> {
        tokio::runtime::worker_index().and_then(|i| self.slots.get(i))
    }

    fn now(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64 + 1
    }
}
