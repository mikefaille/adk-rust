#![cfg(feature = "livekit")]
use crate::error::Result;
pub use ::livekit::*;
pub use ::livekit_api as api;
use crate::runner::{EventHandler, RealtimeRunner};
use crate::audio::{AudioChunk, AudioFormat, AudioEncoding};
use async_trait::async_trait;
// use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::StreamExt;
use livekit::prelude::RemoteAudioTrack;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use std::sync::Arc;

/// LiveKit Integration for Realtime AI Agents.
///
/// This module provides a provider-agnostic bridge between LiveKit's WebRTC transport
/// and the `adk-realtime` facade. It works with any model that implements the
/// `RealtimeModel` trait (e.g., Gemini or OpenAI).
///
/// The bridge handles bidirectional audio by:
/// 1. Subscribing to remote audio tracks and feeding them to the AI (`bridge_input`).
/// 2. Pushing AI-generated audio back to a LiveKit room via a `NativeAudioSource` (`LiveKitEventHandler`).

/// EventHandler that bridges audio to a LiveKit NativeAudioSource.
pub struct LiveKitEventHandler {
    source: NativeAudioSource,
    inner: Arc<dyn EventHandler>,
}

impl LiveKitEventHandler {
    pub fn new(source: NativeAudioSource, inner: Arc<dyn EventHandler>) -> Self {
        Self { source, inner }
    }
}

#[async_trait]
impl EventHandler for LiveKitEventHandler {
    async fn on_audio(&self, audio: &[u8], item_id: &str) -> Result<()> {
        // 1. Convert bytes to i16 (assuming PCM16 LE)
        // use AudioChunk helper
        let chunk = AudioChunk::pcm16_24khz(audio.to_vec());
        let samples = match chunk.to_i16_samples() {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("[LiveKit] Audio conversion error: {}", e);
                return Ok(());
            }
        };

        // 2. Push to LiveKit source
        // Gemini is 24kHz mono
        let num_samples = samples.len();
        self.source
            .capture_frame(&AudioFrame {
                data: samples.into(),
                sample_rate: 24000,
                num_channels: 1,
                samples_per_channel: num_samples as u32,
            })
            .await
            .map_err(|e| crate::error::RealtimeError::audio(e.to_string()))?;

        // 3. Delegate to inner
        self.inner.on_audio(audio, item_id).await
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
        self.inner.on_response_done().await
    }

    async fn on_error(&self, error: &crate::error::RealtimeError) -> Result<()> {
        self.inner.on_error(error).await
    }
}

/// Helper to bridge a LiveKit RemoteAudioTrack to a RealtimeRunner for Gemini.
///
/// This wrapper ensures the audio is resampled to 16kHz mono as required by Gemini Live.
pub fn bridge_gemini_input(track: RemoteAudioTrack, runner: Arc<RealtimeRunner>) {
    bridge_input(track, runner, 16000, 1);
}

/// Bridge a LiveKit RemoteAudioTrack to a RealtimeRunner.
///
/// Spawns a background task that reads audio frames from the track, converts them
/// to the specified sample rate/channels, and sends them to the runner.
///
/// # Note
///
/// If connecting to Gemini Live, you MUST use `sample_rate: 16000` and `channels: 1`
/// or use the `bridge_gemini_input` helper.
pub fn bridge_input(
    track: RemoteAudioTrack,
    runner: Arc<RealtimeRunner>,
    sample_rate: u32,
    channels: u32,
) {
    tokio::spawn(async move {
        tracing::info!("[Bridge] Starting audio bridge (Target: {}Hz, {}ch)...", sample_rate, channels);
        // Create a NativeAudioStream which handles resampling if needed.
        let mut reader =
            NativeAudioStream::new(track.rtc_track(), sample_rate as i32, channels as i32);

        while let Some(frame) = reader.next().await {
            // Convert i16 samples to bytes (LE) and base64 encode using AudioChunk
            let format = AudioFormat::new(sample_rate, channels as u8, 16, AudioEncoding::Pcm16);
            let chunk = AudioChunk::from_i16_samples(frame.data.as_ref(), format);

            let b64 = chunk.to_base64();
            if let Err(e) = runner.send_audio(&b64).await {
                tracing::error!("[Bridge] Failed to send audio to runner: {}", e);
                break;
            }
        }
        tracing::info!("[Bridge] Audio bridge ended");
    });
}
