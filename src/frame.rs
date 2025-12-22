//! Frame types for the pipeline

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

static FRAME_ID: AtomicU64 = AtomicU64::new(0);

fn next_frame_id() -> u64 {
    FRAME_ID.fetch_add(1, Ordering::Relaxed)
}

/// Common frame header - shared by all frames
#[derive(Debug, Clone)]
pub struct FrameHeader {
    pub id: u64,
    pub pts: Option<u64>,
    pub metadata: HashMap<String, String>,
    pub transport_source: Option<String>,
    pub transport_destination: Option<String>,
}

impl Default for FrameHeader {
    fn default() -> Self {
        Self {
            id: next_frame_id(),
            pts: None,
            metadata: HashMap::new(),
            transport_source: None,
            transport_destination: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageRawFrame {
    pub image: Vec<u8>,
    pub size: (u32, u32),
    pub format: Option<String>,
}

/// Simplified Frame enum for the pipeline
#[derive(Debug, Clone)]
pub enum Frame {
    Start {
        header: FrameHeader,
        audio_in_sample_rate: u32,
        audio_out_sample_rate: u32,
        allow_interruptions: bool,
        enable_metrics: bool,
    },
    Cancel {
        header: FrameHeader,
    },
    Error {
        header: FrameHeader,
        message: String,
        fatal: bool,
    },
    Text {
        header: FrameHeader,
        text: String,
    },
    LLMText {
        header: FrameHeader,
        text: String,
    },
    AudioOutput {
        header: FrameHeader,
        data: Vec<u8>,
        sample_rate: u32,
        channels: u16,
    },
    TTSAudio {
        header: FrameHeader,
        data: Vec<u8>,
        sample_rate: u32,
        channels: u16,
    },
    AudioRaw {
        header: FrameHeader,
        audio: Vec<u8>,
        sample_rate: u32,
        num_channels: u16,
        num_frames: usize,
    },
    ImageRaw {
        header: FrameHeader,
        image: Vec<u8>,
        size: (u32, u32),
        format: Option<String>,
    },
    Sprite {
        header: FrameHeader,
        images: Vec<ImageRawFrame>,
    },
    End {
        header: FrameHeader,
    },
    UserStartedSpeaking {
        header: FrameHeader,
        emulated: bool,
    },
    UserStoppedSpeaking {
        header: FrameHeader,
        emulated: bool,
    },
    BotStartedSpeaking {
        header: FrameHeader,
    },
    BotStoppedSpeaking {
        header: FrameHeader,
    },
    BotSpeaking {
        header: FrameHeader,
    },
    MixerControl {
        header: FrameHeader,
        enabled: Option<bool>,
        volume: Option<f32>,
        settings: HashMap<String, String>,
    },
    FilterControl {
        header: FrameHeader,
        enabled: Option<bool>,
        parameters: HashMap<String, String>,
        command: Option<String>,
    },
    MixerEnable {
        header: FrameHeader,
        enable: bool,
    },
    MixerUpdateSettings {
        header: FrameHeader,
        settings: HashMap<String, String>,
    },
    StartInterruption {
        header: FrameHeader,
    },
    StopInterruption {
        header: FrameHeader,
    },
    BotInterruption {
        header: FrameHeader,
    },
    EmulateUserStartedSpeaking {
        header: FrameHeader,
    },
    EmulateUserStoppedSpeaking {
        header: FrameHeader,
    },
    OutputTransportReady {
        header: FrameHeader,
    },
    TransportMessage {
        header: FrameHeader,
        message: String,
    },
    TransportMessageUrgent {
        header: FrameHeader,
        message: String,
    },
    Stop {
        header: FrameHeader,
    },
    InputAudioRaw {
        header: FrameHeader,
        audio: Vec<u8>,
        sample_rate: u32,
        num_channels: u32,
    },
    InputImageRaw {
        header: FrameHeader,
        image: Vec<u8>,
        size: (u32, u32),
        format: String,
    },
    LLMFullResponseStart {
        header: FrameHeader,
    },
    LLMFullResponseEnd {
        header: FrameHeader,
    },
}

impl Frame {
    pub fn id(&self) -> u64 {
        match self {
            Frame::Start { header, .. } => header.id,
            Frame::Cancel { header } => header.id,
            Frame::Error { header, .. } => header.id,
            Frame::Text { header, .. } => header.id,
            Frame::LLMText { header, .. } => header.id,
            Frame::AudioOutput { header, .. } => header.id,
            Frame::TTSAudio { header, .. } => header.id,
            Frame::AudioRaw { header, .. } => header.id,
            Frame::ImageRaw { header, .. } => header.id,
            Frame::Sprite { header, .. } => header.id,
            Frame::End { header } => header.id,
            Frame::UserStartedSpeaking { header, .. } => header.id,
            Frame::UserStoppedSpeaking { header, .. } => header.id,
            Frame::BotStartedSpeaking { header } => header.id,
            Frame::BotStoppedSpeaking { header } => header.id,
            Frame::BotSpeaking { header } => header.id,
            Frame::MixerControl { header, .. } => header.id,
            Frame::FilterControl { header, .. } => header.id,
            Frame::MixerEnable { header, .. } => header.id,
            Frame::MixerUpdateSettings { header, .. } => header.id,
            Frame::StartInterruption { header } => header.id,
            Frame::StopInterruption { header } => header.id,
            Frame::BotInterruption { header } => header.id,
            Frame::EmulateUserStartedSpeaking { header } => header.id,
            Frame::EmulateUserStoppedSpeaking { header } => header.id,
            Frame::OutputTransportReady { header } => header.id,
            Frame::TransportMessage { header, .. } => header.id,
            Frame::TransportMessageUrgent { header, .. } => header.id,
            Frame::Stop { header } => header.id,
            Frame::InputAudioRaw { header, .. } => header.id,
            Frame::InputImageRaw { header, .. } => header.id,
            Frame::LLMFullResponseStart { header } => header.id,
            Frame::LLMFullResponseEnd { header } => header.id,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Frame::Start { .. } => "StartFrame",
            Frame::Cancel { .. } => "CancelFrame",
            Frame::Error { .. } => "ErrorFrame",
            Frame::Text { .. } => "TextFrame",
            Frame::LLMText { .. } => "LLMTextFrame",
            Frame::AudioOutput { .. } => "AudioOutputFrame",
            Frame::TTSAudio { .. } => "TTSAudioFrame",
            Frame::AudioRaw { .. } => "AudioRawFrame",
            Frame::ImageRaw { .. } => "ImageRawFrame",
            Frame::Sprite { .. } => "SpriteFrame",
            Frame::End { .. } => "EndFrame",
            Frame::UserStartedSpeaking { .. } => "UserStartedSpeakingFrame",
            Frame::UserStoppedSpeaking { .. } => "UserStoppedSpeakingFrame",
            Frame::BotStartedSpeaking { .. } => "BotStartedSpeakingFrame",
            Frame::BotStoppedSpeaking { .. } => "BotStoppedSpeakingFrame",
            Frame::BotSpeaking { .. } => "BotSpeakingFrame",
            Frame::MixerControl { .. } => "MixerControlFrame",
            Frame::FilterControl { .. } => "FilterControlFrame",
            Frame::MixerEnable { .. } => "MixerEnableFrame",
            Frame::MixerUpdateSettings { .. } => "MixerUpdateSettingsFrame",
            Frame::StartInterruption { .. } => "StartInterruptionFrame",
            Frame::StopInterruption { .. } => "StopInterruptionFrame",
            Frame::BotInterruption { .. } => "BotInterruptionFrame",
            Frame::EmulateUserStartedSpeaking { .. } => "EmulateUserStartedSpeakingFrame",
            Frame::EmulateUserStoppedSpeaking { .. } => "EmulateUserStoppedSpeakingFrame",
            Frame::OutputTransportReady { .. } => "OutputTransportReadyFrame",
            Frame::TransportMessage { .. } => "TransportMessageFrame",
            Frame::TransportMessageUrgent { .. } => "TransportMessageUrgentFrame",
            Frame::Stop { .. } => "StopFrame",
            Frame::InputAudioRaw { .. } => "InputAudioRawFrame",
            Frame::InputImageRaw { .. } => "InputImageRawFrame",
            Frame::LLMFullResponseStart { .. } => "LLMFullResponseStartFrame",
            Frame::LLMFullResponseEnd { .. } => "LLMFullResponseEndFrame",
        }
    }

    pub fn is_system(&self) -> bool {
        matches!(
            self,
            Frame::Start { .. }
                | Frame::Cancel { .. }
                | Frame::Error { .. }
                | Frame::EmulateUserStartedSpeaking { .. }
                | Frame::EmulateUserStoppedSpeaking { .. }
                | Frame::BotStartedSpeaking { .. }
                | Frame::BotStoppedSpeaking { .. }
                | Frame::BotSpeaking { .. }
                | Frame::BotInterruption { .. }
                | Frame::StartInterruption { .. }
                | Frame::StopInterruption { .. }
                | Frame::UserStartedSpeaking { .. }
                | Frame::UserStoppedSpeaking { .. }
                | Frame::OutputTransportReady { .. }
                | Frame::TransportMessage { .. }
                | Frame::TransportMessageUrgent { .. }
        )
    }

    pub fn is_control(&self) -> bool {
        matches!(
            self,
            Frame::End { .. }
                | Frame::MixerControl { .. }
                | Frame::FilterControl { .. }
                | Frame::MixerEnable { .. }
                | Frame::MixerUpdateSettings { .. }
                | Frame::Stop { .. }
                | Frame::LLMFullResponseStart { .. }
                | Frame::LLMFullResponseEnd { .. }
        )
    }

    pub fn is_data(&self) -> bool {
        matches!(
            self,
            Frame::Text { .. }
                | Frame::LLMText { .. }
                | Frame::AudioOutput { .. }
                | Frame::TTSAudio { .. }
                | Frame::AudioRaw { .. }
                | Frame::ImageRaw { .. }
                | Frame::Sprite { .. }
                | Frame::InputAudioRaw { .. }
                | Frame::InputImageRaw { .. }
        )
    }

    pub fn family(&self) -> FrameFamily {
        if self.is_system() {
            FrameFamily::System
        } else if self.is_control() {
            FrameFamily::Control
        } else {
            FrameFamily::Data
        }
    }
}

/// Direction of frame flow in the pipeline
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDirection {
    Downstream,
    Upstream,
}

/// Frame family classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFamily {
    System,
    Control,
    Data,
}
