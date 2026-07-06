use crate::{
    dispatch::{
        CallControlEvent, CallControlProvider, CallHandle, OriginateContext, RedirectTarget,
    },
    error::{RealtimeError, Result},
};
use async_trait::async_trait;
use futures::Stream;
use livekit_api::services::sip::SipClient;
use livekit_api::services::room::RoomServiceClient;
use std::pin::Pin;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use futures::StreamExt;

/// Call control provider for LiveKit SIP.
pub struct LiveKitCallControlProvider {
    sip_client: SipClient,
    room_client: RoomServiceClient,
    trunk_id: String,
    event_tx: broadcast::Sender<(String, CallControlEvent)>,
}

impl LiveKitCallControlProvider {
    /// Create a new LiveKitCallControlProvider.
    pub fn new(url: &str, api_key: &str, api_secret: &str, trunk_id: &str) -> Self {
        let (event_tx, _) = broadcast::channel(100);
        Self {
            sip_client: SipClient::with_api_key(url, api_key, api_secret),
            room_client: RoomServiceClient::with_api_key(url, api_key, api_secret),
            trunk_id: trunk_id.to_string(),
            event_tx,
        }
    }

    /// Handle a room event from LiveKit (e.g., via webhook or server-side observer).
    /// `participant_identity` should match the `provider_call_id` in `CallHandle`.
    pub fn handle_participant_event(&self, participant_identity: &str, event: CallControlEvent) {
        let _ = self.event_tx.send((participant_identity.to_string(), event));
    }
}

#[async_trait]
impl CallControlProvider for LiveKitCallControlProvider {
    async fn originate(&self, phone_number: &str, context: OriginateContext) -> Result<CallHandle> {
        let room_name = context
            .extra
            .as_ref()
            .and_then(|e| e.get("room_name"))
            .and_then(|v| v.as_str())
            .unwrap_or("sip_room");

        let identity = context
            .extra
            .as_ref()
            .and_then(|e| e.get("identity"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("sip_{}", uuid::Uuid::new_v4()));

        let _sip_call = self
            .sip_client
            .create_sip_participant(
                self.trunk_id.clone(),
                phone_number.to_string(),
                room_name.to_string(),
                identity.clone(),
                None, // metadata
            )
            .await
            .map_err(|e| RealtimeError::provider(format!("LiveKit SIP error: {}", e)))?;

        Ok(CallHandle {
            // Use identity as the provider_call_id because it's what's used
            // to identify the participant in RoomServiceClient methods.
            provider_call_id: identity,
            room_name: Some(room_name.to_string()),
        })
    }

    fn events(
        &self,
        handle: &CallHandle,
    ) -> Pin<Box<dyn Stream<Item = Result<CallControlEvent>> + Send>> {
        let participant_identity = handle.provider_call_id.clone();
        let rx = self.event_tx.subscribe();

        let stream = BroadcastStream::new(rx).filter_map(move |res| {
            let participant_identity = participant_identity.clone();
            async move {
                match res {
                    Ok((id, event)) if id == participant_identity => Some(Ok(event)),
                    _ => None,
                }
            }
        });

        Box::pin(stream)
    }

    async fn redirect(&self, _handle: &CallHandle, _destination: RedirectTarget) -> Result<()> {
        // LiveKit SIP doesn't expose a direct redirect/refer API via SipClient yet.
        Err(RealtimeError::provider("Redirect not implemented for LiveKit SIP"))
    }

    async fn hangup(&self, handle: &CallHandle) -> Result<()> {
        let room_name = handle
            .room_name
            .as_ref()
            .ok_or_else(|| RealtimeError::provider("Missing room_name in CallHandle"))?;

        self.room_client
            .remove_participant(room_name.clone(), handle.provider_call_id.clone())
            .await
            .map_err(|e| RealtimeError::provider(format!("LiveKit remove participant error: {}", e)))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_livekit_event_translation() {
        let provider = LiveKitCallControlProvider::new(
            "http://localhost:7880",
            "key",
            "secret",
            "ST_123",
        );
        let handle = CallHandle {
            provider_call_id: "sip_123".to_string(),
            room_name: Some("room".to_string())
        };

        let mut events = provider.events(&handle);

        provider.handle_participant_event("sip_123", CallControlEvent::Answered);

        let event = events.next().await.unwrap().unwrap();
        assert!(matches!(event, CallControlEvent::Answered));
    }
}
