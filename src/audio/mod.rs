//! Audio processing module
//!
//! This module provides audio-related processors and utilities:
//!
//! - [`vad`]: Voice Activity Detection processors
//! - [`resampler`]: Audio resampling processors

pub mod vad;
pub mod resampler;

pub use vad::SileroVADProcessor;
pub use resampler::AudioResamplerProcessor;
