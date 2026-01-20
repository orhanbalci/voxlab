//! Deepgram service integrations
//!
//! This module provides integrations with Deepgram's AI services
//! including text-to-speech (Aura TTS) and speech-to-text (STT).

mod stt;
mod tts;

pub use stt::{DeepgramModel, DeepgramSTTService};
pub use tts::{DeepgramTTSService, DeepgramVoice};
