//! MediaSender - handles media streaming with async audio queue
//!
//! This module provides the MediaSender which processes audio output through
//! an async queue with a background task. Each destination gets its own
//! MediaSender instance with its own background task.
//!
//! The background task:
//! - Buffers and chunks audio into 10ms * N segments
//! - Writes audio chunks to the transport backend via AudioWriter trait
//! - Detects bot speaking state via timeout (no audio for BOT_VAD_STOP_SECS)
//! - Emits BotStartedSpeaking/BotSpeaking/BotStoppedSpeaking frames upstream

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::debug;

use crate::frame::{Frame, FrameDirection, FrameHeader};
use crate::processor::{PipelineActorRef, ProcessorMsg};
use crate::transport::TransportParams;

/// Time threshold after which bot is considered to have stopped speaking (in seconds)
const BOT_VAD_STOP_SECS: f64 = 0.35;

/// How often to push BotSpeakingFrame (in seconds)
const BOT_SPEAKING_FRAME_PERIOD: f64 = 0.2;

// ============================================================================
// AudioWriter trait - abstracts audio output
// ============================================================================

/// Trait for writing audio data to an output device.
///
/// Implement this trait to provide audio output for a transport backend.
/// The MediaSender will call `write()` with chunked audio data.
pub trait AudioWriter: Send + Sync + 'static {
    /// Write audio data to the output device.
    ///
    /// This is called with chunked audio data (10ms * N segments).
    /// The implementation should buffer or immediately play the audio.
    fn write(&self, data: &[u8]);

    /// Clear any buffered audio data.
    ///
    /// Called on interruption to stop current playback immediately.
    fn clear(&self);
}

/// Null audio writer that discards audio.
///
/// Use this when you don't need actual audio output, such as for testing
/// or when the transport doesn't have audio output.
#[allow(dead_code)]
pub struct NullAudioWriter;

impl AudioWriter for NullAudioWriter {
    fn write(&self, _data: &[u8]) {
        // Discard audio
    }

    fn clear(&self) {
        // Nothing to clear
    }
}

// ============================================================================
// AudioQueueItem - messages sent to the audio task
// ============================================================================

/// Items that can be sent to the audio processing task
enum AudioQueueItem {
    /// An audio frame to process
    Frame(Frame),
    /// Clear the audio buffer (on interruption)
    ClearBuffer,
    /// Stop the audio task
    Stop,
}

// ============================================================================
// AudioTaskState - state for the background audio processing task
// ============================================================================

/// State for the audio processing background task
struct AudioTaskState {
    /// The destination identifier (None for default)
    destination: Option<String>,
    /// Audio sample rate in Hz
    #[allow(dead_code)]
    sample_rate: u32,
    /// Size of audio chunks to buffer before sending
    audio_chunk_size: usize,
    /// Buffer for incoming audio data
    audio_buffer: Vec<u8>,
    /// Whether the bot is currently speaking
    bot_speaking: bool,
    /// Last time a BotSpeakingFrame was pushed
    bot_speaking_frame_time: Instant,
    /// Transport parameters
    #[allow(dead_code)]
    params: TransportParams,
    /// Audio writer for output
    writer: Arc<dyn AudioWriter>,
}

impl AudioTaskState {
    fn new(
        destination: Option<String>,
        sample_rate: u32,
        params: TransportParams,
        writer: Arc<dyn AudioWriter>,
    ) -> Self {
        // Calculate audio chunk size: 10ms * N chunks
        // 10ms of audio = sample_rate / 100 samples
        // Each sample = channels * 2 bytes (16-bit audio)
        let audio_bytes_10ms =
            (sample_rate as usize / 100) * params.audio_out_channels as usize * 2;
        let audio_chunk_size = audio_bytes_10ms * params.audio_out_10ms_chunks as usize;

        Self {
            destination,
            sample_rate,
            audio_chunk_size,
            audio_buffer: Vec::new(),
            bot_speaking: false,
            bot_speaking_frame_time: Instant::now(),
            params,
            writer,
        }
    }
}

// ============================================================================
// Audio task handler - background task for processing audio
// ============================================================================

/// Push a frame upstream to the previous processor
fn push_upstream(upstream: &PipelineActorRef, frame: Frame) {
    let _ = upstream.cast(ProcessorMsg::ProcessFrame {
        frame,
        direction: FrameDirection::Upstream,
    });
}

/// Process an audio frame - buffer, chunk, and emit bot speaking frames
fn process_audio_frame(state: &mut AudioTaskState, frame: &Frame, upstream: &PipelineActorRef) {
    let audio = match frame {
        Frame::AudioOutput { data, .. } => data,
        Frame::TTSAudio { data, .. } => data,
        _ => return,
    };

    // Start speaking if not already
    if !state.bot_speaking {
        state.bot_speaking = true;
        state.bot_speaking_frame_time = Instant::now();
        push_upstream(
            upstream,
            Frame::BotStartedSpeaking {
                header: FrameHeader::default(),
            },
        );
    }

    // Buffer the audio
    state.audio_buffer.extend_from_slice(audio);

    // Chunk and write to output
    while state.audio_buffer.len() >= state.audio_chunk_size {
        let chunk: Vec<u8> = state.audio_buffer.drain(..state.audio_chunk_size).collect();

        // Write to transport output via AudioWriter
        state.writer.write(&chunk);

        debug!(
            "[AudioTask] Wrote {} bytes of audio to {:?}",
            state.audio_chunk_size, state.destination
        );
    }

    // Check if we should send BotSpeakingFrame
    if state.bot_speaking
        && state.bot_speaking_frame_time.elapsed().as_secs_f64() >= BOT_SPEAKING_FRAME_PERIOD
    {
        state.bot_speaking_frame_time = Instant::now();
        push_upstream(
            upstream,
            Frame::BotSpeaking {
                header: FrameHeader::default(),
            },
        );
    }
}

/// Handle interruption - clear buffer and stop speaking
fn handle_audio_interruption(state: &mut AudioTaskState, upstream: &PipelineActorRef) {
    if state.bot_speaking {
        state.bot_speaking = false;
        push_upstream(
            upstream,
            Frame::BotStoppedSpeaking {
                header: FrameHeader::default(),
            },
        );
    }
    state.audio_buffer.clear();
    // Clear the writer's buffer as well
    state.writer.clear();
}

/// Background task that continuously processes audio from the queue
async fn audio_task_handler(
    mut rx: mpsc::UnboundedReceiver<AudioQueueItem>,
    mut state: AudioTaskState,
    upstream: PipelineActorRef,
) {
    debug!(
        "[AudioTask] Started for destination {:?}",
        state.destination
    );

    loop {
        match tokio::time::timeout(Duration::from_secs_f64(BOT_VAD_STOP_SECS), rx.recv()).await {
            Ok(Some(AudioQueueItem::Frame(frame))) => {
                process_audio_frame(&mut state, &frame, &upstream);
            }
            Ok(Some(AudioQueueItem::ClearBuffer)) => {
                handle_audio_interruption(&mut state, &upstream);
            }
            Ok(Some(AudioQueueItem::Stop)) | Ok(None) => {
                // Clean shutdown
                if state.bot_speaking {
                    push_upstream(
                        &upstream,
                        Frame::BotStoppedSpeaking {
                            header: FrameHeader::default(),
                        },
                    );
                }
                debug!(
                    "[AudioTask] Stopped for destination {:?}",
                    state.destination
                );
                break;
            }
            Err(_timeout) => {
                // No audio for BOT_VAD_STOP_SECS - check if bot should stop speaking
                if state.bot_speaking && state.audio_buffer.is_empty() {
                    state.bot_speaking = false;
                    push_upstream(
                        &upstream,
                        Frame::BotStoppedSpeaking {
                            header: FrameHeader::default(),
                        },
                    );
                    debug!(
                        "[AudioTask] Bot stopped speaking (timeout) for {:?}",
                        state.destination
                    );
                }
            }
        }
    }
}

// ============================================================================
// MediaSender - handles media streaming for a specific destination
// ============================================================================

/// Media sender that uses an async queue for audio processing.
///
/// Each destination gets its own MediaSender instance with its own background task.
/// Audio frames are queued and processed asynchronously, with the background task
/// handling:
/// - Audio buffering and chunking (10ms * N segments)
/// - Writing audio to the transport via AudioWriter
/// - Bot speaking state detection via timeout
/// - Emitting BotStartedSpeaking/BotSpeaking/BotStoppedSpeaking frames upstream
///
/// # Example
///
/// ```ignore
/// let writer = Arc::new(MyAudioWriter::new());
/// let sender = MediaSender::new(
///     None,  // default destination
///     16000, // sample rate
///     params,
///     upstream_ref,
///     writer,
/// );
///
/// // Queue audio frames (non-blocking)
/// sender.queue_audio(audio_frame);
///
/// // Handle interruption
/// sender.handle_interruption();
///
/// // Clean shutdown
/// sender.stop().await;
/// ```
pub struct MediaSender {
    /// The destination identifier (None for default)
    #[allow(dead_code)]
    destination: Option<String>,
    /// Sender to queue audio frames to the background task
    audio_tx: mpsc::UnboundedSender<AudioQueueItem>,
    /// Handle to the background audio task
    task_handle: JoinHandle<()>,
}

impl MediaSender {
    /// Create a new MediaSender with a background audio processing task.
    ///
    /// # Arguments
    ///
    /// * `destination` - Optional destination identifier (None for default)
    /// * `sample_rate` - Audio sample rate in Hz
    /// * `params` - Transport parameters for audio configuration
    /// * `upstream` - Reference to push bot speaking frames upstream
    /// * `writer` - Audio writer for output
    pub fn new(
        destination: Option<String>,
        sample_rate: u32,
        params: TransportParams,
        upstream: PipelineActorRef,
        writer: Arc<dyn AudioWriter>,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let task_state = AudioTaskState::new(destination.clone(), sample_rate, params, writer);

        let handle = tokio::spawn(audio_task_handler(rx, task_state, upstream));

        Self {
            destination,
            audio_tx: tx,
            task_handle: handle,
        }
    }

    /// Create a new MediaSender with null audio writer.
    ///
    /// Use this when you don't need actual audio output.
    #[allow(dead_code)]
    pub fn new_null(
        destination: Option<String>,
        sample_rate: u32,
        params: TransportParams,
        upstream: PipelineActorRef,
    ) -> Self {
        Self::new(
            destination,
            sample_rate,
            params,
            upstream,
            Arc::new(NullAudioWriter),
        )
    }

    /// Queue an audio frame for processing.
    ///
    /// This is non-blocking - the frame is sent to the background task's queue.
    pub fn queue_audio(&self, frame: Frame) {
        let _ = self.audio_tx.send(AudioQueueItem::Frame(frame));
    }

    /// Handle interruption - signals the background task to clear its audio buffer.
    pub fn handle_interruption(&self) {
        let _ = self.audio_tx.send(AudioQueueItem::ClearBuffer);
    }

    /// Stop the media sender and wait for the background task to complete.
    ///
    /// This ensures a clean shutdown with proper bot speaking state cleanup.
    pub async fn stop(self) {
        let _ = self.audio_tx.send(AudioQueueItem::Stop);
        let _ = self.task_handle.await;
    }

    /// Cancel the media sender without waiting for completion.
    ///
    /// Use this for immediate shutdown (e.g., on CancelFrame).
    pub fn cancel(self) {
        let _ = self.audio_tx.send(AudioQueueItem::Stop);
        self.task_handle.abort();
    }
}
