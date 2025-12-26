//! Deepgram service integrations
//!
//! This module provides integrations with Deepgram's AI services
//! including text-to-speech (Aura TTS).

mod tts;

pub use tts::{DeepgramTTSService, DeepgramVoice};
