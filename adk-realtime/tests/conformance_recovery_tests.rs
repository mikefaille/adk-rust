#![cfg(feature = "gemini")]

use adk_realtime::audio::AudioChunk;
use adk_realtime::config::{RealtimeConfig, ToolDefinition};
use adk_realtime::events::ClientEvent;
use adk_realtime::session::{ContextMutationOutcome, RealtimeAvailability};
use adk_realtime::{
    RealtimeError, RealtimeSession, RecoveryCapability, RecoveryContinuity, RecoveryDisposition,
    RecoveryOutcome, RecoveryPolicy, Result, ServerEvent, ToolResponse,
};
use async_trait::async_trait;
use futures::{SinkExt, Stream, StreamExt};
use serde_json::{Value, json};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

struct MockSession {
    session_id: String,
    capability: RecoveryCapability,
    attempts: Arc<AtomicUsize>,
    fail_count: usize,
    fatal_on_attempt: Option<usize>,
    hang_on_attempt: Option<usize>,
    continuity: RecoveryContinuity,
    availability: Arc<std::sync::Mutex<RealtimeAvailability>>,
    is_connected_flag: Arc<AtomicBool>,
}

#[async_trait]
impl RealtimeSession for MockSession {
    fn session_id(&self) -> &str {
        &self.session_id
    }
    fn is_connected(&self) -> bool {
        self.is_connected_flag.load(Ordering::SeqCst)
    }
    fn availability(&self) -> RealtimeAvailability {
        self.availability.lock().unwrap().clone()
    }
    fn recovery_capability(&self) -> RecoveryCapability {
        self.capability
    }
    fn recovery_disposition(&self, error: &RealtimeError) -> RecoveryDisposition {
        match error {
            RealtimeError::ConfigError(_) => RecoveryDisposition::Fatal,
            _ => RecoveryDisposition::Retryable,
        }
    }
    async fn recover_once(&self, _config: &RealtimeConfig) -> Result<RecoveryOutcome> {
        let att = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;

        {
            let mut guard = self.availability.lock().unwrap();
            *guard = RealtimeAvailability::Reconnecting { epoch: att as u64 };
        }

        if let Some(hang_att) = self.hang_on_attempt {
            if att == hang_att {
                std::future::pending::<()>().await;
            }
        }

        if let Some(fatal_att) = self.fatal_on_attempt {
            if att == fatal_att {
                let mut guard = self.availability.lock().unwrap();
                *guard = RealtimeAvailability::Exhausted;
                self.is_connected_flag.store(false, Ordering::SeqCst);
                return Err(RealtimeError::ConfigError("fatal error".into()));
            }
        }

        if att <= self.fail_count {
            return Err(RealtimeError::ConnectionError("transient fail".into()));
        }

        let mut guard = self.availability.lock().unwrap();
        *guard = RealtimeAvailability::Connected;
        self.is_connected_flag.store(true, Ordering::SeqCst);

        Ok(RecoveryOutcome { continuity: self.continuity })
    }

    fn set_availability(&self, availability: RealtimeAvailability) {
        let mut guard = self.availability.lock().unwrap();
        *guard = availability;
    }

    async fn send_audio(&self, _audio: &AudioChunk) -> Result<()> {
        Ok(())
    }
    async fn send_audio_base64(&self, _audio_base64: &str) -> Result<()> {
        Ok(())
    }
    async fn send_text(&self, _text: &str) -> Result<()> {
        Ok(())
    }
    async fn send_tool_response(&self, _response: ToolResponse) -> Result<()> {
        Ok(())
    }
    async fn commit_audio(&self) -> Result<()> {
        Ok(())
    }
    async fn clear_audio(&self) -> Result<()> {
        Ok(())
    }
    async fn create_response(&self) -> Result<()> {
        Ok(())
    }
    async fn interrupt(&self) -> Result<()> {
        Ok(())
    }
    async fn send_event(&self, _event: ClientEvent) -> Result<()> {
        Ok(())
    }
    async fn next_event(&self) -> Option<Result<ServerEvent>> {
        None
    }
    fn events(&self) -> Pin<Box<dyn Stream<Item = Result<ServerEvent>> + Send + '_>> {
        Box::pin(futures::stream::empty())
    }
    async fn close(&self) -> Result<()> {
        Ok(())
    }
    async fn mutate_context(&self, _config: RealtimeConfig) -> Result<ContextMutationOutcome> {
        Ok(ContextMutationOutcome::Applied)
    }
}

#[tokio::test]
async fn test_conformance_unsupported_adapter() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let session = MockSession {
        session_id: "test".into(),
        capability: RecoveryCapability::Unsupported,
        attempts: attempts.clone(),
        fail_count: 0,
        fatal_on_attempt: None,
        hang_on_attempt: None,
        continuity: RecoveryContinuity::Reconnected,
        availability: Arc::new(std::sync::Mutex::new(RealtimeAvailability::Connected)),
        is_connected_flag: Arc::new(AtomicBool::new(true)),
    };

    let config = RealtimeConfig::default();
    let policy = RecoveryPolicy::default();

    let result = adk_realtime::session::recover_session(&session, &config, &policy).await;
    assert!(result.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    assert!(result.unwrap_err().to_string().contains("Recovery is unsupported"));
}

#[tokio::test]
async fn test_conformance_transient_failures_and_success() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let availability = Arc::new(std::sync::Mutex::new(RealtimeAvailability::Connected));
    let is_connected_flag = Arc::new(AtomicBool::new(true));

    let session = MockSession {
        session_id: "test".into(),
        capability: RecoveryCapability::Reconnect,
        attempts: attempts.clone(),
        fail_count: 2,
        fatal_on_attempt: None,
        hang_on_attempt: None,
        continuity: RecoveryContinuity::Reconnected,
        availability: availability.clone(),
        is_connected_flag: is_connected_flag.clone(),
    };

    let config = RealtimeConfig::default();
    let policy = RecoveryPolicy {
        max_attempts: std::num::NonZeroU32::new(3).unwrap(),
        deadline: Duration::from_secs(5),
        initial_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(50),
    };

    let result = adk_realtime::session::recover_session(&session, &config, &policy).await;
    assert!(result.is_ok());
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(*availability.lock().unwrap(), RealtimeAvailability::Connected);
}

#[tokio::test]
async fn test_conformance_fatal_error_not_retried() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let session = MockSession {
        session_id: "test".into(),
        capability: RecoveryCapability::Reconnect,
        attempts: attempts.clone(),
        fail_count: 5,
        fatal_on_attempt: Some(2),
        hang_on_attempt: None,
        continuity: RecoveryContinuity::Reconnected,
        availability: Arc::new(std::sync::Mutex::new(RealtimeAvailability::Connected)),
        is_connected_flag: Arc::new(AtomicBool::new(true)),
    };

    let config = RealtimeConfig::default();
    let policy = RecoveryPolicy {
        max_attempts: std::num::NonZeroU32::new(5).unwrap(),
        deadline: Duration::from_secs(5),
        initial_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(50),
    };

    let result = adk_realtime::session::recover_session(&session, &config, &policy).await;
    assert!(result.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_conformance_absolute_deadline_terminates_hanging() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let session = MockSession {
        session_id: "test".into(),
        capability: RecoveryCapability::Reconnect,
        attempts: attempts.clone(),
        fail_count: 5,
        fatal_on_attempt: None,
        hang_on_attempt: Some(1),
        continuity: RecoveryContinuity::Reconnected,
        availability: Arc::new(std::sync::Mutex::new(RealtimeAvailability::Connected)),
        is_connected_flag: Arc::new(AtomicBool::new(true)),
    };

    let config = RealtimeConfig::default();
    let policy = RecoveryPolicy {
        max_attempts: std::num::NonZeroU32::new(5).unwrap(),
        deadline: Duration::from_millis(150),
        initial_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(50),
    };

    let result = adk_realtime::session::recover_session(&session, &config, &policy).await;
    assert!(result.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(result.unwrap_err().to_string().contains("timed out"));
}

#[tokio::test]
async fn test_conformance_retry_count_semantics() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let session = MockSession {
        session_id: "test".into(),
        capability: RecoveryCapability::Reconnect,
        attempts: attempts.clone(),
        fail_count: 10,
        fatal_on_attempt: None,
        hang_on_attempt: None,
        continuity: RecoveryContinuity::Reconnected,
        availability: Arc::new(std::sync::Mutex::new(RealtimeAvailability::Connected)),
        is_connected_flag: Arc::new(AtomicBool::new(true)),
    };

    let config = RealtimeConfig::default();
    let policy = RecoveryPolicy {
        max_attempts: std::num::NonZeroU32::new(4).unwrap(),
        deadline: Duration::from_secs(5),
        initial_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(50),
    };

    let result = adk_realtime::session::recover_session(&session, &config, &policy).await;
    assert!(result.is_err());
    assert_eq!(attempts.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn test_conformance_availability_transitions() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let availability = Arc::new(std::sync::Mutex::new(RealtimeAvailability::Connected));
    let session = MockSession {
        session_id: "test".into(),
        capability: RecoveryCapability::Reconnect,
        attempts: attempts.clone(),
        fail_count: 1,
        fatal_on_attempt: None,
        hang_on_attempt: None,
        continuity: RecoveryContinuity::Reconnected,
        availability: availability.clone(),
        is_connected_flag: Arc::new(AtomicBool::new(true)),
    };

    // Assert initial Connected
    assert_eq!(*availability.lock().unwrap(), RealtimeAvailability::Connected);

    let config = RealtimeConfig::default();
    let policy = RecoveryPolicy {
        max_attempts: std::num::NonZeroU32::new(2).unwrap(),
        deadline: Duration::from_secs(5),
        initial_delay: Duration::from_millis(10),
        max_delay: Duration::from_millis(50),
    };

    let result = adk_realtime::session::recover_session(&session, &config, &policy).await;
    assert!(result.is_ok());
    // Since it failed once and then succeeded, final availability is Connected
    assert_eq!(*availability.lock().unwrap(), RealtimeAvailability::Connected);
}

#[tokio::test]
async fn test_gemini_recovery_transactional_private_until_ready() {
    // Install the explicit default cryptographic provider for the process to avoid double init conflict
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // 1. Bind local TCP listener for WebSocket connection mock
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{}", local_addr);

    // 2. Spawn mock WebSocket Server that verifies Gemini setup frames
    let server_task = tokio::spawn(async move {
        // --- Connection 1: Initial connection & emission of sessionResumptionUpdate ---
        let (stream1, _) = listener.accept().await.unwrap();
        let mut ws1 = accept_async(stream1).await.unwrap();

        // The first frame on Connection 1 must be setup
        if let Some(Ok(Message::Text(msg))) = ws1.next().await {
            let json: Value = serde_json::from_str(&msg).unwrap();
            assert!(json.get("setup").is_some());
        } else {
            panic!("Expected setup frame on Connection 1");
        }

        // Send setupComplete
        let setup_complete = json!({ "setupComplete": {} }).to_string();
        ws1.send(Message::Text(setup_complete.into())).await.unwrap();

        // Send sessionResumptionUpdate
        let resumption_update = json!({
            "sessionResumptionUpdate": {
                "resumable": true,
                "newHandle": "valid_token_123"
            }
        })
        .to_string();
        ws1.send(Message::Text(resumption_update.into())).await.unwrap();

        // Gracefully finish connection 1
        let _ = ws1.close(None).await;
        drop(ws1);

        // --- Connection 2: Reconnection/Recovery ---
        let (stream2, _) = listener.accept().await.unwrap();
        let mut ws2 = accept_async(stream2).await.unwrap();

        // The first frame on Connection 2 must be setup replaying instructions, tools and sessionResumption
        if let Some(Ok(Message::Text(msg))) = ws2.next().await {
            let json: Value = serde_json::from_str(&msg).unwrap();
            let setup =
                json.get("setup").expect("Expected setup frame as first frame on Connection 2");
            assert_eq!(
                setup.get("model").unwrap().as_str(),
                Some("models/gemini-3.1-flash-live-preview")
            );

            // Check replayed config
            let inst =
                setup.get("systemInstruction").unwrap().get("parts").unwrap().as_array().unwrap()
                    [0]
                .get("text")
                .unwrap()
                .as_str()
                .unwrap();
            assert_eq!(inst, "You are a specialized test helper.");

            // Check replayed tools
            let tools = setup.get("tools").unwrap().as_array().unwrap()[0]
                .get("functionDeclarations")
                .unwrap()
                .as_array()
                .unwrap();
            assert_eq!(tools[0].get("name").unwrap().as_str(), Some("test_tool"));

            // Check resumption handle replayed internally
            let resumption = setup.get("sessionResumption").unwrap();
            assert_eq!(resumption.get("handle").unwrap().as_str(), Some("valid_token_123"));
        } else {
            panic!("Expected setup frame on Connection 2");
        }

        // Send setupComplete on Connection 2
        let setup_complete_2 = json!({ "setupComplete": {} }).to_string();
        ws2.send(Message::Text(setup_complete_2.into())).await.unwrap();
    });

    // 3. Connect client session using the mock WebSocket server
    let (ws_client, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let (mut sink, source) = ws_client.split();

    // Send initial setup frame
    let setup_msg = json!({
        "setup": { "model": "models/gemini-3.1-flash-live-preview" }
    })
    .to_string();
    sink.send(Message::Text(setup_msg.into())).await.unwrap();

    let (tx, rx) = mpsc::channel(256);
    let writer_task = tokio::spawn(async move {
        let mut rx = rx;
        let mut sink = sink;
        while let Some(msg) = rx.recv().await {
            let _ = sink.send(msg);
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

    // Consume mock setup created event (ServerEvent::SessionCreated)
    let event1 = session.next_event().await.unwrap().unwrap();
    assert!(matches!(event1, ServerEvent::SessionCreated { .. }));

    // Receive and process the sessionResumptionUpdate (ServerEvent::SessionUpdated)
    let event2 = session.next_event().await.unwrap().unwrap();
    assert!(matches!(event2, ServerEvent::SessionUpdated { .. }));

    // Now last_resume_handle is cached!
    assert_eq!(session.last_resume_handle().as_deref(), Some("valid_token_123"));

    // 4. Test recover_once
    let mut config = RealtimeConfig::default();
    config.instruction = Some("You are a specialized test helper.".into());
    config.tools = Some(vec![ToolDefinition::new("test_tool")]);

    let result = session.recover_once(&config).await;
    assert!(result.is_ok(), "Candidate connect and setup replaying config failed: {:?}", result);

    server_task.await.unwrap();
}
