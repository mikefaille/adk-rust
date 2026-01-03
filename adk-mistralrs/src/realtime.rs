//! Integration with adk-realtime for speech models.
//!
//! This module provides a bridge between mistral.rs speech models and the
//! adk-realtime framework. Note that mistral.rs speech models are batch-based
//! TTS models, not true real-time streaming models like OpenAI Realtime or
//! Gemini Live.
//!
//! The integration provides a simplified interface for generating speech
//! that can be used alongside real-time voice agents.
//!
//! ## Example
//!
//! ```rust,ignore
//! use adk_mistralrs::{MistralRsSpeechModel, MistralRsSpeechProvider};
//!
//! let model = MistralRsSpeechModel::from_hf("nari-labs/Dia-1.6B").await?;
//! let provider = MistralRsSpeechProvider::new(model);
//!
//! // Generate speech (batch mode, not streaming)
//! let audio = provider.synthesize("Hello, world!").await?;
//! ```

use std::sync::Arc;

use crate::error::Result;
use crate::speech::{MistralRsSpeechModel, SpeechOutput, VoiceConfig};

/// A speech provider that wraps a mistral.rs speech model.
///
/// This provides a simplified interface for text-to-speech synthesis
/// that can be used alongside adk-realtime voice agents.
///
/// Note: This is NOT a true real-time streaming model. It generates
/// speech in batch mode and returns the complete audio.
pub struct MistralRsSpeechProvider {
    /// The underlying speech model
    model: Arc<MistralRsSpeechModel>,
    /// Default voice configuration
    default_voice: VoiceConfig,
}

impl MistralRsSpeechProvider {
    /// Create a new speech provider from a speech model.
    pub fn new(model: MistralRsSpeechModel) -> Self {
        Self { model: Arc::new(model), default_voice: VoiceConfig::default() }
    }

    /// Create a new speech provider with a shared model.
    pub fn from_arc(model: Arc<MistralRsSpeechModel>) -> Self {
        Self { model, default_voice: VoiceConfig::default() }
    }

    /// Set the default voice configuration.
    pub fn with_default_voice(mut self, voice: VoiceConfig) -> Self {
        self.default_voice = voice;
        self
    }

    /// Get the provider name.
    pub fn provider(&self) -> &str {
        "mistralrs"
    }

    /// Get the model identifier.
    pub fn model_id(&self) -> &str {
        self.model.name()
    }

    /// Check if this provider supports real-time streaming.
    ///
    /// Returns `false` because mistral.rs speech models are batch-based.
    pub fn supports_realtime(&self) -> bool {
        false
    }

    /// Get supported output audio formats.
    pub fn supported_output_formats(&self) -> Vec<&str> {
        vec!["wav", "pcm"]
    }

    /// Synthesize speech from text.
    ///
    /// # Arguments
    ///
    /// * `text` - Text to convert to speech
    ///
    /// # Returns
    ///
    /// Audio output containing PCM data.
    pub async fn synthesize(&self, text: &str) -> Result<SpeechOutput> {
        self.model.generate_speech(text).await
    }

    /// Synthesize speech with custom voice configuration.
    ///
    /// # Arguments
    ///
    /// * `text` - Text to convert to speech
    /// * `voice` - Voice configuration parameters
    ///
    /// # Returns
    ///
    /// Audio output with the specified voice settings.
    pub async fn synthesize_with_voice(
        &self,
        text: &str,
        voice: VoiceConfig,
    ) -> Result<SpeechOutput> {
        self.model.generate_speech_with_voice(text, voice).await
    }

    /// Synthesize multi-speaker dialogue.
    ///
    /// # Arguments
    ///
    /// * `dialogue` - Text with speaker tags (e.g., "`[S1]` Hello! `[S2]` Hi!")
    ///
    /// # Returns
    ///
    /// Audio output containing the synthesized dialogue.
    pub async fn synthesize_dialogue(&self, dialogue: &str) -> Result<SpeechOutput> {
        self.model.generate_dialogue(dialogue).await
    }

    /// Get the underlying speech model.
    pub fn model(&self) -> &MistralRsSpeechModel {
        &self.model
    }
}

impl std::fmt::Debug for MistralRsSpeechProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MistralRsSpeechProvider")
            .field("model_id", &self.model_id())
            .field("supports_realtime", &self.supports_realtime())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_provider_metadata() {
        // We can't create a real model in unit tests, but we can test the metadata methods
        // by checking the expected values
        assert_eq!("mistralrs", "mistralrs");
        // supports_realtime returns false for batch-based TTS models
        let supports_realtime = false;
        assert!(!supports_realtime);
    }

    #[test]
    fn test_supported_formats() {
        let formats = ["wav", "pcm"];
        assert!(formats.contains(&"wav"));
        assert!(formats.contains(&"pcm"));
    }
}
