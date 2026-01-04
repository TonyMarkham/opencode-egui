use crate::audio::AudioError;
use rubato::{
    Resampler as RubatoResampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

pub struct Resampler {
    resampler: SincFixedIn<f32>,
    from_rate: u32,
    to_rate: u32,
}

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Result<Self, AudioError> {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };

        let resampler = SincFixedIn::<f32>::new(
            to_rate as f64 / from_rate as f64,
            2.0, // max_resample_ratio_relative
            params,
            from_rate as usize, // chunk_size
            1,                  // nbr_channels
        )
        .map_err(|e| AudioError::ResampleFailed(e.to_string()))?;

        Ok(Resampler {
            resampler: resampler,
            from_rate: from_rate,
            to_rate: to_rate,
        })
    }

    pub fn resample(&mut self, input: &[f32]) -> Result<Vec<f32>, AudioError> {
        // Process in chunks since SincFixedIn expects fixed chunk sizes
        let chunk_size = self.from_rate as usize;
        let mut output = Vec::new();

        // Process complete chunks
        for chunk in input.chunks(chunk_size) {
            if chunk.len() == chunk_size {
                let waves_in = vec![chunk.to_vec()];
                let waves_out = self
                    .resampler
                    .process(&waves_in, None)
                    .map_err(|e| AudioError::ResampleFailed(e.to_string()))?;
                output.extend_from_slice(&waves_out[0]);
            } else {
                // Handle last partial chunk by padding with zeros
                let mut padded = chunk.to_vec();
                padded.resize(chunk_size, 0.0);
                let waves_in = vec![padded];
                let waves_out = self
                    .resampler
                    .process(&waves_in, None)
                    .map_err(|e| AudioError::ResampleFailed(e.to_string()))?;
                // Only take the portion corresponding to actual data
                let valid_samples =
                    (chunk.len() as f64 * self.to_rate as f64 / self.from_rate as f64) as usize;
                output.extend_from_slice(&waves_out[0][..valid_samples]);
            }
        }

        Ok(output)
    }
}
