//! Real-time transcription example
//!
//! This example demonstrates a speech-to-text pipeline:
//! 1. Audio is captured from the microphone
//! 2. VAD detects when the user is speaking
//! 3. Deepgram STT transcribes the speech in real-time
//! 4. Transcriptions are printed to the console
//!
//! Based on Pipecat's 13b-deepgram-transcription.py example.
//!
//! Usage:
//!   DEEPGRAM_API_KEY=your_api_key cargo run --example 03-transcription

use std::env;
use tracing::info;

use voxlab::frame::{Frame, FrameHeader};
use voxlab::pipeline::PipelineRunner;
use voxlab::processor::{ProcessorBehavior, ProcessorSetup};
use voxlab::audio::SileroVADProcessor;
use voxlab::services::deepgram::{DeepgramModel, DeepgramSTTService};
use voxlab::transport::local::LocalAudioTransportParams;

/// Simple processor that logs transcription results
struct TranscriptionLogger;

#[async_trait::async_trait]
impl ProcessorBehavior for TranscriptionLogger {
    fn name(&self) -> &str {
        "TranscriptionLogger"
    }

    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        match &frame {
            Frame::Transcription { text, confidence, .. } => {
                let conf = confidence.unwrap_or(0.0);
                println!("\n📝 [Final] {}", text);
                println!("   Confidence: {:.1}%", conf * 100.0);
                Some(frame)
            }
            Frame::InterimTranscription { text, .. } => {
                print!("\r🎤 [Interim] {}                    ", text);
                Some(frame)
            }
            Frame::UserStartedSpeaking { .. } => {
                println!("\n🎙️  User started speaking...");
                Some(frame)
            }
            Frame::UserStoppedSpeaking { .. } => {
                println!("🔇 User stopped speaking, transcribing...");
                Some(frame)
            }
            _ => Some(frame),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!("VoxLab Real-Time Transcription Example");

    let api_key = env::var("DEEPGRAM_API_KEY").expect(
        "DEEPGRAM_API_KEY environment variable is required.\n\
         Get your API key from https://console.deepgram.com/",
    );

    println!("\n🎤 Starting real-time transcription...");
    println!("   Speak into your microphone. Press Ctrl+C to stop.\n");

    // Create VAD processor for detecting speech
    let vad = SileroVADProcessor::new()
        .with_threshold(0.5)
        .with_min_speech_duration_ms(250)
        .with_min_silence_duration_ms(500);

    // Create Deepgram STT service
    let stt = DeepgramSTTService::new(api_key)
        .with_model(DeepgramModel::Nova2)
        .with_language("en")
        .with_interim_results(true)
        .with_smart_format(true);

    // Create transcription logger
    let logger = TranscriptionLogger;

    // Create audio resampler to convert from device sample rate to 16kHz
    // This is essential because:
    // - Most devices capture at 44100 Hz or 48000 Hz
    // - Silero VAD only supports 8kHz or 16kHz
    // - Without resampling, VAD won't detect speech properly
    let resampler = voxlab::audio::AudioResamplerProcessor::new()
        .with_target_sample_rate(16000);

    // Configure local audio transport for microphone input only
    let params = LocalAudioTransportParams {
        base: voxlab::transport::TransportParams::default()
            .with_audio_input()
            .with_audio_in_sample_rate(16000),  // Requested rate (device may use different)
        input_device_name: None,
        output_device_name: None,
    };

    let setup = ProcessorSetup {
        audio_in_sample_rate: 16000,  // Expected rate for VAD
        audio_out_sample_rate: 16000,
        allow_interruptions: true,
        enable_metrics: false,
    };

    info!("Starting pipeline with Resampler -> VAD -> Deepgram STT");

    // Build the pipeline: Input -> Resampler -> VAD -> STT -> Logger
    let processors: Vec<Box<dyn ProcessorBehavior>> = vec![
        Box::new(resampler),
        Box::new(vad),
        Box::new(stt),
        Box::new(logger),
    ];

    let task = PipelineRunner::run(processors, params, setup).await?;

    // Queue start frame to begin processing
    task.queue_frames(vec![Frame::Start {
        header: FrameHeader::default(),
        audio_in_sample_rate: 16000,  // VAD requires 16kHz
        audio_out_sample_rate: 16000,
        allow_interruptions: true,
        enable_metrics: false,
    }]);

    // Run indefinitely until interrupted
    println!("\n✅ Pipeline running. Speak into your microphone!\n");

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;

    println!("\n\n🛑 Shutting down...");

    // Send end frame
    task.queue_frames(vec![Frame::End {
        header: FrameHeader::default(),
    }]);

    // Give time for cleanup
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    println!("👋 Goodbye!");

    Ok(())
}
