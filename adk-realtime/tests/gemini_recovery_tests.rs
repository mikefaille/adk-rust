#![cfg(feature = "gemini")]

use adk_realtime::config::RealtimeConfig;
use adk_realtime::error::RealtimeError;
use adk_realtime::events::ServerEvent;
use adk_realtime::gemini::{GeminiLiveBackend, GeminiRealtimeSession};
use adk_realtime::recovery::{
    RecoveryCause, RecoveryContext, RecoveryContinuity, RecoveryDisposition,
};
use adk_realtime::session::RealtimeSession;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

async fn spawn_mock_ws_server<F, Fut>(handler: F) -> (SocketAddr, tokio::task::JoinHandle<()>)
where
    F: Fn(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let handler = Arc::new(handler);
    let server_handle = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            if let Ok(ws_stream) = accept_async(stream).await {
                let handler = Arc::clone(&handler);
                tokio::spawn(async move {
                    handler(ws_stream).await;
                });
            }
        }
    });

    (addr, server_handle)
}

#[tokio::test]
async fn test_gemini_exposes_recovery_spi() {
    let (addr, _server) = spawn_mock_ws_server(|_| async {}).await;
    let ws = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
    let (_sink, source) = ws.split();

    let (tx, _rx) = tokio::sync::mpsc::channel(10);
    let writer_task = tokio::spawn(async {});

    let session = GeminiRealtimeSession::new_for_test(
        "test-session".to_string(),
        format!("ws://{}", addr),
        "models/gemini-3.1-flash-live-preview".to_string(),
        tx,
        writer_task,
        source,
    );

    assert!(session.recovery().is_some());
}

#[tokio::test]
async fn test_gemini_classification_aligns_with_reset_fact() {
    let (addr, _server) = spawn_mock_ws_server(|_| async {}).await;
    let ws = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
    let (_sink, source) = ws.split();

    let (tx, _rx) = tokio::sync::mpsc::channel(10);
    let writer_task = tokio::spawn(async {});

    let session = GeminiRealtimeSession::new_for_test(
        "test-session".to_string(),
        format!("ws://{}", addr),
        "models/gemini-3.1-flash-live-preview".to_string(),
        tx,
        writer_task,
        source,
    );
    let recovery = session.recovery().unwrap();

    // Reset error is recoverable
    let reset_err = Arc::new(RealtimeError::connection("Connection reset by peer"));
    let cause_reset = RecoveryCause::ReadFailed(reset_err.clone());
    assert_eq!(recovery.classify(&cause_reset), RecoveryDisposition::Recoverable);

    // Auth error is fatal
    let auth_err = Arc::new(RealtimeError::AuthError("Invalid API key".to_string()));
    let cause_auth = RecoveryCause::ReadFailed(auth_err.clone());
    assert_eq!(recovery.classify(&cause_auth), RecoveryDisposition::Fatal);

    // Unexpected EOF is recoverable
    let cause_eof = RecoveryCause::UnexpectedEof;
    assert_eq!(recovery.classify(&cause_eof), RecoveryDisposition::Recoverable);

    // classify_attempt_error
    assert_eq!(
        recovery.classify_attempt_error(&RealtimeError::connection("Connection reset by peer")),
        RecoveryDisposition::Recoverable
    );
    assert_eq!(
        recovery.classify_attempt_error(&RealtimeError::AuthError("Bad auth".to_string())),
        RecoveryDisposition::Fatal
    );
}

#[tokio::test]
async fn test_recover_single_candidate_attempt_and_setup_first() {
    let (addr, _server) = spawn_mock_ws_server(|mut ws| async move {
        while let Some(msg) = ws.next().await {
            if let Ok(Message::Text(text)) = msg {
                let val: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert!(
                    val.get("setup").is_some(),
                    "First client frame must be setup, got: {}",
                    text
                );

                let setup_complete = json!({ "setupComplete": {} });
                ws.send(Message::Text(setup_complete.to_string().into())).await.unwrap();
                break;
            }
        }
    })
    .await;

    let ws = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
    let (_sink, source) = ws.split();
    let (tx, _rx) = tokio::sync::mpsc::channel(10);
    let writer_task = tokio::spawn(async {});

    let mock_url = format!("ws://{}", addr);
    let session = GeminiRealtimeSession::new_for_test(
        "test-session".to_string(),
        mock_url.clone(),
        "models/gemini-3.1-flash-live-preview".to_string(),
        tx,
        writer_task,
        source,
    );

    let recovery = session.recovery().unwrap();
    let cause = RecoveryCause::UnexpectedEof;
    let config = RealtimeConfig::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let context = RecoveryContext::new(NonZeroU32::new(1).unwrap(), &cause, &config, deadline);

    let recovered = recovery.recover(context).await.expect("Recovery should succeed");
    assert_eq!(recovered.continuity(), RecoveryContinuity::Reconnected);
    assert!(recovered.session().is_connected());
}

#[tokio::test]
async fn test_candidate_failure_does_not_mutate_active_generation_n() {
    let (addr, _server) = spawn_mock_ws_server(|mut ws| async move {
        // Read setup then close candidate socket
        let _ = ws.next().await;
        ws.close(None).await.ok();
    })
    .await;

    let ws = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
    let (_sink, source) = ws.split();
    let (tx, _rx) = tokio::sync::mpsc::channel(10);
    let writer_task = tokio::spawn(async {});

    let mock_url = format!("ws://{}", addr);
    let active_session = GeminiRealtimeSession::new_for_test(
        "active-gen-n".to_string(),
        mock_url.clone(),
        "models/gemini-3.1-flash-live-preview".to_string(),
        tx,
        writer_task,
        source,
    );

    assert!(active_session.is_connected());

    let recovery = active_session.recovery().unwrap();
    let cause = RecoveryCause::UnexpectedEof;
    let config = RealtimeConfig::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let context = RecoveryContext::new(NonZeroU32::new(1).unwrap(), &cause, &config, deadline);

    let res = recovery.recover(context).await;
    assert!(res.is_err(), "Candidate failure should return error");

    // Active generation N must remain untouched/connected
    assert_eq!(active_session.session_id(), "active-gen-n");
    assert!(active_session.is_connected());
}

#[tokio::test]
async fn test_consecutive_recovery_carries_resume_handle_anchor() {
    let received_handles = Arc::new(parking_lot::Mutex::new(Vec::<Option<String>>::new()));

    let handles_capture = Arc::clone(&received_handles);
    let (addr, _server) = spawn_mock_ws_server(move |mut ws| {
        let handles = Arc::clone(&handles_capture);
        async move {
            while let Some(msg) = ws.next().await {
                if let Ok(Message::Text(text)) = msg {
                    let val: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if let Some(setup) = val.get("setup") {
                        let handle = setup
                            .get("sessionResumption")
                            .and_then(|r| r.get("handle"))
                            .and_then(|h| h.as_str())
                            .map(|s| s.to_string());
                        handles.lock().push(handle);

                        let setup_complete = json!({ "setupComplete": {} });
                        ws.send(Message::Text(setup_complete.to_string().into())).await.unwrap();

                        // Send sessionResumptionUpdate frame on initial connection only
                        if handles.lock().len() == 1 {
                            let update_frame = json!({
                                "sessionResumptionUpdate": {
                                    "newHandle": "handle-H-123",
                                    "resumable": true
                                }
                            });
                            ws.send(Message::Text(update_frame.to_string().into())).await.unwrap();
                        }
                        break;
                    }
                }
            }
        }
    })
    .await;

    let backend = GeminiLiveBackend::studio("test-key").with_endpoint_url(format!("ws://{}", addr));
    let config = RealtimeConfig::default();

    // Connect session N
    let session_n =
        GeminiRealtimeSession::connect(backend, "models/gemini-live", config).await.unwrap();

    // Read setupComplete
    let ev1 = session_n.next_event().await.unwrap().unwrap();
    assert!(matches!(ev1, ServerEvent::SessionCreated { .. }));

    // Read SessionUpdated carrying handle-H-123 over WebSocket wire
    let ev2 = session_n.next_event().await.unwrap().unwrap();
    assert!(matches!(ev2, ServerEvent::SessionUpdated { .. }));
    assert_eq!(session_n.last_resume_handle(), Some("handle-H-123".to_string()));

    // 1st Recovery: N -> N+1
    let cause = RecoveryCause::UnexpectedEof;
    let config = RealtimeConfig::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);

    let recovered_n1 = session_n
        .recovery()
        .unwrap()
        .recover(RecoveryContext::new(NonZeroU32::new(1).unwrap(), &cause, &config, deadline))
        .await
        .unwrap();

    let session_n1 = recovered_n1.session();
    let gemini_n1 = session_n1.recovery().unwrap();

    // 2nd Recovery: N+1 -> N+2 (without N+1 receiving a new update frame)
    let recovered_n2 = gemini_n1
        .recover(RecoveryContext::new(NonZeroU32::new(2).unwrap(), &cause, &config, deadline))
        .await
        .unwrap();

    let session_n2 = recovered_n2.session();

    // Verify received handle array in server: [Initial setup = None, N->N+1 setup = Some("handle-H-123"), N+1->N+2 setup = Some("handle-H-123")]
    let cap = received_handles.lock();
    assert_eq!(cap.len(), 3);
    assert_eq!(cap[0], None);
    assert_eq!(cap[1], Some("handle-H-123".to_string()));
    assert_eq!(cap[2], Some("handle-H-123".to_string()));

    // Meaningful session authority assertions
    assert!(session_n2.is_connected());
    assert_ne!(session_n2.session_id(), session_n.session_id());
    assert_ne!(session_n1.session_id(), session_n.session_id());
    assert_ne!(session_n2.session_id(), session_n1.session_id());
}

#[tokio::test]
async fn test_effective_config_applied_on_candidate_setup() {
    let (addr, _s) = spawn_mock_ws_server(|mut ws| async move {
        while let Some(msg) = ws.next().await {
            if let Ok(Message::Text(text)) = msg {
                let val: serde_json::Value = serde_json::from_str(&text).unwrap();
                let setup = val.get("setup").unwrap();
                let gen_config = setup.get("generationConfig").unwrap();

                let temp = gen_config.get("temperature").and_then(|v| v.as_f64()).unwrap();
                assert!((temp - 0.85).abs() < 1e-4);

                let resp = json!({ "setupComplete": {} });
                ws.send(Message::Text(resp.to_string().into())).await.unwrap();
                break;
            }
        }
    })
    .await;

    let ws = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
    let (_s_sink, source) = ws.split();
    let (tx, _rx) = tokio::sync::mpsc::channel(10);

    let session = GeminiRealtimeSession::new_for_test(
        "s".to_string(),
        format!("ws://{}", addr),
        "models/gemini-3.1-flash-live-preview".to_string(),
        tx,
        tokio::spawn(async {}),
        source,
    );

    let cause = RecoveryCause::UnexpectedEof;
    let updated_config = RealtimeConfig { temperature: Some(0.85), ..Default::default() };

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let recovered = session
        .recovery()
        .unwrap()
        .recover(RecoveryContext::new(
            NonZeroU32::new(1).unwrap(),
            &cause,
            &updated_config,
            deadline,
        ))
        .await
        .unwrap();

    assert_eq!(recovered.continuity(), RecoveryContinuity::Reconnected);
}

#[tokio::test]
async fn test_recover_obeys_deadline_and_times_out() {
    let (addr, _s) = spawn_mock_ws_server(|mut ws| async move {
        let _msg = ws.next().await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    })
    .await;

    let ws = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
    let (_s_sink, source) = ws.split();
    let (tx, _rx) = tokio::sync::mpsc::channel(10);

    let session = GeminiRealtimeSession::new_for_test(
        "s".to_string(),
        format!("ws://{}", addr),
        "models/gemini-3.1-flash-live-preview".to_string(),
        tx,
        tokio::spawn(async {}),
        source,
    );

    let cause = RecoveryCause::UnexpectedEof;
    let config = RealtimeConfig::default();
    let deadline = std::time::Instant::now() + Duration::from_millis(100);

    let res = session
        .recovery()
        .unwrap()
        .recover(RecoveryContext::new(NonZeroU32::new(1).unwrap(), &cause, &config, deadline))
        .await;

    match res {
        Err(RealtimeError::Timeout(msg)) => {
            assert!(msg.contains("timed out waiting for setupComplete"));
        }
        other => panic!("Expected Timeout error, got {:?}", other),
    }
}

#[tokio::test]
async fn test_candidate_raii_cleanup_on_timeout() {
    let server_closed_signal = Arc::new(tokio::sync::Notify::new());
    let notify = Arc::clone(&server_closed_signal);

    let (addr, _s) = spawn_mock_ws_server(move |mut ws| {
        let notify = Arc::clone(&notify);
        async move {
            let _setup = ws.next().await;
            // Never send setupComplete; wait until client drops/closes connection
            let next_msg = ws.next().await;
            assert!(matches!(next_msg, None | Some(Err(_)) | Some(Ok(Message::Close(_)))));
            notify.notify_one();
        }
    })
    .await;

    let ws = tokio_tungstenite::connect_async(format!("ws://{}", addr)).await.unwrap().0;
    let (_s_sink, source) = ws.split();
    let (tx, _rx) = tokio::sync::mpsc::channel(10);

    let session = GeminiRealtimeSession::new_for_test(
        "s".to_string(),
        format!("ws://{}", addr),
        "models/gemini-3.1-flash-live-preview".to_string(),
        tx,
        tokio::spawn(async {}),
        source,
    );

    let cause = RecoveryCause::UnexpectedEof;
    let config = RealtimeConfig::default();
    let deadline = std::time::Instant::now() + Duration::from_millis(100);

    let res = session
        .recovery()
        .unwrap()
        .recover(RecoveryContext::new(NonZeroU32::new(1).unwrap(), &cause, &config, deadline))
        .await;

    assert!(matches!(res, Err(RealtimeError::Timeout(_))));

    // Verify candidate socket/writer was aborted and closed on timeout via CandidateGuard drop
    let closed_result =
        tokio::time::timeout(Duration::from_secs(1), server_closed_signal.notified()).await;
    assert!(closed_result.is_ok(), "Candidate socket must be closed upon timeout drop");
}

#[tokio::test]
async fn test_event_queue_cancellation_safety_zero_lost_messages() {
    let frame_send_trigger = Arc::new(tokio::sync::Notify::new());
    let trigger_clone = Arc::clone(&frame_send_trigger);

    let (addr, _s) = spawn_mock_ws_server(move |mut ws| {
        let trigger = Arc::clone(&trigger_clone);
        async move {
            // Read setup frame
            let _setup = ws.next().await;
            // Send setupComplete first
            let setup_complete = json!({ "setupComplete": {} });
            ws.send(Message::Text(setup_complete.to_string().into())).await.unwrap();

            // Wait for test thread to start polling and cancel next_event()
            trigger.notified().await;

            // Send two serverContent frames in a single WS burst
            let f1 = json!({
                "serverContent": {
                    "inputTranscription": { "text": "Chunk 1" }
                }
            });
            let f2 = json!({
                "serverContent": {
                    "inputTranscription": { "text": "Chunk 2" }
                }
            });
            ws.send(Message::Text(f1.to_string().into())).await.unwrap();
            ws.send(Message::Text(f2.to_string().into())).await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await;

    let backend = GeminiLiveBackend::studio("test-key").with_endpoint_url(format!("ws://{}", addr));
    let config = RealtimeConfig::default();

    let session =
        GeminiRealtimeSession::connect(backend, "models/gemini-live", config).await.unwrap();

    // Consume setupComplete
    let ev1 = session.next_event().await.unwrap().unwrap();
    assert!(matches!(ev1, ServerEvent::SessionCreated { .. }));

    // Phase 1: Actively poll next_event() while the server has NOT sent frames yet, then cancel/drop the future.
    let mut in_flight_read = Box::pin(session.next_event());
    tokio::select! {
        _ = &mut in_flight_read => {
            panic!("read_fut completed unexpectedly before server sent frames");
        }
        _ = tokio::task::yield_now() => {
            // Future was actively polled and entered receiver.lock().await before cancellation
        }
    }
    drop(in_flight_read);

    // Trigger server to send Chunk 1 + Chunk 2
    frame_send_trigger.notify_one();

    // Phase 2: Read next_event(); it decodes Chunk 1 + Chunk 2 from WS, returns Chunk 1, and synchronously queues Chunk 2
    let ev2 = session.next_event().await.unwrap().unwrap();
    if let ServerEvent::InputTranscriptDelta { delta, .. } = ev2 {
        assert_eq!(delta, "Chunk 1");
    } else {
        panic!("Expected InputTranscriptDelta Chunk 1, got {:?}", ev2);
    }

    // Phase 3: Actively poll next_event() when Chunk 2 is in event_queue
    let mut queued_read = Box::pin(session.next_event());
    let ev3 = match futures::future::poll_immediate(&mut queued_read).await {
        Some(res) => res.unwrap().unwrap(),
        None => panic!("Expected poll_immediate to return Chunk 2 from event_queue"),
    };

    if let ServerEvent::InputTranscriptDelta { delta, .. } = ev3 {
        assert_eq!(delta, "Chunk 2");
    } else {
        panic!("Expected InputTranscriptDelta Chunk 2, got {:?}", ev3);
    }
}

#[tokio::test]
async fn test_empty_event_translation_loop_does_not_signal_eof() {
    let (addr, _s) = spawn_mock_ws_server(|mut ws| async move {
        let _setup = ws.next().await;
        // 1. Send setupComplete
        let setup_complete = json!({ "setupComplete": {} });
        ws.send(Message::Text(setup_complete.to_string().into())).await.unwrap();

        // 2. Send non-resumable sessionResumptionUpdate (translates to Ok(vec![]))
        let non_resumable = json!({
            "sessionResumptionUpdate": {
                "newHandle": "unusable-handle",
                "resumable": false
            }
        });
        ws.send(Message::Text(non_resumable.to_string().into())).await.unwrap();

        // 3. Send subsequent normal event
        let content = json!({
            "serverContent": {
                "inputTranscription": { "text": "Subsequent Hello" }
            }
        });
        ws.send(Message::Text(content.to_string().into())).await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
    })
    .await;

    let backend = GeminiLiveBackend::studio("test-key").with_endpoint_url(format!("ws://{}", addr));
    let config = RealtimeConfig::default();

    let session =
        GeminiRealtimeSession::connect(backend, "models/gemini-live", config).await.unwrap();

    // 1. Consume setupComplete
    let ev1 = session.next_event().await.unwrap().unwrap();
    assert!(matches!(ev1, ServerEvent::SessionCreated { .. }));

    // 2. Next call must NOT return None / EOF on the non-resumable update frame, but loop and return the subsequent content event!
    let ev2 =
        session.next_event().await.expect("Must not return None on empty control frame").unwrap();
    if let ServerEvent::InputTranscriptDelta { delta, .. } = ev2 {
        assert_eq!(delta, "Subsequent Hello");
    } else {
        panic!("Expected InputTranscriptDelta 'Subsequent Hello', got {:?}", ev2);
    }
}

#[tokio::test]
async fn test_gemini_backend_studio_and_vertex() {
    let b1 = GeminiLiveBackend::studio("my-key").with_endpoint_url("wss://example.com/ws");
    match b1 {
        GeminiLiveBackend::Studio { api_key, endpoint_url } => {
            assert_eq!(api_key, "my-key");
            assert_eq!(endpoint_url, Some("wss://example.com/ws".to_string()));
        }
        #[allow(unreachable_patterns)]
        _ => panic!("Expected Studio variant"),
    }
}

#[tokio::test]
async fn test_studio_custom_endpoint_does_not_leak_api_key() {
    let received_uri = Arc::new(parking_lot::Mutex::new(None::<String>));
    let uri_capture = Arc::clone(&received_uri);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_handle = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let uri_ref = Arc::clone(&uri_capture);
            #[allow(clippy::result_large_err)]
            let callback = move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                                 resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                *uri_ref.lock() = Some(req.uri().to_string());
                Ok(resp)
            };
            let _ = tokio_tungstenite::accept_hdr_async(stream, callback).await;
        }
    });

    let custom_url = format!("ws://{}/ws", addr);
    let backend = GeminiLiveBackend::studio("secret-api-key").with_endpoint_url(&custom_url);

    let session_res =
        GeminiRealtimeSession::connect(backend, "models/gemini-live", RealtimeConfig::default())
            .await;

    assert!(session_res.is_ok(), "Connection to custom endpoint should succeed");

    let uri = received_uri.lock().take().expect("Handshake request URI captured");
    assert!(
        !uri.contains("key="),
        "Custom Studio endpoint URI must NOT contain key= query parameter: {uri}"
    );

    server_handle.abort();
}

#[cfg(feature = "vertex-live")]
#[tokio::test]
async fn test_vertex_custom_endpoint_does_not_leak_auth_header() {
    let received_headers = Arc::new(parking_lot::Mutex::new(Vec::<Option<String>>::new()));

    let headers_capture = Arc::clone(&received_headers);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_handle = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let headers_ref = Arc::clone(&headers_capture);
            #[allow(clippy::result_large_err)]
            let callback = move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                                 resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                let auth = req
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                headers_ref.lock().push(auth);
                Ok(resp)
            };
            if let Ok(_ws) = tokio_tungstenite::accept_hdr_async(stream, callback).await {
                // Handshake completed
            }
        }
    });

    let mock_credentials = google_cloud_auth::credentials::Builder::default().build().unwrap();
    let backend_custom = GeminiLiveBackend::Vertex {
        credentials: mock_credentials.clone(),
        region: "us-central1".into(),
        project_id: "test-project".into(),
        endpoint_url: Some(format!("ws://{}", addr)),
    };

    let session_res = GeminiRealtimeSession::connect(
        backend_custom,
        "models/gemini-live",
        RealtimeConfig::default(),
    )
    .await;

    assert!(session_res.is_ok(), "Connection to custom Vertex endpoint should succeed");

    let auth = received_headers.lock().pop();
    assert_eq!(auth, Some(None), "Custom Vertex endpoint must NOT receive Authorization header");

    server_handle.abort();
}

#[tokio::test]
#[ignore]
async fn test_live_gemini_managed_recovery_interruption() {
    use adk_realtime::config::ToolDefinition;
    use adk_realtime::events::ToolResponse;
    use adk_realtime::gemini::GeminiRealtimeModel;
    use adk_realtime::recovery::DeliveryCertainty;
    use adk_realtime::runner::RealtimeRunner;

    adk_core::ensure_crypto_provider();

    let Ok(api_key) = std::env::var("GEMINI_API_KEY").or_else(|_| std::env::var("GOOGLE_API_KEY"))
    else {
        println!(
            "GEMINI_API_KEY or GOOGLE_API_KEY not set; skipping live Gemini interruption proof test."
        );
        return;
    };

    let probe_tool = ToolDefinition::new("recovery_probe")
        .with_description("A test tool to probe recovery readiness after reconnect.")
        .with_parameters(json!({
            "type": "object",
            "properties": {
                "value": { "type": "string", "description": "Echo value" }
            },
            "required": ["value"]
        }));

    let model = Arc::new(GeminiRealtimeModel::new(
        GeminiLiveBackend::studio(api_key),
        "models/gemini-3.1-flash-live-preview",
    ));

    let runner = RealtimeRunner::builder()
        .model(model as adk_realtime::model::BoxedModel)
        .tool_fn(probe_tool, |call| {
            let val = call.arguments.get("value").and_then(|v| v.as_str()).unwrap_or("ok");
            Ok(json!({ "status": "probed", "value": val }))
        })
        .instruction("You are a helpful assistant. Reply concisely.")
        .build()
        .expect("Runner build should succeed");

    let mut gen_watcher = runner.subscribe_generation();

    // Generate a random recovery marker for logical session resumption verification
    let recovery_marker = format!("RESUME-MARKER-{}", uuid::Uuid::new_v4().simple());

    // 1. Healthy generation N (0): Prove initial generation is usable
    runner.connect().await.expect("Initial connect should succeed");
    assert!(runner.is_connected().await);
    let gen_n_id = *gen_watcher.borrow();
    assert_eq!(gen_n_id, 0);

    // Consume setupComplete frame from stream
    let setup_ev = runner.next_event().await;
    assert!(
        matches!(setup_ev, Some(Ok(ServerEvent::SessionCreated { .. }))),
        "Initial generation N must produce SessionCreated setupComplete frame"
    );

    // Perform one complete real turn on generation N to record context and receive a valid resumable checkpoint
    runner
        .send_text(&format!(
            "Please remember this secret recovery marker for later: {recovery_marker}. Say 'UNDERSTOOD' after storing it."
        ))
        .await
        .expect("Send text on N must succeed");

    let turn_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut initial_turn_done = false;
    while tokio::time::Instant::now() < turn_deadline {
        if let Some(Ok(event)) =
            tokio::time::timeout(Duration::from_secs(3), runner.next_event()).await.ok().flatten()
            && matches!(event, ServerEvent::ResponseDone { .. })
        {
            initial_turn_done = true;
            break;
        }
    }
    assert!(initial_turn_done, "Generation N must complete initial turn before interruption");

    // 2. Deliberately induce real transport failure on generation N
    runner.force_transport_break_for_testing().await.expect("Force transport break should succeed");

    // 3. Trigger report_failure in a background task so supervisor enters Recovering
    let runner_arc = Arc::new(runner);
    let runner_clone = Arc::clone(&runner_arc);
    let first_write_task =
        tokio::spawn(async move { runner_clone.send_text("hello during break").await });

    // Wait until supervisor transitions to Recovering (admit_write rejects as NotAttempted)
    let mut write_rejected_as_not_attempted = false;
    for _ in 0..100 {
        let res = runner_arc.send_text("hello during recovery").await;
        if let Err(RealtimeError::WriteFailed {
            certainty: DeliveryCertainty::NotAttempted, ..
        }) = res
        {
            write_rejected_as_not_attempted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Wait for first write task to finish (it reports failure and triggers managed recovery)
    let first_write_res = first_write_task.await.expect("first write task join");
    match first_write_res {
        Err(RealtimeError::WriteFailed { certainty, .. }) => {
            assert_eq!(
                certainty,
                DeliveryCertainty::Indeterminate,
                "First write after break must be Indeterminate"
            );
        }
        other => panic!("Expected WriteFailed Indeterminate for first write, got {:?}", other),
    }

    assert!(
        write_rejected_as_not_attempted,
        "Write during active Recovering state must be rejected as NotAttempted"
    );

    let runner = runner_arc;

    // 4. Background recovery: next_event completes recovery and publishes N+1
    let event_res = runner.next_event().await;
    assert!(event_res.is_some(), "next_event should complete via managed recovery");

    // 5. Verify N+1 publication & public watcher wakeup
    if *gen_watcher.borrow() == gen_n_id {
        gen_watcher.changed().await.expect("Watcher must wake on N+1 publication");
    }
    let gen_n1_id = *gen_watcher.borrow();
    assert_eq!(gen_n1_id, gen_n_id + 1, "Generation must advance exactly from N to N+1");

    // 6. Prove logical session continuity after N+1: ask Gemini for the pre-disconnect secret marker via recovery_probe tool call
    runner
        .send_text("Please call the tool recovery_probe with the secret recovery marker I gave you before the disconnect.")
        .await
        .expect("Send text on N+1 must succeed");

    // 7. Observe function call -> function response -> model continuation on N+1
    let mut received_function_call = false;
    let mut marker_matched = false;
    let mut received_final_response = false;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if let Some(Ok(event)) =
            tokio::time::timeout(Duration::from_secs(3), runner.next_event()).await.ok().flatten()
        {
            match event {
                ServerEvent::FunctionCallDone { call_id, name, arguments, .. } => {
                    assert_eq!(name, "recovery_probe");
                    received_function_call = true;
                    let val = arguments.get("value").and_then(|v| v.as_str()).unwrap_or("");
                    if val == recovery_marker {
                        marker_matched = true;
                    }
                    let response = ToolResponse {
                        call_id,
                        output: json!({ "status": "probed", "value": val }),
                    };
                    runner
                        .send_tool_response(response)
                        .await
                        .expect("send_tool_response on N+1 must succeed");
                }
                ServerEvent::TextDelta { delta, .. }
                    if !delta.is_empty() && received_function_call =>
                {
                    received_final_response = true;
                    break;
                }
                ServerEvent::AudioDelta { delta, .. }
                    if !delta.is_empty() && received_function_call =>
                {
                    received_final_response = true;
                    break;
                }
                _ => {}
            }
        }
    }

    assert!(received_function_call, "Must receive real function call on generation N+1");
    assert!(
        marker_matched,
        "Logical continuity proof failed: Gemini on N+1 did not return the exact pre-disconnect recovery marker ({recovery_marker})"
    );
    assert!(
        received_final_response,
        "Must receive real model response after function response on N+1"
    );
}
