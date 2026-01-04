// Based on chat-poc/audio_service.rs:19-104
use crate::audio::AudioError;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use std::sync::{Arc, Mutex};

pub struct AudioCapturer {
    device: Device,
    config: StreamConfig,
    stream: Option<Stream>,
    samples: Arc<Mutex<Vec<f32>>>,
}

impl AudioCapturer {
    pub fn new() -> Result<Self, AudioError> {
        let host = cpal::default_host();

        let device = host
            .default_input_device()
            .ok_or(AudioError::NoMicrophoneFound)?;

        let config: StreamConfig = device
            .default_input_config()
            .map_err(|e| AudioError::DeviceInitFailed(e.to_string()))?
            .into();

        Ok(AudioCapturer {
            device: device,
            config: config,
            stream: None,
            samples: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub fn start(&mut self) -> Result<(), AudioError> {
        // Clear previous samples
        if let Ok(mut samples) = self.samples.lock() {
            samples.clear();
        }

        let samples = Arc::clone(&self.samples);
        let channels = self.config.channels as usize;

        let stream = self
            .device
            .build_input_stream(
                &self.config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buffer) = samples.lock() {
                        if channels == 1 {
                            buffer.extend_from_slice(data);
                        } else {
                            // Convert stereo to mono by averaging
                            for chunk in data.chunks(channels) {
                                let mono = chunk.iter().sum::<f32>() / channels as f32;
                                buffer.push(mono);
                            }
                        }
                    }
                },
                |err| eprintln!("Stream error: {}", err),
                None,
            )
            .map_err(|e| AudioError::StreamStartFailed(e.to_string()))?;

        stream
            .play()
            .map_err(|e| AudioError::StreamStartFailed(e.to_string()))?;

        self.stream = Some(stream);
        Ok(())
    }

    pub fn stop(&mut self) -> Result<Vec<f32>, AudioError> {
        // Drop the stream to stop recording
        self.stream = None;

        // Give CPAL time to flush any buffered audio chunks
        // CPAL callbacks may still be in flight even after stream is dropped
        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut samples = self
            .samples
            .lock()
            .map_err(|_| AudioError::NoAudioCaptured)?;

        if samples.is_empty() {
            return Err(AudioError::NoAudioCaptured);
        }

        // Clone samples for return, then clear buffer for next recording
        let result = samples.clone();
        samples.clear();

        Ok(result)
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate.0
    }
}
