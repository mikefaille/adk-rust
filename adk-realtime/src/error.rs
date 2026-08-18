//! Error types for realtime operations.
//!
//! Managed [`crate::runner::RealtimeRunner`] writes add one important piece of
//! information that raw provider errors cannot provide by themselves:
//! [`DeliveryCertainty`]. A [`RealtimeError::WriteFailed`] tells callers whether
//! the managed layer rejected the operation before invoking the raw session or
//! whether provider invocation already occurred and the delivery outcome is
//! therefore indeterminate.
//!
//! This is a retry-safety boundary, not an exactly-once guarantee. Application
//! code should inspect [`RealtimeError::delivery_certainty`] before replaying a
//! failed side-effectful managed write. See `adk-realtime/MANAGED_RECOVERY.md`
//! for the full recovery and replay contract.

use crate::recovery::DeliveryCertainty;
use std::sync::Arc;
use thiserror::Error;

/// Result type for realtime operations.
pub type Result<T> = std::result::Result<T, RealtimeError>;

/// Errors that can occur during realtime operations.
#[derive(Error, Debug, Clone)]
pub enum RealtimeError {
    /// WebSocket connection error.
    #[error("WebSocket connection error: {0}")]
    ConnectionError(String),

    /// WebSocket message error.
    #[error("WebSocket message error: {0}")]
    MessageError(String),

    /// Protocol error (malformed provider payloads).
    #[error("Protocol error: {0}")]
    Protocol(String),

    /// Authentication error.
    #[error("Authentication error: {0}")]
    AuthError(String),

    /// Session not connected.
    #[error("Session not connected")]
    NotConnected,

    /// Session already closed.
    #[error("Session already closed")]
    SessionClosed,

    /// Invalid configuration.
    #[error("Invalid configuration: {0}")]
    ConfigError(String),

    /// Audio format error.
    #[error("Audio format error: {0}")]
    AudioFormatError(String),

    /// Tool execution error.
    #[error("Tool execution error: {0}")]
    ToolError(String),

    /// Server returned an error.
    #[error("Server error: {code} - {message}")]
    ServerError {
        /// Error code from the server.
        code: String,
        /// Error message from the server.
        message: String,
    },

    /// Timeout waiting for response.
    #[error("Timeout: {0}")]
    Timeout(String),

    /// Serialization/deserialization error.
    #[error("Serialization error: {0}")]
    SerializationError(Arc<serde_json::Error>),

    /// Provider-specific error.
    #[error("Provider error: {0}")]
    ProviderError(String),

    /// Generic IO error.
    #[error("IO error: {0}")]
    IoError(Arc<std::io::Error>),

    /// Opus codec error.
    #[error("Opus codec error: {0}")]
    OpusCodecError(String),

    /// WebRTC error.
    #[error("WebRTC error: {0}")]
    WebRTCError(String),

    /// LiveKit bridge error (legacy string format).
    #[error("LiveKit error: {0}")]
    LiveKitError(String),

    /// Native LiveKit component error.
    #[cfg(feature = "livekit")]
    #[error(transparent)]
    LiveKitNativeError(Arc<crate::livekit::LiveKitError>),

    /// Managed write failure with local provider-invocation certainty.
    ///
    /// - `NotAttempted`: the managed runner rejected the write before invoking
    ///   the raw provider session.
    /// - `Indeterminate`: the raw provider session was invoked, but peer
    ///   acceptance or processing is not known.
    ///
    /// Do not automatically replay side-effectful operations after an
    /// `Indeterminate` result.
    #[error("Write failed ({certainty:?}): {error}")]
    WriteFailed { error: Arc<RealtimeError>, certainty: DeliveryCertainty },
}

impl From<serde_json::Error> for RealtimeError {
    fn from(err: serde_json::Error) -> Self {
        RealtimeError::SerializationError(Arc::new(err))
    }
}

impl From<std::io::Error> for RealtimeError {
    fn from(err: std::io::Error) -> Self {
        RealtimeError::IoError(Arc::new(err))
    }
}

#[cfg(feature = "livekit")]
/// Manually implemented to wrap the inner error, keeping `Result` small on the happy path.
impl From<crate::livekit::LiveKitError> for RealtimeError {
    fn from(err: crate::livekit::LiveKitError) -> Self {
        RealtimeError::LiveKitNativeError(Arc::new(err))
    }
}

impl RealtimeError {
    /// Create a new connection error.
    pub fn connection<S: Into<String>>(msg: S) -> Self {
        Self::ConnectionError(msg.into())
    }

    /// Create a new server error.
    pub fn server<S: Into<String>>(code: S, message: S) -> Self {
        Self::ServerError { code: code.into(), message: message.into() }
    }

    /// Create a new provider error.
    pub fn provider<S: Into<String>>(msg: S) -> Self {
        Self::ProviderError(msg.into())
    }

    /// Create a new avatar provider error.
    ///
    /// Convenience constructor that prefixes the message with "avatar:"
    /// for clear identification in logs.
    #[cfg(feature = "video-avatar")]
    pub fn avatar<S: Into<String>>(msg: S) -> Self {
        Self::ProviderError(format!("avatar: {}", msg.into()))
    }

    /// Create a new configuration error.
    pub fn config<S: Into<String>>(msg: S) -> Self {
        Self::ConfigError(msg.into())
    }

    /// Create a new protocol error.
    pub fn protocol<S: Into<String>>(msg: S) -> Self {
        Self::Protocol(msg.into())
    }

    /// Create a new audio format error.
    pub fn audio<S: Into<String>>(msg: S) -> Self {
        Self::AudioFormatError(msg.into())
    }

    /// Create a new Opus codec error.
    pub fn opus(msg: impl Into<String>) -> Self {
        Self::OpusCodecError(msg.into())
    }

    /// Create a new WebRTC error.
    pub fn webrtc(msg: impl Into<String>) -> Self {
        Self::WebRTCError(msg.into())
    }

    /// Create a new LiveKit error.
    pub fn livekit(msg: impl Into<String>) -> Self {
        Self::LiveKitError(msg.into())
    }

    /// Create a managed write failure with the supplied delivery certainty.
    ///
    /// Use `NotAttempted` only when the managed layer can prove that it rejected
    /// the operation before raw provider invocation. Once provider invocation
    /// begins, a failed write must be `Indeterminate`.
    pub fn write_failed(err: Arc<RealtimeError>, certainty: DeliveryCertainty) -> Self {
        Self::WriteFailed { error: err, certainty }
    }

    /// Return the delivery certainty carried by a managed write failure.
    ///
    /// Generic/provider errors return `None` because they do not establish
    /// whether the managed write boundary invoked a provider session.
    ///
    /// # Example
    ///
    /// ```rust
    /// use adk_realtime::{DeliveryCertainty, RealtimeError};
    /// use std::sync::Arc;
    ///
    /// let err = RealtimeError::write_failed(
    ///     Arc::new(RealtimeError::connection("transport unavailable")),
    ///     DeliveryCertainty::NotAttempted,
    /// );
    ///
    /// match err.delivery_certainty() {
    ///     Some(DeliveryCertainty::NotAttempted) => {
    ///         // The raw provider session was not invoked by this operation.
    ///     }
    ///     Some(DeliveryCertainty::Indeterminate) => {
    ///         // Do not blindly replay a side-effectful operation.
    ///     }
    ///     None => {
    ///         // This error does not carry managed write certainty.
    ///     }
    ///     _ => {}
    /// }
    /// ```
    pub fn delivery_certainty(&self) -> Option<DeliveryCertainty> {
        match self {
            RealtimeError::WriteFailed { certainty, .. } => Some(*certainty),
            _ => None,
        }
    }

    /// Returns true if this error represents a low-level, conservative transport reset fact
    /// (such as a TCP connection reset, broken pipe, or connection aborted).
    ///
    /// This is a transport-fact predicate ("did the transport actually reset?"), NOT a provider
    /// recovery policy ("should this error cause retry?"). Provider recovery policy belongs in
    /// `RealtimeRecovery::classify` / `classify_attempt_error`.
    pub fn is_connection_reset(&self) -> bool {
        match self {
            RealtimeError::ConnectionError(msg) | RealtimeError::MessageError(msg) => {
                let lower = msg.to_lowercase();
                lower.contains("connection reset")
                    || lower.contains("econnreset")
                    || lower.contains("broken pipe")
                    || lower.contains("connection aborted")
                    || lower.contains("econnaborted")
            }
            RealtimeError::IoError(err) => {
                err.kind() == std::io::ErrorKind::ConnectionReset
                    || err.kind() == std::io::ErrorKind::BrokenPipe
                    || err.kind() == std::io::ErrorKind::ConnectionAborted
            }
            RealtimeError::WriteFailed { error, .. } => error.is_connection_reset(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_connection_reset_classification() {
        // Positive transport reset facts
        let err1 = RealtimeError::ConnectionError("read tcp: connection reset by peer".to_string());
        assert!(err1.is_connection_reset());

        let err2 = RealtimeError::MessageError("ECONNRESET occurred".to_string());
        assert!(err2.is_connection_reset());

        let err3 = RealtimeError::ConnectionError("Broken pipe on socket write".to_string());
        assert!(err3.is_connection_reset());

        let err4 = RealtimeError::MessageError("connection aborted by host".to_string());
        assert!(err4.is_connection_reset());

        let io_reset = RealtimeError::IoError(Arc::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        )));
        assert!(io_reset.is_connection_reset());

        let io_broken = RealtimeError::IoError(Arc::new(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "pipe",
        )));
        assert!(io_broken.is_connection_reset());

        let io_aborted = RealtimeError::IoError(Arc::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionAborted,
            "aborted",
        )));
        assert!(io_aborted.is_connection_reset());

        let write_failed_reset = RealtimeError::write_failed(
            Arc::new(io_reset.clone()),
            DeliveryCertainty::Indeterminate,
        );
        assert!(write_failed_reset.is_connection_reset());

        // Negative cases: Broad phrases or non-transport variants MUST NOT be classified as reset
        let broad_closed =
            RealtimeError::ConnectionError("connection closed gracefully".to_string());
        assert!(!broad_closed.is_connection_reset());

        let broad_recv = RealtimeError::MessageError("receive error on channel".to_string());
        assert!(!broad_recv.is_connection_reset());

        // Protocol & Provider errors must never be classified as transport reset regardless of text
        let protocol_reset_text =
            RealtimeError::Protocol("connection reset by peer in payload".to_string());
        assert!(!protocol_reset_text.is_connection_reset());

        let provider_reset_text =
            RealtimeError::ProviderError("broken pipe on provider stream".to_string());
        assert!(!provider_reset_text.is_connection_reset());

        // Quota / Auth / Config / Setup Rejections
        let quota_err = RealtimeError::ServerError {
            code: "429".into(),
            message: "RESOURCE_EXHAUSTED / Quota exceeded".into(),
        };
        assert!(!quota_err.is_connection_reset());

        let auth_err = RealtimeError::AuthError("401 Unauthorized token expired".into());
        assert!(!auth_err.is_connection_reset());

        let config_err = RealtimeError::ConfigError("invalid parameter in setup".into());
        assert!(!config_err.is_connection_reset());

        let write_failed_config = RealtimeError::write_failed(
            Arc::new(config_err.clone()),
            DeliveryCertainty::NotAttempted,
        );
        assert!(!write_failed_config.is_connection_reset());
    }

    #[cfg(feature = "livekit")]
    #[test]
    fn test_livekit_native_error_conversion() {
        let inner = crate::livekit::LiveKitError::ConfigError("test config error".to_string());

        let realtime_err: crate::error::RealtimeError = inner.into();

        match realtime_err {
            crate::error::RealtimeError::LiveKitNativeError(boxed_err) => {
                assert!(matches!(*boxed_err, crate::livekit::LiveKitError::ConfigError(_)));
                assert_eq!(
                    format!("{}", boxed_err),
                    "LiveKit configuration error: test config error"
                );
            }
            _ => panic!("Expected LiveKitNativeError variant"),
        }
    }
}
