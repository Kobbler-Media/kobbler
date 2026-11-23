use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nats_jobs::StreamPriority;
use tokio::sync::watch;

/// The kobload lane a download fetches through, and the way a viewer who joins it moves
/// the one writer onto a URL of their own without a second writer ever touching the file.
pub struct DownloadLane {
    for_viewer: AtomicBool,
    has_cache_url: bool,
    upgrading: AtomicBool,
    url: watch::Sender<Option<String>>,
}

// By hand so a logged download target never prints the signed URL held here.
impl std::fmt::Debug for DownloadLane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DownloadLane")
            .field("priority", &self.priority())
            .finish_non_exhaustive()
    }
}

pub struct Upgrading(Arc<DownloadLane>);

impl Drop for Upgrading {
    fn drop(&mut self) {
        self.0.upgrading.store(false, Ordering::Release);
    }
}

impl DownloadLane {
    fn new(for_viewer: bool, has_cache_url: bool) -> Arc<Self> {
        Arc::new(Self {
            for_viewer: AtomicBool::new(for_viewer),
            has_cache_url,
            upgrading: AtomicBool::new(false),
            url: watch::Sender::new(None),
        })
    }

    pub fn viewer() -> Arc<Self> {
        Self::new(true, false)
    }

    pub fn warm(has_cache_url: bool) -> Arc<Self> {
        Self::new(false, has_cache_url)
    }

    pub fn priority(&self) -> StreamPriority {
        if self.for_viewer.load(Ordering::Acquire) {
            StreamPriority::User
        } else {
            StreamPriority::Warm
        }
    }

    /// Without a cache URL kobload has no lower rank to move this download out of.
    pub fn needs_upgrade(&self) -> bool {
        self.has_cache_url && !self.for_viewer.load(Ordering::Acquire)
    }

    pub fn promote(&self) {
        self.for_viewer.store(true, Ordering::Release);
    }

    pub fn upgrade(&self, url: String) {
        self.promote();
        self.url.send_replace(Some(url));
    }

    pub fn upgrades(&self) -> watch::Receiver<Option<String>> {
        self.url.subscribe()
    }

    /// `None` while another viewer's upgrade of this download is already running.
    pub fn begin_upgrade(self: &Arc<Self>) -> Option<Upgrading> {
        self.upgrading
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Upgrading(self.clone()))
    }
}
