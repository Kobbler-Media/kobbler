use tracing::warn;
use uuid::Uuid;

use crate::routes::admin::tts::preset_clip_path;
use crate::runtime_config::RuntimeConfigManager;
use crate::tts::{self, TtsBackend};

/// A cast voice's clone taken out of service: its clip off the disk and its
/// voice out of the engine. Neither failing is worth stopping for.
pub async fn discard(
    storage_root: &str,
    runtime_config: &RuntimeConfigManager,
    owner_id: Uuid,
    preset_id: Uuid,
) {
    let path = preset_clip_path(storage_root, owner_id, preset_id);
    if !path.exists() {
        return;
    }
    if let Err(e) = tokio::fs::remove_file(&path).await {
        warn!(error = %e, "failed to remove a cast voice clip");
    }
    forget(runtime_config, preset_id).await;
}

pub async fn forget(runtime_config: &RuntimeConfigManager, preset_id: Uuid) {
    let config = runtime_config.get().await;
    let Some(url) = config.effective_tts_url().filter(|u| !u.is_empty()) else {
        return;
    };
    let backend = TtsBackend::from_config(&config.tts_service_type);
    if let Err(e) = tts::delete_voice(&url, backend, &format!("preset-{preset_id}")).await {
        warn!(error = %e, "failed to delete a cast voice clone from the engine");
    }
}
