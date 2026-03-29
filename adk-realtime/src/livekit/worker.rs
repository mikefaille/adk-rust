//! High-level worker abstraction for automatically integrating LiveKit with a realtime agent.

use crate::error::{RealtimeError, Result};
use crate::livekit::bridge_input;
use crate::runner::RealtimeRunner;
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::audio_source::RtcAudioSource;
use livekit::webrtc::audio_source::native::NativeAudioSource as WebrtcNativeAudioSource;
use livekit_api::access_token::{AccessToken, VideoGrants};
use std::sync::Arc;

use super::config::LiveKitConfig;

/// A high-level helper function that automatically connects to a LiveKit room,
/// sets up the audio bridge (both input and output), and runs the provided `RealtimeRunner`.
///
/// This abstracts away the boilerplate of generating tokens, managing LiveKit tracks,
/// and spawning the audio bridging task.
///
/// # Arguments
///
/// * `config` - The `LiveKitConfig` containing the server URL, API key, and secret.
/// * `room_name` - The name of the LiveKit room to join.
/// * `agent_identity` - The identity the AI agent should use in the room.
/// * `runner` - The pre-configured, **connected** `RealtimeRunner` to bridge.
/// * `audio_source` - The `NativeAudioSource` that the runner's `LiveKitEventHandler` is using.
///                    This must be provided because the `RealtimeRunnerBuilder` needs the handler
///                    *before* it builds the runner, and the handler needs the `NativeAudioSource`.
///
/// # Example
///
/// ```rust,ignore
/// use adk_realtime::livekit::{LiveKitConfig, LiveKitEventHandler, run_agent_in_room};
/// use livekit::webrtc::audio_source::native::NativeAudioSource;
/// use livekit::webrtc::audio_source::AudioSourceOptions;
///
/// // 1. Create native audio source
/// let audio_source = NativeAudioSource::new(AudioSourceOptions::default(), 24000, 1);
///
/// // 2. Create the event handler with the source
/// let handler = LiveKitEventHandler::new(MyInnerHandler, audio_source.clone(), 24000, 1);
///
/// // 3. Build and connect runner
/// let runner = RealtimeRunner::builder().event_handler(handler).build().unwrap();
/// runner.connect().await.unwrap();
///
/// // 4. Automatically join room and bridge audio
/// let config = LiveKitConfig::from_env().unwrap();
/// run_agent_in_room(config, "my-room", "agent-01", Arc::new(runner), audio_source).await.unwrap();
/// ```
pub async fn run_agent_in_room(
    config: LiveKitConfig,
    room_name: &str,
    agent_identity: &str,
    runner: Arc<RealtimeRunner>,
    audio_source: WebrtcNativeAudioSource,
) -> Result<()> {
    // 1. Generate an access token to join the room
    let token = AccessToken::with_api_key(&config.api_key, &config.api_secret)
        .with_identity(agent_identity)
        .with_grants(VideoGrants {
            room_join: true,
            room: room_name.to_string(),
            ..Default::default()
        })
        .to_jwt()
        .map_err(|e| RealtimeError::livekit(format!("Token generation failed: {e}")))?;

    // 2. Connect to the room
    tracing::info!("Connecting to LiveKit room '{}'...", room_name);
    let (room, mut room_events) = Room::connect(&config.url, &token, RoomOptions::default())
        .await
        .map_err(|e| RealtimeError::connection(format!("LiveKit connect failed: {e}")))?;

    tracing::info!("Connected to room as participant '{}'", room.local_participant().identity());

    // 3. Publish the audio source as a local track
    let rtc_source = RtcAudioSource::Native(audio_source);
    let local_track = LocalAudioTrack::create_audio_track("ai-agent-audio", rtc_source);
    let publish_options = TrackPublishOptions::default();

    room.local_participant()
        .publish_track(LocalTrack::Audio(local_track), publish_options)
        .await
        .map_err(|e| RealtimeError::livekit(format!("Failed to publish track: {e}")))?;

    tracing::info!("Published AI agent audio track to room.");

    // 4. Wait for a remote participant's audio track
    tracing::info!("Waiting for a remote participant to publish an audio track...");
    let remote_track = loop {
        if let Some(RoomEvent::TrackSubscribed {
            track: RemoteTrack::Audio(audio_track),
            participant,
            ..
        }) = room_events.recv().await
        {
            tracing::info!(
                "Subscribed to audio track from participant: {}",
                participant.identity()
            );
            break audio_track;
        }
    };

    // 5. Spawn the input bridge background task
    let bridge_runner = Arc::clone(&runner);
    let bridge_handle = tokio::spawn(async move {
        if let Err(e) = bridge_input(remote_track, &bridge_runner).await {
            tracing::error!("Audio input bridge failed: {}", e);
        }
    });

    // 6. Run the agent's main event loop
    tracing::info!("Running agent event loop...");
    let result = runner.run().await;

    // 7. Cleanup
    bridge_handle.abort();
    let _ = runner.close().await;

    result
}
