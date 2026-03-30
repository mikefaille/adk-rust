use thiserror::Error;

#[derive(Error, Debug)]
pub enum LiveKitError {
    #[error("Failed to connect to room: {0}")]
    ConnectionFailed(String),
    #[error("Invalid access token")]
    TokenError,
    #[error("Room disconnected unexpectedly")]
    RoomDisconnected,
    #[error("General LiveKit error: {0}")]
    General(String),
}

pub type Result<T> = std::result::Result<T, LiveKitError>;
