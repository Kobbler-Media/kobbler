use crate::repositories::RepositoryError;
use crate::routes::admin::tts::{ClipError, SampleError};
use crate::tts::TtsError;

#[derive(Debug, thiserror::Error)]
pub enum VettingError {
    #[error("{0}")]
    NotFound(&'static str),
    #[error("{0}")]
    Conflict(&'static str),
    #[error("{0}")]
    BadRequest(&'static str),
    #[error("TTS is not configured")]
    NoTts,
    #[error("the engine did not return wav")]
    NotWav,
    #[error("registering the voice with the TTS service failed: {0}")]
    Register(#[from] TtsError),
    #[error(transparent)]
    Sample(#[from] SampleError),
    #[error(transparent)]
    Clip(#[from] ClipError),
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub const PRESET_GONE: VettingError =
    VettingError::Conflict("the voice's preset is gone; pick another");
