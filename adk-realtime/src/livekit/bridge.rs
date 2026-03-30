//! Bridge functions for connecting LiveKit audio tracks to a [`RealtimeRunner`].

use futures::StreamExt;
use livekit::prelude::{RemoteTrack, RoomEvent};
use livekit::track::RemoteAudioTrack;
use livekit::webrtc::audio_stream::native::NativeAudioStream;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::audio::{AudioChunk, AudioFormat, SmartAudioBuffer};
use crate::error::Result;
use crate::runner::RealtimeRunner;

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
    let mut stream =
        NativeAudioStream::new(track.rtc_track(), DEFAULT_SAMPLE_RATE, DEFAULT_NUM_CHANNELS);
    let mut buffer = SmartAudioBuffer::new(DEFAULT_SAMPLE_RATE as u32, BUFFER_DURATION_MS);

    while let Some(frame) = stream.next().await {
        buffer.push(&frame.data);
        if let Some(samples) = buffer.flush() {
            // Convert i16 samples to little-endian PCM16 bytes
            let chunk = AudioChunk::from_i16_samples(&samples, AudioFormat::pcm16_24khz());
            runner.send_audio(&chunk.to_base64()).await?;
        }
    }

    if let Some(samples) = buffer.flush_remaining() {
        let chunk = AudioChunk::from_i16_samples(&samples, AudioFormat::pcm16_24khz());
        runner.send_audio(&chunk.to_base64()).await?;
    }

    Ok(())
}

/// A high-level helper that waits for the first remote participant to publish an audio track,
/// and automatically bridges it to the `RealtimeRunner`.
///
/// This avoids a common pitfall where users block the main async task on `bridge_input`
/// while continuing to hold the unbounded `RoomEvent` receiver, resulting in an unhandled
/// event memory leak over the life of the room connection.
///
/// # Arguments
///
/// * `room_events` - The LiveKit room event receiver. This function consumes and drops it.
/// * `runner` - The target `RealtimeRunner`.
///
/// # Example
///
/// ```rust,ignore
/// use adk_realtime::livekit::{LiveKitRoomBuilder, wait_and_bridge_audio};
///
/// let (_room, room_events, _) = LiveKitRoomBuilder::new(config).connect("my-room").await?;
///
/// tokio::spawn(async move {
///     wait_and_bridge_audio(room_events, &runner).await.unwrap();
/// });
/// ```
pub async fn wait_and_bridge_audio(
    mut room_events: UnboundedReceiver<RoomEvent>,
    runner: &Arc<RealtimeRunner>,
) -> Result<()> {
    tracing::info!("Listening for remote participant audio tracks...");

    // Continuously poll the unbounded receiver to prevent LiveKit room events
    // from leaking memory over the lifespan of the connection.
    while let Some(event) = room_events.recv().await {
        if let RoomEvent::TrackSubscribed {
            track: RemoteTrack::Audio(audio_track),
            participant,
            ..
        } = event
        {
            tracing::info!(
                "Subscribed to audio track from participant: {}",
                participant.identity()
            );

            // Spawn the blocking bridge stream into its own task so we do not block
            // the main event loop. This allows us to keep draining `room_events`.
            let bridge_runner = Arc::clone(runner);
            tokio::spawn(async move {
                if let Err(e) = bridge_input(audio_track, &bridge_runner).await {
                    tracing::error!(
                        "Audio input bridge failed for participant {}: {}",
                        participant.identity(),
                        e
                    );
                }
            });

            // For a basic bridge, we only bind to the first active audio track we see.
            // A more complex production architecture would mix multiple participant tracks.
            break;
        }
    }

    // By the time we break out of the loop and reach here, `room_events` drops naturally,
    // which cleanly closes the unbounded channel without any hacky explicit `drop()` logic.

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
    // Request 16kHz mono from LiveKit — it handles resampling for us.
    let mut stream =
        NativeAudioStream::new(track.rtc_track(), GEMINI_SAMPLE_RATE, DEFAULT_NUM_CHANNELS);
    let mut buffer = SmartAudioBuffer::new(GEMINI_SAMPLE_RATE as u32, BUFFER_DURATION_MS);

    while let Some(frame) = stream.next().await {
        buffer.push(&frame.data);
        if let Some(samples) = buffer.flush() {
            let chunk = AudioChunk::from_i16_samples(&samples, AudioFormat::pcm16_16khz());
            runner.send_audio(&chunk.to_base64()).await?;
        }
    }

    if let Some(samples) = buffer.flush_remaining() {
        let chunk = AudioChunk::from_i16_samples(&samples, AudioFormat::pcm16_16khz());
        runner.send_audio(&chunk.to_base64()).await?;
    }

    Ok(())
}
