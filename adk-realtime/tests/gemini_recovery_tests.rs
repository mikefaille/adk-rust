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
    let (addr, _s) = spawn_mock_ws_server(|mut ws| async move {
        // Read setup frame
        let _setup = ws.next().await;
        // Send setupComplete first
        let setup_complete = json!({ "setupComplete": {} });
        ws.send(Message::Text(setup_complete.to_string().into())).await.unwrap();

        // Send two serverContent frames in a single burst
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
    })
    .await;

    let backend = GeminiLiveBackend::studio("test-key").with_endpoint_url(format!("ws://{}", addr));
    let config = RealtimeConfig::default();

    let session =
        GeminiRealtimeSession::connect(backend, "models/gemini-live", config).await.unwrap();

    // Consume first event
    let ev1 = session.next_event().await.unwrap().unwrap();
    assert!(matches!(ev1, ServerEvent::SessionCreated { .. }));

    // Read Chunk 1 (which decodes WS message carrying Chunk 1 + Chunk 2 into event_queue synchronously)
    let ev2 = session.next_event().await.unwrap().unwrap();
    if let ServerEvent::InputTranscriptDelta { delta, .. } = ev2 {
        assert_eq!(delta, "Chunk 1");
    } else {
        panic!("Expected InputTranscriptDelta Chunk 1, got {:?}", ev2);
    }

    // Now test cancellation mid-read: create a next_event() future and drop it without awaiting it
    let fut = session.next_event();
    drop(fut);

    // Read next event: Chunk 2 must still be present in event_queue despite the dropped future
    let ev3 = session.next_event().await.unwrap().unwrap();
    if let ServerEvent::InputTranscriptDelta { delta, .. } = ev3 {
        assert_eq!(delta, "Chunk 2");
    } else {
        panic!("Expected InputTranscriptDelta Chunk 2, got {:?}", ev3);
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
