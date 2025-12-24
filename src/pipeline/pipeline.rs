// Frame Pipeline Implementation using Ractor Actors

use ractor::{Actor, ActorRef, SpawnErr};
use tracing::debug;

use crate::pipeline::sink::{PipelineSink, PipelineSinkActor, PipelineSinkState};
use crate::pipeline::source::{PipelineSource, PipelineSourceActor, PipelineSourceState};
use crate::processor::{PipelineActorRef, ProcessorActor};

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("Failed to spawn actor: {0}")]
    SpawnError(#[from] SpawnErr),
    #[error("Pipeline error: {0}")]
    Other(String),
}

pub struct Pipeline {
    name: String,
    processors: Vec<ActorRef<ProcessorActor>>,
    source: Option<ActorRef<PipelineSourceActor>>,
    sink: Option<ActorRef<PipelineSinkActor>>,
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
            Some(s)
        } else {
            let source_logic = PipelineSource::new(
                format!("{}::Source", pipeline_name),
                |frame, _direction| async move {
                    debug!("Source received upstream frame: {:?}", frame);
                },
            );
            let state = PipelineSourceState::new(source_logic);
            let (actor_ref, _) = Actor::spawn(None, PipelineSourceActor, state).await?;
            Some(actor_ref)
        };

        // Create default sink if not provided
        let sink_ref = if let Some(s) = sink {
            Some(s)
        } else {
            let sink_logic = PipelineSink::new(
                format!("{}::Sink", pipeline_name),
                |frame, _direction| async move {
                    debug!("Sink received downstream frame: {:?}", frame);
                },
            );
            let state = PipelineSinkState::new(sink_logic);
            let (actor_ref, _) = Actor::spawn(None, PipelineSinkActor, state).await?;
            Some(actor_ref)
        };

        Ok(Self {
            name: pipeline_name,
            processors,
            source: source_ref,
            sink: sink_ref,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn get_source_ref(&self) -> Option<&ActorRef<PipelineSourceActor>> {
        self.source.as_ref()
    }

    pub fn get_sink_ref(&self) -> Option<&ActorRef<PipelineSinkActor>> {
        self.sink.as_ref()
    }

    pub fn get_processors(&self) -> &[ActorRef<ProcessorActor>] {
        &self.processors
    }

    /// Link all processors in sequence. Each processor will forward frames to the next.
    /// Similar to pipecat's _link_processors method.
    pub fn link_processors(&self) -> Result<(), PipelineError> {
        if self.processors.is_empty() {
            return Ok(());
        }

        // Link processors in sequence: processor[0] -> processor[1] -> ... -> processor[n]
        for i in 0..self.processors.len() - 1 {
            let current = &self.processors[i];
            let next = &self.processors[i + 1];

            // Send LinkNext message to link current processor to the next one
            current
                .cast(crate::processor::ProcessorMsg::LinkNext {
                    next: PipelineActorRef::new(next.clone()),
                })
                .map_err(|e| PipelineError::Other(format!("Failed to link processors: {}", e)))?;
        }

        Ok(())
    }

    /// Link the source to the first processor (if both exist)
    pub fn link_source_to_first_processor(&self) -> Result<(), PipelineError> {
        if let (Some(source), Some(first_processor)) = (self.source.as_ref(), self.processors.first()) {
            source
                .cast(crate::processor::ProcessorMsg::LinkNext {
                    next: PipelineActorRef::new(first_processor.clone()),
                })
                .map_err(|e| PipelineError::Other(format!("Failed to link source to first processor: {}", e)))?;
            debug!("Source linked to first processor");
        }
        Ok(())
    }

    /// Link the last processor to the sink (if both exist)
    pub fn link_last_processor_to_sink(&self) -> Result<(), PipelineError> {
        if let (Some(last_processor), Some(sink)) = (self.processors.last(), self.sink.as_ref()) {
            sink
                .cast(crate::processor::ProcessorMsg::LinkPrevious {
                    previous: PipelineActorRef::new(last_processor.clone()),
                })
                .map_err(|e| PipelineError::Other(format!("Failed to link last processor to sink: {}", e)))?;
            debug!("Last processor linked to sink");
        }
        Ok(())
    }

    /// Link all components: source -> processors -> sink
    /// This is the main method that sets up the entire pipeline chain
    pub fn link_all(&self) -> Result<(), PipelineError> {
        self.link_processors()?;
        self.link_source_to_first_processor()?;
        self.link_last_processor_to_sink()?;
        Ok(())
    }
}
