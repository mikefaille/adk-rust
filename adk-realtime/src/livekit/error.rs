use thiserror::Error;

/// Error type for LiveKit bridging operations.
#[derive(Debug, Error)]
pub enum LiveKitError {
    #[error("LiveKit configuration error: {0}")]
    ConfigError(String),
    #[error(transparent)]
    TokenGenerationError(Box<livekit_api::access_token::AccessTokenError>),
    #[error(transparent)]
    ConnectionError(Box<livekit::prelude::RoomError>),
}

/// Manually implement `From` to box the inner error and avoid `clippy::result_large_err`.
impl From<livekit_api::access_token::AccessTokenError> for LiveKitError {
    fn from(err: livekit_api::access_token::AccessTokenError) -> Self {
        LiveKitError::TokenGenerationError(Box::new(err))
    }
}

/// Manually implement `From` to box the inner error and avoid `clippy::result_large_err`.
impl From<livekit::prelude::RoomError> for LiveKitError {
    fn from(err: livekit::prelude::RoomError) -> Self {
        LiveKitError::ConnectionError(Box::new(err))
    }
}
