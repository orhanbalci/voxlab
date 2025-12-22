//! Kameo Actor-based Pipeline Example for AgentFlow
//!
//! This example demonstrates how to build a frame processing pipeline
//! using the Kameo actor framework.

mod frame;

use frame::{Frame, FrameDirection, FrameHeader};
use kameo::message::{Context, Message};
use kameo::request::MessageSend;
use kameo::Actor;
use tokio::sync::mpsc;

// ============================================================================
// Actor Messages
// ============================================================================

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

// ============================================================================
// Generic Processor Actor
// ============================================================================

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
    frames_processed: u64,
    is_running: bool,
}

impl ProcessorActor {
    pub fn new<B: ProcessorBehavior>(behavior: B) -> Self {
        Self {
            behavior: Box::new(behavior),
            next: None,
            frames_processed: 0,
            is_running: false,
        }
    }
}

impl Actor for ProcessorActor {
    type Mailbox = kameo::mailbox::unbounded::UnboundedMailbox<Self>;

    async fn on_start(&mut self, _actor_ref: kameo::actor::ActorRef<Self>) -> Result<(), kameo::error::BoxError> {
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
            Frame::Start { audio_in_sample_rate, .. } => {
                self.is_running = true;
                self.behavior.on_start(*audio_in_sample_rate).await;
            }
            Frame::End { .. } => {
                self.is_running = false;
                self.behavior.on_stop().await;
            }
            Frame::Cancel { .. } => {
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
        println!(
            "[{}] Linked to next processor",
            self.behavior.name()
        );
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

// ============================================================================
// Example Processor Implementations
// ============================================================================

/// Simple text processor that converts text to uppercase
pub struct UppercaseProcessor;

#[async_trait::async_trait]
impl ProcessorBehavior for UppercaseProcessor {
    fn name(&self) -> &str {
        "UppercaseProcessor"
    }

    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        match frame {
            Frame::Text { header, text } => Some(Frame::Text {
                header,
                text: text.to_uppercase(),
            }),
            Frame::LLMText{ header, text } => Some(Frame::LLMText {
                header,
                text: text.to_uppercase(),
            }),
            other => Some(other), // Pass through other frames
        }
    }
}

/// Simulated LLM processor
pub struct LLMProcessor {
    model_name: String,
}

impl LLMProcessor {
    pub fn new(model_name: &str) -> Self {
        Self {
            model_name: model_name.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl ProcessorBehavior for LLMProcessor {
    fn name(&self) -> &str {
        "LLMProcessor"
    }

    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        match frame {
            Frame::Text { header: _, text } => {
                // Simulate LLM processing
                println!("[LLM:{}] Processing: {}", self.model_name, text);
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                Some(Frame::LLMText {
                    header: FrameHeader::default(),
                    text: format!("LLM Response to: {}", text),
                })
            }
            other => Some(other),
        }
    }

    async fn on_start(&mut self, _sample_rate: u32) {
        println!("[LLM] Model {} initialized", self.model_name);
    }
}

/// Simulated TTS processor
pub struct TTSProcessor {
    sample_rate: u32,
}

impl TTSProcessor {
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }
}

#[async_trait::async_trait]
impl ProcessorBehavior for TTSProcessor {
    fn name(&self) -> &str {
        "TTSProcessor"
    }

    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        match frame {
            Frame::LLMText { header: _, text } | Frame::Text { header: _, text } => {
                // Simulate TTS processing
                println!("[TTS] Converting to speech: {}", text);
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

                // Generate fake audio data
                let fake_audio = vec![0u8; text.len() * 100];

                Some(Frame::TTSAudio {
                    header: FrameHeader::default(),
                    data: fake_audio,
                    sample_rate: self.sample_rate,
                    channels: 1,
                })
            }
            other => Some(other),
        }
    }
}

/// Output sink that collects processed frames
pub struct OutputSink {
    collected_frames: Vec<Frame>,
    output_tx: Option<mpsc::Sender<Frame>>,
}

impl OutputSink {
    pub fn new(output_tx: Option<mpsc::Sender<Frame>>) -> Self {
        Self {
            collected_frames: Vec::new(),
            output_tx,
        }
    }
}

#[async_trait::async_trait]
impl ProcessorBehavior for OutputSink {
    fn name(&self) -> &str {
        "OutputSink"
    }

    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        println!("[Sink] Received: {} (id: {})", frame.name(), frame.id());
        self.collected_frames.push(frame.clone());

        if let Some(ref tx) = self.output_tx {
            let _ = tx.send(frame).await;
        }

        None // Sink doesn't forward frames
    }

    async fn on_stop(&mut self) {
        println!(
            "[Sink] Total frames collected: {}",
            self.collected_frames.len()
        );
    }
}

// ============================================================================
// Pipeline Builder
// ============================================================================

pub struct Pipeline {
    processors: Vec<kameo::actor::ActorRef<ProcessorActor>>,
}

impl Pipeline {
    /// Build a pipeline from a list of processors
    pub async fn new(behaviors: Vec<Box<dyn ProcessorBehavior>>) -> Self {
        let mut processors = Vec::new();

        // Spawn all processor actors
        for behavior in behaviors {
            let actor = ProcessorActor {
                behavior,
                next: None,
                frames_processed: 0,
                is_running: false,
            };
            let actor_ref = kameo::spawn(actor);
            processors.push(actor_ref);
        }

        // Link processors in chain
        for i in 0..processors.len().saturating_sub(1) {
            let next = processors[i + 1].clone();
            let _ = processors[i].tell(LinkNext { next }).await;
        }

        Self { processors }
    }

    /// Send a frame into the pipeline
    pub async fn send(&self, frame: Frame) -> Result<(), kameo::error::SendError<ProcessFrame, kameo::error::Infallible>> {
        if let Some(first) = self.processors.first() {
            first
                .tell(ProcessFrame {
                    frame,
                    direction: FrameDirection::Downstream,
                })
                .await
        } else {
            Ok(())
        }
    }

    /// Get status of all processors
    pub async fn status(&self) -> Vec<ProcessorStatus> {
        let mut statuses = Vec::new();
        for processor in &self.processors {
            if let Ok(status) = processor.ask(GetStatus).send().await {
                statuses.push(status);
            }
        }
        statuses
    }

    /// Gracefully stop the pipeline
    pub async fn stop(&self) {
        // Send End frame through pipeline
        let _ = self
            .send(Frame::End {
                header: FrameHeader::default(),
            })
            .await;

        // Give time for processing
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Stop all actors
        for processor in &self.processors {
            processor.stop_gracefully().await.ok();
        }
    }
}

// ============================================================================
// Main Example
// ============================================================================

#[tokio::main]
async fn main() {
    println!("=== AgentFlow Kameo Pipeline Example ===\n");

    // Create output channel to receive processed frames
    let (output_tx, mut output_rx) = mpsc::channel::<Frame>(100);

    // Build the pipeline: Input -> LLM -> TTS -> Output
    let pipeline = Pipeline::new(vec![
        Box::new(UppercaseProcessor),
        Box::new(LLMProcessor::new("gpt-4")),
        Box::new(TTSProcessor::new(24000)),
        Box::new(OutputSink::new(Some(output_tx))),
    ])
    .await;

    // Spawn a task to collect output
    let _output_handle = tokio::spawn(async move {
        let mut received = Vec::new();
        while let Some(frame) = output_rx.recv().await {
            received.push(frame);
        }
        received
    });

    // Send Start frame
    println!("\n--- Sending Start Frame ---");
    pipeline
        .send(Frame::Start {
            header: FrameHeader::default(),
            audio_in_sample_rate: 16000,
            audio_out_sample_rate: 24000,
            allow_interruptions: true,
            enable_metrics: false,
        })
        .await
        .unwrap();

    // Send some text frames
    println!("\n--- Sending Text Frames ---");
    for text in &["Hello world", "How are you?", "Goodbye!"] {
        pipeline
            .send(Frame::Text {
                header: FrameHeader::default(),
                text: text.to_string(),
            })
            .await
            .unwrap();
    }

    // Wait for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Get pipeline status
    println!("\n--- Pipeline Status ---");
    for status in pipeline.status().await {
        println!(
            "  {}: {} frames processed, running: {}",
            status.name, status.frames_processed, status.is_running
        );
    }

    // Stop the pipeline
    println!("\n--- Stopping Pipeline ---");
    pipeline.stop().await;

    // Check collected output
    drop(pipeline); // Drop to close channels

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    println!("\n=== Example Complete ===");
}