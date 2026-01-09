//! LLM service integrations
//!
//! This module provides integrations with OpenAI-compatible LLM APIs
//! including OpenAI, Qwen (DashScope), and other compatible providers.

mod llm;

pub use llm::{LLMService, OpenAILLMService};
