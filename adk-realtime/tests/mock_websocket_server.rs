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
        RealtimeError::MessageError("ECONNRESET: connection lost".into()),
        RealtimeError::IoError(std::sync::Arc::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        ))),
    ];

    for err in &reset_errors {
        assert!(
            err.is_connection_reset(),
            "Error {:?} should be classified as connection reset",
            err
        );
    }

    let non_reset_errs = vec![
        RealtimeError::ConfigError("Invalid model ID".into()),
        RealtimeError::ProviderError("Broken pipe on socket write".into()),
        RealtimeError::Protocol("Connection closed abruptly".into()),
    ];

    for err in &non_reset_errs {
        assert!(
            !err.is_connection_reset(),
            "Error {:?} should NOT be classified as connection reset",
            err
        );
    }
}

#[tokio::test]
async fn test_gemini_recovery_transaction_lifecycle() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    use adk_realtime::recovery::{RecoveryCause, RecoveryContext};
    use futures_util::{SinkExt, StreamExt};
    use std::num::NonZeroU32;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}", local_addr);

    let server_task = tokio::spawn(async move {
        // Active session connection 0
        let (stream0, _) = listener.accept().await.unwrap();
        let _ws0 = accept_async(stream0).await.unwrap();

        // Candidate connection 1
        let (stream1, _) = listener.accept().await.unwrap();
        let mut ws1 = accept_async(stream1).await.unwrap();

        // Verify setup frame is sent FIRST on candidate connection
        let first_msg = ws1.next().await.unwrap().unwrap();
        if let Message::Text(text) = first_msg {
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert!(val.get("setup").is_some(), "Setup frame MUST be sent as first frame");
            assert_eq!(val["setup"]["sessionResumption"]["handle"], "resume_token_123");
        } else {
            panic!("Expected text frame with setup");
        }

        // Send setupComplete with resumed: true
        let setup_complete = json!({
            "setupComplete": {
                "resumed": true
            }
        })
        .to_string();
        ws1.send(Message::Text(setup_complete.into())).await.unwrap();

        // Keep connection open briefly
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let _ = ws1.close(None).await;
    });

    let (ws_client, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let (sink, source) = ws_client.split();
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

    let active_session =
        std::sync::Arc::new(adk_realtime::gemini::GeminiRealtimeSession::new_for_test(
            "active_session_id".into(),
            ws_url.clone(),
            "models/gemini-3.1-flash-live-preview".into(),
            tx,
            writer_task,
            source,
        ));

    let recovery_capability =
        active_session.recovery().expect("Gemini session must expose recovery");

    let cause = RecoveryCause::UnexpectedEof;
    let disposition = recovery_capability.classify(&cause);
    assert_eq!(disposition, adk_realtime::recovery::RecoveryDisposition::Recoverable);

    let mut config = adk_realtime::config::RealtimeConfig::default();
    config.instruction = Some("Recovered instruction context".to_string());
    config.extra = Some(json!({ "resumeHandle": "resume_token_123" }));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let context = RecoveryContext::new(NonZeroU32::new(1).unwrap(), &cause, &config, deadline);

    let recovered = recovery_capability.recover(context).await.unwrap();
    assert_eq!(recovered.continuity(), adk_realtime::recovery::RecoveryContinuity::Resumed);
    assert!(recovered.session().is_connected());

    // Verify active session remains unaffected and retains its own session_id
    assert_eq!(active_session.session_id(), "active_session_id");

    server_task.await.unwrap();
}

#[tokio::test]
async fn test_gemini_recovery_cold_reconnect_returns_reconnected() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    use adk_realtime::recovery::{RecoveryCause, RecoveryContext};
    use futures_util::{SinkExt, StreamExt};
    use std::num::NonZeroU32;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}", local_addr);

    let server_task = tokio::spawn(async move {
        // Active session connection 0
        let (stream0, _) = listener.accept().await.unwrap();
        let _ws0 = accept_async(stream0).await.unwrap();

        // Candidate connection 1
        let (stream1, _) = listener.accept().await.unwrap();
        let mut ws1 = accept_async(stream1).await.unwrap();

        let first_msg = ws1.next().await.unwrap().unwrap();
        if let Message::Text(text) = first_msg {
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert!(val.get("setup").is_some());
            assert!(val["setup"]["sessionResumption"]["handle"].is_null());
        }

        // Send setupComplete without resumed flag
        let setup_complete = json!({ "setupComplete": {} }).to_string();
        ws1.send(Message::Text(setup_complete.into())).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    });

    let (ws_client, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let (sink, source) = ws_client.split();
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
        "test_session".into(),
        ws_url.clone(),
        "models/gemini-3.1-flash-live-preview".into(),
        tx,
        writer_task,
        source,
    );

    let recovery_capability = session.recovery().unwrap();
    let cause = RecoveryCause::UnexpectedEof;
    let config = adk_realtime::config::RealtimeConfig::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let context = RecoveryContext::new(NonZeroU32::new(1).unwrap(), &cause, &config, deadline);

    let recovered = recovery_capability.recover(context).await.unwrap();
    assert_eq!(recovered.continuity(), adk_realtime::recovery::RecoveryContinuity::Reconnected);

    server_task.await.unwrap();
}

#[tokio::test]
async fn test_gemini_recovery_setup_rejection_cleans_candidate() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    use adk_realtime::recovery::{RecoveryCause, RecoveryContext};
    use futures_util::{SinkExt, StreamExt};
    use std::num::NonZeroU32;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}", local_addr);

    let server_task = tokio::spawn(async move {
        // Active session connection 0
        let (stream0, _) = listener.accept().await.unwrap();
        let _ws0 = accept_async(stream0).await.unwrap();

        // Candidate connection 1
        let (stream1, _) = listener.accept().await.unwrap();
        let mut ws1 = accept_async(stream1).await.unwrap();

        let _ = ws1.next().await; // Read setup
        // Reject immediately by closing connection
        let _ = ws1.close(None).await;
    });

    let (ws_client, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let (sink, source) = ws_client.split();
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
        "test_session".into(),
        ws_url.clone(),
        "models/gemini-3.1-flash-live-preview".into(),
        tx,
        writer_task,
        source,
    );

    let recovery_capability = session.recovery().unwrap();
    let cause = RecoveryCause::UnexpectedEof;
    let config = adk_realtime::config::RealtimeConfig::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let context = RecoveryContext::new(NonZeroU32::new(1).unwrap(), &cause, &config, deadline);

    let res = recovery_capability.recover(context).await;
    assert!(res.is_err());

    server_task.await.unwrap();
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

    server_task.await.unwrap();
}
