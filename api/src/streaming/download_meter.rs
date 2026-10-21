use kob_ffmpeg::hybrid_io::DownloadProgress;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

const RATE_WINDOW_MS: u64 = 500;
/// Long enough to ride out a debrid CDN hiccup, short enough that a player's start
/// watchdog does not fire while a response waits on bytes that are not coming.
pub const STALL_AFTER: Duration = Duration::from_secs(3);

/// The progressive download as the rest of the session sees it: how much of the file
/// is safely on disk, whether more is coming, and how fast.
pub struct DownloadMeter {
    pub progress: Arc<DownloadProgress>,
    epoch: Instant,
    running: AtomicBool,
    grew_at_ms: AtomicU64,
    window_at_ms: AtomicU64,
    window_bytes: AtomicU64,
    bps: AtomicU64,
    grew: Notify,
}

impl DownloadMeter {
    pub fn new(total: u64, on_disk: u64) -> Arc<Self> {
        let progress = Arc::new(DownloadProgress::new((total > 0).then_some(total)));
        progress.update(on_disk);
        if total > 0 && on_disk >= total {
            progress.mark_complete();
        }
        Arc::new(Self {
            progress,
            epoch: Instant::now(),
            running: AtomicBool::new(false),
            grew_at_ms: AtomicU64::new(0),
            window_at_ms: AtomicU64::new(0),
            window_bytes: AtomicU64::new(on_disk),
            bps: AtomicU64::new(0),
            grew: Notify::new(),
        })
    }

    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    pub fn started(&self) {
        let now = self.now_ms();
        self.grew_at_ms.store(now, Ordering::Release);
        self.window_at_ms.store(now, Ordering::Release);
        self.window_bytes
            .store(self.progress.downloaded(), Ordering::Release);
        self.bps.store(0, Ordering::Release);
        self.running.store(true, Ordering::Release);
    }

    /// `on_disk` must already be readable by another file handle.
    pub fn record(&self, on_disk: u64) {
        self.progress.update(on_disk);
        let now = self.now_ms();
        self.grew_at_ms.store(now, Ordering::Release);
        let window_at = self.window_at_ms.load(Ordering::Acquire);
        let elapsed = now.saturating_sub(window_at);
        if elapsed >= RATE_WINDOW_MS {
            let before = self.window_bytes.swap(on_disk, Ordering::AcqRel);
            self.window_at_ms.store(now, Ordering::Release);
            self.bps.store(
                on_disk.saturating_sub(before) * 1000 / elapsed,
                Ordering::Release,
            );
        }
        self.grew.notify_waiters();
    }

    pub fn finished(&self, complete: bool) {
        if complete {
            self.progress.mark_complete();
        }
        self.running.store(false, Ordering::Release);
        self.bps.store(0, Ordering::Release);
        self.grew.notify_waiters();
    }

    pub fn downloaded(&self) -> u64 {
        self.progress.downloaded()
    }

    pub fn total(&self) -> u64 {
        self.progress.total()
    }

    pub fn is_complete(&self) -> bool {
        self.progress.is_complete()
    }

    /// Running and still making progress — worth waiting on.
    pub fn is_live(&self) -> bool {
        self.running.load(Ordering::Acquire)
            && self
                .now_ms()
                .saturating_sub(self.grew_at_ms.load(Ordering::Acquire))
                < STALL_AFTER.as_millis() as u64
    }

    pub fn bps(&self) -> u64 {
        if !self.running.load(Ordering::Acquire) {
            return 0;
        }
        let quiet = self
            .now_ms()
            .saturating_sub(self.grew_at_ms.load(Ordering::Acquire));
        if quiet > RATE_WINDOW_MS * 2 {
            0
        } else {
            self.bps.load(Ordering::Acquire)
        }
    }

    /// How far ahead of the download a read can start and still be worth waiting for:
    /// whatever lands in the next couple of seconds, and never less than a few MB.
    pub fn reach(&self) -> u64 {
        const FLOOR: u64 = 4 * 1024 * 1024;
        self.downloaded() + (self.bps() * 2).max(FLOOR)
    }

    pub async fn wait_for_growth(&self, at_most: Duration) {
        let _ = tokio::time::timeout(at_most, self.grew.notified()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seeded_complete_file_is_complete() {
        let m = DownloadMeter::new(100, 100);
        assert!(m.is_complete());
        assert!(!m.is_live());
        assert_eq!(m.bps(), 0);
    }

    #[test]
    fn a_running_download_is_live_and_reaches_past_its_end() {
        let m = DownloadMeter::new(1 << 30, 0);
        m.started();
        m.record(1000);
        assert!(m.is_live());
        assert!(m.reach() >= 1000 + 4 * 1024 * 1024);
        m.finished(false);
        assert!(!m.is_live());
        assert!(!m.is_complete());
        assert_eq!(m.bps(), 0);
    }
}
