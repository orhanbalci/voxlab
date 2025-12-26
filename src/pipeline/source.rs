use std::sync::Arc;

use async_trait::async_trait;
use ractor::{Actor, ActorProcessingErr, ActorRef};
use tracing::debug;

use crate::{
    frame::{Frame, FrameDirection},
    processor::{PipelineActorRef, ProcessorBehavior, ProcessorMsg},
};

pub struct PipelineSource {
    name: String,
    upstream_handler: Arc<dyn Fn(Frame, FrameDirection) -> BoxedFuture + Send + Sync>,
}

type BoxedFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

impl PipelineSource {
    pub fn new<F, Fut>(name: String, upstream_handler: F) -> Self
    where
        F: Fn(Frame, FrameDirection) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        Self {
            name,
            upstream_handler: Arc::new(move |frame, direction| {
                Box::pin(upstream_handler(frame, direction))
            }),
        }
    }
}

#[async_trait]
impl ProcessorBehavior for PipelineSource {
    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        (self.upstream_handler)(frame, FrameDirection::Upstream).await;
        None
    }
    fn name(&self) -> &str {
        &self.name
    }
}

pub struct PipelineSourceActor;

pub struct PipelineSourceState {
    behaviour: PipelineSource,
    next: Option<PipelineActorRef>,
    is_running: bool,
    cancelling: bool,
}

impl PipelineSourceState {
    pub fn new(behaviour: PipelineSource) -> Self {
        Self {
            behaviour,
            next: None,
            is_running: false,
            cancelling: false,
        }
    }

    pub fn with_next(mut self, next: PipelineActorRef) -> Self {
        self.next = Some(next);
        self
    }
}

#[async_trait::async_trait]
impl Actor for PipelineSourceActor {
    type Msg = ProcessorMsg;
    type State = PipelineSourceState;
    type Arguments = PipelineSourceState;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        debug!("[{}] Source actor started", args.behaviour.name());
        Ok(args)
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        debug!("[{}] Source actor stopped", state.behaviour.name());
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
                debug!(
                    "[{}] Processing {} (id: {})",
                    state.behaviour.name(),
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
                        state.behaviour.on_start(*audio_in_sample_rate).await;
                    }
                    Frame::End { .. } => {
                        state.is_running = false;
                        state.cancelling = false;
                        state.behaviour.on_stop().await;
                    }
                    Frame::Cancel { .. } => {
                        state.cancelling = true;
                        state.is_running = false;
                    }
                    _ => {}
                }

                match direction {
                    FrameDirection::Upstream => {
                        state.behaviour.process(frame).await;
                    }
                    FrameDirection::Downstream => {
                        if let Some(ref next) = state.next {
                            let _ = next.cast(ProcessorMsg::ProcessFrame { frame, direction });
                        } else {
                            debug!(
                                "[{}] No next actor to push frame to",
                                state.behaviour.name()
                            );
                        }
                    }
                }
            }
            ProcessorMsg::LinkNext { next } => {
                debug!("[{}] Linking to next processor", state.behaviour.name());
                state.next = Some(next);
            }
            ProcessorMsg::Setup { setup } => {
                debug!("[{}] Setting up source", state.behaviour.name());
                state.behaviour.setup(&setup).await;
            }
            ProcessorMsg::Cleanup => {
                debug!("[{}] Cleaning up source", state.behaviour.name());
                state.behaviour.cleanup().await;
            }
            _ => {}
        }

        Ok(())
    }
}
