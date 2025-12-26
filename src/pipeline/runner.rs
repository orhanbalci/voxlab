//! PipelineRunner - Orchestrates pipeline lifecycle

use ractor::Actor;
use tracing::debug;

use crate::pipeline::pipeline::{Pipeline, PipelineError};
use crate::pipeline::task::PipelineTask;
use crate::processor::{
    PipelineActorRef, ProcessorActor, ProcessorBehavior, ProcessorMsg, ProcessorSetup,
    ProcessorState,
};
use crate::transport::local::{
    LocalAudioOutputTransportActor, LocalAudioOutputTransportState, LocalAudioTransportParams,
};

pub struct PipelineRunner;

impl PipelineRunner {
    pub async fn run(
        processors: Vec<Box<dyn ProcessorBehavior>>,
        params: LocalAudioTransportParams,
        setup: ProcessorSetup,
    ) -> Result<PipelineTask, PipelineError> {
        debug!(
            "PipelineRunner: Starting pipeline with {} processors",
            processors.len()
        );

        let mut processor_actors = Vec::new();
        for (i, behavior) in processors.into_iter().enumerate() {
            let name = behavior.name().to_string();
            let state = ProcessorState::new_boxed(behavior);
            let (actor_ref, _) = Actor::spawn(
                Some(format!("processor-{}-{}", i, name)),
                ProcessorActor,
                state,
            )
            .await?;
            debug!("PipelineRunner: Spawned processor actor: {}", name);
            processor_actors.push(actor_ref);
        }

        let output_state =
            LocalAudioOutputTransportState::new_local("LocalAudioOutput".to_string(), params)
                .map_err(|e| {
                    PipelineError::Other(format!("Failed to create output transport: {}", e))
                })?;

        let (output_actor, _) = Actor::spawn(
            Some("local-audio-output".to_string()),
            LocalAudioOutputTransportActor::default(),
            output_state,
        )
        .await?;
        debug!("PipelineRunner: Spawned local audio output transport");

        let pipeline =
            Pipeline::new("main-pipeline".to_string(), processor_actors, None, None).await?;

        let output_ref = PipelineActorRef::new(output_actor);

        pipeline.link_processors()?;

        if let Some(last_proc) = pipeline.get_processors().iter().rev().nth(1) {
            last_proc
                .cast(ProcessorMsg::LinkNext {
                    next: output_ref.clone(),
                })
                .map_err(|e| PipelineError::Other(format!("Failed to link to output: {}", e)))?;

            output_ref
                .cast(ProcessorMsg::LinkPrevious {
                    previous: last_proc.clone(),
                })
                .map_err(|e| {
                    PipelineError::Other(format!("Failed to link output previous: {}", e))
                })?;
        }

        output_ref
            .cast(ProcessorMsg::LinkNext {
                next: PipelineActorRef::new(pipeline.sink().clone()),
            })
            .map_err(|e| PipelineError::Other(format!("Failed to link output to sink: {}", e)))?;

        pipeline.setup_processors(setup.clone())?;

        output_ref
            .cast(ProcessorMsg::Setup { setup })
            .map_err(|e| PipelineError::Other(format!("Failed to setup output: {}", e)))?;

        debug!("PipelineRunner: Pipeline ready");

        Ok(PipelineTask::from_source_actor(pipeline.source().clone()))
    }
}
