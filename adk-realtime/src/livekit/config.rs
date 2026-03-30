//! Configuration structures for LiveKit integration.

use crate::error::{RealtimeError, Result};
use livekit_api::access_token::{AccessToken, VideoGrants};
use std::env;

/// Configuration for connecting to a LiveKit server.
///
/// **Design Note:**
/// This library provides two ways to instantiate configuration to satisfy different use cases:
/// 1. `LiveKitConfig::new(...)`: Explicit dependency injection. This avoids hardcoding
///    environment variable parsing into the core library, which is ideal for production deployments
///    using secret managers or custom configuration systems.
/// 2. `LiveKitConfig::from_env()`: Convenience loader. This automatically reads `LIVEKIT_URL`,
///    `LIVEKIT_API_KEY`, and `LIVEKIT_API_SECRET`, matching the standard conventions of the Go and
///    Python LiveKit SDK ecosystems for rapid development.
#[derive(Debug, Clone)]
pub struct LiveKitConfig {
    /// The WebSocket URL of the LiveKit server.
    pub url: String,
    /// The API key for authenticating with the LiveKit server.
    pub api_key: String,
    /// The API secret for authenticating with the LiveKit server.
    pub api_secret: String,
}

impl LiveKitConfig {
    /// Creates a new `LiveKitConfig` with the provided credentials.
    ///
    /// Use this constructor when loading configurations from a custom source (like a database,
    /// secret manager, or a `.toml` file) rather than relying directly on environment variables.
    ///
    /// # Arguments
    ///
    /// * `url` - The WebSocket URL of the LiveKit server (e.g., `ws://localhost:7880`).
    /// * `api_key` - The LiveKit API key.
    /// * `api_secret` - The LiveKit API secret.
    ///
    /// # Example
    ///
    /// ```rust
    /// use adk_realtime::livekit::LiveKitConfig;
    ///
    /// let config = LiveKitConfig::new(
    ///     "ws://localhost:7880".to_string(),
    ///     "your_api_key".to_string(),
    ///     "your_api_secret".to_string(),
    /// );
    /// ```
    pub fn new(url: String, api_key: String, api_secret: String) -> Self {
        Self { url, api_key, api_secret }
    }

    /// Creates a new `LiveKitConfig` from environment variables.
    ///
    /// This aligns with the conventions used in the LiveKit Go and Python SDKs, where clients
    /// will automatically pick up connection details from the environment if not explicitly provided.
    ///
    /// Requires the following environment variables to be set:
    /// - `LIVEKIT_URL`
    /// - `LIVEKIT_API_KEY`
    /// - `LIVEKIT_API_SECRET`
    ///
    /// # Errors
    ///
    /// Returns a `RealtimeError::ConfigError` if any of the required environment variables are missing.
    pub fn from_env() -> Result<Self> {
        let var_to_err = |name, help| env::var(name).map_err(|_| RealtimeError::config(help));

        let url = var_to_err(
            "LIVEKIT_URL",
            "Missing LIVEKIT_URL environment variable. Please set it to the LiveKit server WebSocket URL (e.g. `ws://localhost:7880`).",
        )?;

        let api_key = var_to_err(
            "LIVEKIT_API_KEY",
            "Missing LIVEKIT_API_KEY environment variable. Please set it to your LiveKit API key.",
        )?;

        let api_secret = var_to_err(
            "LIVEKIT_API_SECRET",
            "Missing LIVEKIT_API_SECRET environment variable. Please set it to your LiveKit API secret.",
        )?;

        Ok(Self { url, api_key, api_secret })
    }

    /// Generates a JWT access token for joining a specific LiveKit room.
    ///
    /// This is useful for manual connection management when you do not want
    /// to use the `LiveKitRoomBuilder`.
    ///
    /// # Arguments
    ///
    /// * `room_name` - The name of the room to join.
    /// * `participant_identity` - The identity to use for the participant.
    /// * `grants` - Optional custom permissions. If `None` is provided, defaults to
    ///              basic room join permissions for the specified `room_name`.
    ///
    /// # Returns
    ///
    /// Returns the generated JWT string on success.
    pub fn generate_token(
        &self,
        room_name: &str,
        participant_identity: &str,
        grants: Option<VideoGrants>,
    ) -> Result<String> {
        let grants = grants.unwrap_or_else(|| VideoGrants {
            room_join: true,
            room: room_name.to_string(),
            ..Default::default()
        });

        AccessToken::with_api_key(&self.api_key, &self.api_secret)
            .with_identity(participant_identity)
            .with_grants(grants)
            .to_jwt()
            .map_err(|e| RealtimeError::livekit(format!("Token generation failed: {e}")))
    }
}
