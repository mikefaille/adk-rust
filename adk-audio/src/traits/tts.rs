//! Text-to-speech provider trait and request types.

use std::pin::Pin;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;

use crate::error::AudioResult;
use crate::frame::AudioFrame;

/// Result of a TTS synthesis, which may be raw PCM or encoded bytes.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioPayload {
    /// Raw decoded PCM samples.
    Pcm(AudioFrame),
    /// Un-decoded network chunk preserving the requested encoding.
    Encoded(EncodedAudioChunk),
}

impl AudioPayload {
    /// Returns the duration of the audio in milliseconds.
    ///
    /// For encoded audio formats where duration cannot be statically determined
    /// without decoding, this currently returns 0.
    pub fn duration_ms(&self) -> u32 {
        match self {
            Self::Pcm(frame) => frame.duration_ms,
            Self::Encoded(_) => 0,
        }
    }

    /// Returns the sample rate of the audio.
    ///
    /// For encoded audio formats, this returns 24000 as a default assumption
    /// if the true sample rate cannot be known without decoding.
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Pcm(frame) => frame.sample_rate,
            Self::Encoded(_) => 24000,
        }
    }

    /// Returns the raw PCM frame if this payload is PCM, or None if it is encoded.
    pub fn into_pcm_frame(self) -> Option<AudioFrame> {
        match self {
            Self::Pcm(frame) => Some(frame),
            Self::Encoded(_) => None,
        }
    }
}

/// An audio chunk encoded in a specific transport format.
#[derive(Clone, Debug, PartialEq)]
pub struct EncodedAudioChunk {
    /// Raw byte stream of the encoded format.
    pub data: Bytes,
    /// Format identifier.
    pub encoding: EncodedAudioFormat,
    /// Monotonic sequence number for ordered assembly.
    pub sequence: u64,
    /// Indicates whether this is the final chunk in the stream.
    pub end_of_stream: bool,
}

/// Formats representing a continuous, un-decoded encoded stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodedAudioFormat {
    /// Ogg-containerized Opus packets, returned natively by Gemini / Cloud TTS.
    OggOpus,
}

/// Indicates the requested processing mode for the provider output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TtsOutputMode {
    /// Decode to a PCM `AudioFrame` (default).
    #[default]
    DecodedPcm,
    /// Return the raw byte sequence of an encoded format.
    PreserveEncoding(EncodedAudioFormat),
}

/// Emotion hint for TTS synthesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emotion {
    /// Neutral tone.
    Neutral,
    /// Happy / upbeat tone.
    Happy,
    /// Sad / somber tone.
    Sad,
    /// Angry / forceful tone.
    Angry,
    /// Whispered / quiet tone.
    Whisper,
    /// Excited / energetic tone.
    Excited,
    /// Calm / soothing tone.
    Calm,
}

/// Descriptor for an available voice.
#[derive(Debug, Clone)]
pub struct Voice {
    /// Provider-specific voice identifier.
    pub id: String,
    /// Human-readable voice name.
    pub name: String,
    /// BCP-47 language code.
    pub language: String,
    /// Optional gender label.
    pub gender: Option<String>,
}

/// Request parameters for TTS synthesis.
#[derive(Debug, Clone)]
pub struct TtsRequest {
    /// Text to synthesize.
    pub text: String,
    /// Voice identifier.
    pub voice: String,
    /// Optional BCP-47 language code.
    pub language: Option<String>,
    /// Speaking speed multiplier (0.5–2.0, default 1.0).
    pub speed: f32,
    /// Optional pitch adjustment.
    pub pitch: Option<f32>,
    /// Optional emotion hint.
    pub emotion: Option<Emotion>,
    /// Processing mode for the audio output.
    pub output_mode: TtsOutputMode,
}

impl Default for TtsRequest {
    fn default() -> Self {
        Self {
            text: String::new(),
            voice: String::new(),
            language: None,
            speed: 1.0,
            pitch: None,
            emotion: None,
            output_mode: TtsOutputMode::default(),
        }
    }
}

/// Unified trait for text-to-speech providers.
///
/// Implementors include cloud services (ElevenLabs, OpenAI, Gemini, Cartesia)
/// and local models (MLX Kokoro, ONNX Kokoro).
#[async_trait]
pub trait TtsProvider: Send + Sync {
    /// Synthesize text to a single audio payload (batch mode).
    async fn synthesize(&self, request: &TtsRequest) -> AudioResult<AudioPayload>;

    /// Synthesize text as a stream of audio payloads (streaming mode).
    ///
    /// The default implementation waits for the full synthesis to complete
    /// and yields a single item stream. Providers supporting native streaming
    /// should override this.
    async fn synthesize_stream(
        &self,
        request: &TtsRequest,
    ) -> AudioResult<Pin<Box<dyn Stream<Item = AudioResult<AudioPayload>> + Send>>> {
        let payload = self.synthesize(request).await?;
        Ok(Box::pin(futures::stream::once(async move { Ok(payload) })))
    }

    /// List available voices for this provider.
    fn voice_catalog(&self) -> &[Voice];
}
