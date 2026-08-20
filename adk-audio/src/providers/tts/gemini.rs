//! Gemini native audio TTS provider using the Interactions API.
//!
//! Supports all Gemini TTS models:
//! - `gemini-3.1-flash-tts-preview` — expressive, audio tags, multi-speaker (default)
//! - `gemini-2.5-flash-preview-tts` — fast, multi-speaker
//! - `gemini-2.5-pro-preview-tts` — high-fidelity, multi-speaker

use std::pin::Pin;

use adk_gemini::interactions::{
    CreateInteractionRequest, Input, InteractionSseEvent, ResponseFormat, SpeechConfigEntry,
    StepDelta,
};
use adk_gemini::{Gemini, GeminiBuilder, Model};
use async_stream::try_stream;
use async_trait::async_trait;
use base64::Engine as _;
use bytes::Bytes;
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
    gemini: Gemini,
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
        Self::new(CloudTtsConfig::new(api_key))
    }

    /// Create with explicit config.
    pub fn new(config: CloudTtsConfig) -> AudioResult<Self> {
        let model = models::GEMINI_3_1_FLASH_TTS.to_string();
        let mut builder =
            GeminiBuilder::new(&config.api_key).with_model(Model::from(model.clone()));
        if let Some(ref base) = config.base_url {
            let normalized_base =
                if !base.ends_with('/') { format!("{base}/") } else { base.clone() };
            let url = url::Url::parse(&normalized_base).map_err(|e| AudioError::Tts {
                provider: "gemini".into(),
                message: format!("Invalid base_url '{base}': {e}"),
            })?;
            builder = builder.with_base_url(url);
        }
        let gemini = builder.build().map_err(|e| AudioError::Tts {
            provider: "gemini".into(),
            message: format!("Failed to build Gemini client: {e}"),
        })?;

        Ok(Self { config, gemini, model, voices: build_voice_catalog(), speakers: None })
    }

    /// Set the TTS model.
    pub fn with_model(mut self, model: impl Into<String>) -> AudioResult<Self> {
        self.model = model.into();
        let mut builder =
            GeminiBuilder::new(&self.config.api_key).with_model(Model::from(self.model.clone()));
        if let Some(ref base) = self.config.base_url {
            let normalized_base =
                if !base.ends_with('/') { format!("{base}/") } else { base.clone() };
            let url = url::Url::parse(&normalized_base).map_err(|e| AudioError::Tts {
                provider: "gemini".into(),
                message: format!("Invalid base_url '{base}': {e}"),
            })?;
            builder = builder.with_base_url(url);
        }
        self.gemini = builder.build().map_err(|e| AudioError::Tts {
            provider: "gemini".into(),
            message: format!("Failed to build Gemini client for model '{}': {e}", self.model),
        })?;
        Ok(self)
    }

    /// Configure multi-speaker synthesis.
    ///
    /// Speaker names must match the names used in the transcript text.
    /// Up to 2 speakers are supported.
    pub fn with_speakers(mut self, speakers: Vec<SpeakerConfig>) -> Self {
        self.speakers = Some(speakers);
        self
    }

    fn base_url(&self) -> String {
        self.config.base_url.clone().unwrap_or_else(|| {
            format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
                self.model
            )
        })
    }

    fn build_speech_config(&self, voice: &str) -> Vec<SpeechConfigEntry> {
        match &self.speakers {
            Some(speakers) if !speakers.is_empty() => speakers
                .iter()
                .map(|s| SpeechConfigEntry::speaker_voice(&s.name, &s.voice))
                .collect(),
            _ => {
                let voice_name = if voice.is_empty() { "Kore" } else { voice };
                vec![SpeechConfigEntry::voice(voice_name)]
            }
        }
    }

    fn build_request(&self, request: &TtsRequest, stream: bool) -> CreateInteractionRequest {
        let speech_config = Some(self.build_speech_config(&request.voice));

        CreateInteractionRequest {
            model: Some(self.model.clone()),
            input: Input::Text(request.text.clone()),
            response_format: Some(ResponseFormat::Audio {
                mime_type: Some("audio/l16".to_string()),
                sample_rate: Some(24000),
            }),
            stream: Some(stream),
            store: Some(false),
            generation_config: Some(adk_gemini::interactions::GenerationConfig {
                speech_config,
                ..Default::default()
            }),
            agent_config: None,
            ..Default::default()
        }
    }
}

/// Validate MIME type and enforce audio/l16 encoding contract.
fn validate_audio_mime_type(mime_type: &str) -> AudioResult<()> {
    let base_mime = mime_type.split(';').next().unwrap_or(mime_type).trim();
    if base_mime.is_empty() {
        return Err(AudioError::Tts {
            provider: "gemini".into(),
            message: "Empty audio MIME type".into(),
        });
    }

    if base_mime.starts_with("audio/wav")
        || base_mime.starts_with("audio/mp3")
        || base_mime.starts_with("audio/mpeg")
        || base_mime.starts_with("audio/ogg")
        || base_mime.starts_with("audio/aac")
    {
        return Err(AudioError::Tts {
            provider: "gemini".into(),
            message: format!(
                "Container/encoded audio format not supported for TTS stream: {mime_type}"
            ),
        });
    }

    if !base_mime.eq_ignore_ascii_case("audio/l16") {
        return Err(AudioError::Tts {
            provider: "gemini".into(),
            message: format!(
                "Unsupported audio MIME type for TTS stream (expected audio/l16): {mime_type}"
            ),
        });
    }

    Ok(())
}

/// Helper function to validate MIME type and parse sample rate
fn parse_sample_rate(mime_type: &str, sample_rate: Option<i64>) -> AudioResult<u32> {
    validate_audio_mime_type(mime_type)?;

    if let Some(sr) = sample_rate {
        if sr <= 0 {
            return Err(AudioError::Tts {
                provider: "gemini".into(),
                message: format!("Invalid non-positive sample rate: {sr}"),
            });
        }
        return Ok(sr as u32);
    }
    for part in mime_type.split(';') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("rate=")
            && let Ok(rate) = val.parse::<u32>()
        {
            if rate == 0 {
                return Err(AudioError::Tts {
                    provider: "gemini".into(),
                    message: "Invalid sample rate: 0".into(),
                });
            }
            return Ok(rate);
        }
    }
    Ok(24000)
}

fn parse_channels(mime_type: &str, channels: Option<i64>) -> AudioResult<u8> {
    validate_audio_mime_type(mime_type)?;

    if let Some(ch) = channels {
        if ch <= 0 || ch > 2 {
            return Err(AudioError::Tts {
                provider: "gemini".into(),
                message: format!("Invalid channels count: {ch}"),
            });
        }
        return Ok(ch as u8);
    }
    for part in mime_type.split(';') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("channels=")
            && let Ok(ch) = val.parse::<u8>()
        {
            if ch == 0 || ch > 2 {
                return Err(AudioError::Tts {
                    provider: "gemini".into(),
                    message: format!("Invalid channels count: {ch}"),
                });
            }
            return Ok(ch);
        }
    }
    Ok(1)
}

#[async_trait]
impl TtsProvider for GeminiTts {
    async fn synthesize(&self, request: &TtsRequest) -> AudioResult<AudioFrame> {
        if self.model.contains("gemini-2.5") {
            let url = self.base_url();
            let speech_config = match &self.speakers {
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
                    let voice_name = if request.voice.is_empty() { "Kore" } else { &request.voice };
                    serde_json::json!({
                        "voiceConfig": {
                            "prebuiltVoiceConfig": {
                                "voiceName": voice_name
                            }
                        }
                    })
                }
            };

            let body = serde_json::json!({
                "contents": [{"parts": [{"text": request.text}]}],
                "generationConfig": {
                    "response_modalities": ["AUDIO"],
                    "speech_config": speech_config
                }
            });

            let client = reqwest::Client::new();
            let resp = client
                .post(&url)
                .header("x-goog-api-key", &self.config.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| AudioError::Tts {
                    provider: "gemini".into(),
                    message: e.to_string(),
                })?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(AudioError::Tts {
                    provider: "gemini".into(),
                    message: format!("HTTP {status}: {body}"),
                });
            }

            let json: serde_json::Value = resp.json().await.map_err(|e| AudioError::Tts {
                provider: "gemini".into(),
                message: e.to_string(),
            })?;

            let audio_b64 = json["candidates"][0]["content"]["parts"][0]["inlineData"]["data"]
                .as_str()
                .ok_or_else(|| AudioError::Tts {
                    provider: "gemini".into(),
                    message: "no audio data in response".into(),
                })?;

            let pcm = base64::engine::general_purpose::STANDARD.decode(audio_b64).map_err(|e| {
                AudioError::Tts {
                    provider: "gemini".into(),
                    message: format!("base64 decode failed: {e}"),
                }
            })?;

            return Ok(AudioFrame::new(Bytes::from(pcm), 24000, 1));
        }

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
        if self.model.contains("gemini-2.5") {
            let frame = self.synthesize(request).await?;
            return Ok(Box::pin(futures::stream::once(async { Ok(frame) })));
        }

        let payload = self.build_request(request, true);

        let mut event_stream =
            self.gemini.send_interaction_stream(payload).await.map_err(|e| AudioError::Tts {
                provider: "gemini".into(),
                message: format!("HTTP / stream request failed: {e}"),
            })?;

        let stream = try_stream! {
            let mut audio_chunks_received = 0u64;
            let mut expected_sample_rate: Option<u32> = None;
            let mut expected_channels: Option<u8> = None;
            let mut completed_successfully = false;
            let mut l16_remainder: Option<u8> = None;

            while let Some(event_res) = event_stream.next().await {
                let sse_event = event_res.map_err(|e| AudioError::Tts {
                    provider: "gemini".into(),
                    message: format!("SSE stream read error: {e}"),
                })?;

                match sse_event {
                    InteractionSseEvent::StepDelta { delta: StepDelta::Audio { data, mime_type, sample_rate, channels }, .. } => {
                        let effective_mime = mime_type.as_deref().unwrap_or("audio/l16");
                        let sample_rate_val = parse_sample_rate(effective_mime, sample_rate)?;
                        let channels_val = parse_channels(effective_mime, channels)?;

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
                            let mut decoded = base64::engine::general_purpose::STANDARD.decode(&b64)
                                .map_err(|e| AudioError::Tts {
                                    provider: "gemini".into(),
                                    message: format!("Invalid base64 audio payload: {e}"),
                                })?;

                            if decoded.is_empty() {
                                continue;
                            }

                            // Handle audio/l16 big-endian PCM framing across deltas.
                            // Omitted or missing MIME on subsequent deltas inherits the stream's audio/l16 contract.
                            let base_mime = effective_mime.split(';').next().unwrap_or(effective_mime).trim();
                            if base_mime.eq_ignore_ascii_case("audio/l16") {
                                if let Some(rem) = l16_remainder.take() {
                                    decoded.insert(0, rem);
                                }
                                if decoded.len() % 2 != 0 {
                                    l16_remainder = decoded.pop();
                                }
                                for chunk in decoded.chunks_exact_mut(2) {
                                    chunk.swap(0, 1);
                                }
                            }

                            if !decoded.is_empty() {
                                audio_chunks_received += 1;
                                let frame = AudioFrame::new(Bytes::from(decoded), sample_rate_val, channels_val);
                                yield frame;
                            }
                        }
                    }
                    InteractionSseEvent::Error { error, .. } => {
                        Err(AudioError::Tts {
                            provider: "gemini".into(),
                            message: format!("Provider stream error: {}: {}", error.code.unwrap_or_default(), error.message),
                        })?;
                    }
                    InteractionSseEvent::InteractionCompleted { interaction, .. } => {
                        if interaction.status != adk_gemini::interactions::InteractionStatus::Completed {
                            Err(AudioError::Tts {
                                provider: "gemini".into(),
                                message: format!("Interaction finished with non-successful status: {:?}", interaction.status),
                            })?;
                        }
                        completed_successfully = true;
                        break;
                    }
                    _ => {}
                }
            }

            if !completed_successfully {
                Err(AudioError::Tts {
                    provider: "gemini".into(),
                    message: "Stream terminated abruptly without interaction.completed event".into(),
                })?;
            }

            if l16_remainder.is_some() {
                Err(AudioError::Tts {
                    provider: "gemini".into(),
                    message: "Stream completed with dangling L16 sample byte".into(),
                })?;
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
