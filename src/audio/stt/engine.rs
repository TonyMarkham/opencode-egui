use crate::audio::AudioError;
use std::path::{Path, PathBuf};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct SttEngine {
    whisper_ctx: WhisperContext,
    #[allow(dead_code)]
    model_path: PathBuf,
}

impl SttEngine {
    pub fn new(model_path: &Path) -> Result<Self, AudioError> {
        if !model_path.exists() {
            return Err(AudioError::ModelNotFound {
                path: model_path.display().to_string(),
            });
        }

        let whisper_ctx = WhisperContext::new_with_params(
            model_path.to_str().unwrap(),
            WhisperContextParameters::default(),
        )
        .map_err(|e| AudioError::ModelLoadFailed(e.to_string()))?;

        Ok(SttEngine {
            whisper_ctx: whisper_ctx,
            model_path: model_path.to_path_buf(),
        })
    }

    pub fn transcribe(&mut self, audio_samples: &[f32]) -> Result<String, AudioError> {
        let mut state = self
            .whisper_ctx
            .create_state()
            .map_err(|e| AudioError::TranscriptionFailed(e.to_string()))?;

        let params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        state
            .full(params, audio_samples)
            .map_err(|e| AudioError::TranscriptionFailed(e.to_string()))?;

        let num_segments = state.full_n_segments();

        let mut text = String::new();
        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                let segment_text = segment
                    .to_str()
                    .map_err(|e| AudioError::TranscriptionFailed(e.to_string()))?;
                text.push_str(segment_text);
            }
        }

        Ok(text.trim().to_string())
    }
}
