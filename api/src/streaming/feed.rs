use crate::streaming::download_meter::DownloadMeter;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FeedSource {
    Local = 1,
    Proxy = 2,
}

/// What the player's byte feed for one session is doing: how many responses are open,
/// how much has gone out and how long since any did. A client that cannot tell a slow
/// start from a dead one reads this instead of guessing from its own buffer.
pub struct StreamFeed {
    epoch: Instant,
    requests_open: AtomicUsize,
    bytes_served: AtomicU64,
    /// Milliseconds since `epoch`, plus one so that zero can mean "never".
    last_byte_at: AtomicU64,
    last_open_at: AtomicU64,
    source: AtomicU8,
}

/// One response in flight. Dropping it, with the response body, closes it.
pub struct FeedTicket {
    feed: Arc<StreamFeed>,
}

#[derive(Debug, Serialize)]
pub struct FeedStatus {
    pub requests_open: usize,
    pub bytes_served: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<&'static str>,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub download_bps: u64,
}

impl StreamFeed {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            epoch: Instant::now(),
            requests_open: AtomicUsize::new(0),
            bytes_served: AtomicU64::new(0),
            last_byte_at: AtomicU64::new(0),
            last_open_at: AtomicU64::new(0),
            source: AtomicU8::new(0),
        })
    }

    fn stamp(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64 + 1
    }

    pub fn open(self: &Arc<Self>) -> FeedTicket {
        self.last_open_at.store(self.stamp(), Ordering::Release);
        self.requests_open.fetch_add(1, Ordering::AcqRel);
        FeedTicket { feed: self.clone() }
    }

    pub fn serving_from(&self, source: FeedSource) {
        self.source.store(source as u8, Ordering::Release);
    }

    pub fn status(&self, download: Option<&DownloadMeter>) -> FeedStatus {
        let now = self.stamp();
        let since = |at: u64| (at > 0).then(|| now.saturating_sub(at));
        let requests_open = self.requests_open.load(Ordering::Acquire);
        let last_byte = self.last_byte_at.load(Ordering::Acquire);
        let last_open = self.last_open_at.load(Ordering::Acquire);
        // Progress on any request counts: players leave an early request open and idle
        // while a newer one streams. With nothing open, only the last byte is progress.
        let idle_ms = if requests_open > 0 || last_byte == 0 {
            since(last_byte.max(last_open))
        } else {
            since(last_byte)
        };
        let source = match self.source.load(Ordering::Acquire) {
            1 => Some("local"),
            2 => Some("proxy"),
            _ => None,
        };
        FeedStatus {
            requests_open,
            bytes_served: self.bytes_served.load(Ordering::Acquire),
            idle_ms,
            source,
            downloaded_bytes: download.map_or(0, DownloadMeter::downloaded),
            total_bytes: download.map_or(0, DownloadMeter::total),
            download_bps: download.map_or(0, DownloadMeter::bps),
        }
    }
}

impl FeedTicket {
    fn sent(&self, n: usize) {
        if n == 0 {
            return;
        }
        self.feed.bytes_served.fetch_add(n as u64, Ordering::AcqRel);
        self.feed
            .last_byte_at
            .store(self.feed.stamp(), Ordering::Release);
    }
}

impl Drop for FeedTicket {
    fn drop(&mut self) {
        self.feed.requests_open.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Count what actually goes out on the wire, and hold the ticket open until it has.
pub fn metered<S, E>(inner: S, ticket: FeedTicket) -> impl Stream<Item = Result<Bytes, E>> + Send
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    inner.map(move |item| {
        if let Ok(chunk) = &item {
            ticket.sent(chunk.len());
        }
        item
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn nothing_requested_has_no_idle_time() {
        let feed = StreamFeed::new();
        let s = feed.status(None);
        assert_eq!(s.requests_open, 0);
        assert!(s.idle_ms.is_none());
        assert!(s.source.is_none());
    }

    #[tokio::test]
    async fn a_ticket_counts_bytes_and_closes_with_its_body() {
        let feed = StreamFeed::new();
        let ticket = feed.open();
        feed.serving_from(FeedSource::Local);
        assert_eq!(feed.status(None).requests_open, 1);
        let body = futures::stream::iter([
            Ok::<_, std::io::Error>(Bytes::from_static(b"abc")),
            Ok(Bytes::from_static(b"de")),
        ]);
        let out: Vec<_> = metered(body, ticket).collect().await;
        assert_eq!(out.len(), 2);
        let s = feed.status(None);
        assert_eq!(s.requests_open, 0);
        assert_eq!(s.bytes_served, 5);
        assert_eq!(s.source, Some("local"));
        assert!(s.idle_ms.is_some());
    }

    /// VLC leaves its first `bytes=0-` open and idle while a newer request streams;
    /// that is a healthy feed, not a stall.
    #[tokio::test]
    async fn a_stuck_request_does_not_hide_one_that_is_streaming() {
        let feed = StreamFeed::new();
        let stuck = feed.open();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let streaming = feed.open();
        for _ in 0..6 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            streaming.sent(1000);
            let s = feed.status(None);
            assert_eq!(s.requests_open, 2);
            let idle = s.idle_ms.unwrap();
            assert!(idle < 150, "idle_ms {idle} while a request is streaming");
        }
        drop(stuck);
        drop(streaming);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let s = feed.status(None);
        assert_eq!(s.requests_open, 0);
        assert!(
            s.idle_ms.unwrap() >= 200,
            "closed feed measures from the last byte"
        );
    }

    #[tokio::test]
    async fn a_new_request_counts_as_progress() {
        let feed = StreamFeed::new();
        let first = feed.open();
        first.sent(10);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _second = feed.open();
        assert!(feed.status(None).idle_ms.unwrap() < 150);
    }
}
