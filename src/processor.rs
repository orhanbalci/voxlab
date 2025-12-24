//! Processor trait, actor, and related messages for the Ractor pipeline

use crate::frame::{Frame, FrameDirection};
use ractor::{Actor, ActorProcessingErr, ActorRef, MessagingErr, RpcReplyPort};

/// A trait for any actor that can handle ProcessorMsg
/// This allows heterogeneous actor chains
pub trait ProcessorMessageHandler: Send + Sync {
    fn cast_processor_msg(&self, msg: ProcessorMsg) -> Result<(), MessagingErr>;
}

/// Wrapper for any actor reference that can handle ProcessorMsg
#[derive(Clone)]
pub struct PipelineActorRef {
    handler: std::sync::Arc<dyn ProcessorMessageHandler>,
}

impl PipelineActorRef {
    pub fn new<A>(actor_ref: ActorRef<A>) -> Self
    where
        A: Actor<Msg = ProcessorMsg>,
    {
        struct Handler<A: Actor<Msg = ProcessorMsg>> {
            actor_ref: ActorRef<A>,
        }

        impl<A: Actor<Msg = ProcessorMsg>> ProcessorMessageHandler for Handler<A> {
            fn cast_processor_msg(&self, msg: ProcessorMsg) -> Result<(), MessagingErr> {
                self.actor_ref.cast(msg)
            }
        }

        Self {
            handler: std::sync::Arc::new(Handler { actor_ref }),
        }
    }

    pub fn cast(&self, msg: ProcessorMsg) -> Result<(), MessagingErr> {
        self.handler.cast_processor_msg(msg)
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

/// Trait for processor behavior - implement this for custom processors
#[async_trait::async_trait]
pub trait ProcessorBehavior: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn process(&mut self, frame: Frame) -> Option<Frame>;
    async fn on_start(&mut self, _sample_rate: u32) {}
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
        println!("[{}] Actor started", args.behavior.name());
        Ok(args)
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        println!("[{}] Actor stopped", state.behavior.name());
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
                println!("[{}] Processing frame", state.behavior.name());
                // Handle system frames
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
                println!("[{}] Linked to next processor", state.behavior.name());
                state.next = Some(next);
            }
            ProcessorMsg::LinkPrevious { previous: _ } => {
                // Standard processors don't use previous links, only sinks do
                // This is a no-op for regular processors
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
    LinkPrevious {
        previous: PipelineActorRef,
    },
    GetStatus {
        reply: RpcReplyPort<ProcessorStatus>,
    },
}

// Message trait is automatically implemented via blanket impl for types that are Send + 'static
