use crate::audio::AudioFormat;
use crate::error::RealtimeError;
use crate::events::{ClientEvent, ServerEvent};
use async_trait::async_trait;
// use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

// use rtc::peer_connection::configuration::RTCConfiguration;
// use rtc::api::APIBuilder;
// Media/Track paths are different in rtc 0.8.5, stubbing for now
// use rtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
// use rtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;

use tokio::net::UdpSocket;
// use uuid::Uuid;
// use std::time::Duration;
// use std::time::Instant;

use crate::model::RealtimeModel;
use crate::RealtimeSession; // Fixed import
use crate::config::RealtimeConfig;
use crate::error::Result;

const OPENAI_BASE_URL: &str = "https://api.openai.com/v1/realtime";

pub struct OpenAiWebRtcModel {
    pub model_id: String,
    pub api_key: String,
}

impl OpenAiWebRtcModel {
    pub fn new(model_id: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            model_id: model_id.into(),
            api_key: api_key.into(),
        }
    }
}

pub struct WebRtcSession {
    tx: mpsc::Sender<ClientEvent>,
    receiver: Arc<Mutex<mpsc::Receiver<Result<ServerEvent>>>>,
    close_tx: mpsc::Sender<()>,
}

use futures::Stream;
use std::pin::Pin;

#[async_trait]
impl RealtimeSession for WebRtcSession {
    fn session_id(&self) -> &str {
        "webrtc-session-stub"
    }

    fn is_connected(&self) -> bool {
        true
    }

    async fn send_audio(&self, audio: &crate::audio::AudioChunk) -> Result<()> {
        // In WebRTC we typically use the Track write, but for this trait we wrap in Event
        // or we use a special internal command to write to track.
        // We reused ClientEvent::AudioDelta for internal command in the loop.
        self.send_event(ClientEvent::AudioDelta { 
            event_id: None,
            audio: audio.data.clone(),
            format: audio.format.clone(),
        }).await
    }

    async fn send_audio_base64(&self, _audio_base64: &str) -> Result<()> {
         Err(RealtimeError::connection("Base64 audio not supported in WebRTC"))
    }

    async fn send_text(&self, text: &str) -> Result<()> {
        // Send as ConversationItemCreate? Or just text delta (not supported by OAI Realtime easily)
        // Usually we send a message item.
        self.send_event(ClientEvent::ConversationItemCreate {
            item: serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{ "type": "input_text", "text": text }]
            }),
        }).await
    }

    async fn send_tool_response(&self, response: crate::events::ToolResponse) -> Result<()> {
        self.send_event(ClientEvent::ConversationItemCreate {
            item: serde_json::json!({
                "type": "function_call_output",
                "call_id": response.call_id,
                "output": response.output
            }),
        }).await
    }

    async fn commit_audio(&self) -> Result<()> {
        self.send_event(ClientEvent::InputAudioBufferCommit).await
    }

    async fn clear_audio(&self) -> Result<()> {
        self.send_event(ClientEvent::InputAudioBufferClear).await
    }

    async fn create_response(&self) -> Result<()> {
        self.send_event(ClientEvent::ResponseCreate { config: None }).await
    }

    async fn interrupt(&self) -> Result<()> {
        // WebRTC interrupt usually means clearing buffer and cancelling response
        self.send_event(ClientEvent::ResponseCancel).await?;
        self.send_event(ClientEvent::InputAudioBufferClear).await
    }

    async fn send_event(&self, event: ClientEvent) -> Result<()> {
        self.tx.send(event).await.map_err(|e| RealtimeError::connection(e.to_string()))
    }

    async fn next_event(&self) -> Option<Result<ServerEvent>> {
        let mut rx = self.receiver.lock().await;
        rx.recv().await
    }

    fn events(&self) -> Pin<Box<dyn Stream<Item = Result<ServerEvent>> + Send + '_>> {
        let receiver = self.receiver.clone();
        Box::pin(async_stream::try_stream! {
            loop {
                let mut rx = receiver.lock().await;
                match rx.recv().await {
                    Some(Ok(event)) => yield event,
                    Some(Err(e)) => yield Err(e)?,
                    None => break,
                }
            }
        })
    }

    async fn close(&self) -> Result<()> {
        let _ = self.close_tx.send(()).await;
        Ok(())
    }
}

#[async_trait]
impl RealtimeModel for OpenAiWebRtcModel {
    fn provider(&self) -> &str {
        "openai"
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn supported_input_formats(&self) -> Vec<AudioFormat> {
        vec![AudioFormat::pcm16_24khz()]
    }

    fn supported_output_formats(&self) -> Vec<AudioFormat> {
        vec![AudioFormat::pcm16_24khz()]
    }

    fn available_voices(&self) -> Vec<&str> {
        vec!["alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"]
    }

    async fn connect(&self, _config: RealtimeConfig) -> Result<Box<dyn RealtimeSession>> {
        let (tx, mut rx_cmd) = mpsc::channel(100);
        let (event_tx, rx_event) = mpsc::channel(100);
        let (close_tx, mut close_rx) = mpsc::channel(1);

        // 1. Create UDP Socket
        let addr = "0.0.0.0:0";
        let udp_socket = UdpSocket::bind(addr).await
            .map_err(|e| RealtimeError::connection(e.to_string()))?;
        let _loop_socket = Arc::new(udp_socket);

        // 2. Initialize PeerConnection
        // Attempting standard `webrtc` crate API usage, adjusting for v0.8.5 specifics
        let mut api_builder = rtc::api::APIBuilder::new();
        let api = api_builder.build();
        
        let config_rtc = rtc::peer_connection::configuration::RTCConfiguration {
            ice_servers: vec![],
            ..Default::default()
        }; 

        let mut pc = api.new_peer_connection(config_rtc)
            .await // .await might be needed if new_peer_connection is async in newer versions? Usually safe result.
            .map_err(|e| RealtimeError::connection(format!("PC Creation failed: {}", e)))?;

        // 3. Add Audio Transceiver (SendRecv)
        // OpenAI requires audio media section. 
        // We add a transceiver to ensure "audio" section exists in Offer.
        pc.add_transceiver_from_kind(
            rtc::rtp_transceiver::rtp_codec::RTCRtpCodecType::Audio, 
            None, // Init keys
        )
        .await
        .map_err(|e| RealtimeError::connection(format!("Failed to add transceiver: {}", e)))?;

        // 4. Create Offer
        let offer = pc.create_offer(None).await
            .map_err(|e| RealtimeError::connection(format!("Failed to create offer: {}", e)))?;
        
        pc.set_local_description(offer.clone()).await
            .map_err(|e| RealtimeError::connection(format!("Failed to set local description: {}", e)))?;

        let offer_sdp = offer.sdp;

        // 5. Signaling: Handler SDP Exchange via HTTP
        let client = reqwest::Client::new();
        let url = format!("{}?model={}", OPENAI_BASE_URL, self.model_id);
        
        // ... (Rest of signaling logic)
        let res = client.post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/sdp")
            .body(offer_sdp.clone())
            .send()
            .await
            .map_err(|e| RealtimeError::connection(format!("Signaling failed: {}", e)))?;

        if !res.status().is_success() {
             let text = res.text().await.unwrap_or_default();
             return Err(RealtimeError::connection(format!("OpenAI Signaling Error: {}", text)));
        }

        let answer_sdp = res.text().await
            .map_err(|e| RealtimeError::connection(format!("Failed to get SDP Answer: {}", e)))?;
        
        println!("Received SDP Answer: {}", answer_sdp);

        // 6. Set Remote Description
        let answer_desc = rtc::peer_connection::sdp::session_description::RTCSessionDescription::answer(answer_sdp.clone())
            .map_err(|e| RealtimeError::connection(format!("Invalid Answer SDP: {}", e)))?;

        pc.set_remote_description(answer_desc).await
             .map_err(|e| RealtimeError::connection(format!("Failed to set remote description: {}", e)))?;

        let session = WebRtcSession {
            tx,
            receiver: Arc::new(Mutex::new(rx_event)),
            close_tx,
        };

         // Spawn Loop (Stubbed for now, just keep channel alive)
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = close_rx.recv() => break,
                    Some(_cmd) = rx_cmd.recv() => {
                        // Handle command
                    }
                }
            }
        });

        Ok(Box::new(session))
    }
}
