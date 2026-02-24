//! Bridge functions for connecting LiveKit audio tracks to a [`RealtimeRunner`].

use futures::StreamExt;
use livekit::track::RemoteAudioTrack;
use livekit::webrtc::audio_stream::native::NativeAudioStream;

use crate::audio::{AudioChunk, AudioFormat, SmartAudioBuffer};
use crate::error::Result;
use crate::runner::RealtimeRunner;

/// Native sample rate (48kHz) typically provided by LiveKit/WebRTC.
const NATIVE_SAMPLE_RATE: i32 = 48000;
/// Default sample rate for OpenAI-compatible audio (24kHz).
const DEFAULT_SAMPLE_RATE: i32 = 24000;
/// Gemini-expected sample rate (16kHz).
const GEMINI_SAMPLE_RATE: i32 = 16000;
/// Default number of audio channels (mono).
const DEFAULT_NUM_CHANNELS: i32 = 1;
/// Target duration for smart audio buffering (200ms).
const BUFFER_DURATION_MS: u32 = 200;

/// Reads audio frames from a LiveKit [`RemoteAudioTrack`] and sends them as
/// base64-encoded PCM16 audio (24kHz) to the given [`RealtimeRunner`].
///
/// This function runs continuously until the remote track stream ends, at which
/// point it returns `Ok(())`. If sending audio to the runner fails, the error
/// is propagated to the caller.
///
/// # Arguments
///
/// * `track` — The LiveKit remote audio track to read from.
/// * `runner` — The realtime runner to send audio to.
pub async fn bridge_input(track: RemoteAudioTrack, runner: &RealtimeRunner) -> Result<()> {
    // Request native 48kHz mono from LiveKit.
    let mut stream =
        NativeAudioStream::new(track.rtc_track(), NATIVE_SAMPLE_RATE, DEFAULT_NUM_CHANNELS);
    let mut buffer = SmartAudioBuffer::new(DEFAULT_SAMPLE_RATE as u32, BUFFER_DURATION_MS);

    let send_audio = |samples: &[i16]| {
        let base64 = AudioChunk::encode_i16_to_base64(samples);
        async move { runner.send_audio(&base64).await }
    };

    while let Some(frame) = stream.next().await {
        // Downsample 48kHz -> 24kHz (box filter)
        let downsampled = AudioChunk::downsample_48kHz_to_24kHz(&frame.data);
        buffer.push(&downsampled);

        if let Some(samples) = buffer.flush() {
            send_audio(&samples).await?;
        }
    }

    if let Some(samples) = buffer.flush_remaining() {
        send_audio(&samples).await?;
    }

    Ok(())
}

/// Reads audio frames from a LiveKit [`RemoteAudioTrack`], resamples to 16kHz
/// mono PCM16 (Gemini's expected format), and sends them to the given
/// [`RealtimeRunner`].
///
/// This is the Gemini-specific variant of [`bridge_input`]. Use this when the
/// realtime session is connected to a Gemini model that expects 16kHz input.
///
/// # Arguments
///
/// * `track` — The LiveKit remote audio track to read from.
/// * `runner` — The realtime runner to send audio to.
pub async fn bridge_gemini_input(track: RemoteAudioTrack, runner: &RealtimeRunner) -> Result<()> {
    // Request native 48kHz mono from LiveKit.
    let mut stream =
        NativeAudioStream::new(track.rtc_track(), NATIVE_SAMPLE_RATE, DEFAULT_NUM_CHANNELS);
    let mut buffer = SmartAudioBuffer::new(GEMINI_SAMPLE_RATE as u32, BUFFER_DURATION_MS);

    let send_audio = |samples: &[i16]| {
        let base64 = AudioChunk::encode_i16_to_base64(samples);
        async move { runner.send_audio(&base64).await }
    };

    while let Some(frame) = stream.next().await {
        // Downsample 48kHz -> 16kHz (box filter)
        let downsampled = AudioChunk::downsample_48kHz_to_16kHz(&frame.data);
        buffer.push(&downsampled);

        if let Some(samples) = buffer.flush() {
            send_audio(&samples).await?;
        }
    }

    if let Some(samples) = buffer.flush_remaining() {
        send_audio(&samples).await?;
    }

    Ok(())
}
