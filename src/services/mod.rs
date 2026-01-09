//! Services module - AI service integrations
//!
//! This module provides integrations with various AI services like
//! text-to-speech, speech-to-text, and LLM providers.

pub mod deepgram;
pub mod openai;

pub use deepgram::DeepgramTTSService;
pub use openai::{LLMService, OpenAILLMService};
