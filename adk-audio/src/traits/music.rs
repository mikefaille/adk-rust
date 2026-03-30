//! Music generation provider trait and request types.

use async_trait::async_trait;

use crate::error::AudioResult;
use crate::frame::AudioFrame;

/// Request parameters for music generation.
#[derive(Debug, Clone, Default)]
pub struct MusicRequest<'a> {
    /// Text prompt describing the desired music.
    pub prompt: String,
    /// Desired duration in seconds.
    pub duration_secs: u32,
    /// Optional genre hint.
    pub genre: Option<String>,
    /// Optional tempo in beats per minute.
    pub bpm: Option<u32>,
    /// Optional musical key (e.g. "C major").
    pub key: Option<String>,
    /// Optional audio to continue from.
    pub continuation_audio: Option<AudioFrame<'a>>,
    /// Whether to generate instrumental-only (no vocals).
    pub instrumental: bool,
}

/// Unified trait for music generation providers.
#[async_trait]
pub trait MusicProvider: Send + Sync {
    /// Generate music from a text prompt.
    async fn generate(&self, request: &MusicRequest<'_>) -> AudioResult<AudioFrame<'static>>;

    /// List supported genre strings.
    fn supported_genres(&self) -> &[String];
}
