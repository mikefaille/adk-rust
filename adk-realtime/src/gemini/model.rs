//! Gemini Live model implementation.

use crate::audio::AudioFormat;
use crate::config::RealtimeConfig;
use crate::error::Result;
use crate::model::RealtimeModel;
use crate::session::BoxedSession;
use async_trait::async_trait;

use std::sync::Arc;
use std::sync::RwLock;

use super::session::{GeminiLiveBackend, GeminiRealtimeSession};
use super::{DEFAULT_MODEL, GEMINI_VOICES};

/// Gemini Live model for creating realtime sessions.
///
/// # Example
///
/// ```rust,ignore
/// use adk_realtime::gemini::{GeminiRealtimeModel, GeminiLiveBackend};
/// use adk_realtime::RealtimeModel;
///
/// let backend = GeminiLiveBackend::studio("your-key", "models/gemini-live-2.5-flash-native-audio");
/// let model = GeminiRealtimeModel::new(backend);
/// let session = model.connect(config).await?;
/// ```
#[derive(Debug, Clone)]
pub struct GeminiRealtimeModel {
    backend: GeminiLiveBackend,
    resume_token: Arc<RwLock<Option<String>>>,
}

impl GeminiRealtimeModel {
    /// Create a new Gemini Live model from a backend configuration.
    pub fn new(backend: GeminiLiveBackend) -> Self {
        Self { backend, resume_token: Arc::new(RwLock::new(None)) }
    }

    /// Create with the default Live model.
    pub fn with_default_model(api_key: impl Into<String>) -> Self {
        let backend = GeminiLiveBackend::studio(api_key, DEFAULT_MODEL);
        Self::new(backend)
    }
}

#[async_trait]
impl RealtimeModel for GeminiRealtimeModel {
    fn provider(&self) -> &str {
        "gemini"
    }

    fn model_id(&self) -> &str {
        self.backend.model()
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
        let session = GeminiRealtimeSession::connect(self.backend.clone(), config, self.resume_token.clone()).await?;
        Ok(Box::new(session))
    }
}
