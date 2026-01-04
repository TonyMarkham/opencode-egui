use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("No microphone found. Please check your system audio settings.")]
    NoMicrophoneFound,

    #[error("Failed to initialize audio device: {0}")]
    DeviceInitFailed(String),

    #[error("Failed to start audio stream: {0}")]
    StreamStartFailed(String),

    #[error("No audio captured (silence or too short)")]
    NoAudioCaptured,

    #[error("Failed to resample audio: {0}")]
    ResampleFailed(String),

    #[error("Whisper model not found at: {path}")]
    ModelNotFound { path: String },

    #[error("Failed to load Whisper model: {0}")]
    ModelLoadFailed(String),

    #[error("Transcription failed: {0}")]
    TranscriptionFailed(String),

    #[error("Audio format error: {0}")]
    FormatError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
