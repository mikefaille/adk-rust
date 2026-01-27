use adk_realtime::audio::AudioChunk;
use adk_realtime::config::RealtimeConfig;
use adk_realtime::events::ServerEvent;
use adk_realtime::gemini::GeminiRealtimeModel;
use adk_realtime::model::RealtimeModel;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;

#[tokio::test]
async fn test_gemini_bidi_flow() {
    // 1. Start Mock Server
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_url = format!("ws://{}/test", addr);

    let _server_handle = tokio::spawn(async move {
        let (stream, addr) = listener.accept().await.unwrap();
        println!("Server: Accepted TCP connection from {}", addr);
        let mut ws_stream = match accept_async(stream).await {
            Ok(s) => {
                println!("Server: WebSocket handshake successful");
                s
            }
            Err(e) => {
                eprintln!("Server: WebSocket handshake failed: {:?}", e);
                return;
            }
        };

        while let Some(msg) = ws_stream.next().await {
            let msg = msg.expect("Server: Error receiving message");
            if msg.is_text() {
                let text = msg.to_text().unwrap();
                let json: serde_json::Value = serde_json::from_str(text).unwrap();

                // Handle Setup
                if json.get("setup").is_some() {
                    let response = json!({
                        "setupComplete": {}
                    });
                    ws_stream
                        .send(tokio_tungstenite::tungstenite::Message::Text(response.to_string()))
                        .await
                        .unwrap();
                }
                // Handle Realtime Input (Audio)
                else if json.get("realtimeInput").is_some() {
                    // Send back an audio delta
                    let audio_delta = json!({
                        "serverContent": {
                            "modelTurn": {
                                "parts": [
                                    {
                                        "inlineData": {
                                            "mimeType": "audio/pcm;rate=24000",
                                            "data": "AAAA" // Dummy base64 audio
                                        }
                                    }
                                ]
                            }
                        }
                    });
                    ws_stream
                        .send(tokio_tungstenite::tungstenite::Message::Text(audio_delta.to_string()))
                        .await
                        .unwrap();
                }
            }
        }
    });

    // 2. Client Client
    // Wait a bit for server to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let model = GeminiRealtimeModel::new("dummy-key", "models/gemini-live-2.5-flash-native-audio")
        .with_base_url(&server_url);

    let config = RealtimeConfig::default();
    let session = model.connect(config).await.unwrap();

    // 3. Send Audio
    let audio_data = vec![0u8; 100];
    let chunk = AudioChunk::pcm16_16khz(audio_data);
    session.send_audio(&chunk).await.unwrap();

    // 4. Verify Response
    let mut events = session.events();

    // First event should be SessionCreated (from setupComplete)
    match events.next().await {
        Some(Ok(ServerEvent::SessionCreated { .. })) => {
            println!("Session created successfully");
        }
        other => panic!("Expected SessionCreated, got {:?}", other),
    }

    // Next event should be AudioDelta (response to our input)
    match events.next().await {
        Some(Ok(ServerEvent::AudioDelta { delta, .. })) => {
            assert_eq!(delta, "AAAA");
            println!("Received audio delta");
        }
        other => panic!("Expected AudioDelta, got {:?}", other),
    }

    // Clean up
    session.close().await.unwrap();
}
