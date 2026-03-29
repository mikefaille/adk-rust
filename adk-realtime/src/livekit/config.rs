//! Configuration structures for LiveKit integration.

use crate::error::{RealtimeError, Result};
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit_api::access_token::{AccessToken, VideoGrants};
use std::env;
use tokio::sync::mpsc::UnboundedReceiver;

/// Configuration for connecting to a LiveKit server.
///
/// **Design Note:**
/// This library provides two ways to instantiate configuration to satisfy different use cases:
/// 1. `LiveKitConfig::new(...)`: Explicit dependency injection. This avoids hardcoding
///    environment variable parsing into the core library, which is ideal for production deployments
///    using secret managers or custom configuration systems.
/// 2. `LiveKitConfig::from_env()`: Convenience loader. This automatically reads `LIVEKIT_URL`,
///    `LIVEKIT_API_KEY`, and `LIVEKIT_API_SECRET`, matching the standard conventions of the Go and
///    Python LiveKit SDK ecosystems for rapid development.
#[derive(Debug, Clone)]
pub struct LiveKitConfig {
    /// The WebSocket URL of the LiveKit server.
    pub url: String,
    /// The API key for authenticating with the LiveKit server.
    pub api_key: String,
    /// The API secret for authenticating with the LiveKit server.
    pub api_secret: String,
}

impl LiveKitConfig {
    /// Creates a new `LiveKitConfig` with the provided credentials.
    ///
    /// Use this constructor when loading configurations from a custom source (like a database,
    /// secret manager, or a `.toml` file) rather than relying directly on environment variables.
    ///
    /// # Arguments
    ///
    /// * `url` - The WebSocket URL of the LiveKit server (e.g., `ws://localhost:7880`).
    /// * `api_key` - The LiveKit API key.
    /// * `api_secret` - The LiveKit API secret.
    ///
    /// # Example
    ///
    /// ```rust
    /// use adk_realtime::livekit::LiveKitConfig;
    ///
    /// let config = LiveKitConfig::new(
    ///     "ws://localhost:7880".to_string(),
    ///     "your_api_key".to_string(),
    ///     "your_api_secret".to_string(),
    /// );
    /// ```
    pub fn new(url: String, api_key: String, api_secret: String) -> Self {
        Self { url, api_key, api_secret }
    }

    /// Creates a new `LiveKitConfig` from environment variables.
    ///
    /// This aligns with the conventions used in the LiveKit Go and Python SDKs, where clients
    /// will automatically pick up connection details from the environment if not explicitly provided.
    ///
    /// Requires the following environment variables to be set:
    /// - `LIVEKIT_URL`
    /// - `LIVEKIT_API_KEY`
    /// - `LIVEKIT_API_SECRET`
    ///
    /// # Errors
    ///
    /// Returns a `RealtimeError::ConfigError` if any of the required environment variables are missing.
    pub fn from_env() -> Result<Self> {
        let url = env::var("LIVEKIT_URL").map_err(|_| {
            RealtimeError::config(
                "Missing LIVEKIT_URL environment variable. Please set it to the LiveKit server WebSocket URL (e.g. `ws://localhost:7880`).",
            )
        })?;

        let api_key = env::var("LIVEKIT_API_KEY").map_err(|_| {
            RealtimeError::config(
                "Missing LIVEKIT_API_KEY environment variable. Please set it to your LiveKit API key.",
            )
        })?;

        let api_secret = env::var("LIVEKIT_API_SECRET").map_err(|_| {
            RealtimeError::config(
                "Missing LIVEKIT_API_SECRET environment variable. Please set it to your LiveKit API secret.",
            )
        })?;

        Ok(Self { url, api_key, api_secret })
    }

    /// Connects to a LiveKit room, sets up a local audio track for the agent, and publishes it.
    ///
    /// This method eliminates the boilerplate of token generation, `Room::connect`, and WebRTC
    /// `NativeAudioSource` publishing. It yields the active `Room` and its event receiver (giving
    /// you full control over the session) along with the ready-to-use `NativeAudioSource` that
    /// you can plug directly into `LiveKitEventHandler`.
    ///
    /// # Arguments
    ///
    /// * `room_name` - The name of the room to connect to.
    /// * `agent_identity` - The participant identity for the agent (e.g., `"agent-01"`).
    /// * `sample_rate` - The sample rate for the agent's audio output (e.g., `24000` for OpenAI).
    /// * `num_channels` - The number of audio channels (e.g., `1` for mono).
    pub async fn connect(
        &self,
        room_name: &str,
        agent_identity: &str,
        sample_rate: u32,
        num_channels: u32,
    ) -> Result<(Room, UnboundedReceiver<RoomEvent>, NativeAudioSource)> {
        // 1. Generate an access token
        let token = AccessToken::with_api_key(&self.api_key, &self.api_secret)
            .with_identity(agent_identity)
            .with_grants(VideoGrants {
                room_join: true,
                room: room_name.to_string(),
                ..Default::default()
            })
            .to_jwt()
            .map_err(|e| RealtimeError::livekit(format!("Token generation failed: {e}")))?;

        // 2. Connect to the Room
        tracing::info!("Connecting to LiveKit room '{}'...", room_name);
        let (room, room_events) = Room::connect(&self.url, &token, RoomOptions::default())
            .await
            .map_err(|e| RealtimeError::connection(format!("LiveKit connect failed: {e}")))?;

        tracing::info!(
            "Connected to room as participant '{}'",
            room.local_participant().identity()
        );

        // 3. Create a native audio source for publishing model audio
        let audio_source = NativeAudioSource::new(
            AudioSourceOptions::default(),
            sample_rate,
            num_channels,
            100, // queue_size_ms
        );

        let rtc_source = RtcAudioSource::Native(audio_source.clone());
        let local_track = LocalAudioTrack::create_audio_track("ai-agent-audio", rtc_source);
        let publish_options = TrackPublishOptions::default();

        room.local_participant()
            .publish_track(LocalTrack::Audio(local_track), publish_options)
            .await
            .map_err(|e| {
                RealtimeError::livekit(format!("Failed to publish agent audio track: {e}"))
            })?;

        tracing::info!("Published AI agent audio track to room.");

        Ok((room, room_events, audio_source))
    }
}
