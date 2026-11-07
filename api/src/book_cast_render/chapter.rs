use kob_ffmpeg::pcm_stream::AdtsAacEncoder;
use tokio_util::sync::CancellationToken;

use crate::tts::cast::BIT_RATE;
use crate::tts::timeline::{self, Item, PCM_SAMPLE_RATE, Plan, Sink};

const AAC_FRAME_SAMPLES: u64 = 1024;

pub struct ChapterAudio {
    pub adts: Vec<u8>,
    pub duration_ms: u64,
}

/// One chapter read in full, as ADTS AAC. Any passage that does not come back
/// fails the chapter: a gap in an audiobook is not something to carry on past.
pub async fn render(plan: Plan, cancel: &CancellationToken) -> Result<ChapterAudio, String> {
    let expected = plan.passages.len();
    let mut sink = ChapterSink {
        encoder: AdtsAacEncoder::new(PCM_SAMPLE_RATE as i32, BIT_RATE)
            .map_err(|e| e.to_string())?,
        adts: Vec::new(),
        frames: 0,
        heard: 0,
        failure: None,
        cancel: cancel.clone(),
    };
    timeline::run(plan, &mut sink).await;
    if cancel.is_cancelled() {
        return Err("cancelled".to_string());
    }
    if let Some(failure) = sink.failure {
        return Err(failure);
    }
    if sink.heard < expected {
        return Err(format!(
            "TTS rendered {} of the chapter's {expected} passages",
            sink.heard
        ));
    }
    let tail = sink.encoder.finish().map_err(|e| e.to_string())?;
    sink.append(tail);
    Ok(ChapterAudio {
        adts: sink.adts,
        duration_ms: sink.frames * AAC_FRAME_SAMPLES * 1000 / u64::from(PCM_SAMPLE_RATE),
    })
}

struct ChapterSink {
    encoder: AdtsAacEncoder,
    adts: Vec<u8>,
    frames: u64,
    heard: usize,
    failure: Option<String>,
    cancel: CancellationToken,
}

impl ChapterSink {
    fn append(&mut self, frames: Vec<kob_ffmpeg::pcm_stream::AdtsFrame>) {
        for f in frames {
            self.frames += 1;
            self.adts.extend_from_slice(&f.bytes);
        }
    }

    fn push(&mut self, pcm: &[u8]) {
        match self.encoder.push_s16le(pcm) {
            Ok(frames) => self.append(frames),
            Err(e) => self.failure = Some(format!("AAC encoding failed: {e}")),
        }
    }
}

impl Sink for ChapterSink {
    fn alive(&self) -> bool {
        self.failure.is_none() && !self.cancel.is_cancelled()
    }

    async fn ready(&mut self) -> bool {
        self.alive()
    }

    async fn emit(&mut self, item: Item) -> bool {
        match item {
            Item::Event(v) => match v["type"].as_str() {
                Some("paragraph_start") => self.heard += 1,
                Some("error") => {
                    self.failure = Some(v["message"].as_str().unwrap_or("TTS failed").to_string());
                }
                _ => {}
            },
            Item::Pcm { pcm, .. } => self.push(&pcm),
            Item::Sentence { index, audio, .. } => {
                match timeline::sentence_pcm(index, &audio).await {
                    Some(pcm) => {
                        self.heard += 1;
                        self.push(&pcm);
                    }
                    None => self.failure = Some(format!("sentence {index} would not decode")),
                }
            }
        }
        self.alive()
    }
}
