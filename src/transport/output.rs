//! Base output transport implementation
//!
//! This module provides the generic OutputTransport actor which handles audio and video
//! output processing, including frame buffering, mixing, timing, and media streaming.
//! Concrete transport implementations provide a backend that handles the actual device I/O.

use std::collections::HashMap;
use std::convert::Infallible;
use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use ractor::{Actor, ActorProcessingErr, ActorRef};
use tracing::{debug, warn};

use crate::frame::{Frame, FrameDirection, FrameHeader};
use crate::processor::{
    PipelineActorRef, ProcessorBehavior, ProcessorMsg, ProcessorSetup, ProcessorStatus,
};
use crate::transport::media_sender::{AudioWriter, MediaSender};
use crate::transport::TransportParams;

// ============================================================================
// OutputTransportBackend trait - implemented by concrete transports
// ============================================================================

/// Backend trait for output transport implementations
///
/// Implement this trait to create a custom output transport (e.g., CPAL, WebRTC, file).
/// The backend is responsible for playing audio to the device.
#[async_trait]
pub trait OutputTransportBackend: Send + Sync + 'static {
    /// Backend-specific error type
    type Error: std::error::Error + Send + Sync + 'static;

    /// Start audio playback
    async fn start_playback(&mut self, sample_rate: u32, channels: u32) -> Result<(), Self::Error>;

    /// Stop audio playback
    async fn stop_playback(&mut self) -> Result<(), Self::Error>;

    /// Write audio data to the output device
    ///
    /// Called by MediaSender when audio chunks are ready.
    fn write_audio(&self, data: &[u8]);

    /// Clear any buffered audio data (on interruption)
    fn clear_buffer(&self);

    /// Release all resources
    async fn shutdown(&mut self);

    /// Get backend name for logging
    fn name(&self) -> &str;
}

// ============================================================================
// NullOutputBackend - default backend that does nothing
// ============================================================================

/// Null backend for base transport (no actual I/O)
///
/// Use this when you don't need actual device output, such as for testing
/// or when frames will be consumed elsewhere.
pub struct NullOutputBackend;

impl NullOutputBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NullOutputBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OutputTransportBackend for NullOutputBackend {
    type Error = Infallible;

    async fn start_playback(
        &mut self,
        _sample_rate: u32,
        _channels: u32,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn stop_playback(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn write_audio(&self, _data: &[u8]) {
        // Discard audio
    }

    fn clear_buffer(&self) {
        // Nothing to clear
    }

    async fn shutdown(&mut self) {}

    fn name(&self) -> &str {
        "NullOutput"
    }
}

/// AudioWriter implementation for NullOutputBackend
impl AudioWriter for NullOutputBackend {
    fn write(&self, data: &[u8]) {
        self.write_audio(data);
    }

    fn clear(&self) {
        self.clear_buffer();
    }
}

// ============================================================================
// BaseOutputTransport - the transport behavior
// ============================================================================

/// Base output transport behavior
///
/// Handles outgoing media and sends frames to external sinks.
pub struct BaseOutputTransport {
    name: String,
    params: TransportParams,
}

impl BaseOutputTransport {
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
impl ProcessorBehavior for BaseOutputTransport {
    fn name(&self) -> &str {
        &self.name
    }

    async fn process(&mut self, frame: Frame) -> Option<Frame> {
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
// OutputTransportActor - generic actor over backend
// ============================================================================

/// Generic output transport actor that works with any backend
pub struct OutputTransportActor<B: OutputTransportBackend + AudioWriter> {
    _phantom: PhantomData<B>,
}

impl<B: OutputTransportBackend + AudioWriter> OutputTransportActor<B> {
    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl<B: OutputTransportBackend + AudioWriter> Default for OutputTransportActor<B> {
    fn default() -> Self {
        Self::new()
    }
}

/// State for the generic output transport actor
pub struct OutputTransportState<B: OutputTransportBackend + AudioWriter> {
    behavior: BaseOutputTransport,
    /// The backend implementation
    backend: Arc<B>,
    /// Link to previous processor for upstream frame pushing
    previous: Option<PipelineActorRef>,
    /// Audio sample rate
    sample_rate: u32,
    /// Audio chunk size in bytes
    #[allow(dead_code)]
    audio_chunk_size: usize,
    /// Whether the transport is running
    is_running: bool,
    /// Whether the transport is being cancelled
    #[allow(dead_code)]
    cancelling: bool,
    /// Media senders per destination
    media_senders: HashMap<Option<String>, MediaSender>,
    /// Frame counter for status
    frames_processed: u64,
}

impl<B: OutputTransportBackend + AudioWriter> OutputTransportState<B> {
    pub fn new(behavior: BaseOutputTransport, backend: B) -> Self {
        Self {
            behavior,
            backend: Arc::new(backend),
            previous: None,
            sample_rate: 0,
            audio_chunk_size: 0,
            is_running: false,
            cancelling: false,
            media_senders: HashMap::new(),
            frames_processed: 0,
        }
    }

    /// Get reference to behavior
    pub fn behavior(&self) -> &BaseOutputTransport {
        &self.behavior
    }

    /// Get mutable reference to behavior
    pub fn behavior_mut(&mut self) -> &mut BaseOutputTransport {
        &mut self.behavior
    }

    /// Get reference to backend
    pub fn backend(&self) -> &B {
        &self.backend
    }

    // ========================================================================
    // Frame pushing methods
    // ========================================================================

    /// Push a frame in the specified direction
    fn push_frame(&self, frame: Frame, direction: FrameDirection) {
        match direction {
            FrameDirection::Upstream => {
                if let Some(ref prev) = self.previous {
                    if let Err(e) = prev.cast(ProcessorMsg::ProcessFrame { frame, direction }) {
                        warn!(
                            "[{}] Failed to push frame upstream: {}",
                            self.behavior.name, e
                        );
                    }
                }
            }
            FrameDirection::Downstream => {
                // Output transport is at the end of the pipeline, no downstream
                debug!(
                    "[{}] Cannot push downstream from output transport",
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
            audio_out_sample_rate,
            ..
        } = frame
        {
            self.sample_rate = self
                .behavior
                .params
                .audio_out_sample_rate
                .unwrap_or(*audio_out_sample_rate);

            // Calculate audio chunk size
            let audio_bytes_10ms = (self.sample_rate as usize / 100)
                * self.behavior.params.audio_out_channels as usize
                * 2;
            self.audio_chunk_size =
                audio_bytes_10ms * self.behavior.params.audio_out_10ms_chunks as usize;

            self.is_running = true;

            // Start backend playback - we need to get a mutable reference
            // Since we're using Arc, we need to clone and use the backend differently
            // For now, we'll skip calling start_playback here since we can't mutate through Arc
            // The backend should start playback when write_audio is first called
            debug!(
                "[{}] Started with sample rate: {}, chunk size: {}",
                self.behavior.name, self.sample_rate, self.audio_chunk_size
            );

            // Initialize media senders
            self.set_transport_ready().await;
        }
    }

    /// Stop the transport - called on EndFrame
    async fn stop(&mut self, _frame: &Frame) {
        // Stop all media senders
        let senders: Vec<MediaSender> = self.media_senders.drain().map(|(_, s)| s).collect();
        for sender in senders {
            sender.stop().await;
        }

        self.is_running = false;
        debug!("[{}] Stopped", self.behavior.name);
    }

    /// Cancel the transport - called on CancelFrame
    async fn cancel(&mut self, _frame: &Frame) {
        // Cancel all media senders
        let senders: Vec<MediaSender> = self.media_senders.drain().map(|(_, s)| s).collect();
        for sender in senders {
            sender.cancel();
        }

        self.is_running = false;
        debug!("[{}] Cancelled", self.behavior.name);
    }

    /// Initialize media senders and signal transport ready
    async fn set_transport_ready(&mut self) {
        // Need upstream ref for media senders
        let upstream = match &self.previous {
            Some(prev) => prev.clone(),
            None => {
                warn!(
                    "[{}] No upstream link, cannot create media senders",
                    self.behavior.name
                );
                return;
            }
        };

        // Create default media sender with backend as writer
        let default_sender = MediaSender::new(
            None,
            self.sample_rate,
            self.behavior.params.clone(),
            upstream.clone(),
            self.backend.clone() as Arc<dyn AudioWriter>,
        );
        self.media_senders.insert(None, default_sender);

        // Create senders for all unique destinations
        let destinations: Vec<String> = self
            .behavior
            .params
            .audio_out_destinations
            .iter()
            .chain(self.behavior.params.video_out_destinations.iter())
            .cloned()
            .collect();

        for dest in destinations {
            if !self.media_senders.contains_key(&Some(dest.clone())) {
                let sender = MediaSender::new(
                    Some(dest.clone()),
                    self.sample_rate,
                    self.behavior.params.clone(),
                    upstream.clone(),
                    self.backend.clone() as Arc<dyn AudioWriter>,
                );
                self.media_senders.insert(Some(dest), sender);
            }
        }

        debug!(
            "[{}] Transport ready with {} media senders",
            self.behavior.name,
            self.media_senders.len()
        );

        // Signal transport ready upstream
        self.push_frame(
            Frame::OutputTransportReady {
                header: FrameHeader::default(),
            },
            FrameDirection::Upstream,
        );
    }

    // ========================================================================
    // Frame handling - routes frames to appropriate MediaSender
    // ========================================================================

    /// Handle a frame by routing to the appropriate media sender
    async fn handle_frame(&mut self, frame: &Frame) {
        // Get the destination from the frame header
        let destination = self.get_frame_destination(frame);

        // Check if we have a sender for this destination
        let dest_key = if self.media_senders.contains_key(&destination) {
            destination
        } else if destination.is_some() {
            warn!(
                "[{}] Destination {:?} not registered for frame {}",
                self.behavior.name,
                destination,
                frame.name()
            );
            // Fall back to default sender
            if !self.media_senders.contains_key(&None) {
                return;
            }
            None
        } else {
            None
        };

        // Route frame to appropriate handler
        match frame {
            Frame::StartInterruption { .. } => {
                if let Some(sender) = self.media_senders.get(&dest_key) {
                    sender.handle_interruption();
                }
                // Also clear the backend buffer
                self.backend.clear_buffer();
            }
            Frame::AudioOutput { .. } | Frame::TTSAudio { .. } => {
                if let Some(sender) = self.media_senders.get(&dest_key) {
                    sender.queue_audio(frame.clone());
                }
            }
            Frame::ImageRaw { .. } | Frame::Sprite { .. } => {
                // Video frames - handle synchronously for now
                self.write_video_frame(frame).await;
            }
            Frame::MixerControl { .. } => {
                // TODO: Implement mixer control
            }
            _ => {
                // Check if frame has PTS (timed frame) or is a sync frame
                if self.frame_has_pts(frame) {
                    // Timed frames would go through a clock queue
                    // For now, just push through
                    self.push_frame(frame.clone(), FrameDirection::Upstream);
                } else {
                    // Sync frames pass through
                    self.push_frame(frame.clone(), FrameDirection::Upstream);
                }
            }
        }
    }

    /// Get the transport destination from a frame's header
    fn get_frame_destination(&self, frame: &Frame) -> Option<String> {
        match frame {
            Frame::AudioOutput { header, .. } => header.transport_destination.clone(),
            Frame::TTSAudio { header, .. } => header.transport_destination.clone(),
            Frame::ImageRaw { header, .. } => header.transport_destination.clone(),
            Frame::Sprite { header, .. } => header.transport_destination.clone(),
            Frame::MixerControl { header, .. } => header.transport_destination.clone(),
            _ => None,
        }
    }

    /// Check if a frame has a PTS (presentation timestamp)
    fn frame_has_pts(&self, frame: &Frame) -> bool {
        match frame {
            Frame::AudioOutput { header, .. } => header.pts.is_some(),
            Frame::TTSAudio { header, .. } => header.pts.is_some(),
            Frame::ImageRaw { header, .. } => header.pts.is_some(),
            Frame::Sprite { header, .. } => header.pts.is_some(),
            _ => false,
        }
    }

    // ========================================================================
    // Transport write methods (to be overridden by concrete implementations)
    // ========================================================================

    /// Write video frame to transport
    async fn write_video_frame(&self, _frame: &Frame) -> bool {
        // Override in concrete implementations
        true
    }

    /// Send a transport message
    async fn send_message(&self, frame: &Frame) {
        // Override in concrete implementations
        debug!("[{}] Sending message: {:?}", self.behavior.name, frame);
    }
}

// ============================================================================
// Actor implementation - generic over backend
// ============================================================================

#[async_trait]
impl<B: OutputTransportBackend + AudioWriter> Actor for OutputTransportActor<B> {
    type Msg = ProcessorMsg;
    type State = OutputTransportState<B>;
    type Arguments = OutputTransportState<B>;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        debug!(
            "[{}] Output transport actor started with {} backend",
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
        debug!("[{}] Output transport actor stopped", state.behavior.name);
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

                // This mirrors Pipecat's process_frame method
                match &frame {
                    // StartFrame: push first, then start
                    Frame::Start { .. } => {
                        state.push_frame(frame.clone(), direction);
                        state.start(&frame).await;
                    }

                    // EndFrame: stop first, then push
                    Frame::End { .. } => {
                        state.stop(&frame).await;
                        state.push_frame(frame, direction);
                    }

                    // CancelFrame: cancel, then push
                    Frame::Cancel { .. } => {
                        state.cancel(&frame).await;
                        state.push_frame(frame, direction);
                    }

                    // InterruptionFrame: push, then handle
                    Frame::StartInterruption { .. } => {
                        state.push_frame(frame.clone(), direction);
                        state.handle_frame(&frame).await;
                    }

                    // Urgent transport message: send immediately
                    Frame::TransportMessageUrgent { .. } => {
                        state.send_message(&frame).await;
                    }

                    // System frames: just push through
                    _ if frame.is_system() => {
                        state.push_frame(frame, direction);
                    }

                    // Upstream frames: push upstream
                    _ if direction == FrameDirection::Upstream => {
                        state.push_frame(frame, direction);
                    }

                    // All other frames: route to media sender
                    _ => {
                        state.handle_frame(&frame).await;
                    }
                }
            }

            ProcessorMsg::LinkNext { .. } => {
                // Output transport doesn't use next links (it's the end of pipeline)
            }

            ProcessorMsg::LinkPrevious { previous } => {
                debug!("[{}] Linked to previous processor", state.behavior.name);
                state.previous = Some(previous);
            }

            ProcessorMsg::Setup { setup } => {
                debug!("[{}] Setting up output transport", state.behavior.name);
                state.behavior.setup(&setup).await;
            }

            ProcessorMsg::Cleanup => {
                debug!("[{}] Cleaning up output transport", state.behavior.name);
                // Stop all media senders
                let senders: Vec<MediaSender> =
                    state.media_senders.drain().map(|(_, s)| s).collect();
                for sender in senders {
                    sender.stop().await;
                }
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

/// Base output transport actor (uses null backend)
pub type BaseOutputTransportActor = OutputTransportActor<NullOutputBackend>;

/// Base output transport state (uses null backend)
pub type BaseOutputTransportState = OutputTransportState<NullOutputBackend>;

impl BaseOutputTransportState {
    /// Create a new base output transport state with null backend
    pub fn new_base(behavior: BaseOutputTransport) -> Self {
        Self::new(behavior, NullOutputBackend::new())
    }
}
