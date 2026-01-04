pub mod capture;
pub mod engine;
pub mod resampler;

use crate::audio::AudioError;
use capture::AudioCapturer;
use engine::SttEngine;
use resampler::Resampler;
use std::path::Path;

pub struct AudioManager {
    capturer: AudioCapturer,
    resampler: Resampler,
    stt_engine: SttEngine,
    pub model_path: std::path::PathBuf,
}

impl AudioManager {
    pub fn new(model_path: &Path) -> Result<Self, AudioError> {
        let capturer = AudioCapturer::new()?;
        let device_rate = capturer.sample_rate();
        let resampler = Resampler::new(device_rate, 16000)?;
        let stt_engine = SttEngine::new(model_path)?;

        Ok(AudioManager {
            capturer: capturer,
            resampler: resampler,
            stt_engine: stt_engine,
            model_path: model_path.to_path_buf(),
        })
    }

    pub fn start_recording(&mut self) -> Result<(), AudioError> {
        self.capturer.start()
    }

    /// Stop recording and return raw samples (fast)
    pub fn stop_recording_raw(&mut self) -> Result<Vec<f32>, AudioError> {
        self.capturer.stop()
    }

    /// Get the device sample rate
    pub fn sample_rate(&self) -> u32 {
        self.capturer.sample_rate()
    }

    /// Resample and transcribe audio samples (slow - run on separate thread)
    pub fn transcribe_samples(&mut self, samples: &[f32]) -> Result<String, AudioError> {
        let resampled = self.resampler.resample(samples)?;
        let transcription = self.stt_engine.transcribe(&resampled)?;
        Ok(transcription)
    }

    /// Stop recording and transcribe immediately (blocks until transcription completes)
    pub fn stop_recording(&mut self) -> Result<String, AudioError> {
        let samples = self.stop_recording_raw()?;
        self.transcribe_samples(&samples)
    }
}
