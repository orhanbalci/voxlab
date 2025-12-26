//! Transport error types

use thiserror::Error;

/// Errors that can occur in transport operations
#[derive(Debug, Error)]
pub enum TransportError {
    /// Transport is not connected
    #[error("Transport not connected")]
    NotConnected,

    /// Audio-related error
    #[error("Audio error: {0}")]
    AudioError(String),

    /// Video-related error
    #[error("Video error: {0}")]
    VideoError(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Error sending data
    #[error("Send error: {0}")]
    SendError(String),

    /// Error receiving data
    #[error("Receive error: {0}")]
    ReceiveError(String),

    /// Timeout error
    #[error("Timeout: {0}")]
    Timeout(String),

    /// Actor messaging error
    #[error("Messaging error: {0}")]
    MessagingError(String),

    /// Generic transport error
    #[error("Transport error: {0}")]
    Other(String),
}

impl From<ractor::MessagingErr> for TransportError {
    fn from(err: ractor::MessagingErr) -> Self {
        TransportError::MessagingError(err.to_string())
    }
}

impl From<ractor::SpawnErr> for TransportError {
    fn from(err: ractor::SpawnErr) -> Self {
        TransportError::Other(format!("Failed to spawn actor: {}", err))
    }
}
