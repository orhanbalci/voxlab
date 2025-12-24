// Frame Pipeline Implementation using Ractor Actors

use ractor::{Actor, ActorRef, SpawnErr};
use tracing::debug;

use crate::pipeline::sink::{PipelineSink, PipelineSinkActor, PipelineSinkState};
use crate::pipeline::source::{PipelineSource, PipelineSourceActor, PipelineSourceState};
use crate::processor::{PipelineActorRef, ProcessorActor, ProcessorSetup};

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("Failed to spawn actor: {0}")]
    SpawnError(#[from] SpawnErr),
    #[error("Pipeline error: {0}")]
    Other(String),
}

pub struct Pipeline {
    name: String,
    /// All processors including source and sink: [source] + processors + [sink]
    /// This matches pipecat's _processors list structure
    processors: Vec<PipelineActorRef>,
    /// Direct reference to source for convenience
    source: ActorRef<PipelineSourceActor>,
    /// Direct reference to sink for convenience
    sink: ActorRef<PipelineSinkActor>,
}

impl Pipeline {
    pub async fn new(
        name: String,
        processors: Vec<ActorRef<ProcessorActor>>,
        source: Option<ActorRef<PipelineSourceActor>>,
        sink: Option<ActorRef<PipelineSinkActor>>,
    ) -> Result<Self, PipelineError> {
        let pipeline_name = name.clone();

        // Create default source if not provided
        let source_ref = if let Some(s) = source {
            s
        } else {
            let source_logic = PipelineSource::new(
                format!("{}::Source", pipeline_name),
                |frame, _direction| async move {
                    debug!("Source received upstream frame: {:?}", frame);
                },
            );
            let state = PipelineSourceState::new(source_logic);
            let (actor_ref, _) = Actor::spawn(None, PipelineSourceActor, state).await?;
            actor_ref
        };

        // Create default sink if not provided
        let sink_ref = if let Some(s) = sink {
            s
        } else {
            let sink_logic = PipelineSink::new(
                format!("{}::Sink", pipeline_name),
                |frame, _direction| async move {
                    debug!("Sink received downstream frame: {:?}", frame);
                },
            );
            let state = PipelineSinkState::new(sink_logic);
            let (actor_ref, _) = Actor::spawn(None, PipelineSinkActor, state).await?;
            actor_ref
        };

        // Build unified processors list: [source] + processors + [sink]
        // This matches pipecat's self._processors = [self._source] + processors + [self._sink]
        let mut all_processors = Vec::new();
        all_processors.push(PipelineActorRef::new(source_ref.clone()));
        for proc in processors {
            all_processors.push(PipelineActorRef::new(proc));
        }
        all_processors.push(PipelineActorRef::new(sink_ref.clone()));

        Ok(Self {
            name: pipeline_name,
            processors: all_processors,
            source: source_ref,
            sink: sink_ref,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get all processors including source and sink
    pub fn get_processors(&self) -> &[PipelineActorRef] {
        &self.processors
    }

    /// Get direct reference to the source actor
    pub fn source(&self) -> &ActorRef<PipelineSourceActor> {
        &self.source
    }

    /// Get direct reference to the sink actor
    pub fn sink(&self) -> &ActorRef<PipelineSinkActor> {
        &self.sink
    }

    /// Link all processors in sequence. Each processor will forward frames to the next.
    /// Similar to pipecat's _link_processors method.
    /// Since processors list includes [source] + processors + [sink], this links everything.
    pub fn link_processors(&self) -> Result<(), PipelineError> {
        if self.processors.len() < 2 {
            return Ok(());
        }

        debug!("Linking {} processors in sequence", self.processors.len());

        // Link processors in sequence: processor[0] -> processor[1] -> ... -> processor[n]
        // This now includes: source -> processor1 -> ... -> processorN -> sink
        for i in 0..self.processors.len() - 1 {
            let current = &self.processors[i];
            let next = &self.processors[i + 1];

            // Send LinkNext message to link current processor to the next one
            current
                .cast(crate::processor::ProcessorMsg::LinkNext { next: next.clone() })
                .map_err(|e| PipelineError::Other(format!("Failed to link processors: {}", e)))?;
        }

        debug!("All processors linked successfully");
        Ok(())
    }

    /// Set up all processors in the pipeline (including source and sink)
    /// Similar to pipecat's _setup_processors method
    pub fn setup_processors(&self, setup: ProcessorSetup) -> Result<(), PipelineError> {
        debug!(
            "Setting up {} processors (including source and sink)",
            self.processors.len()
        );

        for processor in &self.processors {
            processor
                .cast(crate::processor::ProcessorMsg::Setup {
                    setup: setup.clone(),
                })
                .map_err(|e| PipelineError::Other(format!("Failed to setup processor: {}", e)))?;
        }

        debug!("All processors set up successfully");
        Ok(())
    }

    /// Clean up all processors in the pipeline (including source and sink)
    /// Similar to pipecat's _cleanup_processors method
    pub fn cleanup_processors(&self) -> Result<(), PipelineError> {
        debug!(
            "Cleaning up {} processors (including source and sink)",
            self.processors.len()
        );

        for processor in &self.processors {
            processor
                .cast(crate::processor::ProcessorMsg::Cleanup)
                .map_err(|e| PipelineError::Other(format!("Failed to cleanup processor: {}", e)))?;
        }

        debug!("All processors cleaned up successfully");
        Ok(())
    }
}
