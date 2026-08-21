#![cfg(feature = "gemini")]

use adk_realtime::RealtimeSession;
use adk_realtime::error::RealtimeError;
use adk_realtime::events::ServerEvent;
use adk_realtime::gemini::normalize_model_id;
use serde_json::json;

#[tokio::test]
async fn test_model_id_normalization_for_google_compliance() {
    assert_eq!(
        normalize_model_id("gemini-3.1-flash-live-preview"),
        "models/gemini-3.1-flash-live-preview"
    );
    assert_eq!(
        normalize_model_id("models/gemini-2.5-flash-native-audio"),
        "models/gemini-2.5-flash-native-audio"
    );
    assert_eq!(
        normalize_model_id("projects/test/publishers/google/models/gemini"),
        "projects/test/publishers/google/models/gemini"
    );
}

#[test]
fn test_tcp_connection_reset_classification() {
    let reset_errors = vec![
        RealtimeError::ConnectionError("read tcp 127.0.0.1:443: connection reset by peer".into()),
        RealtimeError::ConnectionError("ECONNRESET: connection lost".into()),
        RealtimeError::ConnectionError("Broken pipe on socket write".into()),
        RealtimeError::ConnectionError("connection aborted".into()),
    ];

    for err in &reset_errors {
        assert!(
            err.is_connection_reset(),
            "Error {:?} should be classified as connection reset",
            err
        );
    }

    let non_reset_errors = vec![
        RealtimeError::ConfigError("Invalid model ID".into()),
        RealtimeError::ProviderError("Broken pipe on socket write".into()),
        RealtimeError::Protocol("Connection closed abruptly".into()),
        RealtimeError::ConnectionError("connection closed gracefully".into()),
        RealtimeError::MessageError("ECONNRESET: connection lost".into()),
    ];

    for err in &non_reset_errors {
        assert!(
            !err.is_connection_reset(),
            "Error {:?} should NOT be classified as connection reset",
            err
        );
    }
}

#[tokio::test]
async fn test_mock_resumption_token_alignment() {
    // Verify dual-key framing: when Google sends newHandle in sessionResumptionUpdate,
    // translate_event emits ServerEvent::SessionUpdated with both resumeToken and resumeHandle.
    let server_frame = json!({
        "sessionResumptionUpdate": {
            "resumable": true,
            "newHandle": "test_resumption_token_xyz987"
        }
    })
    .to_string();

    let events =
        adk_realtime::gemini::GeminiRealtimeSession::translate_event_static(&server_frame).unwrap();
    assert_eq!(events.len(), 1);

    if let ServerEvent::SessionUpdated { session, .. } = &events[0] {
        assert_eq!(
            session.get("resumeToken").and_then(|v| v.as_str()),
            Some("test_resumption_token_xyz987")
        );
        assert_eq!(
            session.get("resumeHandle").and_then(|v| v.as_str()),
            Some("test_resumption_token_xyz987")
        );
    } else {
        panic!("Expected SessionUpdated event");
    }
}

#[tokio::test]
async fn test_mock_websocket_server_e2e_reconnect_resumption() {
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    // 1. Bind local TCP listener on random port
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}", local_addr);

    // 2. Spawn Mock Gemini Live WebSocket Server
    let server_task = tokio::spawn(async move {
        // --- Connection 1: Initial session setup & sessionResumptionUpdate emission ---
        let (stream1, _) = listener.accept().await.unwrap();
        let mut ws1 = accept_async(stream1).await.unwrap();

        // Expect BidiGenerateContentSetup
        if let Some(Ok(Message::Text(msg))) = ws1.next().await {
            let json: serde_json::Value = serde_json::from_str(&msg).unwrap();
            assert!(json.get("setup").is_some());
        } else {
            panic!("Expected setup frame on connection 1");
        }

        // Send setupComplete & sessionResumptionUpdate
        let setup_complete = json!({ "setupComplete": {} }).to_string();
        ws1.send(Message::Text(setup_complete.into())).await.unwrap();

        let resumption_update = json!({
            "sessionResumptionUpdate": {
                "resumable": true,
                "newHandle": "mock_resume_handle_e2e_123"
            }
        })
        .to_string();
        ws1.send(Message::Text(resumption_update.into())).await.unwrap();

        // Simulate abrupt disconnect / connection reset
        let _ = ws1.close(None).await;
        drop(ws1);
    });

    // 3. Connect client session to mock server URL
    let _backend = adk_realtime::gemini::GeminiLiveBackend::studio("mock_api_key");
    let _config = adk_realtime::config::RealtimeConfig {
        instruction: Some("Test instructions".into()),
        ..Default::default()
    };

    // Connect initially using connect_async via GeminiRealtimeSession
    // We connect to ws_url
    let (ws_client, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let (mut sink, source) = ws_client.split();

    // Send initial setup frame
    let setup_msg = json!({
        "setup": {
            "model": "models/gemini-3.1-flash-live-preview"
        }
    })
    .to_string();
    sink.send(Message::Text(setup_msg.into())).await.unwrap();

    // Build session wrapping the mock socket stream
    let (tx, rx) = tokio::sync::mpsc::channel::<adk_realtime::gemini::OutboundMessage>(256);
    let writer_task = tokio::spawn(async move {
        let mut rx = rx;
        let mut sink = sink;
        while let Some(item) = rx.recv().await {
            let res = sink.send(item.message).await;
            if let Some(ack) = item.ack {
                let _ =
                    ack.send(res.map_err(|e| {
                        adk_realtime::error::RealtimeError::connection(e.to_string())
                    }));
            }
        }
    });

    let session = adk_realtime::gemini::GeminiRealtimeSession::new_for_test(
        "test_session_id".into(),
        ws_url.clone(),
        "models/gemini-3.1-flash-live-preview".into(),
        tx,
        writer_task,
        source,
    );

    // Read the server events (setupComplete and sessionResumptionUpdate) from session to cache resumeHandle
    let event1 = session.next_event().await.unwrap().unwrap();
    assert!(matches!(event1, ServerEvent::SessionCreated { .. }));

    let event2 = session.next_event().await.unwrap().unwrap();
    assert!(matches!(event2, ServerEvent::SessionUpdated { .. }));

    // Verify last_resume_handle is cached on session
    assert_eq!(session.last_resume_handle().as_deref(), Some("mock_resume_handle_e2e_123"));

    server_task.await.unwrap();
}
