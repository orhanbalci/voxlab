use std::sync::Arc;

use async_trait::async_trait;
use ractor::{Actor, ActorProcessingErr, ActorRef};
use tracing::debug;

use crate::{
    frame::{Frame, FrameDirection},
    processor::{PipelineActorRef, ProcessorBehavior, ProcessorMsg},
};

pub struct PipelineSink {
    name: String,
    downstream_handler: Arc<dyn Fn(Frame, FrameDirection) -> BoxedFuture + Send + Sync>,
}

type BoxedFuture = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

impl PipelineSink {
    pub fn new<F, Fut>(name: String, downstream_handler: F) -> Self
    where
        F: Fn(Frame, FrameDirection) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        Self {
            name,
            downstream_handler: Arc::new(move |frame, direction| {
                Box::pin(downstream_handler(frame, direction))
            }),
        }
    }
}

#[async_trait]
impl ProcessorBehavior for PipelineSink {
    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        (self.downstream_handler)(frame, FrameDirection::Downstream).await;
        None
    }
    fn name(&self) -> &str {
        &self.name
    }
}

pub struct PipelineSinkActor;

pub struct PipelineSinkState {
    behaviour: PipelineSink,
    previous: Option<PipelineActorRef>,
    is_running: bool,
    cancelling: bool,
}

impl PipelineSinkState {
    pub fn new(behaviour: PipelineSink) -> Self {
        Self {
            behaviour,
            previous: None,
            is_running: false,
            cancelling: false,
        }
    }

    pub fn with_previous(mut self, previous: PipelineActorRef) -> Self {
        self.previous = Some(previous);
        self
    }
}

#[async_trait::async_trait]
impl Actor for PipelineSinkActor {
    type Msg = ProcessorMsg;
    type State = PipelineSinkState;
    type Arguments = PipelineSinkState;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        debug!("[{}] Sink actor started", args.behaviour.name());
        Ok(args)
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self>,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        debug!("[{}] Sink actor stopped", state.behaviour.name());
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
                    Frame::Start { .. } => {
                        state.is_running = true;
                        state.cancelling = false;
                    }
                    Frame::End { .. } => {
                        state.is_running = false;
                        state.cancelling = false;
                    }
                    Frame::Cancel { .. } => {
                        state.cancelling = true;
                        state.is_running = false;
                    }
                    _ => {}
                }

                match direction {
                    FrameDirection::Downstream => {
                        state.behaviour.process(frame).await;
                    }
                    FrameDirection::Upstream => {
                        if let Some(ref prev) = state.previous {
                            let _ = prev.cast(ProcessorMsg::ProcessFrame { frame, direction });
                        } else {
                            debug!(
                                "[{}] No previous actor to push frame to",
                                state.behaviour.name()
                            );
                        }
                    }
                }
            }
            ProcessorMsg::LinkPrevious { previous } => {
                debug!("[{}] Linking to previous processor", state.behaviour.name());
                state.previous = Some(previous);
            }
            ProcessorMsg::Setup { setup } => {
                debug!("[{}] Setting up sink", state.behaviour.name());
                state.behaviour.setup(&setup).await;
            }
            ProcessorMsg::Cleanup => {
                debug!("[{}] Cleaning up sink", state.behaviour.name());
                state.behaviour.cleanup().await;
            }
            _ => {}
        }

        Ok(())
    }
}
