//! Base input transport implementation
//!
//! This module provides the generic InputTransport actor which handles audio and video
//! input processing, pushing media frames into the pipeline. Concrete transport
//! implementations provide a backend that handles the actual device I/O.

use std::convert::Infallible;
use std::marker::PhantomData;

use async_trait::async_trait;
use ractor::{Actor, ActorProcessingErr, ActorRef};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::frame::{Frame, FrameDirection, FrameHeader};
use crate::processor::{
    PipelineActorRef, ProcessorBehavior, ProcessorMsg, ProcessorSetup, ProcessorStatus,
};
use crate::transport::TransportParams;

// ============================================================================
// InputTransportBackend trait - implemented by concrete transports
// ============================================================================

/// Audio data with metadata
#[derive(Debug, Clone)]
pub struct AudioData {
    /// Raw PCM audio bytes
    pub data: Vec<u8>,
    /// Actual sample rate from the device
    pub sample_rate: u32,
    /// Number of channels
    pub channels: u32,
}

/// Sender type for audio data from backend to actor
pub type AudioDataSender = mpsc::UnboundedSender<AudioData>;

/// Backend trait for input transport implementations
///
/// Implement this trait to create a custom input transport (e.g., CPAL, WebRTC, file).
/// The backend is responsible for capturing audio from the device and sending it
/// via the provided channel.
#[async_trait]
pub trait InputTransportBackend: Send + Sync + 'static {
    /// Backend-specific error type
    type Error: std::error::Error + Send + Sync + 'static;

    /// Start audio capture, sending data via the provided sender
    ///
    /// The backend should begin capturing audio and send raw PCM bytes
    /// through the `audio_sender` channel.
    async fn start_capture(
        &mut self,
        sample_rate: u32,
        channels: u32,
        audio_sender: AudioDataSender,
    ) -> Result<(), Self::Error>;

    /// Stop audio capture
    async fn stop_capture(&mut self) -> Result<(), Self::Error>;

    /// Release all resources
    async fn shutdown(&mut self);

    /// Get backend name for logging
    fn name(&self) -> &str;
}

// ============================================================================
// NullInputBackend - default backend that does nothing
// ============================================================================

/// Null backend for base transport (no actual I/O)
///
/// Use this when you don't need actual device input, such as for testing
/// or when frames will be pushed manually.
pub struct NullInputBackend;

impl NullInputBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NullInputBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl InputTransportBackend for NullInputBackend {
    type Error = Infallible;

    async fn start_capture(
        &mut self,
        _sample_rate: u32,
        _channels: u32,
        _audio_sender: AudioDataSender,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn stop_capture(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn shutdown(&mut self) {}

    fn name(&self) -> &str {
        "NullInput"
    }
}

// ============================================================================
// BaseInputTransport - the transport behavior
// ============================================================================

/// Base input transport behavior
///
/// Handles incoming media from external sources and pushes frames downstream.
pub struct BaseInputTransport {
    name: String,
    params: TransportParams,
}

impl BaseInputTransport {
    pub fn new(name: String, params: TransportParams) -> Self {
        Self { name, params }
    }

    pub fn params(&self) -> &TransportParams {
        &self.params
    }

    pub fn params_mut(&mut self) -> &mut TransportParams {
        &mut self.params
    }
}

#[async_trait]
impl ProcessorBehavior for BaseInputTransport {
    fn name(&self) -> &str {
        &self.name
    }

    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        // Input transport passes through most frames
        Some(frame)
    }

    async fn setup(&mut self, setup: &ProcessorSetup) {
        debug!("[{}] Setting up with {:?}", self.name, setup);
    }

    async fn cleanup(&mut self) {
        debug!("[{}] Cleaning up", self.name);
    }
}

// ============================================================================
// InputTransportActor - generic actor over backend
// ============================================================================

/// Generic input transport actor that works with any backend
pub struct InputTransportActor<B: InputTransportBackend> {
    _phantom: PhantomData<B>,
}

impl<B: InputTransportBackend> InputTransportActor<B> {
    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl<B: InputTransportBackend> Default for InputTransportActor<B> {
    fn default() -> Self {
        Self::new()
    }
}

/// State for the generic input transport actor
pub struct InputTransportState<B: InputTransportBackend> {
    behavior: BaseInputTransport,
    /// The backend implementation
    backend: B,
    /// Link to next processor for downstream frame pushing
    next: Option<PipelineActorRef>,
    /// Audio sample rate
    sample_rate: u32,
    /// Number of audio channels
    num_channels: u32,
    /// Whether the transport is running
    is_running: bool,
    /// Whether the transport is paused
    is_paused: bool,
    /// Whether the transport is being cancelled
    #[allow(dead_code)]
    cancelling: bool,
    /// Whether the bot is currently speaking
    bot_speaking: bool,
    /// Whether the user is currently speaking
    #[allow(dead_code)]
    user_speaking: bool,
    /// Frame counter for status
    frames_processed: u64,
    /// Task for receiving audio from backend
    audio_task: Option<tokio::task::JoinHandle<()>>,
}

impl<B: InputTransportBackend> InputTransportState<B> {
    pub fn new(behavior: BaseInputTransport, backend: B) -> Self {
        Self {
            behavior,
            backend,
            next: None,
            sample_rate: 0,
            num_channels: 1,
            is_running: false,
            is_paused: false,
            cancelling: false,
            bot_speaking: false,
            user_speaking: false,
            frames_processed: 0,
            audio_task: None,
        }
    }

    /// Get reference to behavior
    pub fn behavior(&self) -> &BaseInputTransport {
        &self.behavior
    }

    /// Get mutable reference to behavior
    pub fn behavior_mut(&mut self) -> &mut BaseInputTransport {
        &mut self.behavior
    }

    /// Get reference to backend
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Get mutable reference to backend
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    // ========================================================================
    // Frame pushing methods
    // ========================================================================

    /// Push a frame in the specified direction
    fn push_frame(&self, frame: Frame, direction: FrameDirection) {
        match direction {
            FrameDirection::Downstream => {
                if let Some(ref next) = self.next {
                    if let Err(e) = next.cast(ProcessorMsg::ProcessFrame { frame, direction }) {
                        warn!(
                            "[{}] Failed to push frame downstream: {}",
                            self.behavior.name, e
                        );
                    }
                }
            }
            FrameDirection::Upstream => {
                // Input transport is at the start of the pipeline, no upstream
                debug!(
                    "[{}] Cannot push upstream from input transport",
                    self.behavior.name
                );
            }
        }
    }

    // ========================================================================
    // Transport lifecycle methods
    // ========================================================================

    /// Start the transport - called on StartFrame
    async fn start(&mut self, frame: &Frame) {
        if let Frame::Start {
            audio_in_sample_rate,
            ..
        } = frame
        {
            self.sample_rate = self
                .behavior
                .params
                .audio_in_sample_rate
                .unwrap_or(*audio_in_sample_rate);
            self.num_channels = self.behavior.params.audio_in_channels;

            self.is_running = true;
            self.is_paused = false;
            self.user_speaking = false;

            // Call behavior on_start
            self.behavior.on_start(self.sample_rate).await;

            debug!(
                "[{}] Started with sample rate: {}",
                self.behavior.name, self.sample_rate
            );

            // Start audio streaming if configured
            if self.behavior.params.audio_in_stream_on_start {
                self.start_audio_streaming().await;
            }
        }
    }

    /// Stop the transport - called on EndFrame
    async fn stop(&mut self, _frame: &Frame) {
        self.stop_audio_streaming().await;
        self.is_running = false;
        self.behavior.on_stop().await;
        debug!("[{}] Stopped", self.behavior.name);
    }

    /// Pause the transport - called on StopFrame
    async fn pause(&mut self, _frame: &Frame) {
        self.is_paused = true;
        debug!("[{}] Paused", self.behavior.name);
    }

    /// Cancel the transport - called on CancelFrame
    async fn cancel(&mut self, _frame: &Frame) {
        self.stop_audio_streaming().await;
        self.cancelling = true;
        self.is_running = false;
        debug!("[{}] Cancelled", self.behavior.name);
    }

    /// Start audio input streaming via backend
    async fn start_audio_streaming(&mut self) {
        info!(
            "[{}] Starting audio streaming via {} (sample_rate={}, channels={}, passthrough={}, enabled={})",
            self.behavior.name,
            self.backend.name(),
            self.sample_rate,
            self.num_channels,
            self.behavior.params.audio_in_passthrough,
            self.behavior.params.audio_in_enabled
        );

        // Create channel for audio data from backend
        let (audio_tx, mut audio_rx) = mpsc::unbounded_channel::<AudioData>();

        // Start backend capture
        if let Err(e) = self
            .backend
            .start_capture(self.sample_rate, self.num_channels, audio_tx)
            .await
        {
            error!(
                "[{}] Failed to start capture via {}: {}",
                self.behavior.name,
                self.backend.name(),
                e
            );
            return;
        }

        info!("[{}] Backend capture started successfully", self.behavior.name);

        // Spawn task to receive audio and push downstream
        let next = self.next.clone();
        let passthrough = self.behavior.params.audio_in_passthrough;
        let enabled = self.behavior.params.audio_in_enabled;
        let name = self.behavior.name.clone();

        let has_next = next.is_some();
        info!("[{}] Audio task spawning, has_next={}", name, has_next);

        let task = tokio::spawn(async move {
            let mut frame_count = 0u64;
            info!(
                "[{}] Audio task started: enabled={}, passthrough={}, has_next={}",
                name, enabled, passthrough, next.is_some()
            );
            while let Some(audio_data) = audio_rx.recv().await {
                frame_count += 1;
                if frame_count == 1 {
                    info!("[{}] First audio chunk received ({} bytes, {}Hz)", 
                          name, audio_data.data.len(), audio_data.sample_rate);
                }
                if frame_count % 100 == 0 {
                    info!("[{}] Audio chunks received: {}", name, frame_count);
                }
                if !enabled {
                    debug!("[{}] Skipping audio chunk (not enabled)", name);
                    continue;
                }
                if let Some(ref next) = next {
                    let frame = Frame::InputAudioRaw {
                        header: FrameHeader::default(),
                        audio: audio_data.data,
                        sample_rate: audio_data.sample_rate,
                        num_channels: audio_data.channels,
                    };
                    if passthrough {
                        if frame_count == 1 {
                            info!("[{}] Sending first audio frame downstream", name);
                        }
                        let _ = next.cast(ProcessorMsg::ProcessFrame {
                            frame,
                            direction: FrameDirection::Downstream,
                        });
                    } else {
                        debug!("[{}] Skipping audio chunk (passthrough disabled)", name);
                    }
                } else {
                    warn!("[{}] No next processor to send audio to!", name);
                }
            }
            info!("[{}] Audio task ended after {} chunks", name, frame_count);
        });

        self.audio_task = Some(task);
    }

    /// Stop audio streaming
    async fn stop_audio_streaming(&mut self) {
        if let Err(e) = self.backend.stop_capture().await {
            error!(
                "[{}] Failed to stop capture via {}: {}",
                self.behavior.name,
                self.backend.name(),
                e
            );
        }
        if let Some(task) = self.audio_task.take() {
            task.abort();
        }
    }

    // ========================================================================
    // Audio/Video frame injection methods (for manual frame pushing)
    // ========================================================================

    /// Push an audio frame into the pipeline
    /// Called by concrete transport implementations when audio arrives
    pub fn push_audio_frame(&self, audio: Vec<u8>, sample_rate: u32, num_channels: u32) {
        if !self.behavior.params.audio_in_enabled || self.is_paused {
            return;
        }

        let frame = Frame::InputAudioRaw {
            header: FrameHeader::default(),
            audio,
            sample_rate,
            num_channels,
        };

        if self.behavior.params.audio_in_passthrough {
            self.push_frame(frame, FrameDirection::Downstream);
        }
    }

    /// Push a video frame into the pipeline
    /// Called by concrete transport implementations when video arrives
    pub fn push_video_frame(&self, image: Vec<u8>, size: (u32, u32), format: String) {
        if !self.behavior.params.video_in_enabled || self.is_paused {
            return;
        }

        let frame = Frame::InputImageRaw {
            header: FrameHeader::default(),
            image,
            size,
            format,
        };

        self.push_frame(frame, FrameDirection::Downstream);
    }

    // ========================================================================
    // Bot speaking state handling
    // ========================================================================

    fn handle_bot_started_speaking(&mut self) {
        self.bot_speaking = true;
        debug!("[{}] Bot started speaking", self.behavior.name);
    }

    fn handle_bot_stopped_speaking(&mut self) {
        self.bot_speaking = false;
        debug!("[{}] Bot stopped speaking", self.behavior.name);
    }
}

// ============================================================================
// Actor implementation - generic over backend
// ============================================================================

#[async_trait]
impl<B: InputTransportBackend> Actor for InputTransportActor<B> {
    type Msg = ProcessorMsg;
    type State = InputTransportState<B>;
    type Arguments = InputTransportState<B>;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        debug!(
            "[{}] Input transport actor started with {} backend",
            args.behavior.name,
            args.backend.name()
        );
        Ok(args)
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        state.backend.shutdown().await;
        debug!("[{}] Input transport actor stopped", state.behavior.name);
        Ok(())
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            ProcessorMsg::ProcessFrame { frame, direction } => {
                state.frames_processed += 1;

                info!(
                    "[{}] Received frame: {} (direction: {:?})",
                    state.behavior.name,
                    frame.name(),
                    direction
                );

                // This mirrors Pipecat's process_frame method
                match &frame {
                    // StartFrame: push first, then start
                    Frame::Start { .. } => {
                        info!("[{}] Processing StartFrame", state.behavior.name);
                        state.push_frame(frame.clone(), direction);
                        state.start(&frame).await;
                    }

                    // EndFrame: stop first, then push
                    Frame::End { .. } => {
                        state.stop(&frame).await;
                        state.push_frame(frame, direction);
                    }

                    // StopFrame: push, then pause
                    Frame::Stop { .. } => {
                        state.push_frame(frame.clone(), direction);
                        state.pause(&frame).await;
                    }

                    // CancelFrame: cancel, then push
                    Frame::Cancel { .. } => {
                        state.cancel(&frame).await;
                        state.push_frame(frame, direction);
                    }

                    // Bot speaking state frames
                    Frame::BotStartedSpeaking { .. } => {
                        state.handle_bot_started_speaking();
                        state.push_frame(frame, direction);
                    }

                    Frame::BotStoppedSpeaking { .. } => {
                        state.handle_bot_stopped_speaking();
                        state.push_frame(frame, direction);
                    }

                    // System frames: just push through
                    _ if frame.is_system() => {
                        state.push_frame(frame, direction);
                    }

                    // Downstream direction: process and push
                    _ if direction == FrameDirection::Downstream => {
                        if let Some(output_frame) = state.behavior.process(frame).await {
                            state.push_frame(output_frame, direction);
                        }
                    }

                    // Upstream direction: input transport handles upstream frames
                    // (these come back from downstream processors)
                    _ => {
                        // Process the frame with behavior
                        let _ = state.behavior.process(frame).await;
                        // Input transport is the source, so upstream frames end here
                    }
                }
            }

            ProcessorMsg::LinkNext { next } => {
                debug!("[{}] Linked to next processor", state.behavior.name);
                state.next = Some(next);
            }

            ProcessorMsg::LinkNextSync { next, reply } => {
                debug!("[{}] Linked to next processor (sync)", state.behavior.name);
                state.next = Some(next);
                let _ = reply.send(());
            }

            ProcessorMsg::LinkPrevious { .. } => {
                // Input transport doesn't use previous links (it's the start of pipeline)
            }

            ProcessorMsg::Setup { setup } => {
                debug!("[{}] Setting up input transport", state.behavior.name);
                state.behavior.setup(&setup).await;
            }

            ProcessorMsg::Cleanup => {
                debug!("[{}] Cleaning up input transport", state.behavior.name);
                state.stop_audio_streaming().await;
                state.backend.shutdown().await;
                state.behavior.cleanup().await;
            }

            ProcessorMsg::GetStatus { reply } => {
                let status = ProcessorStatus {
                    name: state.behavior.name.clone(),
                    frames_processed: state.frames_processed,
                    is_running: state.is_running,
                };
                let _ = reply.send(status);
            }
        }
        Ok(())
    }
}

// ============================================================================
// Type aliases for backward compatibility
// ============================================================================

/// Base input transport actor (uses null backend)
pub type BaseInputTransportActor = InputTransportActor<NullInputBackend>;

/// Base input transport state (uses null backend)
pub type BaseInputTransportState = InputTransportState<NullInputBackend>;

impl BaseInputTransportState {
    /// Create a new base input transport state with null backend
    pub fn new_base(behavior: BaseInputTransport) -> Self {
        Self::new(behavior, NullInputBackend::new())
    }
}
