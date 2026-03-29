//! High-level abstraction for easily bridging a LiveKit room to a `RealtimeRunner`.

use crate::error::{RealtimeError, Result};
use crate::livekit::bridge_input;
use crate::runner::RealtimeRunner;
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::audio_source::RtcAudioSource;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

/// A high-level bridge that simplifies binding a LiveKit `Room` to an `adk-realtime` model.
///
/// This avoids the "too much abstraction" trap: the user retains full control over the LiveKit
/// connection, token generation, and the `Room` object (e.g., to subscribe to video tracks
/// or custom DataPackets). `LiveKitBridge` exclusively manages the tedious WebRTC audio boilerplate:
/// creating the `NativeAudioSource`, publishing the agent's voice back to the room, and bridging
/// incoming remote participant audio into the `RealtimeRunner`.
pub struct LiveKitBridge {
    runner: Arc<RealtimeRunner>,
    audio_source: NativeAudioSource,
}

impl LiveKitBridge {
    /// Constructs a new `LiveKitBridge`.
    ///
    /// # Arguments
    ///
    /// * `runner` - The pre-configured, **connected** `RealtimeRunner`.
    /// * `audio_source` - The `NativeAudioSource` that the runner's `LiveKitEventHandler` is using.
    pub fn new(runner: Arc<RealtimeRunner>, audio_source: NativeAudioSource) -> Self {
        Self { runner, audio_source }
    }

    /// Attaches the bridge to an active LiveKit room.
    ///
    /// This method performs two key actions:
    /// 1. It publishes the agent's `NativeAudioSource` as a local audio track in the room.
    /// 2. It spawns a background task that listens to the `room_events` stream. When a remote
    ///    participant publishes an audio track, it automatically routes that audio into the AI model.
    ///
    /// # Returns
    ///
    /// Returns a `JoinHandle` to the background event-listening task. You can abort this task
    /// if you need to tear down the bridge without dropping the `Room`.
    pub async fn attach(
        &self,
        room: &Room,
        mut room_events: UnboundedReceiver<RoomEvent>,
    ) -> Result<JoinHandle<()>> {
        // 1. Publish the agent's audio source as a track in the room.
        let rtc_source = RtcAudioSource::Native(self.audio_source.clone());
        let local_track = LocalAudioTrack::create_audio_track("ai-agent-audio", rtc_source);
        let publish_options = TrackPublishOptions::default();

        room.local_participant()
            .publish_track(LocalTrack::Audio(local_track), publish_options)
            .await
            .map_err(|e| {
                RealtimeError::livekit(format!("Failed to publish agent audio track: {e}"))
            })?;

        tracing::info!("Published AI agent audio track to room.");

        // 2. Spawn a background listener for incoming tracks
        let bridge_runner = Arc::clone(&self.runner);

        let handle = tokio::spawn(async move {
            tracing::info!("Waiting for remote participants to publish audio tracks...");

            // Wait for the first audio track to be subscribed
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

                    // Route this participant's audio into the realtime model in a separate task
                    // so we do not block the room event loop from being drained.
                    tokio::spawn(async move {
                        if let Err(e) = bridge_input(audio_track, &bridge_runner).await {
                            tracing::error!(
                                "Audio input bridge failed for participant {}: {}",
                                participant.identity(),
                                e
                            );
                        }
                    });

                    // For a basic bridge, we only bind to the first active audio track.
                    // Production architectures with many participants should employ an audio mixer.
                    break;
                }
            }

            // Explicitly drop the receiver since we are no longer processing events,
            // preventing the unbounded channel from leaking memory.
            drop(room_events);
        });

        Ok(handle)
    }
}
