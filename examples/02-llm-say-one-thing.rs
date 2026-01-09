//! LLM Say One Thing Example
//!
//! This example demonstrates an LLM-powered TTS pipeline:
//! 1. LLMMessages frames are sent to OpenRouter LLM
//! 2. DeepSeek generates a text response
//! 3. Text is sent to Deepgram TTS
//! 4. Audio is played through local speakers
//!
//! Based on Pipecat's 02-llm-say-one-thing.py example.
//!
//! Usage:
//!   OPENROUTER_API_KEY=your_key DEEPGRAM_API_KEY=your_key cargo run --example 02-llm-say-one-thing

use std::env;
use std::time::Duration;
use tokio::time::sleep;
use tracing::info;

use voxlab::frame::{Frame, FrameHeader, LLMMessage, LLMRole};
use voxlab::pipeline::PipelineRunner;
use voxlab::processor::ProcessorSetup;
use voxlab::services::deepgram::{DeepgramTTSService, DeepgramVoice};
use voxlab::services::openai::LLMService;
use voxlab::transport::local::LocalAudioTransportParams;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    info!("VoxLab LLM Say One Thing Example");

    let openrouter_api_key = env::var("OPENROUTER_API_KEY").expect(
        "OPENROUTER_API_KEY environment variable is required.\n\
         Get your API key from https://openrouter.ai/keys",
    );

    let deepgram_api_key = env::var("DEEPGRAM_API_KEY").expect(
        "DEEPGRAM_API_KEY environment variable is required.\n\
         Get your API key from https://console.deepgram.com/",
    );

    info!("Creating LLM service with OpenRouter (DeepSeek)");
    let llm = LLMService::openrouter(openrouter_api_key)
        .with_model("deepseek/deepseek-r1-0528:free".to_string());

    info!("Creating TTS service with Deepgram Aura");
    let tts = DeepgramTTSService::new(deepgram_api_key, DeepgramVoice::AuraAsteriaEn);

    let params = LocalAudioTransportParams::new().with_output_sample_rate(48000);

    let setup = ProcessorSetup {
        audio_in_sample_rate: 16000,
        audio_out_sample_rate: 48000,
        allow_interruptions: true,
        enable_metrics: false,
    };

    info!("Starting pipeline: LLMMessages -> DeepSeek -> Text -> Deepgram TTS -> Audio");

    // Pipeline: LLM -> TTS
    let task = PipelineRunner::run(vec![Box::new(llm), Box::new(tts)], params, setup).await?;

    sleep(Duration::from_millis(500)).await;

    info!("Sending messages to LLM");

    // Create the conversation messages
    let messages = vec![
        LLMMessage {
            role: LLMRole::System,
            content: "You are a helpful assistant. Keep your responses brief and conversational."
                .to_string(),
        },
        LLMMessage {
            role: LLMRole::User,
            content: "Say hello and tell me one interesting fact about Rust programming language."
                .to_string(),
        },
    ];

    task.queue_frames(vec![
        Frame::Start {
            header: FrameHeader::default(),
            audio_in_sample_rate: 16000,
            audio_out_sample_rate: 48000,
            allow_interruptions: true,
            enable_metrics: false,
        },
        Frame::LLMMessages {
            header: FrameHeader::default(),
            messages,
        },
    ]);

    info!("Waiting for LLM response and audio playback");

    task.wait().await;

    // // Give more time for LLM call + TTS + playback
    // sleep(Duration::from_secs(15)).await;

    // task.queue_frames(vec![
    //     Frame::End {
    //         header: FrameHeader::default(),
    //     },
    // ]);

    info!("Done!");

    Ok(())
}
