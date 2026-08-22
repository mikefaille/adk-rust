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

/// Integration-only controlled WebSocket proxy that connects to the real Gemini Live endpoint.
///
/// Forwards traffic bi-directionally for healthy connections, and provides `trigger_abrupt_disconnect()`
/// to sever the ADK-facing TCP transport abruptly without sending a graceful WebSocket Close frame.
pub struct GeminiLiveWsProxy {
    addr: SocketAddr,
    disconnect_notify: Arc<tokio::sync::Notify>,
    armed_for_checkpoint: Arc<std::sync::atomic::AtomicBool>,
    resume_checkpoint_observed: Arc<std::sync::atomic::AtomicBool>,
    candidate_resume_handle_match: Arc<std::sync::atomic::AtomicBool>,
    proxy_handle: tokio::task::JoinHandle<()>,
}

impl GeminiLiveWsProxy {
    pub async fn start(api_key: String) -> Self {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        adk_core::ensure_crypto_provider();
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("proxy TcpListener bind");
        let addr = listener.local_addr().expect("proxy local_addr");
        let disconnect_notify = Arc::new(tokio::sync::Notify::new());
        let armed_for_checkpoint = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let resume_checkpoint_observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let candidate_resume_handle_match = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let target_url = format!("{}?key={}", adk_realtime::gemini::GEMINI_LIVE_URL, api_key);

        let disconnect_notify_clone = Arc::clone(&disconnect_notify);
        let armed_clone = Arc::clone(&armed_for_checkpoint);
        let resume_obs_clone = Arc::clone(&resume_checkpoint_observed);
        let candidate_match_clone = Arc::clone(&candidate_resume_handle_match);

        let proxy_handle = tokio::spawn(async move {
            let conn_counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let captured_handle = Arc::new(parking_lot::Mutex::new(None::<String>));

            while let Ok((stream, _)) = listener.accept().await {
                let conn_id = conn_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                let target_url = target_url.clone();
                let disconnect_notify = Arc::clone(&disconnect_notify_clone);
                let armed = Arc::clone(&armed_clone);
                let resume_obs = Arc::clone(&resume_obs_clone);
                let candidate_match = Arc::clone(&candidate_match_clone);
                let captured_handle = Arc::clone(&captured_handle);

                tokio::spawn(async move {
                    let client_ws = match accept_async(stream).await {
                        Ok(ws) => ws,
                        Err(e) => {
                            eprintln!("GeminiLiveWsProxy accept handshake error: {e}");
                            return;
                        }
                    };

                    let target_ws = match tokio_tungstenite::connect_async(&target_url).await {
                        Ok((ws, _)) => ws,
                        Err(e) => {
                            eprintln!(
                                "GeminiLiveWsProxy connect to real Gemini upstream error: {e}"
                            );
                            return;
                        }
                    };

                    let (mut client_sink, mut client_stream) = client_ws.split();
                    let (mut target_sink, mut target_stream) = target_ws.split();

                    tracing::info!(conn_id, "GeminiLiveWsProxy accepted connection");
                    if conn_id == 1 {
                        // Connection 1: healthy traffic forwarding until abrupt disconnect signal
                        loop {
                            tokio::select! {
                                _ = disconnect_notify.notified() => {
                                    tracing::info!("GeminiLiveWsProxy: Inducing abrupt transport drop on connection 1");
                                    break;
                                }
                                client_msg = client_stream.next() => {
                                    match client_msg {
                                        Some(Ok(msg)) => {
                                            if target_sink.send(msg).await.is_err() {
                                                break;
                                            }
                                        }
                                        _ => break,
                                    }
                                }
                                target_msg = target_stream.next() => {
                                    match target_msg {
                                        Some(Ok(msg)) => {
                                            let text_opt = match &msg {
                                                Message::Text(t) => Some(t.as_str()),
                                                Message::Binary(b) => std::str::from_utf8(b).ok(),
                                                _ => None,
                                            };
                                            if let Some(text) = text_opt
                                                && let Ok(val) = serde_json::from_str::<serde_json::Value>(text)
                                                && let Some(update) = val.get("sessionResumptionUpdate")
                                                && update.get("resumable").and_then(|r| r.as_bool()) == Some(true)
                                                && let Some(handle) = update.get("newHandle").and_then(|h| h.as_str())
                                            {
                                                *captured_handle.lock() = Some(handle.to_string());
                                                if armed.load(std::sync::atomic::Ordering::SeqCst) {
                                                    resume_obs.store(true, std::sync::atomic::Ordering::SeqCst);
                                                    tracing::info!(resume_checkpoint_observed = true, "Captured post-marker resumable handle in proxy");
                                                }
                                            }
                                            if client_sink.send(msg).await.is_err() {
                                                break;
                                            }
                                        }
                                        _ => break,
                                    }
                                }
                            }
                        }
                    } else {
                        // Connection 2+: Candidate and subsequent recovery connections
                        let mut first_frame_checked = false;
                        loop {
                            tokio::select! {
                                client_msg = client_stream.next() => {
                                    match client_msg {
                                        Some(Ok(msg)) => {
                                            if !first_frame_checked {
                                                let text_opt = match &msg {
                                                    Message::Text(t) => Some(t.as_str()),
                                                    Message::Binary(b) => std::str::from_utf8(b).ok(),
                                                    _ => None,
                                                };
                                                if let Some(text) = text_opt {
                                                    first_frame_checked = true;
                                                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(text) {
                                                        let cand_handle = val
                                                            .get("setup")
                                                            .and_then(|s| s.get("sessionResumption"))
                                                            .and_then(|r| r.get("handle"))
                                                            .and_then(|h| h.as_str());
                                                        let expected_handle = captured_handle.lock().clone();
                                                        if cand_handle.is_some() && cand_handle == expected_handle.as_deref() {
                                                            candidate_match.store(true, std::sync::atomic::Ordering::SeqCst);
                                                            tracing::info!(candidate_resume_handle_match = true, "Candidate handle matched captured checkpoint in proxy");
                                                        }
                                                    }
                                                }
                                            }
                                            if target_sink.send(msg).await.is_err() {
                                                break;
                                            }
                                        }
                                        _ => break,
                                    }
                                }
                                target_msg = target_stream.next() => {
                                    match target_msg {
                                        Some(Ok(msg)) => {
                                            if client_sink.send(msg).await.is_err() {
                                                break;
                                            }
                                        }
                                        _ => break,
                                    }
                                }
                            }
                        }
                    }
                });
            }
        });

        Self {
            addr,
            disconnect_notify,
            armed_for_checkpoint,
            resume_checkpoint_observed,
            candidate_resume_handle_match,
            proxy_handle,
        }
    }

    pub fn url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    pub fn arm_checkpoint_capture(&self) {
        self.armed_for_checkpoint.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn trigger_abrupt_disconnect(&self) {
        self.disconnect_notify.notify_one();
    }

    pub fn resume_checkpoint_observed(&self) -> bool {
        self.resume_checkpoint_observed.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn candidate_resume_handle_match(&self) -> bool {
        self.candidate_resume_handle_match.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for GeminiLiveWsProxy {
    fn drop(&mut self) {
        self.proxy_handle.abort();
    }
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

    // Read until sessionResumptionUpdate is processed internally
    let _ = session_n.next_event().await;
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

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let _ = tracing_subscriber::fmt().with_env_filter("info").try_init();
    adk_core::ensure_crypto_provider();

    let api_key = std::env::var("GEMINI_API_KEY")
        .or_else(|_| std::env::var("GOOGLE_API_KEY"))
        .ok()
        .filter(|k| !k.trim().is_empty())
        .expect("GEMINI_API_KEY or GOOGLE_API_KEY environment variable required for live Gemini recovery proof test");

    let proxy = GeminiLiveWsProxy::start(api_key.clone()).await;

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
        GeminiLiveBackend::studio(api_key).with_endpoint_url(proxy.url()),
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

    // 1. Connect initial generation N (0): prove initial generation receives real setupComplete
    runner.connect().await.expect("Initial connect should succeed");
    assert!(runner.is_connected().await);
    let gen_n_id = *gen_watcher.borrow_and_update();
    assert_eq!(gen_n_id, 0);

    let setup_ev = runner.next_event().await;
    match setup_ev {
        Some(Ok(ServerEvent::SessionCreated { .. })) => {
            tracing::info!("Generation N connected and received setupComplete");
        }
        Some(Ok(other)) => panic!("Expected SessionCreated setupComplete frame, got: {other:?}"),
        Some(Err(err)) => panic!("Initial connect produced error: {err:?}"),
        None => panic!("Initial connect returned EOF"),
    }

    // Generate random secret recovery marker
    let random_marker = format!("marker-{}", uuid::Uuid::new_v4());

    // Tell Gemini the secret recovery marker on Generation N
    runner
        .send_text(&format!(
            "Remember this secret recovery marker: {random_marker}. Say 'understood' in one word."
        ))
        .await
        .expect("Send text on Generation N must succeed");

    let mut marker_turn_done = false;
    let n_usable_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < n_usable_deadline {
        let ev = match tokio::time::timeout(Duration::from_secs(4), runner.next_event()).await {
            Ok(Some(Ok(e))) => e,
            Ok(Some(Err(err))) => panic!("Error on Generation N marker prompt: {err:?}"),
            Ok(None) => panic!("Unexpected EOF on Generation N marker prompt"),
            Err(_) => continue,
        };
        if matches!(ev, ServerEvent::ResponseDone { .. }) {
            marker_turn_done = true;
            break;
        }
    }
    assert!(
        marker_turn_done,
        "Generation N marker turn must reach ResponseDone before interruption"
    );

    // Arm proxy to capture sessionResumptionUpdate AFTER marker turn completes
    proxy.arm_checkpoint_capture();

    // Wait until proxy observes a valid sessionResumptionUpdate checkpoint frame after marker turn
    let checkpoint_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < checkpoint_deadline && !proxy.resume_checkpoint_observed() {
        let _ = tokio::time::timeout(Duration::from_millis(500), runner.next_event()).await;
    }
    assert!(
        proxy.resume_checkpoint_observed(),
        "Proxy must observe a valid sessionResumptionUpdate checkpoint frame on Generation N"
    );
    tracing::info!(
        resume_checkpoint_observed = true,
        "Confirmed checkpoint observed prior to abrupt disconnect"
    );

    // 2. Set test recovery barrier on recovery supervisor
    let barrier = Arc::new(adk_realtime::recovery::TestRecoveryBarrier::new());
    runner.set_recovery_barrier_for_testing(barrier.clone());

    // 3. Induce abrupt transport drop via proxy (genuine TCP drop without WebSocket Close frame)
    proxy.trigger_abrupt_disconnect();

    // 4. Start background read task so managed read path observes EOF/reset and triggers supervisor.report_failure
    let runner_arc = Arc::new(runner);
    let runner_read_clone = Arc::clone(&runner_arc);
    let read_task = tokio::spawn(async move { runner_read_clone.next_event().await });

    // 5. Wait on barrier to confirm supervisor has entered TransportStatus::Recovering (with 5s timeout)
    tokio::time::timeout(Duration::from_secs(5), barrier.wait_until_recovering_entered())
        .await
        .expect("Recovery supervisor must enter Recovering state within 5s");

    // 6. Issue exactly one managed write during held Recovering window and prove it returns WriteFailed(NotAttempted)
    let write_res = runner_arc.send_text("hello during recovery").await;
    match write_res {
        Err(RealtimeError::WriteFailed { certainty: DeliveryCertainty::NotAttempted, .. }) => {
            tracing::info!(
                "Managed write issued during Recovering was correctly rejected as NotAttempted"
            );
        }
        other => panic!(
            "Expected WriteFailed(NotAttempted) during held Recovering window, got: {:?}",
            other
        ),
    }

    // 7. Release recovery barrier, allowing candidate session (N+1) to connect, send setup first, receive setupComplete, and publish
    barrier.release();

    // 8. Verify generation advances monotonically N -> N+1 and subscribe_generation() watcher wakes up without requiring new app traffic
    tokio::time::timeout(Duration::from_secs(10), gen_watcher.changed())
        .await
        .expect("Watcher must wake on N+1 publication within 10s")
        .expect("Watcher channel valid");
    let gen_n1_id = *gen_watcher.borrow_and_update();
    assert_eq!(
        gen_n1_id,
        gen_n_id + 1,
        "Generation must advance monotonically from N ({gen_n_id}) to N+1 ({})",
        gen_n_id + 1
    );

    assert!(
        proxy.candidate_resume_handle_match(),
        "Candidate session N+1 setup frame must present the exact resume handle captured from Generation N checkpoint"
    );
    tracing::info!(
        candidate_resume_handle_match = true,
        "Confirmed candidate handle match on N+1 setup"
    );

    let runner = runner_arc;

    // 9. Post-recovery conversation continuity proof: prompt Gemini on N+1 WITHOUT repeating the secret marker
    runner
        .send_text("Please call recovery_probe with the secret recovery marker I gave you before the disconnect, then confirm verbally or in text.")
        .await
        .expect("Send text on N+1 must succeed");

    // 10. Await first event from background read_task (which is currently polling next_event() on N+1)
    let first_ev = tokio::time::timeout(Duration::from_secs(10), read_task)
        .await
        .expect("read_task must complete within 10s")
        .expect("read_task join");

    let mut received_function_call = false;
    let mut received_post_tool_content = false;
    let mut received_final_response = false;

    match first_ev {
        Some(Ok(event)) => {
            if let ServerEvent::FunctionCallDone { call_id, name, arguments, .. } = event {
                assert_eq!(name, "recovery_probe");
                let probed_val = arguments.get("value").and_then(|v| v.as_str());
                assert_eq!(
                    probed_val,
                    Some(random_marker.as_str()),
                    "Gemini N+1 must recall the exact secret recovery marker from Generation N history without repetition!"
                );
                tracing::info!(
                    marker_continuity_success = true,
                    "Secret recovery marker recalled successfully on N+1"
                );
                received_function_call = true;
                let response = ToolResponse {
                    call_id,
                    output: json!({ "status": "probed", "value": probed_val }),
                };
                runner
                    .send_tool_response(response)
                    .await
                    .expect("send_tool_response on N+1 must succeed");
            }
        }
        Some(Err(err)) => panic!("Error receiving first event post-recovery on N+1: {err:?}"),
        None => panic!("Unexpected EOF receiving first event post-recovery on N+1"),
    }

    // 11. Continue polling next_event() until model continuation content and then ResponseDone are observed
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline && !received_final_response {
        let event_opt =
            match tokio::time::timeout(Duration::from_secs(4), runner.next_event()).await {
                Ok(opt) => opt,
                Err(_) => continue,
            };

        match event_opt {
            Some(Ok(event)) => match event {
                ServerEvent::FunctionCallDone { call_id, name, arguments, .. }
                    if !received_function_call =>
                {
                    assert_eq!(name, "recovery_probe");
                    let probed_val = arguments.get("value").and_then(|v| v.as_str());
                    assert_eq!(
                        probed_val,
                        Some(random_marker.as_str()),
                        "Gemini N+1 must recall the exact secret recovery marker from Generation N history!"
                    );
                    tracing::info!(
                        marker_continuity_success = true,
                        "Secret recovery marker recalled successfully on N+1"
                    );
                    received_function_call = true;
                    let response = ToolResponse {
                        call_id,
                        output: json!({ "status": "probed", "value": probed_val }),
                    };
                    runner
                        .send_tool_response(response)
                        .await
                        .expect("send_tool_response on N+1 must succeed");
                }
                ServerEvent::TextDelta { delta, .. }
                    if !delta.is_empty() && received_function_call =>
                {
                    received_post_tool_content = true;
                }
                ServerEvent::AudioDelta { delta, .. }
                    if !delta.is_empty() && received_function_call =>
                {
                    received_post_tool_content = true;
                }
                ServerEvent::TranscriptDelta { delta, .. }
                    if !delta.is_empty() && received_function_call =>
                {
                    received_post_tool_content = true;
                }
                ServerEvent::ResponseDone { .. } if received_function_call => {
                    if received_post_tool_content {
                        received_final_response = true;
                        tracing::info!(
                            post_tool_continuation_success = true,
                            "Model continuation turn completed post-tool on N+1"
                        );
                        break;
                    } else {
                        tracing::info!(
                            "Received ResponseDone before post-tool content; continuing to wait for model continuation..."
                        );
                    }
                }
                _ => {}
            },
            Some(Err(err)) => panic!("Error during post-recovery interaction on N+1: {err:?}"),
            None => panic!("Unexpected EOF during post-recovery interaction on N+1"),
        }
    }

    assert!(received_function_call, "Must receive real function call on generation N+1");
    assert!(
        received_post_tool_content,
        "Must receive non-empty post-tool content delta on generation N+1"
    );
    assert!(
        received_final_response,
        "Must receive ResponseDone after post-tool content delta on N+1"
    );
}
