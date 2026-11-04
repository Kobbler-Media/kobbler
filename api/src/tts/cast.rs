//! A read rendered for a cast receiver.
//!
//! Synthesis runs once, into memory: the audio as ADTS AAC indexed by frame, the
//! timeline as NDJSON lines without their audio payloads. A receiver listens to the
//! first and the phone follows the second, each from its own cursor, so either can
//! reconnect without the other noticing — the receiver part way in, at a time.
//!
//! The render keeps only so far ahead of what a receiver has pulled. It stops at the
//! next passage or sentence once nobody is pulling, and carries on when someone is.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use kob_ffmpeg::pcm_stream::{AdtsAacEncoder, AdtsFrame, decode_to_pcm};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_stream::Stream;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};
use uuid::Uuid;

use super::timeline::{self, Item, PCM_SAMPLE_RATE, Plan, Sink};

const BIT_RATE: i64 = 64_000;
const AHEAD_OF_LISTENER_MS: u64 = 60_000;
const IDLE_TTL: Duration = Duration::from_secs(3 * 60 * 60);
const FRAMES_PER_WRITE: usize = 64;

#[derive(Default)]
pub struct CastRenders {
    renders: Mutex<HashMap<Uuid, Arc<CastRender>>>,
}

impl CastRenders {
    /// Start rendering `plan` for `owner`. A user casts one read at a time, so their
    /// earlier renders are dropped, and their synthesis with them.
    pub fn start(&self, owner: Uuid, plan: Plan) -> Result<Uuid, kob_ffmpeg::FfmpegError> {
        let encoder = AdtsAacEncoder::new(PCM_SAMPLE_RATE as i32, BIT_RATE)?;
        let id = Uuid::new_v4();
        let render = Arc::new(CastRender::new(owner));
        {
            let mut renders = self.renders.lock().unwrap();
            renders.retain(|_, r| {
                let keep = r.owner != owner && r.last_touch.lock().unwrap().elapsed() < IDLE_TTL;
                if !keep {
                    r.cancel.cancel();
                }
                keep
            });
            renders.insert(id, render.clone());
        }
        tokio::spawn(async move {
            let mut sink = CastSink {
                render: render.clone(),
                encoder,
                pcm_samples: 0,
            };
            timeline::run(plan, &mut sink).await;
            sink.finish();
        });
        Ok(id)
    }

    pub fn get(&self, id: Uuid, user: Uuid) -> Option<Arc<CastRender>> {
        let render = self.renders.lock().unwrap().get(&id)?.clone();
        if render.owner != user {
            return None;
        }
        *render.last_touch.lock().unwrap() = Instant::now();
        Some(render)
    }
}

pub struct CastRender {
    owner: Uuid,
    state: Mutex<RenderState>,
    changed: watch::Sender<()>,
    cancel: CancellationToken,
    last_touch: Mutex<Instant>,
}

#[derive(Default)]
struct RenderState {
    audio: Vec<u8>,
    /// Byte offset and pts (samples) of every frame in `audio`.
    frames: Vec<(usize, i64)>,
    lines: Vec<Bytes>,
    pcm_samples: u64,
    /// Furthest sample any receiver has pulled.
    listened_samples: u64,
    finished: bool,
}

impl CastRender {
    fn new(owner: Uuid) -> Self {
        Self {
            owner,
            state: Mutex::default(),
            changed: watch::channel(()).0,
            cancel: CancellationToken::new(),
            last_touch: Mutex::new(Instant::now()),
        }
    }

    fn update(&self, f: impl FnOnce(&mut RenderState)) {
        f(&mut self.state.lock().unwrap());
        self.changed.send_replace(());
    }

    /// The audio from `from_ms` on, starting at the frame that holds it.
    pub fn audio(self: Arc<Self>, from_ms: u64) -> impl Stream<Item = Result<Bytes, Infallible>> {
        let target = (from_ms * PCM_SAMPLE_RATE as u64 / 1000) as i64;
        async_stream::stream! {
            let mut changed = self.changed.subscribe();
            let mut next: Option<usize> = None;
            loop {
                let chunk = {
                    let mut s = self.state.lock().unwrap();
                    if next.is_none() && (s.finished || s.frames.last().is_some_and(|f| f.1 > target)) {
                        next = Some(s.frames.partition_point(|f| f.1 <= target).saturating_sub(1));
                    }
                    match next {
                        Some(i) if i < s.frames.len() => {
                            let end = (i + FRAMES_PER_WRITE).min(s.frames.len());
                            let from = s.frames[i].0;
                            let to = s.frames.get(end).map_or(s.audio.len(), |f| f.0);
                            let heard = (s.frames[end - 1].1 + 1024).max(0) as u64;
                            s.listened_samples = s.listened_samples.max(heard);
                            next = Some(end);
                            Some(Bytes::copy_from_slice(&s.audio[from..to]))
                        }
                        _ if s.finished => break,
                        _ => None,
                    }
                };
                match chunk {
                    Some(bytes) => {
                        self.changed.send_replace(());
                        yield Ok(bytes);
                    }
                    None => tokio::select! {
                        r = changed.changed() => if r.is_err() { break },
                        () = self.cancel.cancelled() => break,
                    },
                }
            }
        }
    }

    /// Every timeline line so far, then each as it lands, until the read is over.
    pub fn events(self: Arc<Self>) -> impl Stream<Item = Result<Bytes, Infallible>> {
        async_stream::stream! {
            let mut changed = self.changed.subscribe();
            let mut next = 0;
            loop {
                let (lines, finished) = {
                    let s = self.state.lock().unwrap();
                    (s.lines[next..].to_vec(), s.finished)
                };
                next += lines.len();
                for line in lines {
                    yield Ok(line);
                }
                if finished && next == self.state.lock().unwrap().lines.len() {
                    break;
                }
                tokio::select! {
                    r = changed.changed() => if r.is_err() { break },
                    () = self.cancel.cancelled() => break,
                }
            }
        }
    }
}

struct CastSink {
    render: Arc<CastRender>,
    encoder: AdtsAacEncoder,
    pcm_samples: u64,
}

impl CastSink {
    fn push_line(&self, value: &Value) {
        let mut line = serde_json::to_vec(value).unwrap_or_default();
        line.push(b'\n');
        self.render.update(|s| s.lines.push(line.into()));
    }

    fn push_pcm(&mut self, pcm: &[u8]) {
        let frames = match self.encoder.push_s16le(pcm) {
            Ok(frames) => frames,
            Err(e) => {
                error!(error = %e, "AAC encoding of the cast read failed");
                Vec::new()
            }
        };
        self.pcm_samples += pcm.len() as u64 / 2;
        let samples = self.pcm_samples;
        self.render.update(|s| {
            append(s, frames);
            s.pcm_samples = samples;
        });
    }

    fn finish(&mut self) {
        let frames = self.encoder.finish().unwrap_or_else(|e| {
            error!(error = %e, "flushing the cast read's AAC encoder failed");
            Vec::new()
        });
        self.render.update(|s| {
            append(s, frames);
            s.finished = true;
        });
    }
}

fn append(s: &mut RenderState, frames: Vec<AdtsFrame>) {
    for frame in frames {
        s.frames.push((s.audio.len(), frame.pts));
        s.audio.extend_from_slice(&frame.bytes);
    }
}

fn ms(samples: u64) -> u64 {
    samples * 1000 / PCM_SAMPLE_RATE as u64
}

impl Sink for CastSink {
    fn alive(&self) -> bool {
        !self.render.cancel.is_cancelled()
    }

    async fn ready(&mut self) -> bool {
        let mut changed = self.render.changed.subscribe();
        loop {
            if !self.alive() {
                return false;
            }
            let ahead = {
                let s = self.render.state.lock().unwrap();
                ms(s.pcm_samples.saturating_sub(s.listened_samples))
            };
            if ahead < AHEAD_OF_LISTENER_MS {
                return true;
            }
            tokio::select! {
                r = changed.changed() => if r.is_err() { return false },
                () = self.render.cancel.cancelled() => return false,
            }
        }
    }

    async fn emit(&mut self, item: Item) -> bool {
        match item {
            Item::Event(value) => self.push_line(&value),
            Item::Pcm { pcm, .. } => self.push_pcm(&pcm),
            Item::Sentence { index, text, audio } => {
                let bytes = audio.bytes.to_vec();
                let pcm = match tokio::task::spawn_blocking(move || {
                    decode_to_pcm(bytes, PCM_SAMPLE_RATE as i32)
                })
                .await
                {
                    Ok(Ok(pcm)) => pcm,
                    Ok(Err(e)) => {
                        warn!(index, error = %e, "a rendered sentence would not decode; skipped");
                        return self.alive();
                    }
                    Err(e) => {
                        error!(index, error = %e, "sentence decode task failed");
                        return self.alive();
                    }
                };
                self.push_line(&json!({
                    "type": "sentence",
                    "index": index,
                    "text": text,
                    "speed": audio.speed_applied,
                    "audio_offset_ms": ms(self.pcm_samples),
                    "duration_ms": ms(pcm.len() as u64 / 2),
                }));
                self.push_pcm(&pcm);
            }
        }
        self.alive()
    }
}
