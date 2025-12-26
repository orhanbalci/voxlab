//! Transport layer for media input/output abstraction
//!
//! This module provides the foundation for transport implementations including
//! parameter configuration and abstract base classes for input/output transport
//! functionality.

mod base;
mod error;
mod input;
pub mod local;
mod media_sender;
mod output;

pub use base::BaseTransport;
pub use error::TransportError;
pub use input::{BaseInputTransport, BaseInputTransportActor, BaseInputTransportState};
pub use output::{BaseOutputTransport, BaseOutputTransportActor, BaseOutputTransportState};

/// Configuration parameters for transport implementations
#[derive(Debug, Clone)]
pub struct TransportParams {
    // Audio input settings
    /// Enable audio input streaming
    pub audio_in_enabled: bool,
    /// Input audio sample rate in Hz (None = use pipeline default)
    pub audio_in_sample_rate: Option<u32>,
    /// Number of input audio channels
    pub audio_in_channels: u32,
    /// Start audio streaming immediately on transport start
    pub audio_in_stream_on_start: bool,
    /// Pass through input audio frames downstream
    pub audio_in_passthrough: bool,

    // Audio output settings
    /// Enable audio output streaming
    pub audio_out_enabled: bool,
    /// Output audio sample rate in Hz (None = use pipeline default)
    pub audio_out_sample_rate: Option<u32>,
    /// Number of output audio channels
    pub audio_out_channels: u32,
    /// Output audio bitrate in bits per second
    pub audio_out_bitrate: u32,
    /// Number of 10ms chunks to buffer for output (e.g., 4 = 40ms)
    pub audio_out_10ms_chunks: u32,
    /// List of audio output destination identifiers
    pub audio_out_destinations: Vec<String>,

    // Video input settings
    /// Enable video input streaming
    pub video_in_enabled: bool,

    // Video output settings
    /// Enable video output streaming
    pub video_out_enabled: bool,
    /// Enable real-time video output streaming
    pub video_out_is_live: bool,
    /// Video output width in pixels
    pub video_out_width: u32,
    /// Video output height in pixels
    pub video_out_height: u32,
    /// Video output bitrate in bits per second
    pub video_out_bitrate: u32,
    /// Video output frame rate in FPS
    pub video_out_framerate: u32,
    /// Video output color format string (e.g., "RGB", "RGBA", "YUV420")
    pub video_out_color_format: String,
    /// List of video output destination identifiers
    pub video_out_destinations: Vec<String>,
}

impl Default for TransportParams {
    fn default() -> Self {
        Self {
            // Audio input defaults
            audio_in_enabled: false,
            audio_in_sample_rate: None,
            audio_in_channels: 1,
            audio_in_stream_on_start: true,
            audio_in_passthrough: true,

            // Audio output defaults
            audio_out_enabled: false,
            audio_out_sample_rate: None,
            audio_out_channels: 1,
            audio_out_bitrate: 96000,
            audio_out_10ms_chunks: 4,
            audio_out_destinations: Vec::new(),

            // Video input defaults
            video_in_enabled: false,

            // Video output defaults
            video_out_enabled: false,
            video_out_is_live: false,
            video_out_width: 1024,
            video_out_height: 768,
            video_out_bitrate: 800000,
            video_out_framerate: 30,
            video_out_color_format: "RGB".to_string(),
            video_out_destinations: Vec::new(),
        }
    }
}

impl TransportParams {
    /// Create a new TransportParams with audio input enabled
    pub fn with_audio_input(mut self) -> Self {
        self.audio_in_enabled = true;
        self
    }

    /// Create a new TransportParams with audio output enabled
    pub fn with_audio_output(mut self) -> Self {
        self.audio_out_enabled = true;
        self
    }

    /// Create a new TransportParams with video input enabled
    pub fn with_video_input(mut self) -> Self {
        self.video_in_enabled = true;
        self
    }

    /// Create a new TransportParams with video output enabled
    pub fn with_video_output(mut self) -> Self {
        self.video_out_enabled = true;
        self
    }

    /// Set audio input sample rate
    pub fn with_audio_in_sample_rate(mut self, sample_rate: u32) -> Self {
        self.audio_in_sample_rate = Some(sample_rate);
        self
    }

    /// Set audio output sample rate
    pub fn with_audio_out_sample_rate(mut self, sample_rate: u32) -> Self {
        self.audio_out_sample_rate = Some(sample_rate);
        self
    }

    /// Set video output dimensions
    pub fn with_video_dimensions(mut self, width: u32, height: u32) -> Self {
        self.video_out_width = width;
        self.video_out_height = height;
        self
    }

    /// Set video output framerate
    pub fn with_video_framerate(mut self, framerate: u32) -> Self {
        self.video_out_framerate = framerate;
        self
    }

    /// Add an audio output destination
    pub fn with_audio_destination(mut self, destination: String) -> Self {
        self.audio_out_destinations.push(destination);
        self
    }

    /// Add a video output destination
    pub fn with_video_destination(mut self, destination: String) -> Self {
        self.video_out_destinations.push(destination);
        self
    }

    /// Disable audio passthrough
    pub fn without_audio_passthrough(mut self) -> Self {
        self.audio_in_passthrough = false;
        self
    }

    /// Disable audio streaming on start
    pub fn without_audio_stream_on_start(mut self) -> Self {
        self.audio_in_stream_on_start = false;
        self
    }
}
