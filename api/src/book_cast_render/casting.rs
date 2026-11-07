//! The voices a render reads with. Each clone is copied at the start and
//! registered under the render's own name, so an edit made meanwhile, which
//! deletes or replaces the cast's clips and the engine's `preset-` clones,
//! leaves the render reading with what it started with.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::warn;
use uuid::Uuid;

use crate::book_cast_vetting::speaking;
use crate::repositories::user_voice_presets::UserVoicePreset;
use crate::routes::admin::tts::preset_clip_path;
use crate::tts::timeline::Voice;
use crate::tts::{self, TtsBackend};

#[derive(Debug, thiserror::Error)]
pub enum CastingError {
    #[error("a cast voice's preset is gone")]
    PresetGone,
    #[error("keeping a voice's clip: {0}")]
    Clip(#[from] std::io::Error),
    #[error("registering a voice: {0}")]
    Register(#[from] tts::TtsError),
}

pub struct Casting<'a> {
    book_id: Uuid,
    dir: PathBuf,
    service_url: &'a str,
    backend: TtsBackend,
    presets: &'a HashMap<Uuid, UserVoicePreset>,
    registered: Vec<String>,
    voices: HashMap<(Uuid, Option<String>), Arc<Voice>>,
}

impl<'a> Casting<'a> {
    pub fn new(
        book_id: Uuid,
        work: &Path,
        service_url: &'a str,
        backend: TtsBackend,
        presets: &'a HashMap<Uuid, UserVoicePreset>,
    ) -> Self {
        Self {
            book_id,
            dir: work.join("voices"),
            service_url,
            backend,
            presets,
            registered: Vec::new(),
            voices: HashMap::new(),
        }
    }

    pub async fn keep_clips(&self, storage_root: &str) -> Result<(), CastingError> {
        tokio::fs::create_dir_all(&self.dir).await?;
        for p in self.presets.values().filter(|p| p.clip_filename.is_some()) {
            tokio::fs::copy(
                preset_clip_path(storage_root, p.user_id, p.id),
                self.clip(p.id),
            )
            .await?;
        }
        Ok(())
    }

    fn clip(&self, preset_id: Uuid) -> PathBuf {
        self.dir.join(format!("{preset_id}.wav"))
    }

    fn clone_name(&self, preset_id: Uuid) -> String {
        format!("render-{}-{preset_id}", self.book_id)
    }

    pub async fn voice(
        &mut self,
        preset_id: Uuid,
        cue: Option<&str>,
    ) -> Result<Arc<Voice>, CastingError> {
        let key = (preset_id, cue.map(str::to_string));
        if let Some(v) = self.voices.get(&key) {
            return Ok(v.clone());
        }
        let preset = self
            .presets
            .get(&preset_id)
            .ok_or(CastingError::PresetGone)?;
        let mut voice = speaking(preset, cue);
        if preset.clip_filename.is_some() {
            let name = self.clone_name(preset_id);
            if !self.registered.contains(&name) {
                // A render that died before letting go leaves its clone behind,
                // and the engine may keep the old take under a name re-uploaded.
                tts::delete_voice(self.service_url, self.backend, &name).await?;
                let wav = tokio::fs::read(self.clip(preset_id)).await?;
                tts::upload_voice(
                    self.service_url,
                    self.backend,
                    &name,
                    &wav,
                    preset.ref_text.as_deref(),
                )
                .await?;
                self.registered.push(name.clone());
            }
            voice.id = name;
        }
        voice.params.response_format = Some("wav".to_string());
        let voice = Arc::new(voice);
        self.voices.insert(key, voice.clone());
        Ok(voice)
    }

    pub async fn release(self) {
        for name in &self.registered {
            if let Err(e) = tts::delete_voice(self.service_url, self.backend, name).await {
                warn!(error = %e, "failed to delete a render's voice clone from the engine");
            }
        }
        if let Err(e) = tokio::fs::remove_dir_all(&self.dir).await {
            warn!(error = %e, "failed to remove a render's kept clips");
        }
    }
}
