//! One MPEG-TS response for players that take neither HLS nor a byte range, cut from the
//! session's own HLS transcode: its fMP4 segments are fed in order through a stream-copy
//! remux, so decision, encoder, burn-in and tone-mapping stay the HLS path's.

use crate::streaming::session::PlaybackSession;
use bytes::Bytes;
use dashmap::DashMap;
use futures::Stream;
use futures::StreamExt;
use kob_ffmpeg::format::input::InputContext;
use kob_ffmpeg::format::io_context::{CustomIoContext, IoCallbacks};
use kob_ffmpeg::format::output::OutputContext;
use kob_ffmpeg::packet::Packet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

const SEGMENTS_AHEAD: usize = 2;
const CHUNKS_AHEAD: usize = 32;
const POLL: Duration = Duration::from_millis(100);

type Sessions = Arc<DashMap<Uuid, PlaybackSession>>;

pub fn stream(
    sessions: Sessions,
    session_id: Uuid,
    stall: Duration,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> {
    let cancel = CancellationToken::new();
    let (segment_tx, segment_rx) = mpsc::channel(SEGMENTS_AHEAD);
    let (chunk_tx, chunk_rx) = mpsc::channel(CHUNKS_AHEAD);
    tokio::spawn(feed(
        sessions,
        session_id,
        stall,
        segment_tx,
        cancel.clone(),
    ));
    tokio::task::spawn_blocking(move || {
        if let Err(e) = remux(segment_rx, chunk_tx) {
            warn!(session_id = %session_id, error = %e, "MPEG-TS remux ended early");
        }
    });
    let guard = cancel.drop_guard();
    tokio_stream::wrappers::ReceiverStream::new(chunk_rx).map(move |chunk| {
        let _ = &guard;
        Ok(chunk)
    })
}

enum Wait {
    Ready,
    End,
}

async fn feed(
    sessions: Sessions,
    session_id: Uuid,
    stall: Duration,
    tx: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
) {
    let Some(dir) = generation_dir(&sessions, session_id, stall, &cancel).await else {
        return;
    };
    let mut current = dir.join("init.mp4");
    for index in 0u32.. {
        let next = dir.join(format!("seg_{index:05}.m4s"));
        match written(&sessions, session_id, &current, &next, stall, &cancel).await {
            Wait::Ready => {}
            Wait::End => return,
        }
        let Ok(bytes) = tokio::fs::read(&current).await else {
            return;
        };
        if tx.send(bytes).await.is_err() {
            return;
        }
        current = next;
    }
}

/// The transcode starts in the background after the session is created.
async fn generation_dir(
    sessions: &Sessions,
    session_id: Uuid,
    stall: Duration,
    cancel: &CancellationToken,
) -> Option<PathBuf> {
    let started = Instant::now();
    loop {
        {
            let session = sessions.get(&session_id)?;
            if session.error.is_some() {
                return None;
            }
            if let Some(t) = session.transcode.as_ref() {
                return Some(t.gen_dir(t.current_generation()));
            }
        }
        if started.elapsed() > stall || !pause(cancel).await {
            return None;
        }
    }
}

/// A segment is whole once the muxer has opened the one after it, or the transcode ended.
async fn written(
    sessions: &Sessions,
    session_id: Uuid,
    current: &Path,
    next: &Path,
    stall: Duration,
    cancel: &CancellationToken,
) -> Wait {
    let started = Instant::now();
    loop {
        let exists = tokio::fs::metadata(current)
            .await
            .is_ok_and(|m| m.len() > 0);
        if exists && tokio::fs::try_exists(next).await.unwrap_or(false) {
            return Wait::Ready;
        }
        let finished = {
            let Some(mut session) = sessions.get_mut(&session_id) else {
                return Wait::End;
            };
            if session.error.is_some() {
                return Wait::End;
            }
            session.transcode.as_mut().is_some_and(|t| !t.is_running())
        };
        if finished {
            return if exists { Wait::Ready } else { Wait::End };
        }
        if started.elapsed() > stall || !pause(cancel).await {
            return Wait::End;
        }
    }
}

async fn pause(cancel: &CancellationToken) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(POLL) => true,
    }
}

struct SegmentReader {
    rx: mpsc::Receiver<Vec<u8>>,
    buffer: Vec<u8>,
    pos: usize,
}

impl IoCallbacks for SegmentReader {
    fn read(&mut self, buf: &mut [u8]) -> i32 {
        while self.pos == self.buffer.len() {
            match self.rx.blocking_recv() {
                Some(bytes) => {
                    self.buffer = bytes;
                    self.pos = 0;
                }
                None => return kob_ffmpeg::ffi::AVERROR_EOF,
            }
        }
        let n = buf.len().min(self.buffer.len() - self.pos);
        buf[..n].copy_from_slice(&self.buffer[self.pos..self.pos + n]);
        self.pos += n;
        n as i32
    }
}

fn remux(segments: mpsc::Receiver<Vec<u8>>, out: mpsc::Sender<Bytes>) -> kob_ffmpeg::Result<()> {
    let mut io = CustomIoContext::new(SegmentReader {
        rx: segments,
        buffer: Vec::new(),
        pos: 0,
    })?;
    // Segments arrive once and in order; a seekable flag sends the mp4 demuxer looking
    // for an index at the end of a stream that has no end yet.
    unsafe {
        (*io.as_mut_ptr()).seekable = 0;
    }
    let mut input = InputContext::open_custom(&mut io)?;
    input.find_stream_info()?;

    let (muxed_tx, muxed_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let mut output = OutputContext::create_to_channel("mpegts", muxed_tx)?;
    // The transcode's timeline begins at the session's start_time; the renderer's
    // position has to read from zero like the HLS playlist's does.
    output.set_option("avoid_negative_ts", "make_zero")?;
    let mut stream_map = Vec::new();
    for i in 0..input.nb_streams() {
        let stream = input.stream(i).expect("stream index in range");
        stream_map.push(output.add_stream_copy(&stream)?);
    }
    output.write_header()?;

    let forward = || {
        muxed_rx
            .try_iter()
            .all(|chunk| out.blocking_send(Bytes::from(chunk)).is_ok())
    };
    let mut packet = Packet::new()?;
    while input.read_packet(&mut packet)? {
        let Some(&out_index) = usize::try_from(packet.stream_index())
            .ok()
            .and_then(|i| stream_map.get(i))
        else {
            continue;
        };
        let time_base = input
            .stream(packet.stream_index() as u32)
            .expect("stream index in range")
            .time_base();
        output.write_packet_rescaled(&mut packet, out_index, time_base)?;
        if !forward() {
            return Ok(());
        }
    }
    output.write_trailer()?;
    forward();
    Ok(())
}
