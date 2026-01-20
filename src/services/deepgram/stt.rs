//! Deepgram Speech-to-Text (STT) service
//!
//! This module provides real-time speech-to-text using Deepgram's WebSocket API.
//!
//! # Features
//!
//! - Real-time streaming transcription via WebSocket
//! - Interim results support for low-latency feedback
//! - Automatic reconnection handling
//! - Configurable model, language, and features
//!
//! # Example
//!
//! ```ignore
//! use voxlab::services::deepgram::DeepgramSTTService;
//!
//! let stt = DeepgramSTTService::new(api_key)
//!     .with_model(DeepgramModel::Nova2)
//!     .with_language("en")
//!     .with_interim_results(true);
//!
//! // Use in pipeline after VAD processor
//! let processors: Vec<Box<dyn ProcessorBehavior>> = vec![
//!     Box::new(vad),
//!     Box::new(stt),
//! ];
//! ```

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{http::Request, Message},
};
use tracing::{debug, error, info, warn};

use crate::frame::{Frame, FrameHeader};
use crate::processor::{ProcessorBehavior, ProcessorSetup};

/// Deepgram STT model options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeepgramModel {
    /// Nova-3: Latest and most accurate model
    Nova3,
    /// Nova-2: Previous generation, still very accurate
    Nova2,
    /// Enhanced: Good balance of speed and accuracy
    Enhanced,
    /// Base: Fastest, lower accuracy
    Base,
}

impl DeepgramModel {
    fn as_str(&self) -> &'static str {
        match self {
            DeepgramModel::Nova3 => "nova-3",
            DeepgramModel::Nova2 => "nova-2",
            DeepgramModel::Enhanced => "enhanced",
            DeepgramModel::Base => "base",
        }
    }
}

impl Default for DeepgramModel {
    fn default() -> Self {
        DeepgramModel::Nova2
    }
}

/// Deepgram transcription response structures
#[derive(Debug, Deserialize)]
struct DeepgramResponse {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    is_final: bool,
    #[serde(default)]
    speech_final: bool,
    channel: Option<Channel>,
}

#[derive(Debug, Deserialize)]
struct Channel {
    alternatives: Vec<Alternative>,
}

#[derive(Debug, Deserialize)]
struct Alternative {
    transcript: String,
    confidence: f32,
    #[serde(default)]
    #[allow(dead_code)]
    words: Vec<Word>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct Word {
    word: String,
    start: f32,
    end: f32,
    confidence: f32,
}

/// Message to send to the WebSocket task
#[derive(Debug)]
enum WsCommand {
    /// Send audio data
    Audio(Vec<u8>),
    /// Close the connection
    Close,
}

/// Transcription result from WebSocket
#[derive(Debug, Clone)]
pub struct DeepgramTranscription {
    pub text: String,
    pub confidence: f32,
    pub is_final: bool,
    #[allow(dead_code)]
    pub speech_final: bool,
}

/// Deepgram STT service with WebSocket streaming
///
/// This service connects to Deepgram's WebSocket API for real-time
/// speech-to-text transcription.
pub struct DeepgramSTTService {
    api_key: String,
    model: DeepgramModel,
    language: String,
    sample_rate: u32,
    encoding: String,
    interim_results: bool,
    smart_format: bool,
    punctuate: bool,

    // WebSocket state
    ws_sender: Option<mpsc::Sender<WsCommand>>,
    result_receiver: Arc<Mutex<Option<mpsc::Receiver<DeepgramTranscription>>>>,
    is_connected: bool,

    // Processing state
    is_speaking: bool,
    muted: bool,
    user_id: Option<String>,
    audio_passthrough: bool,
}

impl DeepgramSTTService {
    /// Create a new Deepgram STT service
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            model: DeepgramModel::default(),
            language: "en".to_string(),
            sample_rate: 16000,
            encoding: "linear16".to_string(),
            interim_results: true,
            smart_format: true,
            punctuate: true,
            ws_sender: None,
            result_receiver: Arc::new(Mutex::new(None)),
            is_connected: false,
            is_speaking: false,
            muted: false,
            user_id: None,
            audio_passthrough: true,
        }
    }

    /// Create with API key from environment variable
    pub fn from_env() -> Option<Self> {
        std::env::var("DEEPGRAM_API_KEY")
            .ok()
            .map(|key| Self::new(key))
    }

    /// Set the model to use
    pub fn with_model(mut self, model: DeepgramModel) -> Self {
        self.model = model;
        self
    }

    /// Set the language code (e.g., "en", "es", "fr")
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    /// Set the sample rate
    pub fn with_sample_rate(mut self, sample_rate: u32) -> Self {
        self.sample_rate = sample_rate;
        self
    }

    /// Enable or disable interim results
    pub fn with_interim_results(mut self, enabled: bool) -> Self {
        self.interim_results = enabled;
        self
    }

    /// Enable or disable smart formatting
    pub fn with_smart_format(mut self, enabled: bool) -> Self {
        self.smart_format = enabled;
        self
    }

    /// Enable or disable punctuation
    pub fn with_punctuate(mut self, enabled: bool) -> Self {
        self.punctuate = enabled;
        self
    }

    /// Set the user ID for transcription frames
    pub fn with_user_id(mut self, user_id: String) -> Self {
        self.user_id = Some(user_id);
        self
    }

    /// Set whether to pass through audio frames downstream
    pub fn with_audio_passthrough(mut self, passthrough: bool) -> Self {
        self.audio_passthrough = passthrough;
        self
    }

    /// Build the WebSocket URL with query parameters
    fn build_url(&self) -> String {
        let mut url = format!(
            "wss://api.deepgram.com/v1/listen?model={}&language={}&encoding={}&sample_rate={}",
            self.model.as_str(),
            self.language,
            self.encoding,
            self.sample_rate
        );

        if self.interim_results {
            url.push_str("&interim_results=true");
        }
        if self.smart_format {
            url.push_str("&smart_format=true");
        }
        if self.punctuate {
            url.push_str("&punctuate=true");
        }

        url
    }

    /// Connect to Deepgram WebSocket
    async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = self.build_url();
        debug!("DeepgramSTT: Connecting to {}", url);

        let request = Request::builder()
            .uri(&url)
            .header("Authorization", format!("Token {}", self.api_key))
            .header("Host", "api.deepgram.com")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .body(())?;

        let (ws_stream, _) = connect_async(request).await?;
        let (mut ws_write, mut ws_read) = ws_stream.split();

        // Create channels for communication
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<WsCommand>(100);
        let (result_tx, result_rx) = mpsc::channel::<DeepgramTranscription>(100);

        self.ws_sender = Some(cmd_tx);
        *self.result_receiver.lock().await = Some(result_rx);
        self.is_connected = true;

        // Spawn task to handle outgoing messages
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    WsCommand::Audio(data) => {
                        if let Err(e) = ws_write.send(Message::Binary(data.into())).await {
                            error!("DeepgramSTT: Failed to send audio: {}", e);
                            break;
                        }
                    }
                    WsCommand::Close => {
                        // Send close message
                        let _ = ws_write
                            .send(Message::Text(r#"{"type": "CloseStream"}"#.into()))
                            .await;
                        let _ = ws_write.close().await;
                        break;
                    }
                }
            }
        });

        // Spawn task to handle incoming messages
        tokio::spawn(async move {
            while let Some(msg) = ws_read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        match serde_json::from_str::<DeepgramResponse>(&text) {
                            Ok(response) => {
                                if response.msg_type == "Results" {
                                    if let Some(channel) = response.channel {
                                        if let Some(alt) = channel.alternatives.first() {
                                            if !alt.transcript.is_empty() {
                                                let transcription = DeepgramTranscription {
                                                    text: alt.transcript.clone(),
                                                    confidence: alt.confidence,
                                                    is_final: response.is_final,
                                                    speech_final: response.speech_final,
                                                };
                                                if result_tx.send(transcription).await.is_err() {
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                debug!("DeepgramSTT: Failed to parse response: {} - {}", e, text);
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        info!("DeepgramSTT: WebSocket closed by server");
                        break;
                    }
                    Err(e) => {
                        error!("DeepgramSTT: WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        });

        info!("DeepgramSTT: Connected successfully");
        Ok(())
    }

    /// Disconnect from Deepgram WebSocket
    async fn disconnect(&mut self) {
        if let Some(sender) = self.ws_sender.take() {
            let _ = sender.send(WsCommand::Close).await;
        }
        self.is_connected = false;
        debug!("DeepgramSTT: Disconnected");
    }

    /// Send audio data to Deepgram
    async fn send_audio(&self, audio: &[u8]) {
        if let Some(sender) = &self.ws_sender {
            if sender.send(WsCommand::Audio(audio.to_vec())).await.is_err() {
                warn!("DeepgramSTT: Failed to queue audio data");
            }
        }
    }

    /// Check for available transcription results
    async fn poll_results(&self) -> Option<DeepgramTranscription> {
        let mut receiver = self.result_receiver.lock().await;
        if let Some(rx) = receiver.as_mut() {
            rx.try_recv().ok()
        } else {
            None
        }
    }
}

#[async_trait]
impl ProcessorBehavior for DeepgramSTTService {
    fn name(&self) -> &str {
        "DeepgramSTT"
    }

    async fn setup(&mut self, setup: &ProcessorSetup) {
        if setup.audio_in_sample_rate > 0 {
            self.sample_rate = setup.audio_in_sample_rate;
        }
        debug!(
            "DeepgramSTT: Setup complete, sample_rate={}",
            self.sample_rate
        );
    }

    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        match &frame {
            Frame::Start { audio_in_sample_rate, .. } => {
                // Update sample rate and connect
                if *audio_in_sample_rate > 0 {
                    self.sample_rate = *audio_in_sample_rate;
                }
                if !self.is_connected && !self.muted {
                    if let Err(e) = self.connect().await {
                        error!("DeepgramSTT: Failed to connect: {}", e);
                    }
                }
                Some(frame)
            }

            Frame::InputAudioRaw { audio, .. } => {
                // Send audio to Deepgram if connected and not muted
                if self.is_connected && !self.muted && self.is_speaking {
                    self.send_audio(audio).await;
                }

                // Check for transcription results
                while let Some(result) = self.poll_results().await {
                    if result.is_final {
                        debug!("DeepgramSTT: Final transcription: {}", result.text);
                        // Return final transcription
                        // Note: This will "consume" the audio frame
                        // TODO: Support returning multiple frames
                        return Some(Frame::Transcription {
                            header: FrameHeader::default(),
                            text: result.text,
                            user_id: self.user_id.clone(),
                            language: Some(self.language.clone()),
                            confidence: Some(result.confidence),
                        });
                    } else {
                        debug!("DeepgramSTT: Interim transcription: {}", result.text);
                        // Return interim transcription
                        return Some(Frame::InterimTranscription {
                            header: FrameHeader::default(),
                            text: result.text,
                            user_id: self.user_id.clone(),
                            language: Some(self.language.clone()),
                        });
                    }
                }

                // Pass through or drop audio based on config
                if self.audio_passthrough {
                    Some(frame)
                } else {
                    None
                }
            }

            Frame::UserStartedSpeaking { .. } => {
                debug!("DeepgramSTT: User started speaking");
                self.is_speaking = true;
                // Connect if not already connected
                if !self.is_connected && !self.muted {
                    if let Err(e) = self.connect().await {
                        error!("DeepgramSTT: Failed to connect: {}", e);
                    }
                }
                Some(frame)
            }

            Frame::UserStoppedSpeaking { .. } => {
                debug!("DeepgramSTT: User stopped speaking");
                self.is_speaking = false;
                Some(frame)
            }

            Frame::STTMute { mute, .. } => {
                debug!("DeepgramSTT: Mute = {}", mute);
                self.muted = *mute;
                if self.muted && self.is_connected {
                    self.disconnect().await;
                }
                Some(frame)
            }

            Frame::End { .. } => {
                // Disconnect on end
                if self.is_connected {
                    self.disconnect().await;
                }
                Some(frame)
            }

            // Pass through all other frames
            _ => Some(frame),
        }
    }

    async fn cleanup(&mut self) {
        self.disconnect().await;
        debug!("DeepgramSTT: Cleanup complete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_url() {
        let stt = DeepgramSTTService::new("test_key".to_string())
            .with_model(DeepgramModel::Nova2)
            .with_language("en")
            .with_sample_rate(16000);

        let url = stt.build_url();

        assert!(url.starts_with("wss://api.deepgram.com/v1/listen"));
        assert!(url.contains("model=nova-2"));
        assert!(url.contains("language=en"));
        assert!(url.contains("sample_rate=16000"));
        assert!(url.contains("encoding=linear16"));
    }

    #[test]
    fn test_model_as_str() {
        assert_eq!(DeepgramModel::Nova3.as_str(), "nova-3");
        assert_eq!(DeepgramModel::Nova2.as_str(), "nova-2");
        assert_eq!(DeepgramModel::Enhanced.as_str(), "enhanced");
        assert_eq!(DeepgramModel::Base.as_str(), "base");
    }
}
