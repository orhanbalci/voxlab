//! Processor trait, actor, and related messages for the Ractor pipeline

use crate::frame::{Frame, FrameDirection};
use ractor::{Actor, ActorProcessingErr, ActorRef, MessagingErr, RpcReplyPort};
use tracing::debug;

/// A trait for any actor that can handle ProcessorMsg
/// This allows heterogeneous actor chains
#[async_trait::async_trait]
pub trait ProcessorMessageHandler: Send + Sync {
    fn cast_processor_msg(&self, msg: ProcessorMsg) -> Result<(), MessagingErr>;

    /// Synchronously link to the next processor and wait for confirmation
    async fn link_next_sync(&self, next: PipelineActorRef) -> Result<(), MessagingErr>;
}

/// Wrapper for any actor reference that can handle ProcessorMsg
#[derive(Clone)]
pub struct PipelineActorRef {
    handler: std::sync::Arc<dyn ProcessorMessageHandler>,
}

impl PipelineActorRef {
    pub fn new<A>(actor_ref: ActorRef<A>) -> Self
    where
        A: Actor<Msg = ProcessorMsg> + 'static,
    {
        struct Handler<A: Actor<Msg = ProcessorMsg>> {
            actor_ref: ActorRef<A>,
        }

        #[async_trait::async_trait]
        impl<A: Actor<Msg = ProcessorMsg> + 'static> ProcessorMessageHandler for Handler<A> {
            fn cast_processor_msg(&self, msg: ProcessorMsg) -> Result<(), MessagingErr> {
                self.actor_ref.cast(msg)
            }

            async fn link_next_sync(&self, next: PipelineActorRef) -> Result<(), MessagingErr> {
                use ractor::rpc::call;
                call::<A, (), _>(&self.actor_ref, |reply| ProcessorMsg::LinkNextSync { next, reply }, None)
                    .await?;
                Ok(())
            }
        }

        Self {
            handler: std::sync::Arc::new(Handler { actor_ref }),
        }
    }

    pub fn cast(&self, msg: ProcessorMsg) -> Result<(), MessagingErr> {
        self.handler.cast_processor_msg(msg)
    }

    /// Synchronously link to the next processor and wait for confirmation
    pub async fn link_next_sync(&self, next: PipelineActorRef) -> Result<(), MessagingErr> {
        self.handler.link_next_sync(next).await
    }
}

impl std::fmt::Debug for PipelineActorRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineActorRef").finish()
    }
}

/// Message to process a frame
#[derive(Clone, Debug)]
pub struct ProcessFrame {
    pub frame: Frame,
    pub direction: FrameDirection,
}

/// Message to link processors together
#[derive(Clone, Debug)]
pub struct LinkNext {
    pub next: ActorRef<ProcessorActor>,
}

/// Message to get processor status
#[derive(Clone, Debug)]
pub struct GetStatus;

#[derive(Debug, Clone)]
pub struct ProcessorStatus {
    pub name: String,
    pub frames_processed: u64,
    pub is_running: bool,
}

/// Configuration for setting up processors
#[derive(Debug, Clone)]
pub struct ProcessorSetup {
    pub audio_in_sample_rate: u32,
    pub audio_out_sample_rate: u32,
    pub allow_interruptions: bool,
    pub enable_metrics: bool,
}

impl Default for ProcessorSetup {
    fn default() -> Self {
        Self {
            audio_in_sample_rate: 16000,
            audio_out_sample_rate: 16000,
            allow_interruptions: true,
            enable_metrics: false,
        }
    }
}

/// Trait for processor behavior - implement this for custom processors
#[async_trait::async_trait]
pub trait ProcessorBehavior: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn process(&mut self, frame: Frame) -> Option<Frame>;

    /// Called when the processor is being set up (before pipeline starts)
    async fn setup(&mut self, _setup: &ProcessorSetup) {}

    /// Called when the processor is being torn down (after pipeline stops)
    async fn cleanup(&mut self) {}

    /// Called when the pipeline starts processing
    async fn on_start(&mut self, _sample_rate: u32) {}

    /// Called when the pipeline stops processing
    async fn on_stop(&mut self) {}
}

pub struct ProcessorActor;

pub struct ProcessorState {
    behavior: Box<dyn ProcessorBehavior>,
    next: Option<PipelineActorRef>,
    frames_processed: u64,
    is_running: bool,
    cancelling: bool,
}

impl ProcessorState {
    pub fn new<B: ProcessorBehavior>(behavior: B) -> Self {
        Self {
            behavior: Box::new(behavior),
            next: None,
            frames_processed: 0,
            is_running: false,
            cancelling: false,
        }
    }

    pub fn new_boxed(behavior: Box<dyn ProcessorBehavior>) -> Self {
        Self {
            behavior,
            next: None,
            frames_processed: 0,
            is_running: false,
            cancelling: false,
        }
    }
}

#[async_trait::async_trait]
impl Actor for ProcessorActor {
    type Msg = ProcessorMsg;
    type State = ProcessorState;
    type Arguments = ProcessorState;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        debug!("[{}] Actor started", args.behavior.name());
        Ok(args)
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        debug!("[{}] Actor stopped", state.behavior.name());
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
                // Upstream system frames should not be processed or forwarded by regular processors
                if direction == FrameDirection::Upstream {
                    return Ok(());
                }

                debug!(
                    "[{}] Processing {} (id: {})",
                    state.behavior.name(),
                    frame.name(),
                    frame.id()
                );
                match &frame {
                    Frame::Start {
                        audio_in_sample_rate,
                        ..
                    } => {
                        state.is_running = true;
                        state.cancelling = false;
                        state.behavior.on_start(*audio_in_sample_rate).await;
                    }
                    Frame::End { .. } => {
                        state.is_running = false;
                        state.cancelling = false;
                        state.behavior.on_stop().await;
                    }
                    Frame::Cancel { .. } => {
                        state.cancelling = true;
                        state.is_running = false;
                    }
                    _ => {}
                }
                if let Some(output_frame) = state.behavior.process(frame).await {
                    state.frames_processed += 1;
                    if let Some(ref next) = state.next {
                        let _ = next.cast(ProcessorMsg::ProcessFrame {
                            frame: output_frame,
                            direction,
                        })?;
                    }
                }
            }
            ProcessorMsg::LinkNext { next } => {
                debug!("[{}] Linked to next processor", state.behavior.name());
                state.next = Some(next);
            }
            ProcessorMsg::LinkNextSync { next, reply } => {
                debug!("[{}] Linked to next processor (sync)", state.behavior.name());
                state.next = Some(next);
                let _ = reply.send(());
            }
            ProcessorMsg::LinkPrevious { .. } => {}
            ProcessorMsg::Setup { setup } => {
                debug!("[{}] Setting up processor", state.behavior.name());
                state.behavior.setup(&setup).await;
            }
            ProcessorMsg::Cleanup => {
                debug!("[{}] Cleaning up processor", state.behavior.name());
                state.behavior.cleanup().await;
            }
            ProcessorMsg::GetStatus { reply } => {
                let status = ProcessorStatus {
                    name: state.behavior.name().to_string(),
                    frames_processed: state.frames_processed,
                    is_running: state.is_running,
                };
                let _ = reply.send(status);
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum ProcessorMsg {
    ProcessFrame {
        frame: Frame,
        direction: FrameDirection,
    },
    LinkNext {
        next: PipelineActorRef,
    },
    LinkNextSync {
        next: PipelineActorRef,
        reply: RpcReplyPort<()>,
    },
    LinkPrevious {
        previous: PipelineActorRef,
    },
    Setup {
        setup: ProcessorSetup,
    },
    Cleanup,
    GetStatus {
        reply: RpcReplyPort<ProcessorStatus>,
    },
}
