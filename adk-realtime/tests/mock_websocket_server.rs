#![cfg(feature = "gemini")]

use adk_realtime::RealtimeSession;
use adk_realtime::error::RealtimeError;
use adk_realtime::events::ServerEvent;
use adk_realtime::gemini::normalize_model_id;
use adk_realtime::session::{RealtimeAvailability, ReconnectOptions};
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
async fn test_reconnect_options_defaults() {
    let opts = ReconnectOptions::default();
    assert_eq!(opts.max_retries, 3);
    assert_eq!(opts.backoff_budget_ms, 3000);
    assert!(opts.ipv4_fallback);
    assert!(opts.resume_handle.is_none());
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

        // Expect BidiGenerateContentSetup, and keep it so connection 2's frame
        // can be checked against it below.
        let setup1 = if let Some(Ok(Message::Text(msg))) = ws1.next().await {
            let json: serde_json::Value = serde_json::from_str(&msg).unwrap();
            json.get("setup").expect("setup frame on connection 1").clone()
        } else {
            panic!("Expected setup frame on connection 1");
        };

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
        let setup2 = if let Some(Ok(Message::Text(msg))) = ws2.next().await {
            let json: serde_json::Value = serde_json::from_str(&msg).unwrap();
            json.get("setup").expect("setup frame on connection 2").clone()
        } else {
            panic!("Expected setup frame on connection 2");
        };
        let handle = setup2
            .get("sessionResumption")
            .and_then(|sr| sr.get("handle"))
            .and_then(|h| h.as_str());
        assert_eq!(handle, Some("mock_resume_handle_e2e_123"));

        // Session resumption restores a transport, not a contract: the
        // reconnect frame must replay the SAME tools, system instruction, and
        // generation config connection 1 sent, differing only by
        // `sessionResumption`. Before the reconnect-setup-frame fix,
        // `try_reconnect_once` sent a hardcoded skeleton with
        // `tools: None, system_instruction: None, generation_config: None`,
        // so a resumed session had no tools and `submit_conversational_result`
        // — the sole conversational exit — became unreachable after any
        // reconnect. These three assertions fail against that skeleton.
        assert_eq!(
            setup2.get("tools"),
            setup1.get("tools"),
            "reconnect setup frame must replay the original tools: {setup2}"
        );
        assert_eq!(
            setup2.get("systemInstruction"),
            setup1.get("systemInstruction"),
            "reconnect setup frame must replay the original system instruction: {setup2}"
        );
        assert_eq!(
            setup2.get("generationConfig"),
            setup1.get("generationConfig"),
            "reconnect setup frame must replay the original generation config: {setup2}"
        );
        assert_ne!(
            setup2.get("sessionResumption"),
            setup1.get("sessionResumption"),
            "sessionResumption is the one field a reconnect frame must change"
        );

        // Send setupComplete on reconnect socket
        let setup_complete_2 = json!({ "setupComplete": {} }).to_string();
        ws2.send(Message::Text(setup_complete_2.into())).await.unwrap();
    });

    // 3. Connect client session to mock server URL
    let _backend =
        adk_realtime::gemini::GeminiLiveBackend::Studio { api_key: "mock_api_key".into() };
    // This is also what gets passed as `original_config` to `new_for_test`
    // below, so it is the single source of truth for what both connection 1
    // (the frame sent right here) and the later reconnect (built from
    // `original_config`) are expected to carry.
    let config = adk_realtime::config::RealtimeConfig {
        instruction: Some("Test instructions".into()),
        tools: Some(vec![adk_realtime::config::ToolDefinition::new(
            "submit_conversational_result",
        )]),
        ..Default::default()
    };

    // Connect initially using connect_async via GeminiRealtimeSession
    // We connect to ws_url
    let (ws_client, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let (mut sink, source) = ws_client.split();

    // Send initial setup frame. `new_for_test` wraps an already-open socket
    // rather than running `connect_with_dialect`/`send_setup_with_compiled_tools`
    // itself, so this hand-built frame stands in for what that real path
    // sends for `config` above — matching the wire shape `convert_tools` and
    // `build_setup` produce for a tool-less-parameters `ToolDefinition`
    // (see the `tool_declarations` unit tests in `gemini/session.rs`).
    let setup_msg = json!({
        "setup": {
            "model": "models/gemini-3.1-flash-live-preview",
            "systemInstruction": { "parts": [ { "text": "Test instructions" } ] },
            "generationConfig": { "responseModalities": ["AUDIO"] },
            "tools": [
                {
                    "functionDeclarations": [
                        {
                            "name": "submit_conversational_result",
                            "parameters": { "type": "object", "properties": {} }
                        }
                    ]
                }
            ]
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
        config,
    );

    // Read the server events (setupComplete and sessionResumptionUpdate) from session to cache resumeHandle
    let event1 = session.next_event().await.unwrap().unwrap();
    assert!(matches!(event1, ServerEvent::SessionCreated { .. }));

    let event2 = session.next_event().await.unwrap().unwrap();
    assert!(matches!(event2, ServerEvent::SessionUpdated { .. }));

    // Verify last_resume_handle is cached on session
    assert_eq!(session.last_resume_handle().as_deref(), Some("mock_resume_handle_e2e_123"));

    // 4. Perform reconnect_with_backoff to mock server
    let reconnect_opts = ReconnectOptions {
        max_retries: 3,
        backoff_budget_ms: 3000,
        ipv4_fallback: true,
        resume_handle: None,
    };

    let result = session.reconnect_with_backoff(reconnect_opts).await;
    assert!(
        result.is_ok(),
        "reconnect_with_backoff should succeed against mock server: {:?}",
        result
    );

    server_task.await.unwrap();
}

/// Regression test for the empty-vec-means-end-of-stream defect fixed
/// alongside commit `146259fec`. `translate_event_logged`'s `goAway` arm
/// returns `Ok(vec![])` — legitimately, since a goAway frame carries no
/// caller-visible event — but `receive_raw` used to read `first.map(Ok)` on
/// that empty vec and hand the caller a bare `None`, which every caller in
/// this codebase (`RealtimeRunner`, the data-plane bridge) treats as the
/// transport having closed. A `goAway` is a documented pre-close *warning*,
/// not a close; it must not end a live call on its own.
///
/// This asserts at the `next_event()` level, which is `receive_raw` with no
/// intervening logic (`GeminiRealtimeSession::next_event` is a direct
/// passthrough) — the layer the bug actually lived in, not
/// `translate_event_static`, which already asserts (correctly) that `goAway`
/// alone translates to zero events.
#[tokio::test]
async fn test_go_away_frame_does_not_end_the_stream() {
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}", local_addr);

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();

        // Consume the client's setup frame.
        let _ = ws.next().await;

        ws.send(Message::Text(json!({ "setupComplete": {} }).to_string().into()))
            .await
            .unwrap();

        // Carries no caller-visible event: translates to `Ok(vec![])`.
        ws.send(Message::Text(
            json!({ "goAway": { "timeLeft": "10s" } }).to_string().into(),
        ))
        .await
        .unwrap();

        // A real event immediately behind it on the wire. If `receive_raw`
        // treated `goAway` as end-of-stream, the client's second
        // `next_event()` call returns `None` and never observes this frame.
        ws.send(Message::Text(
            json!({
                "sessionResumptionUpdate": {
                    "resumable": true,
                    "newHandle": "post_go_away_handle"
                }
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

        // Give the client a beat to read both frames before the socket goes away.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    });

    let (ws_client, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let (mut sink, source) = ws_client.split();

    let setup_msg = json!({
        "setup": { "model": "models/gemini-3.1-flash-live-preview" }
    })
    .to_string();
    sink.send(Message::Text(setup_msg.into())).await.unwrap();

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
        "test_go_away_session".into(),
        ws_url.clone(),
        "models/gemini-3.1-flash-live-preview".into(),
        tx,
        writer_task,
        source,
        adk_realtime::config::RealtimeConfig::default(),
    );

    let event1 = session.next_event().await;
    assert!(
        matches!(event1, Some(Ok(ServerEvent::SessionCreated { .. }))),
        "expected SessionCreated first, got {event1:?}"
    );

    // The regression assertion: a single `next_event()` call must
    // transparently skip the interleaved `goAway` frame and surface the
    // event that followed it on the wire, not `None`.
    let event2 = session.next_event().await;
    assert!(
        matches!(event2, Some(Ok(ServerEvent::SessionUpdated { .. }))),
        "goAway must not end the stream: expected the event that followed it, got {event2:?}"
    );

    server_task.await.unwrap();
}

/// Regression test, same root cause as `test_go_away_frame_does_not_end_the_stream`.
/// `translate_event_logged`'s `sessionResumptionUpdate` arm also returns
/// `Ok(vec![])` when the server reports `resumable: false` — a state Google
/// specifies ("no safe resume point right now"), not an error. Before the
/// fix that frame alone ended a live call the same way `goAway` did.
#[tokio::test]
async fn test_non_resumable_session_resumption_update_does_not_end_the_stream() {
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}", local_addr);

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();

        let _ = ws.next().await;

        ws.send(Message::Text(json!({ "setupComplete": {} }).to_string().into()))
            .await
            .unwrap();

        // `resumable: false`, no handle: translates to `Ok(vec![])`.
        ws.send(Message::Text(
            json!({ "sessionResumptionUpdate": { "resumable": false } }).to_string().into(),
        ))
        .await
        .unwrap();

        // A real event immediately behind it on the wire.
        ws.send(Message::Text(
            json!({ "serverContent": { "turnComplete": true } }).to_string().into(),
        ))
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    });

    let (ws_client, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let (mut sink, source) = ws_client.split();

    let setup_msg = json!({
        "setup": { "model": "models/gemini-3.1-flash-live-preview" }
    })
    .to_string();
    sink.send(Message::Text(setup_msg.into())).await.unwrap();

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
        "test_non_resumable_session_resumption_update".into(),
        ws_url.clone(),
        "models/gemini-3.1-flash-live-preview".into(),
        tx,
        writer_task,
        source,
        adk_realtime::config::RealtimeConfig::default(),
    );

    let event1 = session.next_event().await;
    assert!(
        matches!(event1, Some(Ok(ServerEvent::SessionCreated { .. }))),
        "expected SessionCreated first, got {event1:?}"
    );

    // The regression assertion: a `resumable: false` sessionResumptionUpdate
    // must not surface as `None`.
    let event2 = session.next_event().await;
    assert!(
        matches!(event2, Some(Ok(ServerEvent::ResponseDone { .. }))),
        "a non-resumable sessionResumptionUpdate must not end the stream: expected the event that followed it, got {event2:?}"
    );

    server_task.await.unwrap();
}
