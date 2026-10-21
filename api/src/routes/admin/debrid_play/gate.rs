use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::AppState;
use crate::ffmpeg::cue_gate::CueGate;
use crate::ffmpeg::mkv_cue_cache::{
    ByteSource, CueTrimPolicy, FileByteSource, HttpByteSource, LocalFirstByteSource, is_matroska,
};

pub(crate) struct GateSpec<'a> {
    pub info_hash: Option<&'a str>,
    pub filename: &'a str,
    pub filesize: i64,
    pub local: Option<&'a Path>,
    pub url: Option<String>,
}

/// The cue gate every response of this session splices through. Built on first use and
/// kept on the session, so the trim decision — and the policy it was made under — holds
/// for the session's lifetime. A play with no live session gets a gate of its own.
pub(crate) async fn session_cue_gate(
    state: &AppState,
    session_id: Uuid,
    spec: GateSpec<'_>,
) -> Arc<CueGate> {
    let sidecars_available = match state.playback_sessions.get(&session_id) {
        Some(s) if s.cue_gate.is_some() => return s.cue_gate.clone().unwrap(),
        // Debrid sessions extract no sidecars, so the client renders subtitles from the
        // container and the language it will pick has to keep its index.
        Some(s) => !s.subtitle_files.is_empty(),
        None => false,
    };

    let rt = state.runtime_config.get().await;
    let gate = if !rt.mkv_cue_trim_enabled || !is_matroska(spec.filename) {
        CueGate::off()
    } else {
        let total = spec.filesize.max(0) as u64;
        let remote = spec.url.and_then(|url| {
            crate::http::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .ok()
                .map(|client| HttpByteSource { client, url })
        });
        let source: Box<dyn ByteSource> = Box::new(LocalFirstByteSource {
            local: spec.local.map(|p| FileByteSource {
                path: p.to_path_buf(),
            }),
            total,
            remote,
        });
        CueGate::new(
            state.cue_trim_cache.clone(),
            format!(
                "remote:{}:{}:{}",
                spec.info_hash.unwrap_or(""),
                spec.filename,
                spec.filesize
            ),
            CueTrimPolicy {
                sidecars_available,
                preferred_languages: vec![rt.preferred_subtitle_language.clone()],
            },
            source,
        )
    };

    let Some(mut live) = state.playback_sessions.get_mut(&session_id) else {
        return gate;
    };
    if let Some(existing) = &live.cue_gate {
        return existing.clone();
    }
    live.cue_gate = Some(gate.clone());
    drop(live);
    let warming = gate.clone();
    tokio::spawn(async move { warming.warm().await });
    gate
}
