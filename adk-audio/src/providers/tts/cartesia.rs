//! Cartesia Sonic TTS provider.

use async_trait::async_trait;

use crate::error::{AudioError, AudioResult};
use crate::frame::AudioFrame;
use crate::providers::tts::CloudTtsConfig;
use crate::traits::{AudioPayload, Emotion, TtsProvider, TtsRequest, Voice};

/// Cartesia Sonic TTS provider.
///
/// Configurable via `CARTESIA_API_KEY` environment variable.
pub struct CartesiaTts {
    config: CloudTtsConfig,
    client: reqwest::Client,
    voices: Vec<Voice>,
}

impl CartesiaTts {
    /// Create from environment variable `CARTESIA_API_KEY`.
    pub fn from_env() -> AudioResult<Self> {
        let api_key = std::env::var("CARTESIA_API_KEY").map_err(|_| AudioError::Tts {
            provider: "cartesia".into(),
            message: "CARTESIA_API_KEY not set".into(),
        })?;
        Ok(Self::new(CloudTtsConfig::new(api_key)))
    }

    /// Create with explicit config.
    pub fn new(config: CloudTtsConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            voices: vec![
                Voice {
                    id: "694f9389-aac1-45b6-b726-9d9369183238".into(),
                    name: "Friendly Female".into(),
                    language: "en".into(),
                    gender: Some("female".into()),
                },
                Voice {
                    id: "a0e99841-438c-4a64-b6a9-62f2c68f5a4a".into(),
                    name: "News Male".into(),
                    language: "en".into(),
                    gender: Some("male".into()),
                },
            ],
        }
    }

    fn base_url(&self) -> &str {
        self.config.base_url.as_deref().unwrap_or("https://api.cartesia.ai")
    }

    fn map_emotion(&self, emotion: Option<&Emotion>) -> Option<&str> {
        match emotion {
            Some(Emotion::Happy) => Some("happy"),
            Some(Emotion::Sad) => Some("sad"),
            Some(Emotion::Angry) => Some("angry"),
            _ => None,
        }
    }
}

#[async_trait]
impl TtsProvider for CartesiaTts {
    async fn synthesize(&self, request: &TtsRequest) -> AudioResult<AudioPayload> {
        let voice_id = if request.voice.is_empty() { &self.voices[0].id } else { &request.voice };
        let url = format!("{}/tts/bytes", self.base_url());

        let mut voice_config = serde_json::json!({
            "mode": "id",
            "id": voice_id,
        });

        if let Some(emotion) = self.map_emotion(request.emotion.as_ref()) {
            if let Some(obj) = voice_config.as_object_mut() {
                obj.insert(
                    "__experimental_controls".to_string(),
                    serde_json::json!({
                        "emotion": [emotion, "highest"]
                    }),
                );
            }
        }

        let body = serde_json::json!({
            "model_id": "sonic-english",
            "transcript": request.text,
            "voice": voice_config,
            "output_format": {
                "container": "raw",
                "encoding": "pcm_s16le",
                "sample_rate": 24000
            },
            "language": request.language.as_deref().unwrap_or("en")
        });

        let resp = self
            .client
            .post(&url)
            .header("X-API-Key", &self.config.api_key)
            .header("Cartesia-Version", "2024-06-10")
            .json(&body)
            .send()
            .await
            .map_err(|e| AudioError::Tts { provider: "cartesia".into(), message: e.to_string() })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AudioError::Tts {
                provider: "cartesia".into(),
                message: format!("HTTP {status}: {body}"),
            });
        }

        let pcm = resp
            .bytes()
            .await
            .map_err(|e| AudioError::Tts { provider: "cartesia".into(), message: e.to_string() })?;

        Ok(AudioPayload::Pcm(AudioFrame::new(pcm, 24000, 1)))
    }

    fn voice_catalog(&self) -> &[Voice] {
        &self.voices
    }
}
