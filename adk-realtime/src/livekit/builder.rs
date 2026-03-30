//! High-level builder for establishing LiveKit agent connections.

use crate::livekit::error::{LiveKitError, Result};
use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
use livekit_api::access_token::VideoGrants;
use tokio::sync::mpsc::UnboundedReceiver;

use super::config::LiveKitConfig;

const DEFAULT_IDENTITY: &str = "ai-agent";
const DEFAULT_SAMPLE_RATE: u32 = 24000;
const DEFAULT_NUM_CHANNELS: u32 = 1;
const DEFAULT_QUEUE_SIZE_MS: u32 = 100;

/// A builder for establishing an active connection to a LiveKit room and preparing
/// the required WebRTC audio interfaces.
///
/// This isolates connection and track-publishing logic (actions) from the `LiveKitConfig` (data).
///
/// # Example
///
/// ```rust,ignore
/// use adk_realtime::livekit::{LiveKitConfig, LiveKitRoomBuilder};
///
/// let config = LiveKitConfig::from_env()?;
///
/// // The builder consumes the config and performs the async actions
/// let (room, events, audio_source) = LiveKitRoomBuilder::new(config)
///     .identity("agent-01")
///     .sample_rate(24000)
///     .connect("my-room")
///     .await?;
/// ```
#[derive(Debug)]
pub struct LiveKitRoomBuilder {
    config: LiveKitConfig,
    identity: String,
    sample_rate: u32,
    num_channels: u32,
    queue_size_ms: u32,
    grants: Option<VideoGrants>,
    metadata: Option<String>,
}

impl LiveKitRoomBuilder {
    /// Creates a new `LiveKitRoomBuilder` initialized with the given configuration.
    pub fn new(config: LiveKitConfig) -> Self {
        Self {
            config,
            identity: DEFAULT_IDENTITY.to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            num_channels: DEFAULT_NUM_CHANNELS,
            queue_size_ms: DEFAULT_QUEUE_SIZE_MS,
            grants: None,
            metadata: None,
        }
    }

    /// Sets the identity the agent will use when joining the room.
    /// Defaults to `"ai-agent"`.
    pub fn identity(mut self, identity: impl Into<String>) -> Result<Self> {
        let id = identity.into();
        if id.trim().is_empty() {
            return Err(LiveKitError::Config("identity cannot be empty".to_string()));
        }
        self.identity = id;
        Ok(self)
    }

    /// Sets custom metadata (like a tenant_id or session context) on the participant.
    pub fn metadata(mut self, metadata: impl Into<String>) -> Self {
        self.metadata = Some(metadata.into());
        self
    }

    /// Sets the sample rate for the agent's audio output.
    /// Defaults to `24000` (which is standard for OpenAI models).
    pub fn sample_rate(mut self, rate: u32) -> Result<Self> {
        if rate == 0 {
            return Err(LiveKitError::Config("sample_rate must be greater than 0".to_string()));
        }
        self.sample_rate = rate;
        Ok(self)
    }

    /// Sets the number of audio channels for the agent's audio output.
    /// Defaults to `1` (mono).
    pub fn num_channels(mut self, channels: u32) -> Result<Self> {
        if channels == 0 {
            return Err(LiveKitError::Config("num_channels must be greater than 0".to_string()));
        }
        self.num_channels = channels;
        Ok(self)
    }

    /// Sets custom permissions (`VideoGrants`) to be encoded into the agent's JWT.
    ///
    /// Despite the name `VideoGrants` in the official LiveKit API, this structure configures
    /// **all** capabilities for the participant (e.g., audio publishing, subscribing to data
    /// channels, connecting as a hidden participant, etc.).
    ///
    /// If not provided, it defaults to basic `room_join` permissions for the specified room.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use adk_realtime::livekit::LiveKitRoomBuilder;
    /// use livekit_api::access_token::VideoGrants;
    ///
    /// // Give the agent permission to join the room and publish data messages
    /// let grants = VideoGrants {
    ///     room_join: true,
    ///     room: "my-room".to_string(),
    ///     can_publish_data: true,
    ///     ..Default::default()
    /// };
    ///
    /// let builder = LiveKitRoomBuilder::new(config)
    ///     .grants(grants);
    /// ```
    pub fn grants(mut self, grants: VideoGrants) -> Self {
        self.grants = Some(grants);
        self
    }

    /// Finalizes the configuration and prepares the connection intent.
    ///
    /// This synchronous validation step ensures all inputs are valid before crossing the
    /// async boundary to perform the physical WebSocket connection.
    pub fn build(self, room_name: &str) -> Result<PreparedLiveKitConnection> {
        if room_name.trim().is_empty() {
            return Err(LiveKitError::Config("room_name cannot be empty".to_string()));
        }

        Ok(PreparedLiveKitConnection { builder: self, room_name: room_name.to_string() })
    }
}

/// A validated, prepared connection intent to a LiveKit room.
///
/// This struct holds a builder that has passed synchronous configuration validation
/// and is guaranteed to be safe for attempting an asynchronous connection.
#[derive(Debug)]
pub struct PreparedLiveKitConnection {
    builder: LiveKitRoomBuilder,
    room_name: String,
}

impl PreparedLiveKitConnection {
    /// Connects to the LiveKit room, sets up a local audio track for the agent, and publishes it.
    ///
    /// This method eliminates the boilerplate of token generation, `Room::connect`, and WebRTC
    /// `NativeAudioSource` publishing. It yields the active `Room` and its event receiver (giving
    /// you full control over the session) along with the ready-to-use `NativeAudioSource` that
    /// you can plug directly into `LiveKitEventHandler`.
    pub async fn connect(self) -> Result<(Room, UnboundedReceiver<RoomEvent>, NativeAudioSource)> {
        let b = self.builder;

        // 1. Generate an access token
        let token = b.config.generate_token(&self.room_name, &b.identity, b.grants, b.metadata)?;

        // 2. Connect to the Room
        tracing::info!("Connecting to LiveKit room '{}'...", self.room_name);
        let (room, room_events) =
            Room::connect(&b.config.url, &token, RoomOptions::default()).await?;

        tracing::info!(
            "Connected to room as participant '{}'",
            room.local_participant().identity()
        );

        // 3. Create a native audio source for publishing model audio
        let audio_source = NativeAudioSource::new(
            AudioSourceOptions::default(),
            b.sample_rate,
            b.num_channels,
            b.queue_size_ms,
        );

        let rtc_source = RtcAudioSource::Native(audio_source.clone());
        let local_track =
            LocalAudioTrack::create_audio_track(&format!("{}-audio", b.identity), rtc_source);
        let publish_options = TrackPublishOptions::default();

        room.local_participant()
            .publish_track(LocalTrack::Audio(local_track), publish_options)
            .await?;

        tracing::info!("Published AI agent audio track to room.");

        Ok((room, room_events, audio_source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use livekit_api::access_token::VideoGrants;

    fn get_dummy_config() -> LiveKitConfig {
        LiveKitConfig::new(
            "ws://localhost:7880".to_string(),
            "dummy_key".to_string(),
            "dummy_secret".to_string(),
        )
    }

    #[test]
    fn test_builder_defaults() {
        let config = get_dummy_config();
        let builder = LiveKitRoomBuilder::new(config);

        assert_eq!(builder.identity, DEFAULT_IDENTITY);
        assert_eq!(builder.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(builder.num_channels, DEFAULT_NUM_CHANNELS);
        assert_eq!(builder.queue_size_ms, DEFAULT_QUEUE_SIZE_MS);
        assert!(builder.grants.is_none());
    }

    #[test]
    fn test_builder_setters() {
        let config = get_dummy_config();
        let grants =
            VideoGrants { room_join: true, room: "test-room".to_string(), ..Default::default() };

        let builder = LiveKitRoomBuilder::new(config)
            .identity("custom-agent")
            .unwrap()
            .sample_rate(16000)
            .unwrap()
            .num_channels(2)
            .unwrap()
            .grants(grants.clone())
            .metadata("tenant-123");

        assert_eq!(builder.identity, "custom-agent");
        assert_eq!(builder.sample_rate, 16000);
        assert_eq!(builder.num_channels, 2);
        assert!(builder.grants.is_some());
        assert_eq!(builder.grants.unwrap().room, "test-room");
        assert_eq!(builder.metadata.unwrap(), "tenant-123");
    }

    #[test]
    fn test_builder_validation_empty_room() {
        let config = get_dummy_config();
        let builder = LiveKitRoomBuilder::new(config);

        let result = builder.build("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("room_name cannot be empty"));
    }

    #[test]
    fn test_builder_validation_empty_identity() {
        let config = get_dummy_config();
        let builder = LiveKitRoomBuilder::new(config).identity("   ");
        assert!(builder.is_err());
        assert!(builder.unwrap_err().to_string().contains("identity cannot be empty"));
    }

    #[test]
    fn test_builder_validation_zero_sample_rate() {
        let config = get_dummy_config();
        let builder = LiveKitRoomBuilder::new(config).sample_rate(0);
        assert!(builder.is_err());
        assert!(builder.unwrap_err().to_string().contains("sample_rate must be greater than 0"));
    }

    #[tokio::test]
    #[ignore]
    async fn test_builder_connect_integration() {
        // This test requires a running LiveKit server and valid credentials in the environment.
        // It's ignored by default so it doesn't break CI.
        let url = std::env::var("LIVEKIT_URL").unwrap_or_default();
        let key = std::env::var("LIVEKIT_API_KEY").unwrap_or_default();
        let secret = std::env::var("LIVEKIT_API_SECRET").unwrap_or_default();

        if url.is_empty() || key.is_empty() || secret.is_empty() {
            println!("Skipping integration test: missing LiveKit credentials in environment.");
            return;
        }

        let config = LiveKitConfig::new(url, key, secret);

        let builder = LiveKitRoomBuilder::new(config)
            .identity("test-agent")
            .unwrap()
            .sample_rate(24000)
            .unwrap()
            .num_channels(1)
            .unwrap();

        let result = builder.build("integration-test-room");
        assert!(result.is_ok(), "Failed to build connection: {:?}", result.err());

        let conn_result = result.unwrap().connect().await;
        assert!(conn_result.is_ok(), "Failed to connect to LiveKit: {:?}", conn_result.err());

        let (room, _events, _audio) = conn_result.unwrap();
        assert_eq!(room.name(), "integration-test-room");

        let _ = room.close().await;
    }
}
