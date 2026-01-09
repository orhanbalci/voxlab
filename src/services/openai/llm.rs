//! LLM Service
//!
//! Large language model service using OpenAI-compatible Chat Completions API.
//! Works with OpenAI, Qwen (DashScope), OpenRouter, and other compatible providers.
//! Converts LLMMessages frames into Text frames by querying the API.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, error};

use crate::frame::{Frame, FrameHeader, LLMMessage, LLMRole};
use crate::processor::{ProcessorBehavior, ProcessorSetup};

/// OpenAI-compatible Chat Completions API request message
#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

impl From<&LLMMessage> for ChatMessage {
    fn from(msg: &LLMMessage) -> Self {
        ChatMessage {
            role: match msg.role {
                LLMRole::System => "system".to_string(),
                LLMRole::User => "user".to_string(),
                LLMRole::Assistant => "assistant".to_string(),
            },
            content: msg.content.clone(),
        }
    }
}

/// OpenAI-compatible Chat Completions API request
#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

/// OpenAI-compatible Chat Completions API response
#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ChatResponseMessage {
    content: String,
}

/// LLM Service
///
/// Converts LLMMessages frames to Text frames using an OpenAI-compatible
/// Chat Completions API. Works with OpenAI, Qwen (DashScope), and other
/// compatible providers.
///
/// # Example with OpenAI
///
/// ```ignore
/// let llm = LLMService::openai("your-api-key".to_string());
/// ```
///
/// # Example with Qwen (DashScope)
///
/// ```ignore
/// let llm = LLMService::qwen("your-dashscope-api-key".to_string());
/// ```
///
/// # Example with OpenRouter
///
/// ```ignore
/// let llm = LLMService::openrouter("your-openrouter-api-key".to_string());
/// ```
///
/// # Example with custom endpoint
///
/// ```ignore
/// let llm = LLMService::new(
///     "your-api-key".to_string(),
///     "https://custom-endpoint.com/v1/chat/completions".to_string(),
///     "model-name".to_string(),
/// );
/// ```
pub struct LLMService {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    name: String,
}

impl LLMService {
    /// Create a new LLM service with custom endpoint
    ///
    /// # Arguments
    ///
    /// * `api_key` - API key for the service
    /// * `base_url` - Base URL for the chat completions endpoint
    /// * `model` - Model name to use
    pub fn new(api_key: String, base_url: String, model: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url,
            model,
            max_tokens: None,
            temperature: None,
            name: "LLM".to_string(),
        }
    }

    /// Create a new LLM service for OpenAI
    ///
    /// Uses the OpenAI API endpoint with gpt-4o-mini as the default model.
    pub fn openai(api_key: String) -> Self {
        Self::new(
            api_key,
            "https://api.openai.com/v1/chat/completions".to_string(),
            "gpt-4o-mini".to_string(),
        )
        .with_name("OpenAILLM")
    }

    /// Create a new LLM service for Qwen (DashScope International)
    ///
    /// Uses the DashScope international endpoint with qwen-plus as the default model.
    pub fn qwen(api_key: String) -> Self {
        Self::new(
            api_key,
            "https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions".to_string(),
            "qwen-mt-lite".to_string(),
        )
        .with_name("QwenLLM")
    }

    /// Create a new LLM service for Qwen (DashScope China)
    ///
    /// Uses the DashScope China endpoint with qwen-plus as the default model.
    pub fn qwen_china(api_key: String) -> Self {
        Self::new(
            api_key,
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions".to_string(),
            "qwen-plus".to_string(),
        )
        .with_name("QwenLLM")
    }

    /// Create a new LLM service for OpenRouter
    ///
    /// OpenRouter provides access to hundreds of AI models through a single endpoint.
    /// Uses google/gemini-2.0-flash-001 as the default model.
    ///
    /// Get your API key from https://openrouter.ai/keys
    pub fn openrouter(api_key: String) -> Self {
        Self::new(
            api_key,
            "https://openrouter.ai/api/v1/chat/completions".to_string(),
            "google/gemini-2.0-flash-001".to_string(),
        )
        .with_name("OpenRouterLLM")
    }

    /// Set a custom name for the service (used in logs)
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// Set the model to use
    pub fn with_model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    /// Set the maximum tokens for the response
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set the temperature for response generation
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    async fn generate(&self, messages: &[LLMMessage]) -> Option<String> {
        debug!(
            "{}: Generating response for {} messages",
            self.name,
            messages.len()
        );

        let chat_messages: Vec<ChatMessage> = messages.iter().map(ChatMessage::from).collect();

        let request = ChatCompletionRequest {
            model: self.model.clone(),
            messages: chat_messages,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
        };

        let response = self
            .client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await;

        match response {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.json::<ChatCompletionResponse>().await {
                        Ok(completion) => {
                            if let Some(choice) = completion.choices.first() {
                                let content = choice.message.content.clone();
                                debug!("{}: Generated response: {}", self.name, content);
                                Some(content)
                            } else {
                                error!("{}: No choices in response", self.name);
                                None
                            }
                        }
                        Err(e) => {
                            error!("{}: Failed to parse response: {}", self.name, e);
                            None
                        }
                    }
                } else {
                    error!("{}: API error: {}", self.name, resp.status());
                    if let Ok(text) = resp.text().await {
                        error!("{}: Error body: {}", self.name, text);
                    }
                    None
                }
            }
            Err(e) => {
                error!("{}: Request failed: {}", self.name, e);
                None
            }
        }
    }
}

#[async_trait]
impl ProcessorBehavior for LLMService {
    fn name(&self) -> &str {
        &self.name
    }

    async fn process(&mut self, frame: Frame) -> Option<Frame> {
        match frame {
            Frame::LLMMessages { messages, .. } => {
                // Generate response from LLM
                if let Some(text) = self.generate(&messages).await {
                    Some(Frame::Text {
                        header: FrameHeader::default(),
                        text,
                    })
                } else {
                    error!("{}: Failed to generate response", self.name);
                    None
                }
            }
            // Pass through other frames unchanged
            _ => Some(frame),
        }
    }

    async fn setup(&mut self, _setup: &ProcessorSetup) {
        debug!("{}: Setup complete, model={}", self.name, self.model);
    }

    async fn cleanup(&mut self) {
        debug!("{}: Cleanup complete", self.name);
    }
}

// Backwards compatibility alias
pub type OpenAILLMService = LLMService;
