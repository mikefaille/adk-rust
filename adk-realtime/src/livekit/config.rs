//! Configuration structures for LiveKit integration.

use crate::livekit::error::Result;
use livekit_api::access_token::{AccessToken, VideoGrants};
use secrecy::{ExposeSecret, SecretString};

/// Configuration for connecting to a LiveKit server.
///
/// **Design Note:**
/// This struct strictly relies on explicit dependency injection via `LiveKitConfig::new(...)`.
/// This avoids hardcoding environment variable parsing into the core library, which is ideal for
/// production deployments using secret managers or custom configuration systems.
#[derive(Clone)]
pub struct LiveKitConfig {
    /// The WebSocket URL of the LiveKit server.
    pub url: String,
    /// The API key for authenticating with the LiveKit server.
    pub api_key: SecretString,
    /// The API secret for authenticating with the LiveKit server.
    pub api_secret: SecretString,
}

impl std::fmt::Debug for LiveKitConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveKitConfig")
            .field("url", &self.url)
            .field("api_key", &"***REDACTED***")
            .field("api_secret", &"***REDACTED***")
            .finish()
    }
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
    ///     "ws://localhost:7880",
    ///     "your_api_key",
    ///     "your_api_secret",
    /// );
    /// ```
    pub fn new(
        url: impl Into<String>,
        api_key: impl Into<String>,
        api_secret: impl Into<String>,
    ) -> Self {
        Self {
            url: url.into(),
            api_key: SecretString::from(api_key.into()),
            api_secret: SecretString::from(api_secret.into()),
        }
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
    /// * `grants` - Optional custom permissions. Despite the name `VideoGrants` in the official
    ///              LiveKit API, this struct configures **all** participant capabilities,
    ///              including audio publishing, data channel usage, hidden presence, etc.
    ///              If `None` is provided, it defaults to basic room join permissions.
    ///
    /// # Returns
    ///
    /// Returns the generated JWT string on success.
    ///
    /// # Example
    ///
    /// ```rust
    /// use adk_realtime::livekit::LiveKitConfig;
    /// use livekit_api::access_token::VideoGrants;
    ///
    /// let config = LiveKitConfig::new(
    ///     "wss://example.livekit.cloud".to_string(),
    ///     "api_key".to_string(),
    ///     "api_secret".to_string()
    /// );
    ///
    /// // Create custom grants (e.g. allowing the agent to publish data channels)
    /// let grants = VideoGrants {
    ///     room_join: true,
    ///     room: "my-room".to_string(),
    ///     can_publish_data: true,
    ///     ..Default::default()
    /// };
    ///
    /// let token = config.generate_token("my-room", "agent-01", Some(grants), None).unwrap();
    /// ```
    pub fn generate_token(
        &self,
        room_name: &str,
        participant_identity: &str,
        grants: Option<VideoGrants>,
        metadata: Option<String>,
    ) -> Result<String> {
        let grants = grants.unwrap_or_else(|| VideoGrants {
            room_join: true,
            room: room_name.to_string(),
            ..Default::default()
        });

        let mut token = AccessToken::with_api_key(
            self.api_key.expose_secret(),
            self.api_secret.expose_secret(),
        )
        .with_identity(participant_identity)
        .with_grants(grants);

        if let Some(md) = metadata {
            token = token.with_metadata(&md);
        }

        Ok(token.to_jwt()?)
    }
}
