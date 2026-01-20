//! Base STT (Speech-to-Text) service infrastructure
//!
//! This module provides the foundational components for building STT services:
//!
//! - [`AudioBuffer`]: Accumulates audio data with automatic capacity management
//! - [`STTService`]: Trait for implementing speech-to-text services
//! - [`SegmentedSTTService`]: Base struct for VAD-based segmented transcription
//!
//! # Architecture
//!
//! STT services typically work in one of two modes:
//!
//! 1. **Continuous/Streaming**: Audio is sent continuously to the service (e.g., WebSocket-based)
//! 2. **Segmented**: Audio is accumulated during speech and sent when the user stops speaking
//!
//! This module provides infrastructure for the segmented approach, which is simpler
//! and works well with REST-based STT APIs like OpenAI Whisper.
//!
//! # Example
//!
//! ```ignore
//! use voxlab::services::stt::{SegmentedSTTService, STTService};
//!
//! struct MySTTService {
//!     base: SegmentedSTTService,
//!     api_key: String,
//! }
//!
//! impl MySTTService {
//!     async fn transcribe_audio(&self, audio: &[u8], sample_rate: u32) -> Option<String> {
//!         // Call your STT API here
//!         todo!()
//!     }
//! }
//! ```

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::frame::{Frame, FrameHeader};
use crate::processor::{ProcessorBehavior, ProcessorSetup};

// ============================================================================
// Audio Buffer
// ============================================================================

/// A buffer for accumulating audio data with automatic capacity management
///
/// This buffer is designed for STT services that need to accumulate audio
/// during speech segments. It maintains a configurable amount of "pre-speech"
/// audio to compensate for VAD detection latency.
///
/// # Features
///
/// - Automatic pre-buffer management (keeps audio before speech starts)
/// - Configurable maximum duration to prevent unbounded memory growth
/// - WAV encoding support for APIs that require WAV format
/// - Sample rate tracking for proper audio handling
#[derive(Debug)]
pub struct AudioBuffer {
    /// Raw audio data (16-bit PCM, little-endian)
    data: Vec<u8>,
    /// Sample rate of the audio
    sample_rate: u32,
    /// Number of audio channels
    channels: u16,
    /// Maximum buffer duration in seconds (0 = unlimited)
    max_duration_secs: f32,
    /// Pre-speech buffer duration in seconds
    /// Keeps this much audio before VAD triggers to compensate for detection latency
    pre_speech_secs: f32,
}

impl AudioBuffer {
    /// Create a new audio buffer
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - Audio sample rate in Hz
    /// * `channels` - Number of audio channels (typically 1 for STT)
    pub fn new(sample_rate: u32, channels: u16) -> Self {
        Self {
            data: Vec::new(),
            sample_rate,
            channels,
            max_duration_secs: 30.0, // Default 30 second max
            pre_speech_secs: 1.0,    // Default 1 second pre-buffer
        }
    }

    /// Set the maximum buffer duration
    ///
    /// Audio older than this will be discarded. Set to 0 for unlimited.
    pub fn with_max_duration(mut self, secs: f32) -> Self {
        self.max_duration_secs = secs;
        self
    }

    /// Set the pre-speech buffer duration
    ///
    /// This amount of audio is kept before speech starts to compensate
    /// for VAD detection latency.
    pub fn with_pre_speech_duration(mut self, secs: f32) -> Self {
        self.pre_speech_secs = secs;
        self
    }

    /// Update the sample rate (useful during setup)
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
    }

    /// Get the current sample rate
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Append audio data to the buffer
    ///
    /// # Arguments
    ///
    /// * `audio` - Raw audio bytes (16-bit PCM, little-endian)
    pub fn append(&mut self, audio: &[u8]) {
        self.data.extend_from_slice(audio);

        // Enforce max duration if set
        if self.max_duration_secs > 0.0 {
            let max_bytes = self.duration_to_bytes(self.max_duration_secs);
            if self.data.len() > max_bytes {
                let trim = self.data.len() - max_bytes;
                self.data.drain(0..trim);
            }
        }
    }

    /// Trim the buffer to keep only the pre-speech duration
    ///
    /// Call this when the user is NOT speaking to maintain only the
    /// pre-buffer audio for VAD latency compensation.
    pub fn trim_to_pre_speech(&mut self) {
        let keep_bytes = self.duration_to_bytes(self.pre_speech_secs);
        if self.data.len() > keep_bytes {
            let trim = self.data.len() - keep_bytes;
            self.data.drain(0..trim);
        }
    }

    /// Clear the buffer
    pub fn clear(&mut self) {
        self.data.clear();
    }

    /// Check if the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Get the current buffer length in bytes
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Get the current buffer duration in seconds
    pub fn duration_secs(&self) -> f32 {
        self.bytes_to_duration(self.data.len())
    }

    /// Get the raw audio data
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Take the audio data, consuming the buffer contents
    pub fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.data)
    }

    /// Encode the buffer as a WAV file
    ///
    /// Returns the WAV file as bytes, suitable for APIs that require WAV format.
    pub fn to_wav(&self) -> Vec<u8> {
        encode_wav(&self.data, self.sample_rate, self.channels)
    }

    /// Take the audio and encode as WAV
    pub fn take_as_wav(&mut self) -> Vec<u8> {
        let data = self.take();
        encode_wav(&data, self.sample_rate, self.channels)
    }

    // Helper: convert duration to bytes
    fn duration_to_bytes(&self, secs: f32) -> usize {
        let bytes_per_sample = 2; // 16-bit
        let samples = (secs * self.sample_rate as f32) as usize;
        samples * bytes_per_sample * self.channels as usize
    }

    // Helper: convert bytes to duration
    fn bytes_to_duration(&self, bytes: usize) -> f32 {
        let bytes_per_sample = 2; // 16-bit
        let samples = bytes / (bytes_per_sample * self.channels as usize);
        samples as f32 / self.sample_rate as f32
    }
}

impl Default for AudioBuffer {
    fn default() -> Self {
        Self::new(16000, 1)
    }
}

// ============================================================================
// WAV Encoding
// ============================================================================

/// Encode raw PCM audio as a WAV file
///
/// # Arguments
///
/// * `pcm_data` - Raw 16-bit PCM audio (little-endian)
/// * `sample_rate` - Sample rate in Hz
/// * `channels` - Number of channels
///
/// # Returns
///
/// WAV file as bytes
pub fn encode_wav(pcm_data: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
    let block_align = channels * bits_per_sample / 8;
    let data_size = pcm_data.len() as u32;
    let file_size = 36 + data_size;

    let mut wav = Vec::with_capacity(44 + pcm_data.len());

    // RIFF header
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");

    // fmt chunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // Chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // Audio format (PCM)
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());

    // data chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm_data);

    wav
}

// ============================================================================
// STT Service Trait
// ============================================================================

/// Result of a transcription operation
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    /// The transcribed text
    pub text: String,
    /// Detected language (if available)
    pub language: Option<String>,
    /// Confidence score (0.0 to 1.0, if available)
    pub confidence: Option<f32>,
}

/// Trait for STT services that perform transcription
///
/// Implement this trait to create a custom STT service. The service
/// receives audio data and returns transcription results.
#[async_trait]
pub trait STTService: Send + Sync {
    /// Transcribe audio data
    ///
    /// # Arguments
    ///
    /// * `audio` - Raw audio bytes (16-bit PCM, little-endian)
    /// * `sample_rate` - Sample rate in Hz
    /// * `channels` - Number of audio channels
    ///
    /// # Returns
    ///
    /// Transcription result, or None if transcription failed
    async fn transcribe(
        &self,
        audio: &[u8],
        sample_rate: u32,
        channels: u16,
    ) -> Option<TranscriptionResult>;

    /// Transcribe audio from a WAV file
    ///
    /// Default implementation extracts PCM from WAV and calls `transcribe`.
    /// Override if your API prefers WAV format directly.
    async fn transcribe_wav(&self, wav_data: &[u8]) -> Option<TranscriptionResult> {
        // Parse WAV header to get format info
        if wav_data.len() < 44 {
            return None;
        }

        // Extract sample rate (bytes 24-27)
        let sample_rate = u32::from_le_bytes([wav_data[24], wav_data[25], wav_data[26], wav_data[27]]);

        // Extract channels (bytes 22-23)
        let channels = u16::from_le_bytes([wav_data[22], wav_data[23]]);

        // PCM data starts at byte 44
        let pcm_data = &wav_data[44..];

        self.transcribe(pcm_data, sample_rate, channels).await
    }
}

// ============================================================================
// Segmented STT Service
// ============================================================================

/// State for tracking speech activity
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpeechState {
    /// User is not speaking
    NotSpeaking,
    /// User is currently speaking
    Speaking,
}

/// Base struct for VAD-based segmented STT services
///
/// This struct handles:
/// - Audio accumulation during speech
/// - Pre-speech buffering for VAD latency compensation
/// - Mute/unmute control via STTMute frames
/// - Automatic transcription trigger on UserStoppedSpeaking
///
/// # Usage
///
/// Embed this struct in your STT service implementation and delegate
/// frame processing to it. Override `run_stt` to perform actual transcription.
///
/// # Example
///
/// ```ignore
/// struct WhisperSTTService {
///     base: SegmentedSTTService,
///     client: reqwest::Client,
///     api_key: String,
/// }
///
/// #[async_trait]
/// impl ProcessorBehavior for WhisperSTTService {
///     fn name(&self) -> &str { "WhisperSTT" }
///
///     async fn process(&mut self, frame: Frame) -> Option<Frame> {
///         self.base.process_frame(frame, |audio, sr, ch| {
///             Box::pin(self.transcribe_with_whisper(audio, sr, ch))
///         }).await
///     }
/// }
/// ```
pub struct SegmentedSTTService<S: STTService> {
    /// The underlying STT service for transcription
    stt_service: S,
    /// Audio buffer for accumulating speech
    buffer: AudioBuffer,
    /// Current speech state
    speech_state: SpeechState,
    /// Whether STT is muted
    muted: bool,
    /// User ID for transcription frames
    user_id: Option<String>,
    /// Whether to pass through audio frames downstream
    audio_passthrough: bool,
    /// Minimum audio duration to trigger transcription (seconds)
    min_audio_duration: f32,
}

impl<S: STTService> SegmentedSTTService<S> {
    /// Create a new segmented STT service
    ///
    /// # Arguments
    ///
    /// * `stt_service` - The underlying STT service for transcription
    pub fn new(stt_service: S) -> Self {
        Self {
            stt_service,
            buffer: AudioBuffer::default(),
            speech_state: SpeechState::NotSpeaking,
            muted: false,
            user_id: None,
            audio_passthrough: true,
            min_audio_duration: 0.1, // 100ms minimum
        }
    }

    /// Set the sample rate for the audio buffer
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.buffer.set_sample_rate(sample_rate);
        self
    }

    /// Set the user ID for transcription frames
    pub fn with_user_id(mut self, user_id: String) -> Self {
        self.user_id = Some(user_id);
        self
    }

    /// Set whether to pass through audio frames downstream
    pub fn with_audio_passthrough(mut self, passthrough: bool) -> Self {
        self.audio_passthrough = passthrough;
        self
    }

    /// Set the minimum audio duration to trigger transcription
    pub fn with_min_audio_duration(mut self, secs: f32) -> Self {
        self.min_audio_duration = secs;
        self
    }

    /// Set the pre-speech buffer duration
    pub fn with_pre_speech_duration(mut self, secs: f32) -> Self {
        self.buffer = self.buffer.with_pre_speech_duration(secs);
        self
    }

    /// Set the maximum buffer duration
    pub fn with_max_duration(mut self, secs: f32) -> Self {
        self.buffer = self.buffer.with_max_duration(secs);
        self
    }

    /// Get a reference to the underlying STT service
    pub fn stt_service(&self) -> &S {
        &self.stt_service
    }

    /// Get a mutable reference to the underlying STT service
    pub fn stt_service_mut(&mut self) -> &mut S {
        &mut self.stt_service
    }

    /// Check if the service is currently muted
    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Process incoming audio data
    fn process_audio(&mut self, audio: &[u8]) {
        if self.muted {
            return;
        }

        self.buffer.append(audio);

        // If not speaking, trim to pre-speech buffer
        if self.speech_state == SpeechState::NotSpeaking {
            self.buffer.trim_to_pre_speech();
        }
    }

    /// Handle user started speaking event
    fn on_user_started_speaking(&mut self) {
        debug!("SegmentedSTT: User started speaking");
        self.speech_state = SpeechState::Speaking;
    }

    /// Handle user stopped speaking event and trigger transcription
    async fn on_user_stopped_speaking(&mut self) -> Option<Frame> {
        debug!(
            "SegmentedSTT: User stopped speaking, buffer duration: {:.2}s",
            self.buffer.duration_secs()
        );
        self.speech_state = SpeechState::NotSpeaking;

        if self.muted {
            self.buffer.clear();
            return None;
        }

        // Check minimum duration
        if self.buffer.duration_secs() < self.min_audio_duration {
            debug!("SegmentedSTT: Audio too short, skipping transcription");
            self.buffer.clear();
            return None;
        }

        // Get audio and transcribe
        let audio = self.buffer.take();
        let sample_rate = self.buffer.sample_rate();

        if audio.is_empty() {
            return None;
        }

        debug!(
            "SegmentedSTT: Transcribing {} bytes of audio",
            audio.len()
        );

        match self.stt_service.transcribe(&audio, sample_rate, 1).await {
            Some(result) => {
                if result.text.trim().is_empty() {
                    debug!("SegmentedSTT: Empty transcription result");
                    return None;
                }

                debug!("SegmentedSTT: Transcription: {}", result.text);

                Some(Frame::Transcription {
                    header: FrameHeader::default(),
                    text: result.text,
                    user_id: self.user_id.clone(),
                    language: result.language,
                    confidence: result.confidence,
                })
            }
            None => {
                warn!("SegmentedSTT: Transcription failed");
                None
            }
        }
    }
}

#[async_trait]
impl<S: STTService + 'static> ProcessorBehavior for SegmentedSTTService<S> {
    fn name(&self) -> &str {
        "SegmentedSTT"
    }

    async fn setup(&mut self, setup: &ProcessorSetup) {
        if setup.audio_in_sample_rate > 0 {
            self.buffer.set_sample_rate(setup.audio_in_sample_rate);
        }
        debug!(
            "SegmentedSTT: Setup complete, sample_rate={}",
            self.buffer.sample_rate()
        );
    }

    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        match &frame {
            Frame::InputAudioRaw {
                audio,
                sample_rate,
                num_channels,
                ..
            } => {
                // Update sample rate if it changed
                if *sample_rate != self.buffer.sample_rate() {
                    self.buffer.set_sample_rate(*sample_rate);
                }
                let _ = num_channels; // Currently assuming mono

                self.process_audio(audio);

                // Pass through or drop based on config
                if self.audio_passthrough {
                    Some(frame)
                } else {
                    None
                }
            }

            Frame::UserStartedSpeaking { .. } => {
                self.on_user_started_speaking();
                Some(frame)
            }

            Frame::UserStoppedSpeaking { .. } => {
                // Transcribe and potentially emit a Transcription frame
                // We need to return both the UserStoppedSpeaking frame AND
                // the Transcription frame. For now, we'll return the transcription
                // if available, otherwise the original frame.
                // TODO: Support emitting multiple frames
                if let Some(transcription) = self.on_user_stopped_speaking().await {
                    Some(transcription)
                } else {
                    Some(frame)
                }
            }

            Frame::STTMute { mute, .. } => {
                debug!("SegmentedSTT: Mute = {}", mute);
                self.muted = *mute;
                if self.muted {
                    self.buffer.clear();
                }
                Some(frame)
            }

            // Pass through all other frames
            _ => Some(frame),
        }
    }

    async fn cleanup(&mut self) {
        self.buffer.clear();
        debug!("SegmentedSTT: Cleanup complete");
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_buffer_append() {
        let mut buffer = AudioBuffer::new(16000, 1);

        buffer.append(&[0u8; 1000]);
        assert_eq!(buffer.len(), 1000);

        buffer.append(&[0u8; 500]);
        assert_eq!(buffer.len(), 1500);
    }

    #[test]
    fn test_audio_buffer_duration() {
        let mut buffer = AudioBuffer::new(16000, 1);

        // 16000 samples/sec * 2 bytes/sample * 1 channel = 32000 bytes/sec
        // 32000 bytes = 1 second
        buffer.append(&[0u8; 32000]);
        assert!((buffer.duration_secs() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_audio_buffer_trim_to_pre_speech() {
        let mut buffer = AudioBuffer::new(16000, 1)
            .with_pre_speech_duration(0.5);

        // Add 2 seconds of audio
        buffer.append(&[0u8; 64000]);

        // Trim to 0.5 seconds
        buffer.trim_to_pre_speech();

        // Should have ~0.5 seconds (16000 bytes)
        assert!((buffer.duration_secs() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_audio_buffer_max_duration() {
        let mut buffer = AudioBuffer::new(16000, 1)
            .with_max_duration(1.0);

        // Add 2 seconds of audio
        buffer.append(&[0u8; 64000]);

        // Should be trimmed to 1 second
        assert!((buffer.duration_secs() - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_wav_encoding() {
        let pcm = vec![0u8; 1000];
        let wav = encode_wav(&pcm, 16000, 1);

        // Check RIFF header
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");

        // Total size should be 44 header + 1000 data
        assert_eq!(wav.len(), 44 + 1000);
    }
}
