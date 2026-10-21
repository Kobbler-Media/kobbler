use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::upstream::Remainder;
use crate::ffmpeg::cue_gate::{CueGate, gated};
use crate::streaming::byte_range::ByteRange;
use crate::streaming::download_meter::DownloadMeter;
use crate::streaming::feed::{FeedSource, StreamFeed};

const READ_CHUNK: usize = 256 * 1024;
const GROWTH_POLL: Duration = Duration::from_millis(100);

pub(super) fn reply(
    partial: bool,
    content_type: &str,
    range: ByteRange,
    total: u64,
    body: Body,
) -> Response {
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, range.length().to_string())
        .header(header::ACCEPT_RANGES, "bytes");
    builder = if partial {
        builder
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, range.content_range(total))
    } else {
        builder.status(StatusCode::OK)
    };
    builder.body(body).unwrap()
}

/// A read served from the session's cache file, which may still be downloading.
pub(super) struct LocalRead {
    pub path: PathBuf,
    pub total: u64,
    pub range: ByteRange,
    pub partial: bool,
    pub content_type: &'static str,
    pub gate: Arc<CueGate>,
    pub meter: Option<Arc<DownloadMeter>>,
    pub feed: Option<Arc<StreamFeed>>,
    pub remainder: Remainder,
}

pub(super) async fn serve(read: LocalRead) -> Response {
    let LocalRead {
        path,
        total,
        range,
        partial,
        content_type,
        gate,
        meter,
        feed,
        remainder,
    } = read;
    if let Some(bytes) = gate.whole_range(range.start, range.length()).await {
        return reply(partial, content_type, range, total, Body::from(bytes));
    }
    let body = follow(path, range, meter, feed, remainder);
    reply(
        partial,
        content_type,
        range,
        total,
        Body::from_stream(gated(body, range.start, gate)),
    )
}

/// Read `range` from a file the download is still appending to, waiting at its end for
/// as long as the download keeps growing it. When it stops — finished short, failed,
/// cancelled or stalled — the rest comes from the debrid host instead, so the response
/// still delivers every byte its Content-Length promised.
fn follow(
    path: PathBuf,
    range: ByteRange,
    meter: Option<Arc<DownloadMeter>>,
    feed: Option<Arc<StreamFeed>>,
    remainder: Remainder,
) -> impl futures::Stream<Item = std::io::Result<Bytes>> + Send + 'static {
    async_stream::try_stream! {
        let mut file = tokio::fs::File::open(&path).await?;
        file.seek(std::io::SeekFrom::Start(range.start)).await?;
        let end = range.end + 1;
        let mut pos = range.start;
        let mut buf = vec![0u8; READ_CHUNK];
        let mut rechecked = false;
        while pos < end {
            let want = ((end - pos) as usize).min(READ_CHUNK);
            let n = file.read(&mut buf[..want]).await?;
            if n > 0 {
                pos += n as u64;
                rechecked = false;
                yield Bytes::copy_from_slice(&buf[..n]);
                continue;
            }
            if let Some(m) = meter.as_ref().filter(|m| m.is_live()) {
                m.wait_for_growth(GROWTH_POLL).await;
                continue;
            }
            // The download can finish between the empty read and the check above, with
            // its last bytes landing in that gap.
            if !rechecked {
                rechecked = true;
                continue;
            }
            if let Some(f) = &feed {
                f.serving_from(FeedSource::Proxy);
            }
            let mut rest = remainder.open(ByteRange { start: pos, end: range.end }).await?;
            while let Some(chunk) = rest.next().await {
                let chunk = chunk?;
                let take = chunk.len().min((end - pos) as usize);
                pos += take as u64;
                yield chunk.slice(..take);
                if pos >= end {
                    break;
                }
            }
            if pos < end {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "upstream ended before the range did",
                ))?;
            }
        }
    }
}
