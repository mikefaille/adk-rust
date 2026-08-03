//! Gemini native audio TTS provider.
//!
//! Supports all Gemini TTS models:
//! - `gemini-3.1-flash-tts-preview` — expressive, audio tags, multi-speaker (default)
//! - `gemini-2.5-flash-preview-tts` — fast, multi-speaker
//! - `gemini-2.5-pro-preview-tts` — high-fidelity, multi-speaker

use std::pin::Pin;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;

use crate::error::{AudioError, AudioResult};
use crate::frame::AudioFrame;
use crate::providers::tts::CloudTtsConfig;
use crate::traits::{TtsProvider, TtsRequest, Voice};

/// Available Gemini TTS model IDs.
#[allow(dead_code)]
pub mod models {
    /// Gemini 3.1 Flash TTS — expressive audio tags, multi-speaker, low-latency.
    pub const GEMINI_3_1_FLASH_TTS: &str = "gemini-3.1-flash-tts-preview";
    /// Gemini 2.5 Flash TTS — fast, multi-speaker.
    pub const GEMINI_2_5_FLASH_TTS: &str = "gemini-2.5-flash-preview-tts";
    /// Gemini 2.5 Pro TTS — high-fidelity, multi-speaker.
    pub const GEMINI_2_5_PRO_TTS: &str = "gemini-2.5-pro-preview-tts";
}

/// Speaker configuration for multi-speaker TTS.
#[derive(Debug, Clone)]
pub struct SpeakerConfig {
    /// Speaker name (must match the name used in the transcript).
    pub name: String,
    /// Voice name from the 30 available voices.
    pub voice: String,
}

impl SpeakerConfig {
    /// Create a new speaker configuration.
    pub fn new(name: impl Into<String>, voice: impl Into<String>) -> Self {
        Self { name: name.into(), voice: voice.into() }
    }
}

/// Gemini TTS provider using `generateContent` with audio response modality.
///
/// # Example
///
/// ```rust,ignore
/// use adk_audio::GeminiTts;
///
/// // Default: gemini-3.1-flash-tts-preview
/// let tts = GeminiTts::from_env()?;
///
/// // Specific model
/// let tts = GeminiTts::from_env()?.with_model("gemini-2.5-pro-preview-tts");
///
/// // Multi-speaker
/// let tts = GeminiTts::from_env()?.with_speakers(vec![
///     SpeakerConfig::new("Alice", "Kore"),
///     SpeakerConfig::new("Bob", "Puck"),
/// ]);
/// ```
pub struct GeminiTts {
    config: CloudTtsConfig,
    #[allow(dead_code)]
    client: reqwest::Client,
    model: String,
    voices: Vec<Voice>,
    speakers: Option<Vec<SpeakerConfig>>,
}

impl GeminiTts {
    /// Create from environment variable `GEMINI_API_KEY`.
    pub fn from_env() -> AudioResult<Self> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .map_err(|_| AudioError::Tts {
                provider: "gemini".into(),
                message: "GEMINI_API_KEY or GOOGLE_API_KEY not set".into(),
            })?;
        Ok(Self::new(CloudTtsConfig::new(api_key)))
    }

    /// Create with explicit config.
    pub fn new(config: CloudTtsConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            model: models::GEMINI_3_1_FLASH_TTS.into(),
            voices: build_voice_catalog(),
            speakers: None,
        }
    }

    /// Set the TTS model.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Configure multi-speaker synthesis.
    ///
    /// Speaker names must match the names used in the transcript text.
    /// Up to 2 speakers are supported.
    pub fn with_speakers(mut self, speakers: Vec<SpeakerConfig>) -> Self {
        self.speakers = Some(speakers);
        self
    }

    #[allow(dead_code)]
    fn base_url(&self) -> String {
        self.config.base_url.clone().unwrap_or_else(|| {
            format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
                self.model
            )
        })
    }

    #[allow(dead_code)]
    fn build_speech_config(&self, voice: &str) -> serde_json::Value {
        match &self.speakers {
            Some(speakers) if !speakers.is_empty() => {
                let speaker_configs: Vec<serde_json::Value> = speakers
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "speaker": s.name,
                            "voiceConfig": {
                                "prebuiltVoiceConfig": {
                                    "voiceName": s.voice
                                }
                            }
                        })
                    })
                    .collect();
                serde_json::json!({
                    "multiSpeakerVoiceConfig": {
                        "speakerVoiceConfigs": speaker_configs
                    }
                })
            }
            _ => {
                let voice_name = if voice.is_empty() { "Kore" } else { voice };
                serde_json::json!({
                    "voiceConfig": {
                        "prebuiltVoiceConfig": {
                            "voiceName": voice_name
                        }
                    }
                })
            }
        }
    }

    fn build_speech_config_typed(&self, voice: &str) -> adk_gemini::interactions::SpeechConfig {
        use adk_gemini::interactions::{
            MultiSpeakerVoiceConfig, PrebuiltVoiceConfig, SpeakerVoiceConfig, SpeechConfig,
            VoiceConfig,
        };
        match &self.speakers {
            Some(speakers) if !speakers.is_empty() => {
                let speaker_configs: Vec<SpeakerVoiceConfig> = speakers
                    .iter()
                    .map(|s| SpeakerVoiceConfig {
                        speaker: s.name.clone(),
                        voice_config: VoiceConfig {
                            prebuilt_voice_config: PrebuiltVoiceConfig {
                                voice_name: s.voice.clone(),
                            },
                        },
                    })
                    .collect();
                SpeechConfig {
                    voice_config: None,
                    multi_speaker_voice_config: Some(MultiSpeakerVoiceConfig {
                        speaker_voice_configs: speaker_configs,
                    }),
                }
            }
            _ => {
                let voice_name =
                    if voice.is_empty() { "Kore".to_string() } else { voice.to_string() };
                SpeechConfig {
                    voice_config: Some(VoiceConfig {
                        prebuilt_voice_config: PrebuiltVoiceConfig { voice_name },
                    }),
                    multi_speaker_voice_config: None,
                }
            }
        }
    }
}

#[async_trait]
impl TtsProvider for GeminiTts {
    async fn synthesize(&self, request: &TtsRequest) -> AudioResult<AudioFrame> {
        let mut builder = adk_gemini::GeminiBuilder::new(&self.config.api_key)
            .with_model(adk_gemini::Model::Custom(self.model.clone()));

        if let Some(url) =
            self.config.base_url.as_ref().and_then(|base| reqwest::Url::parse(base).ok())
        {
            builder = builder.with_base_url(url);
        }

        let client = builder
            .build()
            .map_err(|e| AudioError::Tts { provider: "gemini".into(), message: e.to_string() })?;

        let speech_config = self.build_speech_config_typed(&request.voice);
        let inter_builder = client
            .create_interaction()
            .input_text(&request.text)
            .response_modalities(vec![adk_gemini::interactions::ResponseModality::Audio])
            .generation_config(adk_gemini::interactions::GenerationConfig {
                speech_config: Some(speech_config),
                ..Default::default()
            });

        let interaction = inter_builder
            .send()
            .await
            .map_err(|e| AudioError::Tts { provider: "gemini".into(), message: e.to_string() })?;

        let mut audio_content: Option<&adk_gemini::interactions::AudioContent> = None;
        for step in interaction.steps.iter().rev() {
            if let adk_gemini::interactions::Step::ModelOutput { content } = step {
                for c in content.iter().rev() {
                    if let adk_gemini::interactions::Content::Audio(a) = c {
                        audio_content = Some(a);
                        break;
                    }
                }
            }
            if audio_content.is_some() {
                break;
            }
        }

        let audio = audio_content.ok_or_else(|| AudioError::Tts {
            provider: "gemini".into(),
            message: "no audio content in response".into(),
        })?;

        if audio.data.is_none() && audio.uri.is_some() {
            return Err(AudioError::Tts {
                provider: "gemini".into(),
                message: "URI-only audio is not supported".into(),
            });
        }

        let mime = audio.mime_type.as_deref().ok_or_else(|| AudioError::Tts {
            provider: "gemini".into(),
            message: "missing initial metadata: mime_type".into(),
        })?;
        let sample_rate = audio.sample_rate.ok_or_else(|| AudioError::Tts {
            provider: "gemini".into(),
            message: "missing initial metadata: sample_rate".into(),
        })?;
        let channels = audio.channels.ok_or_else(|| AudioError::Tts {
            provider: "gemini".into(),
            message: "missing initial metadata: channels".into(),
        })?;

        let mime_lower = mime.to_lowercase();
        if !mime_lower.contains("audio/l16")
            && !mime_lower.contains("audio/pcm")
            && !mime_lower.contains("audio/wav")
            && !mime_lower.contains("audio/x-wav")
        {
            return Err(AudioError::Tts {
                provider: "gemini".into(),
                message: format!("unsupported audio format: {mime}"),
            });
        }

        let audio_b64 = audio.data.as_deref().unwrap_or_default();
        use base64::Engine;
        let pcm = base64::engine::general_purpose::STANDARD.decode(audio_b64).map_err(|e| {
            AudioError::Tts { provider: "gemini".into(), message: format!("invalid base64: {e}") }
        })?;

        Ok(AudioFrame::new(Bytes::from(pcm), sample_rate as u32, channels as u8))
    }

    async fn synthesize_stream(
        &self,
        request: &TtsRequest,
    ) -> AudioResult<Pin<Box<dyn Stream<Item = AudioResult<AudioFrame>> + Send>>> {
        let mut builder = adk_gemini::GeminiBuilder::new(&self.config.api_key)
            .with_model(adk_gemini::Model::Custom(self.model.clone()));

        if let Some(url) =
            self.config.base_url.as_ref().and_then(|base| reqwest::Url::parse(base).ok())
        {
            builder = builder.with_base_url(url);
        }

        let client = builder
            .build()
            .map_err(|e| AudioError::Tts { provider: "gemini".into(), message: e.to_string() })?;

        let speech_config = self.build_speech_config_typed(&request.voice);
        let inter_builder = client
            .create_interaction()
            .input_text(&request.text)
            .response_modalities(vec![adk_gemini::interactions::ResponseModality::Audio])
            .generation_config(adk_gemini::interactions::GenerationConfig {
                speech_config: Some(speech_config),
                ..Default::default()
            });

        let sse_stream = inter_builder
            .stream()
            .await
            .map_err(|e| AudioError::Tts { provider: "gemini".into(), message: e.to_string() })?;

        struct TtsStreamState {
            mime_type: Option<String>,
            sample_rate: Option<i64>,
            channels: Option<i64>,
        }

        let mut state = TtsStreamState { mime_type: None, sample_rate: None, channels: None };

        let stream = async_stream::try_stream! {
            use futures::StreamExt;
            use adk_gemini::interactions::{InteractionSseEvent, StepDelta};
            let mut sse_stream = sse_stream;
            while let Some(item) = sse_stream.next().await {
                let event = item.map_err(|e| AudioError::Tts {
                    provider: "gemini".into(),
                    message: e.to_string(),
                })?;

                match event {
                    InteractionSseEvent::Error { error, .. } => {
                        Err(AudioError::Tts {
                            provider: "gemini".into(),
                            message: format!("SSE error (code={:?}): {}", error.code, error.message),
                        })?;
                    }
                    InteractionSseEvent::StepDelta { delta: StepDelta::Audio { data, mime_type, sample_rate, channels, uri }, .. } => {
                        if data.is_none() && uri.is_some() {
                            Err(AudioError::Tts {
                                provider: "gemini".into(),
                                message: "URI-only audio is not supported".into(),
                            })?;
                        }

                        if state.mime_type.is_none() {
                            let m = mime_type.clone().ok_or_else(|| AudioError::Tts {
                                provider: "gemini".into(),
                                message: "missing initial metadata: mime_type".into(),
                            })?;
                            let sr = sample_rate.ok_or_else(|| AudioError::Tts {
                                provider: "gemini".into(),
                                message: "missing initial metadata: sample_rate".into(),
                            })?;
                            let ch = channels.ok_or_else(|| AudioError::Tts {
                                provider: "gemini".into(),
                                message: "missing initial metadata: channels".into(),
                            })?;

                            let m_lower = m.to_lowercase();
                            if !m_lower.contains("audio/l16") && !m_lower.contains("audio/pcm") && !m_lower.contains("audio/wav") && !m_lower.contains("audio/x-wav") {
                                Err(AudioError::Tts {
                                    provider: "gemini".into(),
                                    message: format!("unsupported audio format: {m}"),
                                })?;
                            }

                            state.mime_type = Some(m);
                            state.sample_rate = Some(sr);
                            state.channels = Some(ch);
                        } else {
                            if mime_type.is_some() && mime_type.as_ref() != state.mime_type.as_ref() {
                                Err(AudioError::Tts {
                                    provider: "gemini".into(),
                                    message: "contradictory metadata: mime_type changed mid-stream".into(),
                                })?;
                            }
                            if sample_rate.is_some() && sample_rate != state.sample_rate {
                                Err(AudioError::Tts {
                                    provider: "gemini".into(),
                                    message: "contradictory metadata: sample_rate changed mid-stream".into(),
                                })?;
                            }
                            if channels.is_some() && channels != state.channels {
                                Err(AudioError::Tts {
                                    provider: "gemini".into(),
                                    message: "contradictory metadata: channels changed mid-stream".into(),
                                })?;
                            }
                        }

                        let d = data.as_deref().unwrap_or_default();
                        if d.is_empty() {
                            continue;
                        }

                        use base64::Engine;
                        let pcm = base64::engine::general_purpose::STANDARD.decode(d).map_err(|e| {
                            AudioError::Tts {
                                provider: "gemini".into(),
                                message: format!("invalid base64: {e}"),
                            }
                        })?;

                        let sr = state.sample_rate.unwrap() as u32;
                        let ch = state.channels.unwrap() as u8;
                        yield AudioFrame::new(Bytes::from(pcm), sr, ch);
                    }
                    _ => {}
                }
            }
        };

        Ok(Box::pin(stream))
    }

    fn voice_catalog(&self) -> &[Voice] {
        &self.voices
    }
}

/// Build the full 30-voice catalog.
fn build_voice_catalog() -> Vec<Voice> {
    let voices = [
        ("Zephyr", "Bright"),
        ("Puck", "Upbeat"),
        ("Charon", "Informative"),
        ("Kore", "Firm"),
        ("Fenrir", "Excitable"),
        ("Leda", "Youthful"),
        ("Orus", "Firm"),
        ("Aoede", "Breezy"),
        ("Callirrhoe", "Easy-going"),
        ("Autonoe", "Bright"),
        ("Enceladus", "Breathy"),
        ("Iapetus", "Clear"),
        ("Umbriel", "Easy-going"),
        ("Algieba", "Smooth"),
        ("Despina", "Smooth"),
        ("Erinome", "Clear"),
        ("Algenib", "Gravelly"),
        ("Rasalgethi", "Informative"),
        ("Laomedeia", "Upbeat"),
        ("Achernar", "Soft"),
        ("Alnilam", "Firm"),
        ("Schedar", "Even"),
        ("Gacrux", "Mature"),
        ("Pulcherrima", "Forward"),
        ("Achird", "Friendly"),
        ("Zubenelgenubi", "Casual"),
        ("Vindemiatrix", "Gentle"),
        ("Sadachbia", "Lively"),
        ("Sadaltager", "Knowledgeable"),
        ("Sulafat", "Warm"),
    ];

    voices
        .iter()
        .map(|(name, style)| Voice {
            id: name.to_string(),
            name: format!("{name} — {style}"),
            language: "multilingual".into(),
            gender: None,
        })
        .collect()
}
