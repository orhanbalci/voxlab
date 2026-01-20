//! PipelineRunner - Orchestrates pipeline lifecycle

use ractor::Actor;
use tracing::{debug, info};

use crate::pipeline::pipeline::{Pipeline, PipelineError};
use crate::pipeline::task::PipelineTask;
use crate::processor::{
    PipelineActorRef, ProcessorActor, ProcessorBehavior, ProcessorMsg, ProcessorSetup,
    ProcessorState,
};
use crate::transport::local::{
    LocalAudioInputTransportActor, LocalAudioInputTransportState, LocalAudioOutputTransportActor,
    LocalAudioOutputTransportState, LocalAudioTransportParams,
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

        // Check if audio input is enabled
        let audio_in_enabled = params.base.audio_in_enabled;
        let audio_out_enabled = params.base.audio_out_enabled;

        info!(
            "PipelineRunner: audio_in={}, audio_out={}",
            audio_in_enabled, audio_out_enabled
        );

        // Spawn processor actors
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

        // Create input transport if audio input is enabled
        let input_ref = if audio_in_enabled {
            let input_state = LocalAudioInputTransportState::new_local(
                "LocalAudioInput".to_string(),
                params.clone(),
            )
            .map_err(|e| {
                PipelineError::Other(format!("Failed to create input transport: {}", e))
            })?;

            let (input_actor, _) = Actor::spawn(
                Some("local-audio-input".to_string()),
                LocalAudioInputTransportActor::default(),
                input_state,
            )
            .await?;
            info!("PipelineRunner: Spawned local audio input transport");
            Some(PipelineActorRef::new(input_actor))
        } else {
            None
        };

        // Create output transport if audio output is enabled
        let output_ref = if audio_out_enabled {
            let output_state = LocalAudioOutputTransportState::new_local(
                "LocalAudioOutput".to_string(),
                params.clone(),
            )
            .map_err(|e| {
                PipelineError::Other(format!("Failed to create output transport: {}", e))
            })?;

            let (output_actor, _) = Actor::spawn(
                Some("local-audio-output".to_string()),
                LocalAudioOutputTransportActor::default(),
                output_state,
            )
            .await?;
            info!("PipelineRunner: Spawned local audio output transport");
            Some(PipelineActorRef::new(output_actor))
        } else {
            None
        };

        let (pipeline, completion_rx) =
            Pipeline::new("main-pipeline".to_string(), processor_actors, None, None).await?;

        // Link processors together
        pipeline.link_processors()?;

        // Link input transport -> first processor (if input enabled)
        // Use synchronous linking to ensure the link is established before returning
        if let Some(ref input) = input_ref {
            if let Some(first_proc) = pipeline.get_processors().get(1) {
                // Index 1 is first user processor (0 is source)
                input
                    .link_next_sync(first_proc.clone())
                    .await
                    .map_err(|e| {
                        PipelineError::Other(format!("Failed to link input to first processor: {}", e))
                    })?;
            }

            // Setup input transport
            input
                .cast(ProcessorMsg::Setup {
                    setup: setup.clone(),
                })
                .map_err(|e| PipelineError::Other(format!("Failed to setup input: {}", e)))?;
        }

        // Link last processor -> output transport -> sink (if output enabled)
        if let Some(ref output) = output_ref {
            if let Some(last_proc) = pipeline.get_processors().iter().rev().nth(1) {
                last_proc
                    .cast(ProcessorMsg::LinkNext {
                        next: output.clone(),
                    })
                    .map_err(|e| {
                        PipelineError::Other(format!("Failed to link to output: {}", e))
                    })?;

                output
                    .cast(ProcessorMsg::LinkPrevious {
                        previous: last_proc.clone(),
                    })
                    .map_err(|e| {
                        PipelineError::Other(format!("Failed to link output previous: {}", e))
                    })?;
            }

            output
                .cast(ProcessorMsg::LinkNext {
                    next: PipelineActorRef::new(pipeline.sink().clone()),
                })
                .map_err(|e| {
                    PipelineError::Other(format!("Failed to link output to sink: {}", e))
                })?;

            output
                .cast(ProcessorMsg::Setup {
                    setup: setup.clone(),
                })
                .map_err(|e| PipelineError::Other(format!("Failed to setup output: {}", e)))?;
        } else {
            // No output transport, link last processor directly to sink
            if let Some(last_proc) = pipeline.get_processors().iter().rev().nth(1) {
                last_proc
                    .cast(ProcessorMsg::LinkNext {
                        next: PipelineActorRef::new(pipeline.sink().clone()),
                    })
                    .map_err(|e| {
                        PipelineError::Other(format!("Failed to link to sink: {}", e))
                    })?;
            }
        }

        pipeline.setup_processors(setup)?;

        info!("PipelineRunner: Pipeline ready");

        // Return task with input ref if available, otherwise source
        let entry_point = input_ref.unwrap_or_else(|| PipelineActorRef::new(pipeline.source().clone()));

        Ok(PipelineTask::new(entry_point, completion_rx))
    }
}
