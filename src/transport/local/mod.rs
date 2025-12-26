//! Local audio transport implementation
//!
//! This module provides a local audio transport that uses CPAL for real-time
//! audio input and output through the system's default audio devices.

mod input;
mod output;

pub use input::{
    list_input_devices, CpalInputBackend, LocalAudioInputTransport, LocalAudioInputTransportActor,
    LocalAudioInputTransportState,
};
pub use output::{
    list_output_devices, CpalOutputBackend, LocalAudioOutputTransport, LocalAudioOutputTransportActor,
    LocalAudioOutputTransportState,
};

use thiserror::Error;

use crate::processor::PipelineActorRef;
use crate::transport::{BaseTransport, TransportParams};

/// Errors that can occur in local audio transport
#[derive(Error, Debug)]
pub enum LocalAudioError {
    #[error("No input device available")]
    NoInputDevice,
    #[error("No output device available")]
    NoOutputDevice,
    #[error("Failed to build stream: {0}")]
    StreamBuildError(String),
    #[error("Failed to play stream: {0}")]
    StreamPlayError(String),
    #[error("Device not found: {0}")]
    DeviceNotFound(String),
    #[error("Unsupported stream config: {0}")]
    UnsupportedConfig(String),
}

/// Configuration parameters for local audio transport
#[derive(Debug, Clone)]
pub struct LocalAudioTransportParams {
    /// Base transport parameters
    pub base: TransportParams,
    /// Name of the input device (None = use default)
    pub input_device_name: Option<String>,
    /// Name of the output device (None = use default)
    pub output_device_name: Option<String>,
}

impl Default for LocalAudioTransportParams {
    fn default() -> Self {
        Self {
            base: TransportParams::default()
                .with_audio_input()
                .with_audio_output(),
            input_device_name: None,
            output_device_name: None,
        }
    }
}

impl LocalAudioTransportParams {
    /// Create new params with audio input and output enabled
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the input device by name
    pub fn with_input_device(mut self, name: String) -> Self {
        self.input_device_name = Some(name);
        self
    }

    /// Set the output device by name
    pub fn with_output_device(mut self, name: String) -> Self {
        self.output_device_name = Some(name);
        self
    }

    /// Set the audio input sample rate
    pub fn with_input_sample_rate(mut self, sample_rate: u32) -> Self {
        self.base = self.base.with_audio_in_sample_rate(sample_rate);
        self
    }

    /// Set the audio output sample rate
    pub fn with_output_sample_rate(mut self, sample_rate: u32) -> Self {
        self.base = self.base.with_audio_out_sample_rate(sample_rate);
        self
    }
}

/// Complete local audio transport with input and output capabilities.
///
/// Provides a unified interface for local audio I/O using CPAL, supporting
/// both audio capture and playback through the system's audio devices.
///
/// # Example
///
/// ```ignore
/// use voxlab::transport::local::{LocalAudioTransport, LocalAudioTransportParams};
///
/// let params = LocalAudioTransportParams::new()
///     .with_input_sample_rate(16000)
///     .with_output_sample_rate(16000);
///
/// let transport = LocalAudioTransport::new(params).await?;
///
/// // Use in pipeline
/// let input = transport.input();
/// let output = transport.output();
/// ```
pub struct LocalAudioTransport {
    #[allow(dead_code)]
    params: LocalAudioTransportParams,
    input: Option<PipelineActorRef>,
    output: Option<PipelineActorRef>,
}

impl LocalAudioTransport {
    /// Create a new local audio transport (actors not yet spawned)
    pub fn new(params: LocalAudioTransportParams) -> Self {
        Self {
            params,
            input: None,
            output: None,
        }
    }

    /// Set the input actor reference
    pub fn set_input(&mut self, input: PipelineActorRef) {
        self.input = Some(input);
    }

    /// Set the output actor reference
    pub fn set_output(&mut self, output: PipelineActorRef) {
        self.output = Some(output);
    }
}

impl BaseTransport for LocalAudioTransport {
    fn name(&self) -> &str {
        "LocalAudioTransport"
    }

    fn input(&self) -> PipelineActorRef {
        self.input.clone().expect("Input transport not initialized")
    }

    fn output(&self) -> PipelineActorRef {
        self.output.clone().expect("Output transport not initialized")
    }
}
