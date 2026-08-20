//! Gemini native audio TTS provider using the Gemini API.
//!
//! Supports all Gemini TTS models:
//! - `gemini-3.1-flash-tts-preview` — expressive, audio tags, multi-speaker (default)
//! - `gemini-2.5-flash-preview-tts` — fast, multi-speaker
//! - `gemini-2.5-pro-preview-tts` — high-fidelity, multi-speaker

use std::pin::Pin;

use adk_gemini::{
    Gemini, GeminiBuilder, Model, Part,
    generation::{SpeakerVoiceConfig, SpeechConfig},
};
use async_stream::try_stream;
use async_trait::async_trait;
use base64::Engine as _;
use bytes::Bytes;
use futures::{Stream, StreamExt, TryStreamExt};

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

/// Gemini TTS provider using the Gemini API for low-latency audio streaming.
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

    fn build_speech_config(&self, voice: &str) -> SpeechConfig {
        match &self.speakers {
            Some(speakers) if !speakers.is_empty() => {
                let speaker_configs = speakers
                    .iter()
                    .map(|s| SpeakerVoiceConfig::new(&s.name, &s.voice))
                    .collect();
                SpeechConfig::multi_speaker(speaker_configs)
            }
            _ => {
                let voice_name = if voice.is_empty() { "Kore" } else { voice };
                SpeechConfig::single_voice(voice_name)
            }
        }
    }
}

/// Validate MIME type and enforce PCM audio encoding contract.
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

    if !base_mime.eq_ignore_ascii_case("audio/l16") && !base_mime.eq_ignore_ascii_case("audio/pcm")
    {
        return Err(AudioError::Tts {
            provider: "gemini".into(),
            message: format!(
                "Unsupported audio MIME type for TTS stream (expected audio/l16 or audio/pcm): {mime_type}"
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
        let speech_config = self.build_speech_config(&request.voice);
        let builder = self
            .gemini
            .generate_content()
            .with_user_message(&request.text)
            .with_speech_config(speech_config)
            .with_audio_output();

        let mut response_stream = Box::pin(builder.execute_stream().await.map_err(|e| AudioError::Tts {
            provider: "gemini".into(),
            message: format!("HTTP / stream request failed: {e}"),
        })?);

        let stream = try_stream! {
            let mut audio_chunks_received = 0u64;
            let mut expected_sample_rate: Option<u32> = None;
            let mut expected_channels: Option<u8> = None;
            let mut l16_remainder: Option<u8> = None;

            while let Some(response) = response_stream.try_next().await.map_err(|e| AudioError::Tts {
                provider: "gemini".into(),
                message: format!("SSE stream read error: {e}"),
            })? {
                for candidate in response.candidates {
                    if let Some(parts) = candidate.content.parts {
                        for part in parts {
                            if let Part::InlineData { inline_data } = part {
                                let sample_rate_val = parse_sample_rate(&inline_data.mime_type, None)?;
                                let channels_val = parse_channels(&inline_data.mime_type, None)?;

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

                                if !inline_data.data.is_empty() {
                                    let mut decoded = base64::engine::general_purpose::STANDARD.decode(&inline_data.data)
                                        .map_err(|e| AudioError::Tts {
                                            provider: "gemini".into(),
                                            message: format!("Invalid base64 audio payload: {e}"),
                                        })?;

                                    if decoded.is_empty() {
                                        continue;
                                    }

                                    // Handle audio/l16 big-endian PCM framing across deltas.
                                    let base_mime = inline_data.mime_type.split(';').next().unwrap_or(&inline_data.mime_type).trim();
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
                        }
                    }
                }
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
                    message: "TTS stream finished without emitting audio frames".into(),
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
