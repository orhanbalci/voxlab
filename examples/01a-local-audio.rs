//! Basic local audio example
//!
//! This example demonstrates a simple TTS pipeline:
//! 1. Text frames are sent to Deepgram TTS
//! 2. Deepgram returns audio frames
//! 3. Audio is played through local speakers
//!
//! Based on Pipecat's 01a-local-audio.py example.
//!
//! Usage:
//!   DEEPGRAM_API_KEY=your_api_key cargo run --example 01a-local-audio

use std::env;
use std::time::Duration;
use tokio::time::sleep;
use tracing::info;

use voxlab::frame::{Frame, FrameHeader};
use voxlab::pipeline::PipelineRunner;
use voxlab::processor::ProcessorSetup;
use voxlab::services::deepgram::{DeepgramTTSService, DeepgramVoice};
use voxlab::transport::local::LocalAudioTransportParams;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    info!("VoxLab Local Audio Example");

    let api_key = env::var("DEEPGRAM_API_KEY").expect(
        "DEEPGRAM_API_KEY environment variable is required.\n\
         Get your API key from https://console.deepgram.com/",
    );

    info!("Creating TTS service with Deepgram Aura");

    let tts = DeepgramTTSService::new(api_key, DeepgramVoice::AuraAsteriaEn);

    let params = LocalAudioTransportParams::new().with_output_sample_rate(48000);

    let setup = ProcessorSetup {
        audio_in_sample_rate: 16000,
        audio_out_sample_rate: 48000,
        allow_interruptions: true,
        enable_metrics: false,
    };

    info!("Starting pipeline");

    let task = PipelineRunner::run(vec![Box::new(tts)], params, setup).await?;

    sleep(Duration::from_millis(500)).await;

    info!("Sending text to TTS");

    task.queue_frames(vec![
        Frame::Start {
            header: FrameHeader::default(),
            audio_in_sample_rate: 16000,
            audio_out_sample_rate: 48000,
            allow_interruptions: true,
            enable_metrics: false,
        },
        Frame::Text {
            header: FrameHeader::default(),
            text: "Hello there! This is a test of the VoxLab pipeline with Deepgram text to speech. How are you doing today?".to_string(),
        },
        Frame::End {
            header: FrameHeader::default(),
        },
    ]);

    info!("Waiting for audio playback to complete");

    sleep(Duration::from_secs(10)).await;

    info!("Done!");

    Ok(())
}
