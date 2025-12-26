//! Deepgram TTS Service
//!
//! Text-to-speech service using Deepgram's Aura API.
//! Converts text frames into audio frames using Deepgram's neural voices.

use async_trait::async_trait;
use reqwest::Client;
use tracing::{debug, error};

use crate::frame::Frame;
use crate::processor::{ProcessorBehavior, ProcessorSetup};

/// Deepgram Aura voice models
#[derive(Debug, Clone)]
pub enum DeepgramVoice {
    /// Female, warm voice
    AuraAsteriaEn,
    /// Female, soft voice
    AuraLunaEn,
    /// Female, confident voice
    AuraStellaEn,
    /// Male, clear voice
    AuraOrionEn,
    /// Male, deep voice
    AuraArcasEn,
    /// Custom voice ID
    Custom(String),
}

impl DeepgramVoice {
    fn as_str(&self) -> &str {
        match self {
            DeepgramVoice::AuraAsteriaEn => "aura-asteria-en",
            DeepgramVoice::AuraLunaEn => "aura-luna-en",
            DeepgramVoice::AuraStellaEn => "aura-stella-en",
            DeepgramVoice::AuraOrionEn => "aura-orion-en",
            DeepgramVoice::AuraArcasEn => "aura-arcas-en",
            DeepgramVoice::Custom(id) => id,
        }
    }
}

impl Default for DeepgramVoice {
    fn default() -> Self {
        DeepgramVoice::AuraAsteriaEn
    }
}

/// Deepgram TTS Service
///
/// Converts text frames to audio using Deepgram's Aura text-to-speech API.
/// The service makes HTTP requests to Deepgram's REST API and returns
/// linear16 PCM audio data.
///
/// # Example
///
/// ```ignore
/// let tts = DeepgramTTSService::new(
///     "your-api-key".to_string(),
///     DeepgramVoice::AuraAsteriaEn,
/// );
///
/// // Use in a pipeline
/// let task = PipelineRunner::run(vec![Box::new(tts)], params, setup).await?;
/// ```
pub struct DeepgramTTSService {
    client: Client,
    api_key: String,
    voice: DeepgramVoice,
    sample_rate: u32,
}

impl DeepgramTTSService {
    /// Create a new Deepgram TTS service
    ///
    /// # Arguments
    ///
    /// * `api_key` - Deepgram API key
    /// * `voice` - Voice to use for synthesis
    pub fn new(api_key: String, voice: DeepgramVoice) -> Self {
        Self {
            client: Client::new(),
            api_key,
            voice,
            sample_rate: 24000,
        }
    }

    /// Create a new Deepgram TTS service with default voice
    pub fn with_api_key(api_key: String) -> Self {
        Self::new(api_key, DeepgramVoice::default())
    }

    /// Set the sample rate for audio output
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    async fn synthesize(&self, text: &str) -> Option<Vec<u8>> {
        debug!("DeepgramTTS: Synthesizing text: {}", text);

        let url = format!(
            "https://api.deepgram.com/v1/speak?model={}&encoding=linear16&sample_rate={}&container=none",
            self.voice.as_str(),
            self.sample_rate
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Token {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "text": text }))
            .send()
            .await;

        match response {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.bytes().await {
                        Ok(bytes) => {
                            debug!("DeepgramTTS: Received {} bytes of audio", bytes.len());
                            Some(bytes.to_vec())
                        }
                        Err(e) => {
                            error!("DeepgramTTS: Failed to read response bytes: {}", e);
                            None
                        }
                    }
                } else {
                    error!("DeepgramTTS: API error: {}", resp.status());
                    if let Ok(text) = resp.text().await {
                        error!("DeepgramTTS: Error body: {}", text);
                    }
                    None
                }
            }
            Err(e) => {
                error!("DeepgramTTS: Request failed: {}", e);
                None
            }
        }
    }
}

#[async_trait]
impl ProcessorBehavior for DeepgramTTSService {
    fn name(&self) -> &str {
        "DeepgramTTS"
    }

    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        match frame {
            Frame::Text { text, header } => {
                // Synthesize text to audio
                if let Some(audio_data) = self.synthesize(&text).await {
                    Some(Frame::TTSAudio {
                        header,
                        data: audio_data,
                        sample_rate: self.sample_rate,
                        channels: 1,
                    })
                } else {
                    // Return an error frame or just drop
                    error!("DeepgramTTS: Failed to synthesize text");
                    None
                }
            }
            // Pass through other frames unchanged
            _ => Some(frame),
        }
    }

    async fn setup(&mut self, setup: &ProcessorSetup) {
        // Use the output sample rate from setup if available
        if setup.audio_out_sample_rate > 0 {
            self.sample_rate = setup.audio_out_sample_rate;
        }
        debug!(
            "DeepgramTTS: Setup complete, sample_rate={}",
            self.sample_rate
        );
    }

    async fn cleanup(&mut self) {
        debug!("DeepgramTTS: Cleanup complete");
    }
}
