//! Processor trait, actor, and related messages for the Kameo pipeline

use crate::frame::{Frame, FrameDirection};
use kameo::message::{Context, Message};
use kameo::Actor;

/// Message to process a frame
#[derive(Clone)]
pub struct ProcessFrame {
    pub frame: Frame,
    pub direction: FrameDirection,
}

/// Message to link processors together
pub struct LinkNext {
    pub next: kameo::actor::ActorRef<ProcessorActor>,
}

/// Message to get processor status
pub struct GetStatus;

#[derive(Debug, Clone, kameo::Reply)]
pub struct ProcessorStatus {
    pub name: String,
    pub frames_processed: u64,
    pub is_running: bool,
}

/// Trait for processor behavior - implement this for custom processors
#[async_trait::async_trait]
pub trait ProcessorBehavior: Send + 'static {
    fn name(&self) -> &str;

    /// Process a frame, optionally returning a transformed frame
    async fn process(&mut self, frame: Frame) -> Option<Frame>;

    /// Called on Start frame
    async fn on_start(&mut self, _sample_rate: u32) {}

    /// Called on End frame
    async fn on_stop(&mut self) {}
}

/// Generic processor actor that wraps any ProcessorBehavior
pub struct ProcessorActor {
    behavior: Box<dyn ProcessorBehavior>,
    next: Option<kameo::actor::ActorRef<ProcessorActor>>,
    previous: Option<kameo::actor::ActorRef<ProcessorActor>>,
    frames_processed: u64,
    is_running: bool,
    cancelling: bool,
    allow_interruptions: bool,
    enable_metrics: bool,
    enable_usage_metrics: bool,
    report_only_initial_ttfb: bool,
}

impl ProcessorActor {
    pub fn new<B: ProcessorBehavior>(behavior: B) -> Self {
        Self {
            behavior: Box::new(behavior),
            next: None,
            previous: None,
            frames_processed: 0,
            is_running: false,
            cancelling: false,
            allow_interruptions: false,
            enable_metrics: false,
            enable_usage_metrics: false,
            report_only_initial_ttfb: false,
        }
    }

    /// Returns true if the processor is started (running)
    pub fn is_started(&self) -> bool {
        self.is_running
    }

    /// Returns true if the processor is in cancelling state
    pub fn is_cancelling(&self) -> bool {
        self.cancelling
    }

    pub fn behavior(&self) -> &dyn ProcessorBehavior {
        &*self.behavior
    }
    pub fn next(&self) -> Option<&kameo::actor::ActorRef<ProcessorActor>> {
        self.next.as_ref()
    }
    pub fn previous(&self) -> Option<&kameo::actor::ActorRef<ProcessorActor>> {
        self.previous.as_ref()
    }
    pub fn frames_processed(&self) -> u64 {
        self.frames_processed
    }
    pub fn allow_interruptions(&self) -> bool {
        self.allow_interruptions
    }
    pub fn enable_metrics(&self) -> bool {
        self.enable_metrics
    }
    pub fn enable_usage_metrics(&self) -> bool {
        self.enable_usage_metrics
    }
    pub fn report_only_initial_ttfb(&self) -> bool {
        self.report_only_initial_ttfb
    }
}

impl Actor for ProcessorActor {
    type Mailbox = kameo::mailbox::unbounded::UnboundedMailbox<Self>;

    async fn on_start(
        &mut self,
        _actor_ref: kameo::actor::ActorRef<Self>,
    ) -> Result<(), kameo::error::BoxError> {
        println!("[{}] Actor started", self.behavior.name());
        Ok(())
    }

    async fn on_stop(
        &mut self,
        _actor_ref: kameo::actor::WeakActorRef<Self>,
        _reason: kameo::error::ActorStopReason,
    ) -> Result<(), kameo::error::BoxError> {
        println!("[{}] Actor stopped", self.behavior.name());
        Ok(())
    }
}

// Handle ProcessFrame message
impl Message<ProcessFrame> for ProcessorActor {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: ProcessFrame,
        _ctx: Context<'_, Self, Self::Reply>,
    ) -> Self::Reply {
        let frame_name = msg.frame.name();
        let frame_id = msg.frame.id();
        println!(
            "[{}] Processing {} (id: {})",
            self.behavior.name(),
            frame_name,
            frame_id
        );

        // Handle system frames
        match &msg.frame {
            Frame::Start {
                audio_in_sample_rate,
                ..
            } => {
                self.is_running = true;
                self.cancelling = false;
                self.behavior.on_start(*audio_in_sample_rate).await;
            }
            Frame::End { .. } => {
                self.is_running = false;
                self.cancelling = false;
                self.behavior.on_stop().await;
            }
            Frame::Cancel { .. } => {
                self.cancelling = true;
                self.is_running = false;
            }
            _ => {}
        }

        // Process the frame
        if let Some(output_frame) = self.behavior.process(msg.frame).await {
            self.frames_processed += 1;

            // Forward to next processor if linked
            if let Some(ref next) = self.next {
                let _ = next
                    .tell(ProcessFrame {
                        frame: output_frame,
                        direction: msg.direction,
                    })
                    .await;
            }
        }
    }
}

// Handle LinkNext message
impl Message<LinkNext> for ProcessorActor {
    type Reply = ();

    async fn handle(&mut self, msg: LinkNext, _ctx: Context<'_, Self, Self::Reply>) -> Self::Reply {
        println!("[{}] Linked to next processor", self.behavior.name());
        self.next = Some(msg.next);
    }
}

// Handle GetStatus message
impl Message<GetStatus> for ProcessorActor {
    type Reply = ProcessorStatus;

    async fn handle(
        &mut self,
        _msg: GetStatus,
        _ctx: Context<'_, Self, Self::Reply>,
    ) -> Self::Reply {
        ProcessorStatus {
            name: self.behavior.name().to_string(),
            frames_processed: self.frames_processed,
            is_running: self.is_running,
        }
    }
}
