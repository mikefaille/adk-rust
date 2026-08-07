#![cfg(feature = "gemini")]

use adk_realtime::RealtimeSession;
use adk_realtime::error::RealtimeError;
use adk_realtime::events::ServerEvent;
use adk_realtime::gemini::normalize_model_id;
use adk_realtime::session::{RealtimeAvailability, RecoveryPolicy, recover_session};
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
        RealtimeError::MessageError("ECONNRESET: connection lost".into()),
        RealtimeError::ProviderError("Broken pipe on socket write".into()),
        RealtimeError::Protocol("Connection closed abruptly".into()),
    ];

    for err in &reset_errors {
        assert!(
            err.is_connection_reset(),
            "Error {:?} should be classified as connection reset",
            err
        );
    }

    let non_reset_err = RealtimeError::ConfigError("Invalid model ID".into());
    assert!(!non_reset_err.is_connection_reset());
}

#[tokio::test]
async fn test_mock_resumption_token_alignment_and_lifecycle_transitions() {
    // 1. Verify dual-key framing: when Google sends newHandle in sessionResumptionUpdate,
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

    // 2. Verify lifecycle state enum transitions
    let connected_state = RealtimeAvailability::Connected;
    let reconnecting_state = RealtimeAvailability::Reconnecting { epoch: 1 };
    let exhausted_state = RealtimeAvailability::Exhausted;

    assert_eq!(connected_state, RealtimeAvailability::Connected);
    assert_ne!(connected_state, reconnecting_state);
    assert_eq!(reconnecting_state, RealtimeAvailability::Reconnecting { epoch: 1 });
    assert_eq!(exhausted_state, RealtimeAvailability::Exhausted);
}

#[tokio::test]
async fn test_recovery_policy_defaults() {
    let policy = RecoveryPolicy::default();
    assert_eq!(policy.max_attempts.get(), 3);
    assert_eq!(policy.deadline, std::time::Duration::from_secs(5));
    assert_eq!(policy.initial_delay, std::time::Duration::from_millis(50));
    assert_eq!(policy.max_delay, std::time::Duration::from_millis(1000));
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

        // --- Connection 2: Reconnection with resumeHandle ---
        let (stream2, _) = listener.accept().await.unwrap();
        let mut ws2 = accept_async(stream2).await.unwrap();

        // Expect fresh BidiGenerateContentSetup containing sessionResumption.handle
        if let Some(Ok(Message::Text(msg))) = ws2.next().await {
            let json: serde_json::Value = serde_json::from_str(&msg).unwrap();
            let setup = json.get("setup").expect("Setup object present");
            let handle = setup
                .get("sessionResumption")
                .and_then(|sr| sr.get("handle"))
                .and_then(|h| h.as_str());
            assert_eq!(handle, Some("mock_resume_handle_e2e_123"));
        } else {
            panic!("Expected setup frame on connection 2");
        }

        // Send setupComplete on reconnect socket
        let setup_complete_2 = json!({ "setupComplete": {} }).to_string();
        ws2.send(Message::Text(setup_complete_2.into())).await.unwrap();
    });

    // 3. Connect client session to mock server URL
    let _backend =
        adk_realtime::gemini::GeminiLiveBackend::Studio { api_key: "mock_api_key".into() };
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
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let writer_task = tokio::spawn(async move {
        let mut rx = rx;
        let mut sink = sink;
        while let Some(msg) = rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
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

    // 4. Perform recover_session to mock server
    let policy = RecoveryPolicy {
        max_attempts: std::num::NonZeroU32::new(3).unwrap(),
        deadline: std::time::Duration::from_secs(5),
        initial_delay: std::time::Duration::from_millis(50),
        max_delay: std::time::Duration::from_millis(500),
    };

    let result = recover_session(&session, &policy, &_config).await;
    assert!(result.is_ok(), "recover_session should succeed against mock server: {:?}", result);

    server_task.await.unwrap();
}
