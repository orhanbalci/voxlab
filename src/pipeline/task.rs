//! PipelineTask - A wrapper for queuing frames into a running pipeline
//!
//! This module provides PipelineTask which wraps a pipeline and provides
//! a clean API for queuing frames from external code.

use crate::frame::{Frame, FrameDirection};
use crate::pipeline::source::PipelineSourceActor;
use crate::processor::{PipelineActorRef, ProcessorMsg};
use ractor::ActorRef;

/// A running pipeline task that allows queuing frames
///
/// PipelineTask wraps a pipeline's source actor reference and provides
/// methods to queue frames into the pipeline. This is the primary interface
/// for feeding data into a running pipeline.
///
/// # Example
///
/// ```ignore
/// let task = PipelineRunner::run(processors, params).await?;
///
/// // Queue frames for processing
/// task.queue_frames(vec![
///     Frame::Start { ... },
///     Frame::Text { text: "Hello".into(), ... },
///     Frame::End { ... },
/// ]);
/// ```
pub struct PipelineTask {
    source: PipelineActorRef,
}

impl PipelineTask {
    /// Create a new PipelineTask with the given source actor reference
    pub fn new(source: PipelineActorRef) -> Self {
        Self { source }
    }

    /// Create a new PipelineTask from a PipelineSourceActor reference
    pub fn from_source_actor(source: ActorRef<PipelineSourceActor>) -> Self {
        Self {
            source: PipelineActorRef::new(source),
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

    /// Get a reference to the source actor
    pub fn source(&self) -> &PipelineActorRef {
        &self.source
    }
}
