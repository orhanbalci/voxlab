//! Local audio output transport implementation
//!
//! Provides CPAL backend for audio playback that integrates with the generic
//! OutputTransportActor from the base transport module.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::StreamConfig;
use tracing::{debug, error};

use crate::transport::media_sender::AudioWriter;
use crate::transport::output::{OutputTransportActor, OutputTransportBackend, OutputTransportState, BaseOutputTransport};
use crate::transport::local::{LocalAudioError, LocalAudioTransportParams};
use crate::transport::TransportParams;

// ============================================================================
// Stream management - runs in a separate thread
// ============================================================================

/// Message to control the audio stream thread
enum StreamCommand {
    Start {
        sample_rate: u32,
        channels: u32,
        buffer: Arc<Mutex<VecDeque<u8>>>,
    },
    Stop,
}

/// Runs the audio output stream in a blocking thread
fn run_output_stream(
    device_name: Option<String>,
    cmd_rx: std::sync::mpsc::Receiver<StreamCommand>,
) {
    let host = cpal::default_host();

    // Get output device
    let device = if let Some(ref name) = device_name {
        match host.output_devices() {
            Ok(mut devices) => devices.find(|d| d.name().map(|n| n == *name).unwrap_or(false)),
            Err(e) => {
                error!("Failed to enumerate devices: {}", e);
                None
            }
        }
    } else {
        host.default_output_device()
    };

    let device = match device {
        Some(d) => d,
        None => {
            error!("No output device found");
            return;
        }
    };

    debug!("Using output device: {:?}", device.name());

    let mut _current_stream: Option<cpal::Stream> = None;

    loop {
        match cmd_rx.recv() {
            Ok(StreamCommand::Start {
                sample_rate,
                channels,
                buffer,
            }) => {
                // Stop existing stream
                _current_stream = None;

                let config = StreamConfig {
                    channels: channels as u16,
                    sample_rate: cpal::SampleRate(sample_rate),
                    buffer_size: cpal::BufferSize::Default,
                };

                match device.build_output_stream(
                    &config,
                    move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        let mut buf = buffer.lock().unwrap();
                        for sample in data.iter_mut() {
                            if buf.len() >= 2 {
                                let lo = buf.pop_front().unwrap();
                                let hi = buf.pop_front().unwrap();
                                *sample = i16::from_le_bytes([lo, hi]);
                            } else {
                                *sample = 0; // Silence if buffer empty
                            }
                        }
                    },
                    |err| error!("Output stream error: {}", err),
                    None,
                ) {
                    Ok(stream) => {
                        if let Err(e) = stream.play() {
                            error!("Failed to play stream: {}", e);
                        } else {
                            debug!("Started audio output stream");
                            _current_stream = Some(stream);
                        }
                    }
                    Err(e) => error!("Failed to build output stream: {}", e),
                }
            }
            Ok(StreamCommand::Stop) => {
                _current_stream = None;
                debug!("Stopped audio output stream");
            }
            Err(_) => {
                // Channel closed, exit thread
                break;
            }
        }
    }
}

// ============================================================================
// CpalOutputBackend - implements OutputTransportBackend + AudioWriter
// ============================================================================

/// CPAL-based output backend for local audio playback
///
/// This backend manages a background thread that handles CPAL stream operations,
/// since CPAL streams are not Send-safe. Commands are sent via a sync channel.
/// Audio data is written to a shared ring buffer that the CPAL callback reads from.
pub struct CpalOutputBackend {
    /// Device name to use (None = default device)
    device_name: Option<String>,
    /// Command sender to the stream thread
    stream_cmd_tx: Option<std::sync::mpsc::Sender<StreamCommand>>,
    /// Playback buffer shared with CPAL callback
    playback_buffer: Arc<Mutex<VecDeque<u8>>>,
}

impl CpalOutputBackend {
    /// Create a new CPAL output backend
    ///
    /// # Arguments
    ///
    /// * `device_name` - Optional device name (None for default output device)
    pub fn new(device_name: Option<String>) -> Self {
        // Spawn the stream management thread
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();

        let device_name_clone = device_name.clone();
        std::thread::spawn(move || {
            run_output_stream(device_name_clone, cmd_rx);
        });

        Self {
            device_name,
            stream_cmd_tx: Some(cmd_tx),
            playback_buffer: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Get the device name
    pub fn device_name(&self) -> Option<&str> {
        self.device_name.as_deref()
    }
}

#[async_trait]
impl OutputTransportBackend for CpalOutputBackend {
    type Error = LocalAudioError;

    async fn start_playback(&mut self, sample_rate: u32, channels: u32) -> Result<(), Self::Error> {
        // Send start command to stream thread with shared buffer
        if let Some(ref cmd_tx) = self.stream_cmd_tx {
            cmd_tx
                .send(StreamCommand::Start {
                    sample_rate,
                    channels,
                    buffer: self.playback_buffer.clone(),
                })
                .map_err(|_| {
                    LocalAudioError::StreamBuildError("Stream thread closed".to_string())
                })?;
        }

        debug!(
            "CpalOutputBackend: Started playback at {} Hz, {} channels",
            sample_rate, channels
        );
        Ok(())
    }

    async fn stop_playback(&mut self) -> Result<(), Self::Error> {
        // Send stop command to stream thread
        if let Some(ref cmd_tx) = self.stream_cmd_tx {
            let _ = cmd_tx.send(StreamCommand::Stop);
        }
        // Clear the buffer
        self.clear_buffer();
        debug!("CpalOutputBackend: Stopped playback");
        Ok(())
    }

    fn write_audio(&self, data: &[u8]) {
        if let Ok(mut buf) = self.playback_buffer.lock() {
            buf.extend(data);
        }
    }

    fn clear_buffer(&self) {
        if let Ok(mut buf) = self.playback_buffer.lock() {
            buf.clear();
        }
    }

    async fn shutdown(&mut self) {
        // Drop the command sender to signal the stream thread to exit
        self.stream_cmd_tx = None;
        debug!("CpalOutputBackend: Shutdown complete");
    }

    fn name(&self) -> &str {
        "CpalOutput"
    }
}

/// Implement AudioWriter for CpalOutputBackend so it can be used with MediaSender
impl AudioWriter for CpalOutputBackend {
    fn write(&self, data: &[u8]) {
        self.write_audio(data);
    }

    fn clear(&self) {
        self.clear_buffer();
    }
}

// ============================================================================
// LocalAudioOutputTransport - behavior wrapper for CPAL
// ============================================================================

/// Local audio output transport behavior
///
/// This is a thin wrapper that provides access to CPAL-specific parameters.
pub struct LocalAudioOutputTransport {
    params: LocalAudioTransportParams,
}

impl LocalAudioOutputTransport {
    pub fn new(params: LocalAudioTransportParams) -> Self {
        Self { params }
    }

    pub fn params(&self) -> &LocalAudioTransportParams {
        &self.params
    }

    /// Get base transport params
    pub fn base_params(&self) -> &TransportParams {
        &self.params.base
    }
}

// ============================================================================
// Type aliases for convenience
// ============================================================================

/// Local audio output transport actor (uses CPAL backend)
pub type LocalAudioOutputTransportActor = OutputTransportActor<CpalOutputBackend>;

/// Local audio output transport state (uses CPAL backend)
pub type LocalAudioOutputTransportState = OutputTransportState<CpalOutputBackend>;

impl LocalAudioOutputTransportState {
    /// Create a new local audio output transport state
    ///
    /// # Arguments
    ///
    /// * `name` - Name for the transport
    /// * `params` - Local audio transport parameters
    pub fn new_local(name: String, params: LocalAudioTransportParams) -> Result<Self, LocalAudioError> {
        let backend = CpalOutputBackend::new(params.output_device_name.clone());
        let behavior = BaseOutputTransport::new(name, params.base);
        Ok(Self::new(behavior, backend))
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Helper to list available output devices
pub fn list_output_devices() -> Result<Vec<String>, LocalAudioError> {
    let host = cpal::default_host();
    let devices = host
        .output_devices()
        .map_err(|e| LocalAudioError::StreamBuildError(e.to_string()))?;

    let names: Vec<String> = devices.filter_map(|d| d.name().ok()).collect();

    Ok(names)
}
