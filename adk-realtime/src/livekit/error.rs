//! Error types for the LiveKit integration.

use livekit::prelude::RoomError;
use livekit_api::access_token::AccessTokenError;
use thiserror::Error;

/// Specific error conditions encountered within the LiveKit bridge.
#[derive(Debug, Error)]
pub enum LiveKitError {
    /// Error originating from the LiveKit room connection, network stack, or tracks.
    #[error(transparent)]
    Connection(#[from] RoomError),

    /// Error related to generating or parsing a LiveKit access token.
    #[error(transparent)]
    Token(#[from] AccessTokenError),

    /// Error related to invalid configuration or builder inputs.
    #[error("Invalid configuration: {0}")]
    Config(String),

    /// Error occurring during the actual audio frame bridging sequence.
    #[error("Audio bridge failure: {0}")]
    Bridge(String),
}

/// A specialized Result type for LiveKit bridge operations.
pub type Result<T> = std::result::Result<T, LiveKitError>;
