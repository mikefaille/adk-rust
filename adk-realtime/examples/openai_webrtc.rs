//! OpenAI WebRTC Voice Test
//!
//! This example demonstrates a direct connection to the OpenAI Realtime API via WebRTC
//! using the adk-realtime crate.
//!
//! # Usage
//!
//! ```bash
//! export OPENAI_API_KEY="your-api-key"
//! cargo run --example openai_webrtc --features webrtc,openai
//! ```

use adk_realtime::openai::OpenAiWebRtcModel;
use adk_realtime::config::RealtimeConfig;
use adk_realtime::events::ServerEvent;
use adk_realtime::model::RealtimeModel;

use std::process::ExitCode;
use tracing::{error, info};

const TEST_PROMPT: &str = "Hello! Please introduce yourself using WebRTC.";

async fn run_webrtc_test(api_key: &str) -> Result<(), Box<dyn std::error::Error>> {
    info!("Initializing OpenAI WebRTC connection...");

    // 1. Create the realtime model
    let model = OpenAiWebRtcModel::new("gpt-4o-realtime-preview-2024-12-17", api_key);
    info!(model_id = model.model_id(), provider = model.provider(), "Model configured");

    // 2. Build config
    let config = RealtimeConfig::default()
        .with_instruction("You are a helpful assistant.");

    // 3. Connect (Stubbed)
    info!("Connecting to OpenAI Realtime API (WebRTC)...");
    let session = model.connect(config).await?;
    info!(session_id = session.session_id(), "Connected successfully!");

    // 4. Send text input
    info!(prompt = TEST_PROMPT, "Sending text prompt...");
    session.send_text(TEST_PROMPT).await?;

    // 5. Wait for events (Stubbed loop will likely just exit or hang)
    info!("Waiting for events...");
    let mut events = session.events();
    use futures::StreamExt;
    
    // In stub implementation, this might return nothing immediately or hang
    match tokio::time::timeout(tokio::time::Duration::from_secs(5), events.next()).await {
        Ok(Some(Ok(event))) => {
            info!("Received event: {:?}", event);
        },
        Ok(None) => info!("Stream closed"),
        Err(_) => info!("Timeout waiting for events (Expected for stub)"),
        _ => {}
    }

    session.close().await?;
    info!("Session closed");

    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            error!("OPENAI_API_KEY environment variable is required");
            return ExitCode::FAILURE;
        }
    };

    match run_webrtc_test(&api_key).await {
        Ok(()) => {
            info!("Test completed");
            ExitCode::SUCCESS
        }
        Err(e) => {
            error!("Test failed: {}", e);
            ExitCode::FAILURE
        }
    }
}
