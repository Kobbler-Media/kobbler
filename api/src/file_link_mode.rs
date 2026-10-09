use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Key on the kobload download/want metadata blob. Kobload echoes metadata back
/// verbatim on the ingest callback — nested under `original_metadata` when the
/// download came from a watch pattern — so this is how the intent behind a grab
/// reaches the linking step minutes later.
pub const METADATA_KEY: &str = "additional_copy";

/// What a newly ingested file does to the files already on the entity.
///
/// `Replace` is the default on every path: a rescan or a re-import of the same
/// episode must not leave duplicates behind. `AddCopy` is a regrab — the old file
/// stays, the player prefers the newest (`tv_episode_files::get_preferred_available`),
/// and cleanup of the superseded copy is a human action in the web UI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileLinkMode {
    #[default]
    Replace,
    AddCopy,
}

impl FileLinkMode {
    pub fn from_additional_copy(additional_copy: bool) -> Self {
        if additional_copy {
            Self::AddCopy
        } else {
            Self::Replace
        }
    }

    pub fn is_add_copy(self) -> bool {
        matches!(self, Self::AddCopy)
    }
}

/// Stamp the mode onto an outgoing kobload metadata blob. Every producer goes
/// through this so the key cannot drift out of step with the ingest-side reader.
pub fn tag_metadata(mut metadata: Value, mode: FileLinkMode) -> Value {
    if let Some(obj) = metadata.as_object_mut() {
        obj.insert(METADATA_KEY.to_string(), Value::Bool(mode.is_add_copy()));
    }
    metadata
}

/// Query string opt-ins for the routes that start an acquisition without a body.
/// Both absent is a plain want: replace on ingest, and Kobload's usual
/// upgrade-eligibility rules apply.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct AcquireQuery {
    #[serde(default)]
    pub additional_copy: bool,
    /// A person asked for this exact thing right now, so Kobload treats an
    /// already-imported item as a candidate again. It still picks the release
    /// under the quality profile — this only reopens the door.
    #[serde(default)]
    pub manual: bool,
}

impl AcquireQuery {
    pub fn mode(&self) -> FileLinkMode {
        FileLinkMode::from_additional_copy(self.additional_copy)
    }
}
