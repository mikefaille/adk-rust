use crate::livekit::config::LiveKitConfig;
use crate::livekit::error::LiveKitError;
use livekit::prelude::{Room, RoomOptions};

/// A builder for creating and connecting to a LiveKit room.
pub struct LiveKitRoomBuilder {
    config: LiveKitConfig,
    identity: String,
    name: Option<String>,
    room_name: Option<String>,
    options: RoomOptions,
    grants: Option<livekit_api::access_token::VideoGrants>,
}

impl LiveKitRoomBuilder {
    /// Create a new builder with the given configuration and identity.
    pub fn new(config: LiveKitConfig, identity: impl Into<String>) -> Self {
        Self {
            config,
            identity: identity.into(),
            name: None,
            room_name: None,
            options: RoomOptions::default(),
            grants: None,
        }
    }

    /// Set the participant name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the room name to join.
    pub fn room_name(mut self, room_name: impl Into<String>) -> Self {
        self.room_name = Some(room_name.into());
        self
    }

    /// Set the room options.
    pub fn options(mut self, options: RoomOptions) -> Self {
        self.options = options;
        self
    }

    /// Set auto subscribe option.
    pub fn auto_subscribe(mut self, auto_subscribe: bool) -> Self {
        self.options.auto_subscribe = auto_subscribe;
        self
    }

    /// Set explicit video grants for the participant.
    /// If not set, standard room join grants are automatically generated.
    pub fn grants(mut self, grants: livekit_api::access_token::VideoGrants) -> Self {
        self.grants = Some(grants);
        self
    }

    /// Connect to the LiveKit room.
    pub async fn connect(
        self,
    ) -> Result<
        (Room, tokio::sync::mpsc::UnboundedReceiver<livekit::prelude::RoomEvent>),
        LiveKitError,
    > {
        if self.identity.is_empty() {
            return Err(LiveKitError::ConfigError(
                "Cannot connect to LiveKit with an empty identity".to_string(),
            ));
        }

        if let Some(room) = &self.room_name {
            if room.is_empty() {
                return Err(LiveKitError::ConfigError(
                    "Cannot connect to LiveKit with an empty room_name".to_string(),
                ));
            }
        }

        let mut final_grants = self.grants.unwrap_or_default();
        if let Some(room) = &self.room_name {
            final_grants.room_join = true;
            final_grants.room = room.clone();
        }

        let token = self.config.generate_token_with_name(
            &self.identity,
            self.name.as_deref(),
            Some(final_grants),
        )?;

        tracing::info!(room_name = ?self.room_name, identity = %self.identity, "connecting to livekit.room");
        let (room, events) = Room::connect(&self.config.url, &token, self.options)
            .await
            .map_err(|e| LiveKitError::ConnectionError(e))?;

        tracing::info!(participant = %room.local_participant().identity(), "connected to livekit.room");
        Ok((room, events))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_livekit_builder_options() {
        let config = LiveKitConfig::new("wss://test.url", "key", "secret").unwrap();
        let grants = livekit_api::access_token::VideoGrants {
            room_join: true,
            room: "test-room".to_string(),
            ..Default::default()
        };

        let builder = LiveKitRoomBuilder::new(config, "agent1")
            .name("Agent")
            .room_name("test-room")
            .auto_subscribe(false)
            .grants(grants.clone());

        assert_eq!(builder.identity, "agent1");
        assert_eq!(builder.name.as_deref(), Some("Agent"));
        assert_eq!(builder.room_name.as_deref(), Some("test-room"));
        assert_eq!(builder.options.auto_subscribe, false);
        assert_eq!(builder.grants.unwrap().room, "test-room");
    }

    #[tokio::test]
    #[ignore]
    async fn test_livekit_builder_connect_integration() {
        let url = std::env::var("LIVEKIT_URL").unwrap_or_else(|_| "ws://localhost:7880".into());
        let key = std::env::var("LIVEKIT_API_KEY").unwrap_or_else(|_| "devkey".into());
        let secret = std::env::var("LIVEKIT_API_SECRET").unwrap_or_else(|_| "secret".into());

        let config = LiveKitConfig::new(url, key, secret).unwrap();
        let builder = LiveKitRoomBuilder::new(config, "test-agent")
            .room_name("test-room");

        // Should fail gracefully if credentials are bad, or succeed if a local server is running.
        let _ = builder.connect().await;
    }

    #[tokio::test]
    async fn test_livekit_builder_empty_identity_connect() {
        let config = LiveKitConfig::new("wss://test.url", "key", "secret").unwrap();
        let builder = LiveKitRoomBuilder::new(config, "");

        let result = builder.connect().await;
        assert!(matches!(result, Err(LiveKitError::ConfigError(_))));
    }
}
