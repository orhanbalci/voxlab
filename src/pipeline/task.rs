//! PipelineTask - A wrapper for queuing frames into a running pipeline
//!
//! This module provides PipelineTask which wraps a pipeline and provides
//! a clean API for queuing frames from external code.

use crate::frame::{Frame, FrameDirection};
use crate::pipeline::pipeline::CompletionReceiver;
use crate::processor::{PipelineActorRef, ProcessorMsg};

/// A running pipeline task that allows queuing frames
///
/// PipelineTask wraps a pipeline's source actor reference and provides
/// methods to queue frames into the pipeline. This is the primary interface
/// for feeding data into a running pipeline.
///
/// # Example
///
/// ```ignore
/// let task = PipelineRunner::run(processors, params, setup).await?;
///
/// // Queue frames for processing
/// task.queue_frames(vec![
///     Frame::Start { ... },
///     Frame::Text { text: "Hello".into(), ... },
///     Frame::End { ... },
/// ]);
///
/// // Wait for pipeline to complete (when End frame reaches sink)
/// task.wait().await;
/// ```
pub struct PipelineTask {
    source: PipelineActorRef,
    completion_rx: Option<CompletionReceiver>,
}

impl PipelineTask {
    /// Create a new PipelineTask with the given source actor reference and completion receiver
    pub fn new(source: PipelineActorRef, completion_rx: CompletionReceiver) -> Self {
        Self {
            source,
            completion_rx: Some(completion_rx),
        }
    }

    /// Queue multiple frames into the pipeline
    ///
    /// Frames are queued in order and processed downstream.
    /// This is a fire-and-forget operation - it doesn't wait for frames to be processed.
    pub fn queue_frames(&self, frames: Vec<Frame>) {
        for frame in frames {
            self.queue_frame(frame);
        }
    }

    /// Queue a single frame into the pipeline
    ///
    /// The frame is sent downstream through the pipeline.
    /// This is a fire-and-forget operation - it doesn't wait for the frame to be processed.
    pub fn queue_frame(&self, frame: Frame) {
        let _ = self.source.cast(ProcessorMsg::ProcessFrame {
            frame,
            direction: FrameDirection::Downstream,
        });
    }

    /// Wait for the pipeline to complete
    ///
    /// This blocks until an End frame reaches the pipeline sink.
    /// After this returns, all frames have been fully processed.
    pub async fn wait(mut self) {
        if let Some(rx) = self.completion_rx.take() {
            let _ = rx.await;
        }
    }

    /// Wait for the pipeline to complete with a timeout
    ///
    /// Returns `true` if the pipeline completed, or `false` if the timeout elapsed.
    pub async fn wait_timeout(mut self, timeout: std::time::Duration) -> bool {
        if let Some(rx) = self.completion_rx.take() {
            tokio::time::timeout(timeout, rx).await.is_ok()
        } else {
            true
        }
    }

    /// Get a reference to the source actor
    pub fn source(&self) -> &PipelineActorRef {
        &self.source
    }
}
