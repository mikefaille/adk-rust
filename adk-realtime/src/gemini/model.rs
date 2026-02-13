//! Gemini Live model implementation.

use crate::audio::AudioFormat;
use crate::config::RealtimeConfig;
use crate::error::Result;
use crate::model::RealtimeModel;
use crate::session::BoxedSession;
use async_trait::async_trait;

use super::session::GeminiRealtimeSession;
use super::{DEFAULT_MODEL, GEMINI_LIVE_URL, GEMINI_VOICES};

/// Gemini Live model for creating realtime sessions.
///
/// # Example
///
/// ```rust,ignore
/// use adk_realtime::gemini::GeminiRealtimeModel;
/// use adk_realtime::RealtimeModel;
///
/// let model = GeminiRealtimeModel::new("your-api-key", "models/gemini-live-2.5-flash-native-audio");
/// let session = model.connect(config).await?;
/// ```
#[derive(Debug, Clone)]
pub struct GeminiRealtimeModel {
    api_key: String,
    model_id: String,
    base_url: Option<String>,
}

impl GeminiRealtimeModel {
    /// Create a new Gemini Live model.
    ///
    /// # Arguments
    ///
    /// * `api_key` - Your Google API key
    /// * `model_id` - The model ID (e.g., "models/gemini-live-2.5-flash-native-audio")
    pub fn new(api_key: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self { api_key: api_key.into(), model_id: model_id.into(), base_url: None }
    }

    /// Create with the default Live model.
    pub fn with_default_model(api_key: impl Into<String>) -> Self {
        Self::new(api_key, DEFAULT_MODEL)
    }

    /// Set a custom base URL.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// Get the WebSocket URL for connection.
    pub fn websocket_url(&self) -> String {
        let base = self.base_url.as_deref().unwrap_or(GEMINI_LIVE_URL);
        format!("{}?key={}", base, self.api_key)
    }

    /// Get the API key.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }
}

#[async_trait]
impl RealtimeModel for GeminiRealtimeModel {
    fn provider(&self) -> &str {
        "gemini"
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn supported_input_formats(&self) -> Vec<AudioFormat> {
        vec![AudioFormat::pcm16_16khz()]
    }

    fn supported_output_formats(&self) -> Vec<AudioFormat> {
        vec![AudioFormat::pcm16_24khz()]
    }

    fn available_voices(&self) -> Vec<&str> {
        GEMINI_VOICES.to_vec()
    }

    async fn connect(&self, config: RealtimeConfig) -> Result<BoxedSession> {
        let session =
            GeminiRealtimeSession::connect(&self.websocket_url(), &self.model_id, config).await?;

        Ok(Box::new(session))
    }
}

impl Default for GeminiRealtimeModel {
    fn default() -> Self {
        Self { api_key: String::new(), model_id: DEFAULT_MODEL.to_string(), base_url: None }
    }
}
