//! Gemini Live model implementation.

use crate::audio::AudioFormat;
use crate::config::RealtimeConfig;
use crate::error::Result;
use crate::model::RealtimeModel;
use crate::session::BoxedSession;
use async_trait::async_trait;

use super::session::{GeminiLiveBackend, GeminiRealtimeSession};
use super::{DEFAULT_MODEL, GEMINI_VOICES};
use adk_gemini::schema_adapter::GeminiSchemaDialect;

/// Gemini Live model for creating realtime sessions.
///
/// # Example
///
/// ```rust,ignore
/// use adk_realtime::gemini::{GeminiRealtimeModel, GeminiLiveBackend};
/// use adk_realtime::RealtimeModel;
///
/// let backend = GeminiLiveBackend::studio("your-key");
/// let model = GeminiRealtimeModel::new(backend, "models/gemini-live-2.5-flash-native-audio");
/// let session = model.connect(config).await?;
/// ```
#[derive(Debug, Clone)]
pub struct GeminiRealtimeModel {
    backend: GeminiLiveBackend,
    model_id: String,
    schema_dialect: GeminiSchemaDialect,
}

impl GeminiRealtimeModel {
    /// Create a new Gemini Live model.
    pub fn new(backend: GeminiLiveBackend, model_id: impl Into<String>) -> Self {
        let schema_dialect = GeminiRealtimeSession::default_schema_dialect(&backend);
        let model_id = super::session::normalize_model_id(&model_id.into());
        Self { backend, model_id, schema_dialect }
    }

    /// Create with the default Live model.
    pub fn with_default_model(backend: GeminiLiveBackend) -> Self {
        Self::new(backend, DEFAULT_MODEL)
    }

    /// Choose the dialect tool schemas are written in.
    ///
    /// [`GeminiSchemaDialect::JsonSchema`] keeps `additionalProperties`,
    /// `allOf`/`anyOf`, `if`/`then` and the string/numeric bounds that the
    /// default OpenAPI subset must strip, so the model is shown the same
    /// contract the caller validates against. It is an opt-in because the field
    /// it uses is undocumented and verified only by probe — read
    /// [`GeminiSchemaDialect`] before selecting it, and re-probe before
    /// assuming it on another model, surface, or endpoint version.
    pub fn with_schema_dialect(mut self, dialect: GeminiSchemaDialect) -> Self {
        self.schema_dialect = dialect;
        self
    }

    /// The dialect this model writes tool schemas in.
    pub fn schema_dialect(&self) -> GeminiSchemaDialect {
        self.schema_dialect
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
        let session = GeminiRealtimeSession::connect_with_dialect(
            self.backend.clone(),
            &self.model_id,
            config,
            self.schema_dialect,
        )
        .await?;
        Ok(Box::new(session))
    }
}
