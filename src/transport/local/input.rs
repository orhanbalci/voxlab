//! Local audio input transport implementation
//!
//! Provides CPAL backend for audio capture that integrates with the generic
//! InputTransportActor from the base transport module.

use async_trait::async_trait;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::StreamConfig;
use tokio::sync::mpsc;
use tracing::{debug, error};

use crate::transport::input::{
    AudioDataSender, BaseInputTransport, InputTransportActor, InputTransportBackend,
    InputTransportState,
};
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
        audio_tx: mpsc::UnboundedSender<Vec<u8>>,
    },
    Stop,
}

/// Starts the audio input stream in a blocking thread
fn run_input_stream(device_name: Option<String>, cmd_rx: std::sync::mpsc::Receiver<StreamCommand>) {
    let host = cpal::default_host();

    // Get input device
    let device = if let Some(ref name) = device_name {
        match host.input_devices() {
            Ok(mut devices) => devices.find(|d| d.name().map(|n| n == *name).unwrap_or(false)),
            Err(e) => {
                error!("Failed to enumerate devices: {}", e);
                None
            }
        }
    } else {
        host.default_input_device()
    };

    let device = match device {
        Some(d) => d,
        None => {
            error!("No input device found");
            return;
        }
    };

    debug!("Using input device: {:?}", device.name());

    let mut _current_stream: Option<cpal::Stream> = None;

    loop {
        match cmd_rx.recv() {
            Ok(StreamCommand::Start {
                sample_rate,
                channels,
                audio_tx,
            }) => {
                // Stop existing stream
                _current_stream = None;

                let config = StreamConfig {
                    channels: channels as u16,
                    sample_rate: cpal::SampleRate(sample_rate),
                    buffer_size: cpal::BufferSize::Default,
                };

                match device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let bytes: Vec<u8> = data.iter().flat_map(|&s| s.to_le_bytes()).collect();
                        let _ = audio_tx.send(bytes);
                    },
                    |err| error!("Input stream error: {}", err),
                    None,
                ) {
                    Ok(stream) => {
                        if let Err(e) = stream.play() {
                            error!("Failed to play stream: {}", e);
                        } else {
                            debug!("Started audio input stream");
                            _current_stream = Some(stream);
                        }
                    }
                    Err(e) => error!("Failed to build input stream: {}", e),
                }
            }
            Ok(StreamCommand::Stop) => {
                _current_stream = None;
                debug!("Stopped audio input stream");
            }
            Err(_) => {
                // Channel closed, exit thread
                break;
            }
        }
    }
}

// ============================================================================
// CpalInputBackend - implements InputTransportBackend
// ============================================================================

/// CPAL-based input backend for local audio capture
///
/// This backend manages a background thread that handles CPAL stream operations,
/// since CPAL streams are not Send-safe. Commands are sent via a sync channel.
pub struct CpalInputBackend {
    /// Device name to use (None = default device)
    device_name: Option<String>,
    /// Command sender to the stream thread
    stream_cmd_tx: Option<std::sync::mpsc::Sender<StreamCommand>>,
}

impl CpalInputBackend {
    /// Create a new CPAL input backend
    ///
    /// # Arguments
    ///
    /// * `device_name` - Optional device name (None for default input device)
    pub fn new(device_name: Option<String>) -> Self {
        // Spawn the stream management thread
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();

        let device_name_clone = device_name.clone();
        std::thread::spawn(move || {
            run_input_stream(device_name_clone, cmd_rx);
        });

        Self {
            device_name,
            stream_cmd_tx: Some(cmd_tx),
        }
    }

    /// Get the device name
    pub fn device_name(&self) -> Option<&str> {
        self.device_name.as_deref()
    }
}

#[async_trait]
impl InputTransportBackend for CpalInputBackend {
    type Error = LocalAudioError;

    async fn start_capture(
        &mut self,
        sample_rate: u32,
        channels: u32,
        audio_sender: AudioDataSender,
    ) -> Result<(), Self::Error> {
        if let Some(ref cmd_tx) = self.stream_cmd_tx {
            cmd_tx
                .send(StreamCommand::Start {
                    sample_rate,
                    channels,
                    audio_tx: audio_sender,
                })
                .map_err(|_| {
                    LocalAudioError::StreamBuildError("Stream thread closed".to_string())
                })?;
        }

        debug!(
            "CpalInputBackend: Started capture at {} Hz, {} channels",
            sample_rate, channels
        );
        Ok(())
    }

    async fn stop_capture(&mut self) -> Result<(), Self::Error> {
        if let Some(ref cmd_tx) = self.stream_cmd_tx {
            let _ = cmd_tx.send(StreamCommand::Stop);
        }
        debug!("CpalInputBackend: Stopped capture");
        Ok(())
    }

    async fn shutdown(&mut self) {
        // Drop the command sender to signal the stream thread to exit
        self.stream_cmd_tx = None;
        debug!("CpalInputBackend: Shutdown complete");
    }

    fn name(&self) -> &str {
        "CpalInput"
    }
}

// ============================================================================
// LocalAudioInputTransport - behavior wrapper for CPAL
// ============================================================================

/// Local audio input transport behavior
///
/// This is a thin wrapper that provides access to CPAL-specific parameters.
pub struct LocalAudioInputTransport {
    params: LocalAudioTransportParams,
}

impl LocalAudioInputTransport {
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

    /// Create the behavior for the generic transport actor
    pub fn into_base_behavior(self, name: String) -> BaseInputTransport {
        BaseInputTransport::new(name, self.params.base)
    }

    /// Create the CPAL backend
    pub fn create_backend(&self) -> CpalInputBackend {
        CpalInputBackend::new(self.params.input_device_name.clone())
    }
}

// ============================================================================
// Type aliases for convenience
// ============================================================================

/// Local audio input transport actor (uses CPAL backend)
pub type LocalAudioInputTransportActor = InputTransportActor<CpalInputBackend>;

/// Local audio input transport state (uses CPAL backend)
pub type LocalAudioInputTransportState = InputTransportState<CpalInputBackend>;

impl LocalAudioInputTransportState {
    /// Create a new local audio input transport state
    ///
    /// # Arguments
    ///
    /// * `name` - Name for the transport
    /// * `params` - Local audio transport parameters
    pub fn new_local(
        name: String,
        params: LocalAudioTransportParams,
    ) -> Result<Self, LocalAudioError> {
        let backend = CpalInputBackend::new(params.input_device_name.clone());
        let behavior = BaseInputTransport::new(name, params.base);
        Ok(Self::new(behavior, backend))
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Helper to list available input devices
pub fn list_input_devices() -> Result<Vec<String>, LocalAudioError> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|e| LocalAudioError::StreamBuildError(e.to_string()))?;

    let names: Vec<String> = devices.filter_map(|d| d.name().ok()).collect();

    Ok(names)
}
