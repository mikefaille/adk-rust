//! Gemini native audio TTS provider using the Interactions API.
//!
//! Supports all Gemini TTS models:
//! - `gemini-3.1-flash-tts-preview` — expressive, audio tags, multi-speaker (default)
//! - `gemini-2.5-flash-preview-tts` — fast, multi-speaker
//! - `gemini-2.5-pro-preview-tts` — high-fidelity, multi-speaker

use std::pin::Pin;

use adk_gemini::interactions::{
    CreateInteractionRequest, Input, InteractionSseEvent, ResponseFormat, StepDelta,
};
use async_stream::try_stream;
use async_trait::async_trait;
use base64::Engine as _;
use bytes::Bytes;
use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt};

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

/// Gemini TTS provider using the Interactions API for low-latency audio streaming.
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

    fn interactions_url(&self) -> String {
        if let Some(ref base) = self.config.base_url {
            if base.contains("/interactions") {
                base.clone()
            } else {
                format!("{}/interactions", base.trim_end_matches('/'))
            }
        } else {
            "https://generativelanguage.googleapis.com/v1beta/interactions".to_string()
        }
    }

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

    fn build_request(&self, request: &TtsRequest, stream: bool) -> CreateInteractionRequest {
        let speech_config_val = self.build_speech_config(&request.voice);
        let speech_config: Option<adk_gemini::generation::SpeechConfig> =
            serde_json::from_value(speech_config_val).ok();

        CreateInteractionRequest {
            model: Some(self.model.clone()),
            input: Input::Text(request.text.clone()),
            response_format: Some(ResponseFormat::Audio {
                mime_type: Some("audio/pcm".to_string()),
                sample_rate: Some(24000),
            }),
            stream: Some(stream),
            generation_config: Some(adk_gemini::interactions::GenerationConfig {
                speech_config,
                ..Default::default()
            }),
            agent_config: None,
            ..Default::default()
        }
    }
}

/// Helper function to parse audio mime_type string like "audio/pcm;rate=24000" or sample_rate header field
fn parse_sample_rate(mime_type: Option<&str>, sample_rate: Option<i64>) -> AudioResult<u32> {
    if let Some(sr) = sample_rate
        && sr > 0
    {
        return Ok(sr as u32);
    }
    if let Some(mime) = mime_type {
        for part in mime.split(';') {
            let part = part.trim();
            if let Some(val) = part.strip_prefix("rate=")
                && let Ok(rate) = val.parse::<u32>()
            {
                return Ok(rate);
            }
        }
        if mime.starts_with("audio/pcm")
            || mime.starts_with("audio/raw")
            || mime.starts_with("audio/wav")
        {
            return Ok(24000);
        }
    }
    Ok(24000)
}

fn parse_channels(mime_type: Option<&str>, channels: Option<i64>) -> AudioResult<u8> {
    if let Some(ch) = channels
        && ch > 0
    {
        return Ok(ch as u8);
    }
    if let Some(mime) = mime_type {
        for part in mime.split(';') {
            let part = part.trim();
            if let Some(val) = part.strip_prefix("channels=")
                && let Ok(ch) = val.parse::<u8>()
            {
                return Ok(ch);
            }
        }
    }
    Ok(1)
}

#[async_trait]
impl TtsProvider for GeminiTts {
    async fn synthesize(&self, request: &TtsRequest) -> AudioResult<AudioFrame> {
        let mut stream = self.synthesize_stream(request).await?;
        let mut frames = Vec::new();

        while let Some(res) = stream.next().await {
            let frame = res?;
            frames.push(frame);
        }

        if frames.is_empty() {
            return Err(AudioError::Tts {
                provider: "gemini".into(),
                message: "stream completed without generating any audio".into(),
            });
        }

        Ok(crate::frame::merge_frames(&frames))
    }

    async fn synthesize_stream(
        &self,
        request: &TtsRequest,
    ) -> AudioResult<Pin<Box<dyn Stream<Item = AudioResult<AudioFrame>> + Send>>> {
        let url = self.interactions_url();
        let payload = self.build_request(request, true);

        let response = self
            .client
            .post(&url)
            .header("x-goog-api-key", &self.config.api_key)
            .header("Api-Revision", adk_gemini::interactions::API_REVISION)
            .json(&payload)
            .send()
            .await
            .map_err(|e| AudioError::Tts {
                provider: "gemini".into(),
                message: format!("HTTP request failed: {e}"),
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AudioError::Tts {
                provider: "gemini".into(),
                message: format!("HTTP {status}: {body}"),
            });
        }

        let mut event_stream = response.bytes_stream().eventsource();

        let stream = try_stream! {
            let mut audio_chunks_received = 0u64;
            let mut expected_sample_rate: Option<u32> = None;
            let mut expected_channels: Option<u8> = None;

            while let Some(event_res) = event_stream.next().await {
                let event = event_res.map_err(|e| AudioError::Tts {
                    provider: "gemini".into(),
                    message: format!("SSE stream read error: {e}"),
                })?;

                let sse_event: InteractionSseEvent = serde_json::from_str(&event.data)
                    .map_err(|e| AudioError::Tts {
                        provider: "gemini".into(),
                        message: format!("Failed to parse SSE event JSON: {e}"),
                    })?;

                match sse_event {
                    InteractionSseEvent::StepDelta { delta: StepDelta::Audio { data, mime_type, sample_rate, channels }, .. } => {
                        let sample_rate_val = parse_sample_rate(mime_type.as_deref(), sample_rate)?;
                        let channels_val = parse_channels(mime_type.as_deref(), channels)?;

                        if let Some(exp_sr) = expected_sample_rate {
                            if exp_sr != sample_rate_val {
                                Err(AudioError::Tts {
                                    provider: "gemini".into(),
                                    message: format!("MIME/Metadata mismatch: expected sample rate {exp_sr}, got {sample_rate_val}"),
                                })?;
                            }
                        } else {
                            expected_sample_rate = Some(sample_rate_val);
                        }

                        if let Some(exp_ch) = expected_channels {
                            if exp_ch != channels_val {
                                Err(AudioError::Tts {
                                    provider: "gemini".into(),
                                    message: format!("MIME/Metadata mismatch: expected channels {exp_ch}, got {channels_val}"),
                                })?;
                            }
                        } else {
                            expected_channels = Some(channels_val);
                        }

                        if let Some(b64) = data
                            && !b64.is_empty()
                        {
                            let decoded = base64::engine::general_purpose::STANDARD.decode(&b64)
                                .map_err(|e| AudioError::Tts {
                                    provider: "gemini".into(),
                                    message: format!("Invalid base64 audio payload: {e}"),
                                })?;

                            if decoded.is_empty() {
                                continue;
                            }

                            audio_chunks_received += 1;
                            let frame = AudioFrame::new(Bytes::from(decoded), sample_rate_val, channels_val);
                            yield frame;
                        }
                    }
                    InteractionSseEvent::Error { error, .. } => {
                        Err(AudioError::Tts {
                            provider: "gemini".into(),
                            message: format!("Provider stream error: {}: {}", error.code.unwrap_or_default(), error.message),
                        })?;
                    }
                    InteractionSseEvent::InteractionCompleted { interaction, .. } => {
                        if interaction.status == adk_gemini::interactions::InteractionStatus::Failed {
                            Err(AudioError::Tts {
                                provider: "gemini".into(),
                                message: "Interaction completed with status failed".into(),
                            })?;
                        }
                        break;
                    }
                    _ => {}
                }
            }

            if audio_chunks_received == 0 {
                Err(AudioError::Tts {
                    provider: "gemini".into(),
                    message: "Interactions stream finished without emitting audio frames".into(),
                })?;
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
