//! Voice Activity Detection (VAD) processor
//!
//! This module provides a VAD processor that detects speech in audio streams
//! and emits `UserStartedSpeaking` and `UserStoppedSpeaking` frames.
//!
//! # Architecture
//!
//! The VAD processor uses the Silero VAD model (via `voice_activity_detector` crate)
//! to analyze incoming audio and detect speech segments. It maintains internal state
//! to track transitions between speaking and non-speaking states.
//!
//! # Example
//!
//! ```ignore
//! use voxlab::processors::SileroVADProcessor;
//!
//! let vad = SileroVADProcessor::new()
//!     .with_threshold(0.5)
//!     .with_min_speech_duration(0.25)
//!     .with_min_silence_duration(0.3);
//!
//! // Use in pipeline
//! let processors: Vec<Box<dyn ProcessorBehavior>> = vec![
//!     Box::new(vad),
//!     Box::new(stt_service),
//! ];
//! ```

use async_trait::async_trait;
use tracing::{debug, info, warn};
use voice_activity_detector::VoiceActivityDetector;

use crate::frame::{Frame, FrameHeader};
use crate::processor::{ProcessorBehavior, ProcessorSetup};

/// VAD processor using Silero VAD model
///
/// Analyzes incoming `InputAudioRaw` frames and emits `UserStartedSpeaking`
/// and `UserStoppedSpeaking` frames based on detected speech activity.
///
/// # Configuration
///
/// - `threshold`: Speech probability threshold (0.0-1.0, default: 0.5)
/// - `min_speech_duration_ms`: Minimum speech duration before triggering start (default: 250ms)
/// - `min_silence_duration_ms`: Minimum silence duration before triggering stop (default: 300ms)
pub struct SileroVADProcessor {
    /// The underlying VAD detector
    vad: Option<VoiceActivityDetector>,
    /// Speech probability threshold (0.0 to 1.0)
    threshold: f32,
    /// Minimum speech duration in ms before triggering UserStartedSpeaking
    min_speech_duration_ms: u32,
    /// Minimum silence duration in ms before triggering UserStoppedSpeaking
    min_silence_duration_ms: u32,
    /// Current sample rate
    sample_rate: u32,
    /// Whether user is currently speaking
    is_speaking: bool,
    /// Accumulated speech duration in samples
    speech_samples: u32,
    /// Accumulated silence duration in samples
    silence_samples: u32,
    /// Audio buffer for resampling/chunking
    audio_buffer: Vec<i16>,
    /// Whether to pass through audio frames
    audio_passthrough: bool,
    /// Sample counter for periodic logging
    log_sample_counter: u32,
}

impl SileroVADProcessor {
    /// Create a new VAD processor with default settings
    pub fn new() -> Self {
        Self {
            vad: None,
            threshold: 0.5,
            min_speech_duration_ms: 250,
            min_silence_duration_ms: 300,
            sample_rate: 16000,
            is_speaking: false,
            speech_samples: 0,
            silence_samples: 0,
            audio_buffer: Vec::new(),
            audio_passthrough: true,
            log_sample_counter: 0,
        }
    }

    /// Set the speech probability threshold (0.0 to 1.0)
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Set the minimum speech duration before triggering start (in milliseconds)
    pub fn with_min_speech_duration_ms(mut self, ms: u32) -> Self {
        self.min_speech_duration_ms = ms;
        self
    }

    /// Set the minimum silence duration before triggering stop (in milliseconds)
    pub fn with_min_silence_duration_ms(mut self, ms: u32) -> Self {
        self.min_silence_duration_ms = ms;
        self
    }

    /// Set whether to pass through audio frames downstream
    pub fn with_audio_passthrough(mut self, passthrough: bool) -> Self {
        self.audio_passthrough = passthrough;
        self
    }

    /// Initialize the VAD detector for the given sample rate
    fn init_vad(&mut self, sample_rate: u32) -> bool {
        // Silero VAD only supports 8kHz and 16kHz
        let vad_sample_rate = if sample_rate <= 8000 { 8000 } else { 16000 };
        
        // Warn if there's a mismatch (indicates missing resampling)
        if sample_rate != vad_sample_rate {
            warn!(
                "SileroVAD: Sample rate mismatch! Audio is {}Hz but VAD expects {}Hz. \
                 Audio will be mislabeled. Add resampling processor or configure your \
                 microphone to use 16kHz in Audio MIDI Setup.",
                sample_rate, vad_sample_rate
            );
        }

        // Chunk size: 256 for 8kHz, 512 for 16kHz
        let chunk_size = if vad_sample_rate == 8000 { 256usize } else { 512usize };

        match VoiceActivityDetector::builder()
            .sample_rate(vad_sample_rate)
            .chunk_size(chunk_size)
            .build()
        {
            Ok(vad) => {
                self.vad = Some(vad);
                self.sample_rate = sample_rate;
                debug!(
                    "SileroVAD: Initialized with sample_rate={}, vad_sample_rate={}, chunk_size={}",
                    sample_rate, vad_sample_rate, chunk_size
                );
                true
            }
            Err(e) => {
                warn!("SileroVAD: Failed to initialize: {:?}", e);
                false
            }
        }
    }

    /// Convert milliseconds to samples
    fn ms_to_samples(&self, ms: u32) -> u32 {
        (ms as u64 * self.sample_rate as u64 / 1000) as u32
    }

    /// Process audio chunk and return speech probability
    fn process_audio_chunk(&mut self, samples: &[i16]) -> Option<f32> {
        let vad = self.vad.as_mut()?;

        // Get the expected chunk size from VAD
        let vad_sample_rate = if self.sample_rate <= 8000 { 8000 } else { 16000 };
        let chunk_size = if vad_sample_rate == 8000 { 256 } else { 512 };

        // Add samples to buffer
        self.audio_buffer.extend_from_slice(samples);

        // Process complete chunks
        let mut max_prob = 0.0f32;
        let mut processed = false;
        let mut first_chunk_max = 0i16;
        let log_stats = self.speech_samples == 0 && self.silence_samples == 0;

        while self.audio_buffer.len() >= chunk_size {
            let chunk: Vec<i16> = self.audio_buffer.drain(..chunk_size).collect();
            
            // Capture first chunk stats
            if processed == false && log_stats {
                first_chunk_max = chunk.iter().map(|s| s.abs()).max().unwrap_or(0);
            }
            
            let prob = vad.predict(chunk);
            max_prob = max_prob.max(prob);
            processed = true;
        }

        if log_stats && processed {
            info!("[SileroVAD] First VAD chunk max_amp={}", first_chunk_max);
        }

        if processed {
            Some(max_prob)
        } else {
            None
        }
    }

    /// Update speaking state based on probability
    fn update_state(&mut self, probability: f32, num_samples: u32) -> Option<Frame> {
        let is_speech = probability >= self.threshold;
        
        // Log probability periodically (every second)
        self.log_sample_counter += num_samples;
        if self.log_sample_counter >= self.sample_rate {
            info!("[{}] VAD probability: {:.3} (threshold: {:.3}, is_speech: {})",
                  self.name(), probability, self.threshold, is_speech);
            self.log_sample_counter = 0;
        }

        if is_speech {
            self.speech_samples += num_samples;
            self.silence_samples = 0;

            // Check if we should trigger start
            if !self.is_speaking {
                let min_samples = self.ms_to_samples(self.min_speech_duration_ms);
                if self.speech_samples >= min_samples {
                    self.is_speaking = true;
                    debug!(
                        "SileroVAD: User started speaking (prob={:.2}, duration={}ms)",
                        probability,
                        self.speech_samples * 1000 / self.sample_rate
                    );
                    return Some(Frame::UserStartedSpeaking {
                        header: FrameHeader::default(),
                        emulated: false,
                    });
                }
            }
        } else {
            self.silence_samples += num_samples;
            self.speech_samples = 0;

            // Check if we should trigger stop
            if self.is_speaking {
                let min_samples = self.ms_to_samples(self.min_silence_duration_ms);
                if self.silence_samples >= min_samples {
                    self.is_speaking = false;
                    debug!(
                        "SileroVAD: User stopped speaking (prob={:.2}, silence={}ms)",
                        probability,
                        self.silence_samples * 1000 / self.sample_rate
                    );
                    return Some(Frame::UserStoppedSpeaking {
                        header: FrameHeader::default(),
                        emulated: false,
                    });
                }
            }
        }

        None
    }
}

impl Default for SileroVADProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProcessorBehavior for SileroVADProcessor {
    fn name(&self) -> &str {
        "SileroVAD"
    }

    async fn setup(&mut self, setup: &ProcessorSetup) {
        let sample_rate = if setup.audio_in_sample_rate > 0 {
            setup.audio_in_sample_rate
        } else {
            16000
        };
        self.init_vad(sample_rate);
    }

    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        match &frame {
            Frame::InputAudioRaw {
                audio,
                sample_rate,
                ..
            } => {
                if self.speech_samples == 0 && self.silence_samples == 0 {
                    info!("[{}] Processing first audio frame ({} bytes, {}Hz)", 
                          self.name(), audio.len(), sample_rate);
                }
                
                // Initialize VAD if needed or sample rate changed
                if self.vad.is_none() || *sample_rate != self.sample_rate {
                    info!("[{}] Initializing VAD for sample rate: {}", self.name(), sample_rate);
                    if !self.init_vad(*sample_rate) {
                        // VAD init failed, pass through
                        return if self.audio_passthrough {
                            Some(frame)
                        } else {
                            None
                        };
                    }
                }

                // Convert bytes to i16 samples (assuming little-endian 16-bit PCM)
                let samples: Vec<i16> = audio
                    .chunks_exact(2)
                    .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
                    .collect();

                let num_samples = samples.len() as u32;
                
                // Log audio stats on first few frames
                if self.speech_samples == 0 && self.silence_samples == 0 {
                    let max_amplitude = samples.iter().map(|s| s.abs()).max().unwrap_or(0);
                    let avg_amplitude = if !samples.is_empty() {
                        samples.iter().map(|s| s.abs() as i64).sum::<i64>() / samples.len() as i64
                    } else {
                        0
                    };
                    info!("[{}] Audio stats: {} samples, max_amp={}, avg_amp={}", 
                          self.name(), samples.len(), max_amplitude, avg_amplitude);
                }

                // Process audio and get probability
                if let Some(probability) = self.process_audio_chunk(&samples) {
                    // Update state and check for transitions
                    if let Some(event_frame) = self.update_state(probability, num_samples) {
                        // We have a state transition event
                        // TODO: Support emitting multiple frames
                        // For now, return the event frame (audio continues in buffer)
                        return Some(event_frame);
                    }
                }

                // Pass through or drop audio based on config
                if self.audio_passthrough {
                    Some(frame)
                } else {
                    None
                }
            }

            Frame::Start { audio_in_sample_rate, .. } => {
                // Initialize VAD on start
                if *audio_in_sample_rate > 0 {
                    self.init_vad(*audio_in_sample_rate);
                }
                // Reset state
                self.is_speaking = false;
                self.speech_samples = 0;
                self.silence_samples = 0;
                self.audio_buffer.clear();
                Some(frame)
            }

            Frame::End { .. } => {
                // If user was speaking, emit stop event
                if self.is_speaking {
                    self.is_speaking = false;
                    // TODO: Support emitting multiple frames (stop + end)
                }
                Some(frame)
            }

            // Pass through all other frames
            _ => Some(frame),
        }
    }

    async fn cleanup(&mut self) {
        self.vad = None;
        self.audio_buffer.clear();
        self.is_speaking = false;
        self.speech_samples = 0;
        self.silence_samples = 0;
        debug!("SileroVAD: Cleanup complete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vad_processor_creation() {
        let vad = SileroVADProcessor::new()
            .with_threshold(0.6)
            .with_min_speech_duration_ms(200)
            .with_min_silence_duration_ms(400);

        assert!((vad.threshold - 0.6).abs() < 0.001);
        assert_eq!(vad.min_speech_duration_ms, 200);
        assert_eq!(vad.min_silence_duration_ms, 400);
    }

    #[test]
    fn test_ms_to_samples() {
        let mut vad = SileroVADProcessor::new();
        vad.sample_rate = 16000;

        assert_eq!(vad.ms_to_samples(1000), 16000);
        assert_eq!(vad.ms_to_samples(500), 8000);
        assert_eq!(vad.ms_to_samples(250), 4000);
    }

    #[test]
    fn test_threshold_clamping() {
        let vad = SileroVADProcessor::new().with_threshold(1.5);
        assert!((vad.threshold - 1.0).abs() < 0.001);

        let vad = SileroVADProcessor::new().with_threshold(-0.5);
        assert!((vad.threshold - 0.0).abs() < 0.001);
    }
}
