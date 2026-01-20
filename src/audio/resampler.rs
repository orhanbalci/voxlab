//! Audio Resampling Processor
//!
//! This module provides a processor that resamples audio from one sample rate to another.
//! Uses the `rubato` library for high-quality resampling with proper anti-aliasing.
//!
//! # Example
//!
//! ```ignore
//! use voxlab::processors::AudioResamplerProcessor;
//!
//! let resampler = AudioResamplerProcessor::new()
//!     .with_target_sample_rate(16000);
//!
//! // Use in pipeline before VAD
//! let processors: Vec<Box<dyn ProcessorBehavior>> = vec![
//!     Box::new(resampler),
//!     Box::new(vad),
//!     Box::new(stt_service),
//! ];
//! ```

use async_trait::async_trait;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

use crate::frame::Frame;
use crate::processor::{ProcessorBehavior, ProcessorSetup};

/// Audio resampling processor
///
/// Resamples incoming `InputAudioRaw` frames from their original sample rate
/// to a target sample rate. This is essential when the audio input device
/// captures at a different rate than what downstream processors (like VAD) expect.
///
/// # Configuration
///
/// - `target_sample_rate`: The desired output sample rate (default: 16000 Hz)
/// - `channels`: Number of audio channels (default: 1 - mono)
pub struct AudioResamplerProcessor {
    /// Target sample rate for output
    target_sample_rate: u32,
    /// Number of audio channels
    channels: usize,
    /// The resampler instance (created when input rate is known)
    /// Wrapped in Arc<Mutex<>> to satisfy Send + Sync requirements
    resampler: Arc<Mutex<Option<SincFixedIn<f32>>>>,
    /// Current input sample rate
    input_sample_rate: Arc<Mutex<u32>>,
    /// Buffer for incomplete frames
    input_buffer: Arc<Mutex<Vec<i16>>>,
    /// Buffer for resampled output
    output_buffer: Arc<Mutex<Vec<f32>>>,
}

impl AudioResamplerProcessor {
    /// Create a new resampling processor with default settings (16 kHz output)
    pub fn new() -> Self {
        Self {
            target_sample_rate: 16000,
            channels: 1,
            resampler: Arc::new(Mutex::new(None)),
            input_sample_rate: Arc::new(Mutex::new(0)),
            input_buffer: Arc::new(Mutex::new(Vec::new())),
            output_buffer: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Set the target sample rate for output audio
    pub fn with_target_sample_rate(mut self, sample_rate: u32) -> Self {
        self.target_sample_rate = sample_rate;
        self
    }

    /// Set the number of audio channels (default: 1 for mono)
    pub fn with_channels(mut self, channels: usize) -> Self {
        self.channels = channels;
        self
    }

    /// Initialize or reinitialize the resampler for the given input sample rate
    fn init_resampler(&mut self, input_rate: u32) -> bool {
        if input_rate == self.target_sample_rate {
            info!(
                "[AudioResampler] No resampling needed: input and output both {}Hz",
                input_rate
            );
            *self.resampler.lock().unwrap() = None;
            *self.input_sample_rate.lock().unwrap() = input_rate;
            return true;
        }

        // Create high-quality resampler with sinc interpolation
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };

        // Calculate chunk size: process ~30ms chunks
        let chunk_frames = (input_rate as f64 * 0.03) as usize;

        match SincFixedIn::<f32>::new(
            self.target_sample_rate as f64 / input_rate as f64,
            2.0, // max_resample_ratio_relative
            params,
            chunk_frames,
            self.channels,
        ) {
            Ok(resampler_instance) => {
                info!(
                    "[AudioResampler] Initialized: {}Hz -> {}Hz (ratio: {:.4}, chunk_frames: {})",
                    input_rate,
                    self.target_sample_rate,
                    self.target_sample_rate as f64 / input_rate as f64,
                    chunk_frames
                );
                *self.resampler.lock().unwrap() = Some(resampler_instance);
                *self.input_sample_rate.lock().unwrap() = input_rate;
                self.input_buffer.lock().unwrap().clear();
                self.output_buffer.lock().unwrap().clear();
                true
            }
            Err(e) => {
                warn!("[AudioResampler] Failed to initialize resampler: {:?}", e);
                false
            }
        }
    }

    /// Resample audio from input rate to target rate
    fn resample_audio(&mut self, samples: &[i16]) -> Option<Vec<u8>> {
        // If no resampling needed, just convert to bytes
        if self.resampler.lock().unwrap().is_none() {
            let bytes: Vec<u8> = samples
                .iter()
                .flat_map(|&s| s.to_le_bytes())
                .collect();
            return Some(bytes);
        }

        let mut resampler_guard = self.resampler.lock().unwrap();
        let resampler = resampler_guard.as_mut()?;

        // Add samples to input buffer
        let mut input_buffer = self.input_buffer.lock().unwrap();
        input_buffer.extend_from_slice(samples);

        let chunk_size = resampler.input_frames_next();
        
        // If we don't have enough samples yet, wait for more
        if input_buffer.len() < chunk_size * self.channels {
            return None;
        }

        // Convert i16 to f32 for rubato (normalized to -1.0 to 1.0)
        let input_f32: Vec<f32> = input_buffer
            .iter()
            .take(chunk_size * self.channels)
            .map(|&s| s as f32 / 32768.0)
            .collect();

        // Remove processed samples from buffer
        input_buffer.drain(..chunk_size * self.channels);

        // Prepare input in channel-separated format (rubato expects Vec<Vec<f32>>)
        let mut waves_in = vec![Vec::with_capacity(chunk_size); self.channels];
        for (i, &sample) in input_f32.iter().enumerate() {
            waves_in[i % self.channels].push(sample);
        }

        // Resample
        match resampler.process(&waves_in, None) {
            Ok(waves_out) => {
                // Convert back to interleaved i16
                let output_frames = waves_out[0].len();
                let mut output_samples = Vec::with_capacity(output_frames * self.channels);

                for frame_idx in 0..output_frames {
                    for channel_idx in 0..self.channels {
                        let sample_f32 = waves_out[channel_idx][frame_idx];
                        // Clamp and convert to i16
                        let sample_i16 = (sample_f32.clamp(-1.0, 1.0) * 32767.0) as i16;
                        output_samples.push(sample_i16);
                    }
                }

                // Convert to bytes
                let bytes: Vec<u8> = output_samples
                    .iter()
                    .flat_map(|&s| s.to_le_bytes())
                    .collect();

                Some(bytes)
            }
            Err(e) => {
                warn!("[AudioResampler] Resampling failed: {:?}", e);
                None
            }
        }
    }
}

impl Default for AudioResamplerProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProcessorBehavior for AudioResamplerProcessor {
    fn name(&self) -> &str {
        "AudioResampler"
    }

    async fn setup(&mut self, _setup: &ProcessorSetup) {
        debug!("[AudioResampler] Setup with target_sample_rate={}", self.target_sample_rate);
    }

    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        match frame {
            Frame::InputAudioRaw {
                audio,
                sample_rate,
                num_channels,
                header,
            } => {
                let current_input_rate = *self.input_sample_rate.lock().unwrap();
                
                // Initialize resampler if needed or sample rate changed
                if self.resampler.lock().unwrap().is_none() || sample_rate != current_input_rate {
                    if !self.init_resampler(sample_rate) {
                        // Failed to init, pass through unchanged
                        return Some(Frame::InputAudioRaw {
                            audio,
                            sample_rate,
                            num_channels,
                            header,
                        });
                    }
                }

                // If no resampling needed, pass through
                if sample_rate == self.target_sample_rate {
                    return Some(Frame::InputAudioRaw {
                        audio,
                        sample_rate,
                        num_channels,
                        header,
                    });
                }

                // Convert bytes to i16 samples
                let samples: Vec<i16> = audio
                    .chunks_exact(2)
                    .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
                    .collect();

                // Resample
                if let Some(resampled_bytes) = self.resample_audio(&samples) {
                    Some(Frame::InputAudioRaw {
                        audio: resampled_bytes,
                        sample_rate: self.target_sample_rate,
                        num_channels,
                        header,
                    })
                } else {
                    // Not enough samples yet, wait for next frame
                    None
                }
            }

            Frame::Start { 
                audio_in_sample_rate, 
                audio_out_sample_rate,
                allow_interruptions,
                enable_metrics,
                header,
            } => {
                // Initialize resampler on start
                if audio_in_sample_rate > 0 {
                    self.init_resampler(audio_in_sample_rate);
                }
                // Pass through Start frame with updated sample rate
                Some(Frame::Start {
                    audio_in_sample_rate: self.target_sample_rate,
                    audio_out_sample_rate,
                    allow_interruptions,
                    enable_metrics,
                    header,
                })
            }

            Frame::End { .. } => {
                // Clear buffers on end
                self.input_buffer.lock().unwrap().clear();
                self.output_buffer.lock().unwrap().clear();
                Some(frame)
            }

            // Pass through all other frames unchanged
            _ => Some(frame),
        }
    }

    async fn cleanup(&mut self) {
        *self.resampler.lock().unwrap() = None;
        self.input_buffer.lock().unwrap().clear();
        self.output_buffer.lock().unwrap().clear();
        debug!("[AudioResampler] Cleanup complete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resampler_creation() {
        let resampler = AudioResamplerProcessor::new()
            .with_target_sample_rate(16000)
            .with_channels(1);

        assert_eq!(resampler.target_sample_rate, 16000);
        assert_eq!(resampler.channels, 1);
    }

    #[test]
    fn test_no_resampling_needed() {
        let mut resampler = AudioResamplerProcessor::new().with_target_sample_rate(16000);
        
        // Should succeed without needing resampler
        assert!(resampler.init_resampler(16000));
        assert!(resampler.resampler.lock().unwrap().is_none());
    }
}
