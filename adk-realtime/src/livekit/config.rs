//! Configuration structures for LiveKit integration.

/// Configuration for connecting to a LiveKit server.
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
}
