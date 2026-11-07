//! One read, synthesised once, as the timeline every listener follows.
//!
//! The phone hears it inline as NDJSON; a cast receiver hears it re-encoded as one
//! continuous stream while the phone follows the same events without the audio. Both
//! drive [`run`] with their own [`Sink`], so the chunking, the offsets and the failure
//! policy exist once.
//!
//! Wire envelope (shared with koblem's playground via the koblibs `FaEvent` shape):
//! - `{"type":"paragraph_start","index":0,"text":"...","word_offset":0,"audio_offset_ms":0,"sample_rate":24000,"speed":1.0}`
//! - `{"type":"audio","paragraph_index":0,"pcm_b64":"..."}` (24 kHz s16le mono)
//! - `{"type":"chunk","paragraph_index":0,"index":0,"chunks_total":4,"word_offset":0,"word_count":31,"audio_offset_ms":0,"text":"..."}`
//! - `{"type":"alignment_partial","paragraph_index":0,"audio_seen_ms":M,"words":[...]}`
//! - `{"type":"alignment_final","paragraph_index":0,"audio_total_ms":M,"words":[...]}`
//! - `{"type":"sentence","index":0,"text":"...","audio_b64":"...","speed":1.0}` (buffered engines)
//! - `{"type":"done"}`
//!
//! Each passage is one `paragraph_index`, and the offsets on `paragraph_start`
//! globalise it: `word_offset` is where its words begin in the whole text and
//! `audio_offset_ms` is where its audio begins in the whole read. Word indices
//! inside `alignment_*` stay LOCAL to their passage — clients add the offset.
//!
//! `chunk` is the engine's own sentence boundary, stamped with where it starts.
//! It needs no aligner, so it arrives while the first alignment is still being
//! computed and keeps arriving when alignment is off entirely — it is what lets
//! a reader highlight a block and turn the page without word timings.
//!
//! `paragraph_start` is emitted once the upstream response lands rather than
//! immediately, because `speed` is on it: an engine that refuses the requested
//! rate renders at 1.0 instead, and the client must label the audio it is about
//! to receive rather than the rate it asked for. Both the audio and the word
//! timings are already in that rendered timebase — clients must NOT rescale.
//!
//! ⚠️ An `alignment_partial` carries only the PREFIX the voice has reached. The
//! prefix itself is monotonic (the engine clamps it to a high-water mark), but
//! a word's TIMINGS are re-estimated on every pass and do change. `word_index`
//! is absolute within its paragraph and stable, so clients overwrite by
//! `word_index` — never append, which would duplicate the passage.

use std::future::Future;

use base64::Engine;
use bytes::Bytes;
use serde_json::{Value, json};
use tokio_stream::StreamExt;
use tracing::{error, warn};

use std::sync::Arc;

use super::{SentenceAudio, SpeechEndpoint, TtsBackend, TtsError, TtsParams, TtsStreamEvent};
use crate::gpu::GpuLock;
use crate::runtime_config::RuntimeConfig;

/// Sample rate of the paragraph path's PCM, and of every cast stream.
pub const PCM_SAMPLE_RATE: u32 = 24_000;

pub struct Voice {
    pub id: String,
    pub params: TtsParams,
}

pub struct Passage {
    pub text: String,
    pub voice: Arc<Voice>,
}

/// One voice reading every passage.
pub fn read_by(texts: Vec<String>, voice: Voice) -> Vec<Passage> {
    let voice = Arc::new(voice);
    texts
        .into_iter()
        .map(|text| Passage {
            text,
            voice: voice.clone(),
        })
        .collect()
}

/// A read whose passages, voices and route are already settled. On the
/// sentence route each passage is one sentence.
pub struct Plan {
    pub passages: Vec<Passage>,
    pub route: Route,
}

pub enum Route {
    /// Aligning engine via koblem: whole passages, streamed PCM, and word
    /// timings when someone reads along.
    Passages {
        koblem_url: String,
        koblem_api_key: String,
        align: bool,
    },
    /// Any other engine: one buffered render per sentence.
    Sentences {
        service_url: String,
        backend: TtsBackend,
        via_koblem: bool,
        koblem_api_key: Option<String>,
        gpu_lock: GpuLock,
    },
}

impl Route {
    /// Alignment-capable engine (breeze) + koblem-with-api-key → paragraph
    /// streaming with word-level forced alignment, routed to that engine's
    /// aligned-generate endpoint. Any other backend (chatterbox/kokoro/openai),
    /// or koblem unconfigured, renders per sentence: kobbler then talks to
    /// `service_url` directly so turning koblem off doesn't break TTS — it just
    /// loses streaming + FA.
    pub fn for_config(config: &RuntimeConfig, service_url: String, gpu_lock: GpuLock) -> Self {
        let backend = TtsBackend::from_config(&config.tts_service_type);
        let filled = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.is_empty());
        if backend.supports_alignment()
            && filled(&config.koblem_service_url)
            && filled(&config.koblem_api_key)
        {
            return Route::Passages {
                koblem_url: config.koblem_service_url.clone().unwrap_or_default(),
                koblem_api_key: config.koblem_api_key.clone().unwrap_or_default(),
                align: true,
            };
        }
        gpu_lock.set_enabled(config.gpu_preempt_tts);
        gpu_lock.set_service_urls(
            config.stt_service_url.clone(),
            config.tts_service_url.clone(),
            config.vision_service_url.clone(),
        );
        Route::Sentences {
            service_url,
            backend,
            via_koblem: config.tts_routes_via_koblem(),
            koblem_api_key: config.koblem_api_key.clone(),
            gpu_lock,
        }
    }

    /// Already-normalised text cut to what one request of this route reads.
    /// For a render nobody reads along to: word timings cost GPU time.
    pub fn unaligned(self) -> Self {
        match self {
            Route::Passages {
                koblem_url,
                koblem_api_key,
                ..
            } => Route::Passages {
                koblem_url,
                koblem_api_key,
                align: false,
            },
            other => other,
        }
    }

    pub fn split(&self, text: &str) -> Vec<String> {
        match self {
            Route::Passages { .. } => super::split_passages(text),
            Route::Sentences { .. } => super::split_sentences(text),
        }
    }
}

/// A buffered sentence render as 24 kHz s16le PCM, or `None` when it would not
/// decode and is skipped.
pub async fn sentence_pcm(index: usize, audio: &SentenceAudio) -> Option<Vec<u8>> {
    let bytes = audio.bytes.to_vec();
    match tokio::task::spawn_blocking(move || {
        kob_ffmpeg::pcm_stream::decode_to_pcm(bytes, PCM_SAMPLE_RATE as i32)
    })
    .await
    {
        Ok(Ok(pcm)) => Some(pcm),
        Ok(Err(e)) => {
            warn!(index, error = %e, "a rendered sentence would not decode; skipped");
            None
        }
        Err(e) => {
            error!(index, error = %e, "sentence decode task failed");
            None
        }
    }
}

pub enum Item {
    Event(Value),
    Pcm {
        paragraph_index: u32,
        pcm: Bytes,
    },
    Sentence {
        index: usize,
        text: String,
        audio: SentenceAudio,
    },
}

impl Item {
    /// The line the phone's inline read receives, audio included.
    pub fn into_wire(self) -> Value {
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        match self {
            Item::Event(v) => v,
            Item::Pcm {
                paragraph_index,
                pcm,
            } => json!({"type": "audio", "paragraph_index": paragraph_index, "pcm_b64": b64(&pcm)}),
            Item::Sentence { index, text, audio } => json!({
                "type": "sentence",
                "index": index,
                "text": text,
                "audio_b64": b64(&audio.bytes),
                // Per sentence because each one is its own render, and the client
                // must label what it plays rather than what it asked for.
                "speed": audio.speed_applied,
            }),
        }
    }
}

pub trait Sink: Send {
    fn alive(&self) -> bool;
    /// Called before each passage or sentence is rendered: the place a listener that
    /// has fallen behind holds the read back without stalling an upstream mid-render.
    fn ready(&mut self) -> impl Future<Output = bool> + Send;
    fn emit(&mut self, item: Item) -> impl Future<Output = bool> + Send;
}

pub async fn run(plan: Plan, sink: &mut impl Sink) {
    let Plan { passages, route } = plan;
    match route {
        Route::Passages {
            koblem_url,
            koblem_api_key,
            align,
        } => run_passages(&passages, &koblem_url, &koblem_api_key, align, sink).await,
        Route::Sentences {
            service_url,
            backend,
            via_koblem,
            koblem_api_key,
            gpu_lock,
        } => {
            for (index, Passage { text, voice }) in passages.into_iter().enumerate() {
                if !sink.ready().await {
                    break;
                }
                let _gpu_guard = gpu_lock.acquire_for_tts().await;
                if !sink.alive() {
                    break;
                }
                // Only matters when TTS is routed through koblem: tier-2 keeps
                // mp3, the keyless tier-1 fallback is clamped to wav.
                let endpoint = SpeechEndpoint::resolve(via_koblem, koblem_api_key.as_deref());
                match super::synthesize_sentence(
                    &service_url,
                    backend,
                    endpoint,
                    &text,
                    &voice.id,
                    &voice.params,
                )
                .await
                {
                    Ok(audio) => {
                        let item = Item::Sentence { index, text, audio };
                        if !sink.emit(item).await {
                            break;
                        }
                    }
                    Err(e) => error!(index, error = %e, "TTS sentence synthesis failed"),
                }
            }
        }
    }
    sink.emit(Item::Event(json!({"type": "done"}))).await;
}

/// Passages are synthesised in turn, sequentially — the audio has to arrive in order,
/// and the GPU serialises the renders anyway. Sizes ramp (see `split_passages`): a
/// short first request so the aligner has something to publish almost immediately,
/// then long ones, which by the time the listener reaches them have had a whole
/// passage's playback to be aligned in.
async fn run_passages(
    passages: &[Passage],
    koblem_url: &str,
    koblem_api_key: &str,
    align: bool,
    sink: &mut impl Sink,
) {
    let mut ctx = PassageCtx::default();
    // 24 kHz s16le mono: 48 bytes per millisecond. Counted rather than taken from
    // the aligner's totals because the offsets have to be right with the aligner off.
    let mut pcm_bytes: u64 = 0;

    'passages: for (index, passage) in passages.iter().enumerate() {
        if !sink.ready().await {
            break;
        }
        ctx.index = index as u32;
        ctx.audio_offset_ms = (pcm_bytes / 48) as u32;
        let stream = super::synthesize_paragraph_stream(
            koblem_url,
            koblem_api_key,
            &passage.text,
            &passage.voice.id,
            &passage.voice.params,
            align,
        );
        tokio::pin!(stream);

        while let Some(event) = stream.next().await {
            if !sink.alive() {
                break 'passages;
            }
            if let Ok(TtsStreamEvent::Audio { ref pcm }) = event {
                pcm_bytes += pcm.len() as u64;
            }
            let (item, last) = match passage_line(&passage.text, &ctx, event) {
                PassageLine::Skip => continue,
                PassageLine::Send(item) => (item, false),
                PassageLine::Fail(item) => (item, true),
            };
            if !sink.emit(item).await || last {
                break 'passages;
            }
        }

        ctx.word_offset += super::word_count(&passage.text) as u32;
    }
}

/// Where the passage being streamed sits in the whole read. The read is one
/// continuous timeline even though it arrives as several renders, and these are
/// what make a passage's local word indices and timings addressable in it.
#[derive(Debug, Default, Clone, Copy)]
struct PassageCtx {
    index: u32,
    /// Words in every passage before this one.
    word_offset: u32,
    /// Milliseconds of audio emitted before this passage started.
    audio_offset_ms: u32,
}

enum PassageLine {
    /// Nothing the client needs to hear about.
    Skip,
    Send(Item),
    /// Sent, and then the passage is over.
    Fail(Item),
}

fn passage_line(
    text: &str,
    ctx: &PassageCtx,
    event: Result<TtsStreamEvent, TtsError>,
) -> PassageLine {
    let send = |v: Value| PassageLine::Send(Item::Event(v));
    let fail = |v: Value| PassageLine::Fail(Item::Event(v));
    match event {
        Ok(TtsStreamEvent::Started { speed_applied }) => send(json!({
            "type": "paragraph_start",
            "index": ctx.index,
            "text": text,
            "word_offset": ctx.word_offset,
            "audio_offset_ms": ctx.audio_offset_ms,
            "sample_rate": PCM_SAMPLE_RATE,
            "speed": speed_applied,
        })),
        Ok(TtsStreamEvent::Audio { pcm }) => PassageLine::Send(Item::Pcm {
            paragraph_index: ctx.index,
            pcm,
        }),
        // Sentence boundary the engine reached, with no aligner involved. Its
        // offsets are globalised here — unlike the word indices on an
        // `alignment_*`, which stay local to the paragraph — because it is the
        // event a client without alignment steers the whole read by, and making
        // it self-contained keeps that path from having to stitch anything.
        Ok(TtsStreamEvent::Chunk {
            index,
            chunks_total,
            word_offset,
            word_count,
            audio_offset_ms,
            text: chunk_text,
        }) => send(json!({
            "type": "chunk",
            "paragraph_index": ctx.index,
            "index": index,
            "chunks_total": chunks_total,
            "word_offset": ctx.word_offset + word_offset,
            "word_count": word_count,
            "audio_offset_ms": ctx.audio_offset_ms + audio_offset_ms,
            "text": chunk_text,
        })),
        Ok(TtsStreamEvent::AlignmentPartial {
            audio_seen_ms,
            words,
        }) => send(json!({
            "type": "alignment_partial",
            "paragraph_index": ctx.index,
            "audio_seen_ms": audio_seen_ms,
            "words": words,
        })),
        Ok(TtsStreamEvent::AlignmentFinal {
            audio_total_ms,
            words,
        }) => send(json!({
            "type": "alignment_final",
            "paragraph_index": ctx.index,
            "audio_total_ms": audio_total_ms,
            "words": words,
        })),
        // Abandoned part way. The stream closes cleanly with real audio behind
        // it, so silence here reads as a finished chapter and the reader turns
        // the page on the half it never heard. Reported before `truncated`
        // because a cancelled render is usually truncated as well, and the
        // cancel is the thing that explains it.
        //
        // Fatal to the read, not just to this passage: whatever abandoned it —
        // a lost client, a pre-empted GPU — is still true for the passage after
        // it, and continuing would paper a gap over with audio that carries on
        // as if nothing was missed.
        Ok(TtsStreamEvent::Done {
            cancelled: true, ..
        }) => {
            error!("TTS abandoned the passage part way");
            fail(json!({
                "type": "error",
                "paragraph_index": ctx.index,
                "message": "TTS stopped part way through the passage",
            }))
        }
        Ok(TtsStreamEvent::Done {
            truncated: false, ..
        }) => PassageLine::Skip,
        Ok(TtsStreamEvent::Done {
            truncated: true, ..
        }) => {
            // A short mid-sentence clip that plays fine — the reader would just stop
            // early with no explanation. Not fatal: the audio already streamed is
            // still worth playing out.
            error!("TTS truncated the passage; long-form should prevent this");
            send(json!({
                "type": "error",
                "paragraph_index": ctx.index,
                "message": "TTS stopped early — the passage was truncated mid-sentence",
            }))
        }
        // Every error ends the read, not just the passage it landed on. A
        // severed body, a refused auth and an unreachable service alike mean no
        // further audio is coming, and the passage after this one would be
        // asking the same service the same question. A client told otherwise
        // reads the failure as a gap and moves on to the next chapter — which,
        // with TTS down, walks the whole book at network speed.
        Err(e) => {
            let message = match &e {
                TtsError::ServiceError {
                    status: 401 | 403, ..
                } => {
                    error!("koblem rejected the TTS stream auth — check koblem_api_key");
                    "koblem rejected the request — verify koblem_api_key in Search settings"
                        .to_string()
                }
                _ => {
                    error!(error = %e, "TTS passage stream failed");
                    e.to_string()
                }
            };
            fail(json!({
                "type": "error",
                "paragraph_index": ctx.index,
                "message": message,
                "fatal": true,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value_of(line: &PassageLine) -> Option<&Value> {
        match line {
            PassageLine::Send(Item::Event(v)) | PassageLine::Fail(Item::Event(v)) => Some(v),
            _ => None,
        }
    }

    fn fatal_of(line: &PassageLine) -> Option<bool> {
        value_of(line).map(|v| v["fatal"] == json!(true))
    }

    #[test]
    fn a_broken_upstream_ends_the_passage() {
        for e in [
            TtsError::Unavailable("no koblem".into()),
            TtsError::ServiceError {
                status: 500,
                body: String::new(),
            },
            TtsError::ServiceError {
                status: 401,
                body: String::new(),
            },
        ] {
            let line = passage_line("hello", &PassageCtx::default(), Err(e));
            assert!(
                matches!(line, PassageLine::Fail(_)),
                "a dead upstream was reported as something to carry on from",
            );
            assert_eq!(fatal_of(&line), Some(true));
        }
    }

    #[test]
    fn a_truncated_passage_is_still_worth_playing_out() {
        let line = passage_line(
            "hello",
            &PassageCtx::default(),
            Ok(TtsStreamEvent::Done {
                truncated: true,
                cancelled: false,
            }),
        );
        assert!(matches!(line, PassageLine::Send(_)));
        assert_eq!(fatal_of(&line), Some(false));
    }

    /// The failure this exists for: an abandoned render closes its stream the
    /// way a finished one does, so saying nothing lets the reader auto-advance
    /// past the half of the chapter it never heard. It ends the read as well —
    /// whatever abandoned this passage is still true for the next one.
    #[test]
    fn an_abandoned_passage_says_so_and_ends_the_read() {
        let line = passage_line(
            "hello",
            &PassageCtx::default(),
            Ok(TtsStreamEvent::Done {
                truncated: false,
                cancelled: true,
            }),
        );
        assert!(matches!(line, PassageLine::Fail(_)));
        assert_eq!(fatal_of(&line), Some(false));
    }

    /// The offsets are the only thing making several renders into one read:
    /// a client adds them to the paragraph-local indices the aligner reports.
    #[test]
    fn a_later_passage_reports_where_it_sits_in_the_read() {
        let ctx = PassageCtx {
            index: 2,
            word_offset: 520,
            audio_offset_ms: 187_000,
        };
        let line = passage_line(
            "the third passage",
            &ctx,
            Ok(TtsStreamEvent::Started { speed_applied: 1.0 }),
        );
        let v = value_of(&line).expect("paragraph_start was not sent");
        assert_eq!(v["index"], json!(2));
        assert_eq!(v["word_offset"], json!(520));
        assert_eq!(v["audio_offset_ms"], json!(187_000));
        assert_eq!(v["text"], json!("the third passage"));
    }

    /// A chunk anchor is what a reader without alignment steers by, so it is
    /// globalised where the alignment word indices are not.
    #[test]
    fn a_chunk_anchor_is_globalised_against_the_whole_read() {
        let ctx = PassageCtx {
            index: 1,
            word_offset: 120,
            audio_offset_ms: 44_000,
        };
        let line = passage_line(
            "unused",
            &ctx,
            Ok(TtsStreamEvent::Chunk {
                index: 3,
                chunks_total: 5,
                word_offset: 90,
                word_count: 31,
                audio_offset_ms: 25_500,
                text: "A sentence or three.".to_string(),
            }),
        );
        let v = value_of(&line).expect("chunk was not sent");
        assert_eq!(v["type"], json!("chunk"));
        assert_eq!(v["paragraph_index"], json!(1));
        assert_eq!(v["word_offset"], json!(210));
        assert_eq!(v["audio_offset_ms"], json!(69_500));
        // Local to the chunk, not the read — a count, not a position.
        assert_eq!(v["word_count"], json!(31));
    }

    #[test]
    fn a_clean_finish_says_nothing() {
        let line = passage_line(
            "hello",
            &PassageCtx::default(),
            Ok(TtsStreamEvent::Done {
                truncated: false,
                cancelled: false,
            }),
        );
        assert!(matches!(line, PassageLine::Skip));
    }
}
