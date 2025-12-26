//! Base transport trait definition
//!
//! This module provides the abstract trait that all transport implementations
//! must implement to integrate with the pipeline system.

use crate::processor::PipelineActorRef;

/// Base trait for transport implementations
///
/// A transport provides input and output frame processors that handle
/// media streaming between external sources/sinks and the pipeline.
///
/// # Example
///
/// ```ignore
/// struct MyTransport {
///     input: PipelineActorRef,
///     output: PipelineActorRef,
/// }
///
/// impl BaseTransport for MyTransport {
///     fn name(&self) -> &str {
///         "MyTransport"
///     }
///
///     fn input(&self) -> PipelineActorRef {
///         self.input.clone()
///     }
///
///     fn output(&self) -> PipelineActorRef {
///         self.output.clone()
///     }
/// }
/// ```
pub trait BaseTransport: Send + Sync {
    /// Get the name of this transport
    fn name(&self) -> &str;

    /// Get the input frame processor for this transport
    ///
    /// The input processor handles incoming frames from external sources
    /// (e.g., microphone, camera, network) and pushes them downstream
    /// through the pipeline.
    fn input(&self) -> PipelineActorRef;

    /// Get the output frame processor for this transport
    ///
    /// The output processor handles outgoing frames from the pipeline
    /// and sends them to external sinks (e.g., speaker, display, network).
    fn output(&self) -> PipelineActorRef;
}
