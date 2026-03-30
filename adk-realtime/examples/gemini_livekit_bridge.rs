//! # Gemini LiveKit Bridge Example
//!
//! Demonstrates bridging a LiveKit room with a realtime AI model using the
//! `adk-realtime` LiveKit bridge module and the Gemini Live API.
//!
//! Shows how to:
//! - Connect to a LiveKit room
//! - Wrap an event handler with [`LiveKitEventHandler`] to publish model audio
//! - Use [`bridge_gemini_input`] to feed participant audio resampled to 16kHz mono to Gemini
//! - Run the event loop via [`RealtimeRunner`]
//!
//! ## Environment Variables
//!
//! | Variable          | Required | Description                                      |
//! |-------------------|----------|--------------------------------------------------|
//! | `GEMINI_API_KEY`  | **Yes**  | Gemini API key with realtime model access        |
//! | `LIVEKIT_URL`     | **Yes**  | LiveKit server WebSocket URL (e.g. `ws://localhost:7880`) |
//! | `LIVEKIT_TOKEN`   | **Yes**  | LiveKit access token for the room                |
//!
//! ## Running
//!
//! ```sh
//! cargo run -p adk-realtime --example gemini_livekit_bridge --features "livekit,gemini"
//! ```

use std::sync::Arc;

use adk_realtime::RealtimeConfig;
use adk_realtime::gemini::GeminiRealtimeModel;
use adk_realtime::livekit::{LiveKitEventHandler, bridge_gemini_input};
use adk_realtime::runner::{EventHandler, RealtimeRunner};

use livekit::options::TrackPublishOptions;
use livekit::prelude::*;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};

async fn connect_to_livekit()
-> Result<(Room, NativeAudioSource, livekit::track::RemoteAudioTrack), Box<dyn std::error::Error>> {
    let url = std::env::var("LIVEKIT_URL").expect("LIVEKIT_URL env var is required");
    let api_key = std::env::var("LIVEKIT_API_KEY").expect("LIVEKIT_API_KEY env var is required");
    let api_secret =
        std::env::var("LIVEKIT_API_SECRET").expect("LIVEKIT_API_SECRET env var is required");

    // Generate a token for the agent
    let token = livekit_api::access_token::AccessToken::with_api_key(&api_key, &api_secret)
        .with_identity("gemini-agent")
        .with_grants(livekit_api::access_token::VideoGrants {
            room_join: true,
            room: "ai-room".into(),
            can_publish: true,
            can_subscribe: true,
            can_publish_data: true,
            ..Default::default()
        })
        .to_jwt()?;

    // Connect to the room
    let (room, mut room_events) = Room::connect(&url, &token, RoomOptions::default()).await?;
    println!("Connected to LiveKit room: {}", room.name());

    // Create a native audio source for publishing model audio back to LiveKit.
    // Gemini Live natively outputs 24kHz audio.
    let audio_source = NativeAudioSource::new(AudioSourceOptions::default(), 24000, 1, 100);

    let rtc_source = RtcAudioSource::Native(audio_source.clone());
    let local_track = LocalAudioTrack::create_audio_track("ai-agent-audio", rtc_source);
    let publish_options = TrackPublishOptions::default();
    room.local_participant().publish_track(LocalTrack::Audio(local_track), publish_options).await?;
    println!("Published AI agent audio track to room.");

    // Wait for a remote participant's audio track to start listening
    println!("Waiting for a remote participant's audio track...");
    let remote_track = loop {
        if let Some(RoomEvent::TrackSubscribed { track: RemoteTrack::Audio(audio_track), .. }) =
            room_events.recv().await
        {
            println!("Subscribed to remote audio track.");
            break audio_track;
        }
    };

    Ok((room, audio_source, remote_track))
}

struct PrintingEventHandler;

#[async_trait::async_trait]
impl EventHandler for PrintingEventHandler {
    async fn on_text(&self, text: &str, _item_id: &str) -> adk_realtime::Result<()> {
        print!("{text}");
        Ok(())
    }

    async fn on_transcript(&self, transcript: &str, _item_id: &str) -> adk_realtime::Result<()> {
        print!("[transcript] {transcript}");
        Ok(())
    }

    async fn on_response_done(&self) -> adk_realtime::Result<()> {
        println!("\n--- Response complete ---");
        Ok(())
    }

    async fn on_error(&self, error: &adk_realtime::RealtimeError) -> adk_realtime::Result<()> {
        eprintln!("Realtime error: {}", error);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // 1. Create the Gemini realtime model
    let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY env var is required");
    let backend = adk_realtime::gemini::GeminiLiveBackend::Studio { api_key };
    let model = GeminiRealtimeModel::new(backend, "gemini-2.0-flash-exp");

    // 2. Connect to LiveKit room
    let (_room, audio_source, remote_track) = connect_to_livekit().await?;

    // 3. Wrap event handler with LiveKit audio output. Gemini returns 24kHz audio.
    let inner_handler = PrintingEventHandler;
    let lk_handler = LiveKitEventHandler::new(inner_handler, audio_source, 24000, 1);

    // 4. Build the RealtimeRunner configuration
    let config = RealtimeConfig::default()
        .with_instruction("You are a helpful and concise voice assistant.")
        .with_voice("Aoede"); // Use a Gemini voice

    let runner = Arc::new(
        RealtimeRunner::builder()
            .model(Arc::new(model))
            .config(config)
            .event_handler(lk_handler)
            .build()?,
    );

    // 5. Connect the runner to the AI model
    runner.connect().await?;
    println!("Connected to Gemini Live API.");

    // 6. Bridge participant audio to the model
    // Gemini Live requires 16kHz PCM audio input, so we use bridge_gemini_input
    // which automatically resamples LiveKit's audio to 16kHz.
    let bridge_runner = Arc::clone(&runner);
    let bridge_handle = tokio::spawn(async move {
        if let Err(e) = bridge_gemini_input(remote_track, &bridge_runner).await {
            eprintln!("Bridge input error: {e}");
        }
    });

    // 7. Run the event loop
    println!("Running event loop — speak into the LiveKit room...\n");
    if let Err(e) = runner.run().await {
        eprintln!("Runner error: {e}");
    }

    bridge_handle.abort();
    runner.close().await?;
    println!("Session closed.");
    Ok(())
}
