//! # LiveKit WebRTC Bridge Example
//!
//! Demonstrates bridging a LiveKit room with a realtime AI model using the
//! `adk-realtime` LiveKit bridge module. Shows how to:
//!
//! - Connect to a LiveKit room
//! - Wrap an event handler with [`LiveKitEventHandler`] to publish model audio
//! - Use [`bridge_input`] to feed participant audio to the AI model
//! - Run the event loop via [`RealtimeRunner`]
//!
//! ## Prerequisites
//!
//! 1. A running [LiveKit](https://livekit.io/) server (local or cloud).
//! 2. An OpenAI API key with realtime access (or swap for Gemini).
//! 3. The `livekit` and `openai` features enabled for `adk-realtime`.
//!
//! ## Environment Variables
//!
//! | Variable          | Required | Description                                      |
//! |-------------------|----------|--------------------------------------------------|
//! | `OPENAI_API_KEY`  | **Yes**  | OpenAI API key with realtime model access        |
//! | `LIVEKIT_URL`     | **Yes**  | LiveKit server WebSocket URL (e.g. `ws://localhost:7880`) |
//! | `LIVEKIT_API_KEY` | **Yes**  | LiveKit server API Key                           |
//! | `LIVEKIT_API_SECRET`| **Yes**| LiveKit server API Secret                        |
//!
//! ## Running
//!
//! ```sh
//! cargo run -p adk-realtime --example livekit_bridge --features "livekit,openai"
//! ```
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────┐     audio frames      ┌──────────────────┐
//! │  LiveKit Room │ ──────────────────▶  │  bridge_input()  │
//! │  (participant) │  RemoteAudioTrack   │  PCM16 → Runner  │
//! └──────────────┘                       └────────┬─────────┘
//!                                                 │
//!                                                 ▼
//!                                        ┌──────────────────┐
//!                                        │ RealtimeRunner   │
//!                                        │ (OpenAI session) │
//!                                        └────────┬─────────┘
//!                                                 │
//!                                                 ▼
//! ┌──────────────┐     audio publish     ┌──────────────────────┐
//! │  LiveKit Room │ ◀────────────────── │ LiveKitEventHandler  │
//! │  (AI agent)   │  NativeAudioSource  │ wraps inner handler  │
//! └──────────────┘                       └──────────────────────┘
//! ```
//!
//! ## Note
//!
//! This example requires a real LiveKit server and room. It demonstrates loading
//! connection details from the environment via `LiveKitConfig::new` and using
//! the `LiveKitRoomBuilder` to seamlessly connect and wire the bridge elements.

use std::sync::Arc;

use adk_realtime::RealtimeConfig;
use adk_realtime::livekit::{
    LiveKitConfig, LiveKitEventHandler, LiveKitRoomBuilder, wait_and_bridge_audio,
};
use adk_realtime::openai::OpenAIRealtimeModel;
use adk_realtime::runner::{EventHandler, RealtimeRunner};

/// A simple event handler that prints text and transcript events.
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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    const SAMPLE_RATE: u32 = 24000;
    const NUM_CHANNELS: u32 = 1;

    // --- 1. Create the OpenAI realtime model ---
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY env var is required");
    let model = OpenAIRealtimeModel::new(api_key, "gpt-4o-realtime-preview-2024-12-17");

    // --- 2. Build the LiveKit Config ---
    // Manually load credentials from environment in the consumer app.
    // The `LiveKitConfig` stores the URL and securely wraps the key/secret.
    let lk_url = std::env::var("LIVEKIT_URL").expect("LIVEKIT_URL is required");
    let lk_api_key = std::env::var("LIVEKIT_API_KEY").expect("LIVEKIT_API_KEY is required");
    let lk_api_secret =
        std::env::var("LIVEKIT_API_SECRET").expect("LIVEKIT_API_SECRET is required");

    let lk_config = LiveKitConfig::new(lk_url, lk_api_key, lk_api_secret);

    // --- 3. Automatically Connect to LiveKit via Builder ---
    // The Builder separates the Config (data) from the connection action.
    // It synchronously validates your inputs, then generates the JWT token,
    // connects to the room, and publishes the agent's audio track automatically.
    println!("Connecting to LiveKit...");
    let builder = LiveKitRoomBuilder::new(lk_config)
        .identity("agent-01")?
        .sample_rate(SAMPLE_RATE)?
        .num_channels(NUM_CHANNELS)?;

    let connection = builder.build("my-room")?;
    let (_room, room_events, audio_source) = connection.connect().await?;

    // --- 4. Wrap event handler with LiveKit audio output ---
    // The LiveKitEventHandler intercepts `on_audio` events emitted by the
    // RealtimeRunner and pushes those PCM bytes to the NativeAudioSource.
    let inner_handler = PrintingEventHandler;
    let lk_handler =
        LiveKitEventHandler::new(inner_handler, audio_source, SAMPLE_RATE, NUM_CHANNELS);

    // --- 5. Build the RealtimeRunner ---
    // The RealtimeRunner acts as the core AI orchestrator in this example. It manages
    // the underlying connection to the OpenAI Realtime model and continuously routes
    // the generated PCM audio events back to our LiveKitEventHandler so the room can hear it.
    let config = RealtimeConfig::default()
        .with_instruction("You are a helpful voice assistant in a LiveKit room.")
        .with_voice("alloy");

    let runner = Arc::new(
        RealtimeRunner::builder()
            .model(Arc::new(model))
            .config(config)
            .event_handler(lk_handler)
            .build()?,
    );

    // --- 6. Connect the runner to the AI model ---
    runner.connect().await?;
    println!("Connected to OpenAI Realtime API.");

    // --- 7. Bridge incoming participant audio to the model ---
    // We spawn a background task that listens for a remote participant to speak.
    // When an audio track appears, `wait_and_bridge_audio` automatically binds it
    // to the `RealtimeRunner` so the AI can hear them.
    let bridge_runner = Arc::clone(&runner);
    let bridge_handle = tokio::spawn(async move {
        if let Err(e) = wait_and_bridge_audio(room_events, &bridge_runner).await {
            eprintln!("Audio input bridge failed: {e}");
        }
    });

    // --- 8. Run the event loop ---
    // This processes model responses and routes them through the
    // LiveKitEventHandler (which publishes audio back to the room).
    println!("Running event loop — speak into the LiveKit room...\n");
    if let Err(e) = runner.run().await {
        eprintln!("Runner error: {e}");
    }

    bridge_handle.abort();
    runner.close().await?;
    println!("Session closed.");
    Ok(())
}
