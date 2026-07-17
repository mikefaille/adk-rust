//! LiveKit event handler wrapper that publishes model audio to a LiveKit room.

use std::borrow::Cow;

use async_trait::async_trait;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;

use crate::error::{RealtimeError, Result};
use crate::runner::EventHandler;

#[derive(Default)]
pub(crate) struct RemainderState {
    pub(crate) pending_byte: Option<u8>,
    pub(crate) item_id: Option<String>,
}

impl RemainderState {
    pub(crate) fn new() -> Self {
        Self { pending_byte: None, item_id: None }
    }

    pub(crate) fn clear_pending_state(&mut self, boundary: &str) {
        if let Some(discarded) = self.pending_byte.take() {
            let item_id = self.item_id.take().unwrap_or_else(|| "unknown".to_string());
            tracing::warn!(
                item_id = %item_id,
                discarded_byte = discarded,
                boundary = %boundary,
                "Discarding incomplete PCM16 sample trailing byte at boundary"
            );
        }
        self.item_id = None;
    }

    pub(crate) fn assemble<'a>(&mut self, audio: &'a [u8], item_id: &str) -> Cow<'a, [i16]> {
        // 1. Handle item_id transition / boundary
        if let Some(ref old_id) = self.item_id {
            if old_id != item_id {
                if let Some(discarded) = self.pending_byte.take() {
                    tracing::warn!(
                        item_id = old_id,
                        next_item_id = item_id,
                        discarded_byte = discarded,
                        "Discarding incomplete PCM16 sample trailing byte on item boundary transition"
                    );
                }
                self.item_id = None;
            }
        }

        // 2. Process bytes
        if let Some(p_byte) = self.pending_byte.take() {
            // There is a pending byte from the SAME item_id!
            // We must prepend it to the new audio bytes.
            let total_len = audio.len() + 1;
            let mut fallback = Vec::with_capacity(total_len / 2);

            if !audio.is_empty() {
                let first_sample = i16::from_le_bytes([p_byte, audio[0]]);
                fallback.push(first_sample);

                let remaining = &audio[1..];
                let chunks_exact = remaining.chunks_exact(2);
                let rem = chunks_exact.remainder();
                for chunk in chunks_exact {
                    fallback.push(i16::from_le_bytes([chunk[0], chunk[1]]));
                }

                if !rem.is_empty() {
                    self.pending_byte = Some(rem[0]);
                    self.item_id = Some(item_id.to_string());
                } else {
                    self.item_id = None;
                }
            } else {
                self.pending_byte = Some(p_byte);
                self.item_id = Some(item_id.to_string());
            }

            Cow::Owned(fallback)
        } else {
            // No pending byte.
            let len = audio.len();
            if len % 2 == 0 {
                #[cfg(target_endian = "little")]
                if let Ok(aligned_slice) = bytemuck::try_cast_slice::<u8, i16>(audio) {
                    Cow::Borrowed(aligned_slice)
                } else {
                    let fallback: Vec<i16> = audio
                        .chunks_exact(2)
                        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
                        .collect();
                    Cow::Owned(fallback)
                }

                #[cfg(not(target_endian = "little"))]
                {
                    let fallback: Vec<i16> = audio
                        .chunks_exact(2)
                        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
                        .collect();
                    Cow::Owned(fallback)
                }
            } else {
                let chunks_exact = audio.chunks_exact(2);
                let rem = chunks_exact.remainder();
                self.pending_byte = Some(rem[0]);
                self.item_id = Some(item_id.to_string());

                let fallback: Vec<i16> =
                    chunks_exact.map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]])).collect();
                Cow::Owned(fallback)
            }
        }
    }
}

/// Wraps an inner [`EventHandler`] and intercepts `on_audio` to push PCM16 data
/// to a LiveKit [`NativeAudioSource`].
///
/// All non-audio event methods are delegated to the inner handler without modification.
/// If pushing audio to the `NativeAudioSource` fails, the error is logged via
/// `tracing::warn` and processing continues — audio push failures are never propagated.
pub struct LiveKitEventHandler<H: EventHandler> {
    inner: H,
    audio_source: NativeAudioSource,
    sample_rate: u32,
    num_channels: u32,
    state: parking_lot::Mutex<RemainderState>,
}

impl<H: EventHandler> LiveKitEventHandler<H> {
    /// Create a new `LiveKitEventHandler` wrapping the given inner handler.
    ///
    /// # Arguments
    ///
    /// * `inner` — The inner event handler to delegate to.
    /// * `audio_source` — The LiveKit native audio source to push model audio to.
    /// * `sample_rate` — Sample rate of the audio (e.g., 24000 for OpenAI, 16000 for Gemini).
    /// * `num_channels` — Number of audio channels (typically 1 for mono).
    pub fn new(
        inner: H,
        audio_source: NativeAudioSource,
        sample_rate: u32,
        num_channels: u32,
    ) -> Self {
        Self {
            inner,
            audio_source,
            sample_rate,
            num_channels,
            state: parking_lot::Mutex::new(RemainderState::new()),
        }
    }
}

#[async_trait]
impl<H: EventHandler> EventHandler for LiveKitEventHandler<H> {
    async fn on_audio(&self, audio: &[u8], item_id: &str) -> Result<()> {
        // Forward to inner handler first
        self.inner.on_audio(audio, item_id).await?;

        // Zero-Copy Architecture:
        // Local Edge: O(0) allocation via `bytemuck` pointer casts directly to C++ WebRTC FFI.
        // Global Core: `Cow::Borrowed` prevents `'a` lifetime infection of the async graph state.
        let samples_cow = self.state.lock().assemble(audio, item_id);

        if samples_cow.is_empty() {
            return Ok(());
        }

        if self.num_channels == 0 {
            return Err(RealtimeError::provider(
                "Cannot push audio to LiveKit NativeAudioSource: num_channels is 0",
            ));
        }

        if samples_cow.len() % (self.num_channels as usize) != 0 {
            tracing::warn!(
                samples_len = samples_cow.len(),
                num_channels = self.num_channels,
                "Skipping invalid audio frame: sample count is not an exact multiple of channels"
            );
            return Ok(());
        }

        // Guaranteed exact division (modulo == 0) and non-zero denominator by safety guards above.
        let samples_per_channel = samples_cow.len() as u32 / self.num_channels;
        let frame = AudioFrame {
            data: samples_cow,
            sample_rate: self.sample_rate,
            num_channels: self.num_channels,
            samples_per_channel,
        };
        if let Err(e) = self.audio_source.capture_frame(&frame).await {
            tracing::warn!(error = %e, "Failed to push audio to LiveKit NativeAudioSource");
        }
        Ok(())
    }

    async fn on_text(&self, text: &str, item_id: &str) -> Result<()> {
        self.inner.on_text(text, item_id).await
    }

    async fn on_transcript(&self, transcript: &str, item_id: &str) -> Result<()> {
        self.inner.on_transcript(transcript, item_id).await
    }

    async fn on_speech_started(&self, audio_start_ms: u64) -> Result<()> {
        self.inner.on_speech_started(audio_start_ms).await
    }

    async fn on_speech_stopped(&self, audio_end_ms: u64) -> Result<()> {
        self.inner.on_speech_stopped(audio_end_ms).await
    }

    async fn on_response_done(&self) -> Result<()> {
        self.state.lock().clear_pending_state("response_done");
        self.inner.on_response_done().await
    }

    async fn on_error(&self, error: &RealtimeError) -> Result<()> {
        self.state.lock().clear_pending_state("error");
        self.inner.on_error(error).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aligned_even_length() {
        let mut state = RemainderState::new();
        let input = vec![0x01, 0x02, 0x03, 0x04];
        let samples = state.assemble(&input, "item_a");
        assert_eq!(samples.as_ref(), &[0x0201, 0x0403]);
        assert_eq!(state.pending_byte, None);
        assert_eq!(state.item_id, None);
    }

    #[test]
    fn test_unaligned_even_length() {
        let mut state = RemainderState::new();
        // Create an unaligned slice by offsetting
        let input_vec = vec![0x00, 0x01, 0x02, 0x03, 0x04, 0x00];
        let input = &input_vec[1..5]; // [0x01, 0x02, 0x03, 0x04]
        let samples = state.assemble(input, "item_a");
        assert_eq!(samples.as_ref(), &[0x0201, 0x0403]);
        assert_eq!(state.pending_byte, None);
        assert_eq!(state.item_id, None);
    }

    #[test]
    fn test_two_same_item_split_reconstruct() {
        let mut state = RemainderState::new();

        // Chunk 1: odd length [0x01, 0x02, 0x03] -> produces [0x0201], leaves 0x03
        let samples1 = state.assemble(&[0x01, 0x02, 0x03], "item_a");
        assert_eq!(samples1.as_ref(), &[0x0201]);
        assert_eq!(state.pending_byte, Some(0x03));
        assert_eq!(state.item_id.as_deref(), Some("item_a"));

        // Chunk 2: odd length [0x04] -> prepends 0x03, produces [0x0403]
        let samples2 = state.assemble(&[0x04], "item_a");
        assert_eq!(samples2.as_ref(), &[0x0403]);
        assert_eq!(state.pending_byte, None);
        assert_eq!(state.item_id, None);
    }

    #[test]
    fn test_different_items_prevent_contamination() {
        let mut state = RemainderState::new();

        // Chunk 1 from item_a: [0x01, 0x02, 0x03] -> produces [0x0201], leaves 0x03
        let samples1 = state.assemble(&[0x01, 0x02, 0x03], "item_a");
        assert_eq!(samples1.as_ref(), &[0x0201]);
        assert_eq!(state.pending_byte, Some(0x03));

        // Chunk 2 from item_b: [0x04, 0x05] -> should discard 0x03, produce [0x0504]
        let samples2 = state.assemble(&[0x04, 0x05], "item_b");
        assert_eq!(samples2.as_ref(), &[0x0504]);
        assert_eq!(state.pending_byte, None);
        assert_eq!(state.item_id, None);
    }

    #[test]
    fn test_clear_at_boundary_response_done() {
        let mut state = RemainderState::new();

        // Chunk 1 from item_a: [0x01, 0x02, 0x03] -> leaves 0x03
        state.assemble(&[0x01, 0x02, 0x03], "item_a");
        assert_eq!(state.pending_byte, Some(0x03));

        state.clear_pending_state("response_done");
        assert_eq!(state.pending_byte, None);
        assert_eq!(state.item_id, None);
    }

    #[test]
    fn test_clear_at_boundary_error() {
        let mut state = RemainderState::new();

        // Chunk 1 from item_a: [0x01, 0x02, 0x03] -> leaves 0x03
        state.assemble(&[0x01, 0x02, 0x03], "item_a");
        assert_eq!(state.pending_byte, Some(0x03));

        state.clear_pending_state("error");
        assert_eq!(state.pending_byte, None);
        assert_eq!(state.item_id, None);
    }
}
