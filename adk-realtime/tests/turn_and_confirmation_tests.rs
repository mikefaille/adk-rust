use adk_realtime::{
    ClientEvent, ConfirmationId, FrozenToolCall, InputTurnCompleted,
    InputTurnCompletionSource, RealtimeConfig, RealtimeModel,
    RealtimeRunner, RealtimeSession, Result, ServerEvent, ToolConfirmationDecision,
    ToolConfirmationPolicyHook, ToolConfirmationRequest, ToolHandler, ToolResponse,
};
use adk_realtime::audio::AudioChunk;
use adk_realtime::config::ToolDefinition;
use adk_realtime::runner::EventHandler;

use async_trait::async_trait;
use serde_json::json;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// ── Mock structures ──────────────────────────────────────────────────

struct MockSession {
    rx: tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<Result<ServerEvent>>>>,
}

#[async_trait]
impl RealtimeSession for MockSession {
    fn session_id(&self) -> &str {
        "mock-session-123"
    }

    fn is_connected(&self) -> bool {
        true
    }

    async fn send_audio(&self, _audio: &AudioChunk) -> Result<()> {
        Ok(())
    }

    async fn send_audio_base64(&self, _audio: &str) -> Result<()> {
        Ok(())
    }

    async fn send_text(&self, _text: &str) -> Result<()> {
        Ok(())
    }

    async fn send_tool_response(&self, _response: ToolResponse) -> Result<()> {
        Ok(())
    }

    async fn send_tool_output(&self, _response: ToolResponse) -> Result<()> {
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
        let mut rx_guard = self.rx.lock().await;
        if let Some(ref mut rx) = *rx_guard { rx.recv().await } else { None }
    }

    fn events(&self) -> Pin<Box<dyn futures::Stream<Item = Result<ServerEvent>> + Send + '_>> {
        Box::pin(futures::stream::empty())
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }

    async fn mutate_context(
        &self,
        _config: RealtimeConfig,
    ) -> Result<adk_realtime::session::ContextMutationOutcome> {
        Ok(adk_realtime::session::ContextMutationOutcome::Applied)
    }
}

struct MockModel {
    session_rx: Mutex<Option<tokio::sync::mpsc::Receiver<Result<ServerEvent>>>>,
}

#[async_trait]
impl RealtimeModel for MockModel {
    fn provider(&self) -> &str {
        "mock"
    }

    fn model_id(&self) -> &str {
        "mock-model"
    }

    fn supported_input_formats(&self) -> Vec<adk_realtime::AudioFormat> {
        vec![]
    }

    fn supported_output_formats(&self) -> Vec<adk_realtime::AudioFormat> {
        vec![]
    }

    fn available_voices(&self) -> Vec<&str> {
        vec![]
    }

    async fn connect(&self, _config: RealtimeConfig) -> Result<adk_realtime::BoxedSession> {
        let rx = self.session_rx.lock().unwrap().take();
        Ok(Box::new(MockSession { rx: tokio::sync::Mutex::new(rx) }))
    }
}

#[derive(Default)]
struct RecordingEventHandler {
    speech_starts: AtomicUsize,
    speech_stops: AtomicUsize,
    transcripts_completed: AtomicUsize,
    turns_completed: Mutex<Vec<InputTurnCompleted>>,
    confirmations_requested: Mutex<Vec<ToolConfirmationRequest>>,
}

#[async_trait]
impl EventHandler for RecordingEventHandler {
    async fn on_speech_started(&self, _audio_start_ms: u64) -> Result<()> {
        self.speech_starts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn on_speech_stopped(&self, _audio_end_ms: u64) -> Result<()> {
        self.speech_stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn on_input_transcript_completed(&self, _transcript: &str, _item_id: &str) -> Result<()> {
        self.transcripts_completed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn on_input_turn_completed(&self, turn: &InputTurnCompleted) -> Result<()> {
        self.turns_completed.lock().unwrap().push(turn.clone());
        Ok(())
    }

    async fn on_tool_confirmation_requested(
        &self,
        request: &ToolConfirmationRequest,
    ) -> Result<()> {
        self.confirmations_requested.lock().unwrap().push(request.clone());
        Ok(())
    }
}

struct StaticToolConfirmationPolicy {
    require_confirm: bool,
    hint: Option<String>,
}

#[async_trait]
impl ToolConfirmationPolicyHook for StaticToolConfirmationPolicy {
    async fn require_confirmation(&self, _call: &FrozenToolCall) -> Result<Option<String>> {
        if self.require_confirm {
            Ok(Some(self.hint.clone().unwrap_or_else(|| "Confirm please".to_string())))
        } else {
            Ok(None)
        }
    }
}

struct DummyToolHandler {
    calls: Arc<AtomicUsize>,
    last_args: Mutex<Option<serde_json::Value>>,
}

#[async_trait]
impl ToolHandler for DummyToolHandler {
    async fn execute(&self, call: &adk_realtime::events::ToolCall) -> Result<serde_json::Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_args.lock().unwrap() = Some(call.arguments.clone());
        Ok(json!({"status": "success"}))
    }
}

// ── PART A: Normalized completed-input-turn tests ───────────────────

#[tokio::test]
async fn test_transcript_completed_emits_one_turn() {
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let model = MockModel { session_rx: Mutex::new(Some(rx)) };
    let handler = Arc::new(RecordingEventHandler::default());

    let runner = RealtimeRunner::builder()
        .model(Arc::new(model))
        .event_handler_arc(handler.clone())
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    // Trigger user text or activity to start a turn
    runner.send_text("Hello").await.unwrap();

    // Spawn the runner's event loop in the background
    let r = Arc::new(runner);
    let r_clone = r.clone();
    tokio::spawn(async move {
        r_clone.run().await.unwrap();
    });

    // Send final transcript event
    tx.send(Ok(ServerEvent::InputTranscriptCompleted {
        item_id: "turn-1".to_string(),
        content_index: 0,
        transcript: "hello".to_string(),
    }))
    .await
    .unwrap();

    // Yield to let the runner process
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let turns = handler.turns_completed.lock().unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].source, InputTurnCompletionSource::FinalTranscript);
    assert_eq!(turns[0].provider_item_id, Some("turn-1".to_string()));
}

#[tokio::test]
async fn test_speech_start_alone_emits_no_turn() {
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let model = MockModel { session_rx: Mutex::new(Some(rx)) };
    let handler = Arc::new(RecordingEventHandler::default());

    let runner = RealtimeRunner::builder()
        .model(Arc::new(model))
        .event_handler_arc(handler.clone())
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    let r = Arc::new(runner);
    let r_clone = r.clone();
    tokio::spawn(async move {
        r_clone.run().await.unwrap();
    });

    // Send only speech started
    tx.send(Ok(ServerEvent::SpeechStarted { event_id: "evt-1".to_string(), audio_start_ms: 100 }))
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(handler.speech_starts.load(Ordering::SeqCst), 1);
    let turns = handler.turns_completed.lock().unwrap();
    assert_eq!(turns.len(), 0);
}

#[tokio::test]
async fn test_activity_end_fallback_when_transcription_unavailable() {
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let model = MockModel { session_rx: Mutex::new(Some(rx)) };
    let handler = Arc::new(RecordingEventHandler::default());

    // Build WITHOUT transcription
    let runner = RealtimeRunner::builder()
        .model(Arc::new(model))
        .event_handler_arc(handler.clone())
        .config(RealtimeConfig::default()) // no input_audio_transcription
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    let r = Arc::new(runner);
    let r_clone = r.clone();
    tokio::spawn(async move {
        r_clone.run().await.unwrap();
    });

    // Start speech, then stop
    tx.send(Ok(ServerEvent::SpeechStarted { event_id: "evt-1".to_string(), audio_start_ms: 100 }))
        .await
        .unwrap();
    tx.send(Ok(ServerEvent::SpeechStopped { event_id: "evt-2".to_string(), audio_end_ms: 500 }))
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let turns = handler.turns_completed.lock().unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].source, InputTurnCompletionSource::ProviderActivityEnd);
}

#[tokio::test]
async fn test_transcript_plus_fallback_boundary_is_deduplicated() {
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let model = MockModel { session_rx: Mutex::new(Some(rx)) };
    let handler = Arc::new(RecordingEventHandler::default());

    // Build WITH transcription
    let runner = RealtimeRunner::builder()
        .model(Arc::new(model))
        .event_handler_arc(handler.clone())
        .config(RealtimeConfig::default().with_transcription())
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    let r = Arc::new(runner);
    let r_clone = r.clone();
    tokio::spawn(async move {
        r_clone.run().await.unwrap();
    });

    // Simulate speech start -> speech stop -> transcript completion
    tx.send(Ok(ServerEvent::SpeechStarted { event_id: "evt-1".to_string(), audio_start_ms: 100 }))
        .await
        .unwrap();
    tx.send(Ok(ServerEvent::SpeechStopped { event_id: "evt-2".to_string(), audio_end_ms: 500 }))
        .await
        .unwrap();
    tx.send(Ok(ServerEvent::InputTranscriptCompleted {
        item_id: "turn-1".to_string(),
        content_index: 0,
        transcript: "yes".to_string(),
    }))
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // VAD SpeechStopped should NOT emit completed turn because transcription is enabled,
    // only the InputTranscriptCompleted should. Result: exactly 1 turn.
    let turns = handler.turns_completed.lock().unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].source, InputTurnCompletionSource::FinalTranscript);
}

#[tokio::test]
async fn test_gemini_turn_complete_fallback() {
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let model = MockModel { session_rx: Mutex::new(Some(rx)) };
    let handler = Arc::new(RecordingEventHandler::default());

    let runner = RealtimeRunner::builder()
        .model(Arc::new(model))
        .event_handler_arc(handler.clone())
        .config(RealtimeConfig::default().with_transcription())
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    let r = Arc::new(runner);
    let r_clone = r.clone();
    tokio::spawn(async move {
        r_clone.run().await.unwrap();
    });

    // Simulate Speech start, but transcription is delayed/missing, and instead we receive ResponseCreated
    tx.send(Ok(ServerEvent::SpeechStarted { event_id: "evt-1".to_string(), audio_start_ms: 100 }))
        .await
        .unwrap();
    tx.send(Ok(ServerEvent::ResponseCreated {
        event_id: "evt-2".to_string(),
        response: json!({}),
    }))
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // ResponseCreated acts as an authoritative turn fallback if no transcript-finished arrived
    let turns = handler.turns_completed.lock().unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].source, InputTurnCompletionSource::ProviderTurnComplete);
}

// ── PART B: Identity-preserving confirmation transaction tests ───────

#[tokio::test]
async fn test_confirmation_request_preserves_the_exact_original_call() {
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let model = MockModel { session_rx: Mutex::new(Some(rx)) };
    let handler = Arc::new(RecordingEventHandler::default());
    let policy =
        StaticToolConfirmationPolicy { require_confirm: true, hint: Some("Really perform transaction?".to_string()) };
    let tool_calls = Arc::new(AtomicUsize::new(0));

    let runner = RealtimeRunner::builder()
        .model(Arc::new(model))
        .event_handler_arc(handler.clone())
        .confirmation_policy(policy)
        .tool(
            ToolDefinition::new("pay_bill"),
            DummyToolHandler { calls: tool_calls.clone(), last_args: Mutex::new(None) },
        )
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    let r = Arc::new(runner);
    let r_clone = r.clone();
    tokio::spawn(async move {
        r_clone.run().await.unwrap();
    });

    // Send tool call event
    tx.send(Ok(ServerEvent::FunctionCallDone {
        event_id: "evt-1".to_string(),
        response_id: "resp-1".to_string(),
        item_id: "item-1".to_string(),
        output_index: 0,
        call_id: "call-1".to_string(),
        name: "pay_bill".to_string(),
        arguments: json!({"amount": 100.0, "to": "Electricity Corp"}),
    }))
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Tool should not run yet
    assert_eq!(tool_calls.load(Ordering::SeqCst), 0);

    // Verify confirmation request details are preserved exactly
    let reqs = handler.confirmations_requested.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].hint, Some("Really perform transaction?".to_string()));
    assert_eq!(reqs[0].original_call.call_id, "call-1");
    assert_eq!(reqs[0].original_call.name, "pay_bill");
    assert_eq!(reqs[0].original_call.arguments, json!({"amount": 100.0, "to": "Electricity Corp"}));
}

#[tokio::test]
async fn test_confirmed_call_executes_once() {
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let model = MockModel { session_rx: Mutex::new(Some(rx)) };
    let handler = Arc::new(RecordingEventHandler::default());
    let policy =
        StaticToolConfirmationPolicy { require_confirm: true, hint: Some("Please confirm".to_string()) };
    let tool_calls = Arc::new(AtomicUsize::new(0));

    let runner = RealtimeRunner::builder()
        .model(Arc::new(model))
        .event_handler_arc(handler.clone())
        .confirmation_policy(policy)
        .tool(
            ToolDefinition::new("pay_bill"),
            DummyToolHandler { calls: tool_calls.clone(), last_args: Mutex::new(None) },
        )
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    let r = Arc::new(runner);
    let r_clone = r.clone();
    tokio::spawn(async move {
        r_clone.run().await.unwrap();
    });

    tx.send(Ok(ServerEvent::FunctionCallDone {
        event_id: "evt-1".to_string(),
        response_id: "resp-1".to_string(),
        item_id: "item-1".to_string(),
        output_index: 0,
        call_id: "call-1".to_string(),
        name: "pay_bill".to_string(),
        arguments: json!({"amount": 50.0}),
    }))
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Get the generated confirmation ID
    let conf_id = {
        let reqs = handler.confirmations_requested.lock().unwrap();
        reqs[0].confirmation_id
    };

    // Approve the confirmation
    r.resolve_confirmation(&conf_id, ToolConfirmationDecision::Confirmed { payload: None })
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify it executed exactly once
    assert_eq!(tool_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_rejected_call_executes_zero_times() {
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let model = MockModel { session_rx: Mutex::new(Some(rx)) };
    let handler = Arc::new(RecordingEventHandler::default());
    let policy =
        StaticToolConfirmationPolicy { require_confirm: true, hint: Some("Please confirm".to_string()) };
    let tool_calls = Arc::new(AtomicUsize::new(0));

    let runner = RealtimeRunner::builder()
        .model(Arc::new(model))
        .event_handler_arc(handler.clone())
        .confirmation_policy(policy)
        .tool(
            ToolDefinition::new("pay_bill"),
            DummyToolHandler { calls: tool_calls.clone(), last_args: Mutex::new(None) },
        )
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    let r = Arc::new(runner);
    let r_clone = r.clone();
    tokio::spawn(async move {
        r_clone.run().await.unwrap();
    });

    tx.send(Ok(ServerEvent::FunctionCallDone {
        event_id: "evt-1".to_string(),
        response_id: "resp-1".to_string(),
        item_id: "item-1".to_string(),
        output_index: 0,
        call_id: "call-1".to_string(),
        name: "pay_bill".to_string(),
        arguments: json!({"amount": 50.0}),
    }))
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let conf_id = {
        let reqs = handler.confirmations_requested.lock().unwrap();
        reqs[0].confirmation_id
    };

    // Reject the confirmation
    r.resolve_confirmation(
        &conf_id,
        ToolConfirmationDecision::Rejected { reason: Some("Insincere caller".to_string()) },
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Verify it executed ZERO times
    assert_eq!(tool_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_duplicate_confirmation_rejected() {
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let model = MockModel { session_rx: Mutex::new(Some(rx)) };
    let handler = Arc::new(RecordingEventHandler::default());
    let policy =
        StaticToolConfirmationPolicy { require_confirm: true, hint: Some("Confirm".to_string()) };
    let tool_calls = Arc::new(AtomicUsize::new(0));

    let runner = RealtimeRunner::builder()
        .model(Arc::new(model))
        .event_handler_arc(handler.clone())
        .confirmation_policy(policy)
        .tool(
            ToolDefinition::new("pay_bill"),
            DummyToolHandler { calls: tool_calls.clone(), last_args: Mutex::new(None) },
        )
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    let r = Arc::new(runner);
    let r_clone = r.clone();
    tokio::spawn(async move {
        r_clone.run().await.unwrap();
    });

    tx.send(Ok(ServerEvent::FunctionCallDone {
        event_id: "evt-1".to_string(),
        response_id: "resp-1".to_string(),
        item_id: "item-1".to_string(),
        output_index: 0,
        call_id: "call-1".to_string(),
        name: "pay_bill".to_string(),
        arguments: json!({"amount": 50.0}),
    }))
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let conf_id = {
        let reqs = handler.confirmations_requested.lock().unwrap();
        reqs[0].confirmation_id
    };

    // Approve the confirmation first time
    r.resolve_confirmation(&conf_id, ToolConfirmationDecision::Confirmed { payload: None })
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(tool_calls.load(Ordering::SeqCst), 1);

    // Double resolve must fail explicitly
    let res = r
        .resolve_confirmation(&conf_id, ToolConfirmationDecision::Confirmed { payload: None })
        .await;
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("Unknown, stale, or already-consumed"));

    // Ensure it was still only executed once
    assert_eq!(tool_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_stale_and_cross_session_confirmation_ids_are_rejected() {
    let model = MockModel { session_rx: Mutex::new(None) };
    let runner = RealtimeRunner::builder().model(Arc::new(model)).build().unwrap();

    let random_conf_id = ConfirmationId::new();
    let res = runner
        .resolve_confirmation(
            &random_conf_id,
            ToolConfirmationDecision::Confirmed { payload: None },
        )
        .await;
    assert!(res.is_err());
}

#[tokio::test]
async fn test_two_parallel_pending_calls_resolve_independently() {
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let model = MockModel { session_rx: Mutex::new(Some(rx)) };
    let handler = Arc::new(RecordingEventHandler::default());
    let policy =
        StaticToolConfirmationPolicy { require_confirm: true, hint: Some("Confirm".to_string()) };
    let tool_calls = Arc::new(AtomicUsize::new(0));

    let runner = RealtimeRunner::builder()
        .model(Arc::new(model))
        .event_handler_arc(handler.clone())
        .confirmation_policy(policy)
        .tool(
            ToolDefinition::new("pay_bill"),
            DummyToolHandler { calls: tool_calls.clone(), last_args: Mutex::new(None) },
        )
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    let r = Arc::new(runner);
    let r_clone = r.clone();
    tokio::spawn(async move {
        r_clone.run().await.unwrap();
    });

    // Send two separate function calls
    tx.send(Ok(ServerEvent::FunctionCallDone {
        event_id: "evt-1".to_string(),
        response_id: "resp-1".to_string(),
        item_id: "item-1".to_string(),
        output_index: 0,
        call_id: "call-1".to_string(),
        name: "pay_bill".to_string(),
        arguments: json!({"amount": 10.0}),
    }))
    .await
    .unwrap();

    tx.send(Ok(ServerEvent::FunctionCallDone {
        event_id: "evt-2".to_string(),
        response_id: "resp-1".to_string(),
        item_id: "item-1".to_string(),
        output_index: 1,
        call_id: "call-2".to_string(),
        name: "pay_bill".to_string(),
        arguments: json!({"amount": 20.0}),
    }))
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Both should be pending
    assert_eq!(tool_calls.load(Ordering::SeqCst), 0);

    let (conf_id_1, conf_id_2) = {
        let reqs = handler.confirmations_requested.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        (reqs[0].confirmation_id, reqs[1].confirmation_id)
    };

    // Resolve second one first
    r.resolve_confirmation(&conf_id_2, ToolConfirmationDecision::Confirmed { payload: None })
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(tool_calls.load(Ordering::SeqCst), 1);

    // Resolve first one
    r.resolve_confirmation(&conf_id_1, ToolConfirmationDecision::Confirmed { payload: None })
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(tool_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_validation_failure_creates_no_pending_confirmation() {
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let model = MockModel { session_rx: Mutex::new(Some(rx)) };
    let handler = Arc::new(RecordingEventHandler::default());
    let policy =
        StaticToolConfirmationPolicy { require_confirm: true, hint: Some("Confirm".to_string()) };

    let runner = RealtimeRunner::builder()
        .model(Arc::new(model))
        .event_handler_arc(handler.clone())
        .confirmation_policy(policy)
        // No tools registered
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    let r = Arc::new(runner);
    let r_clone = r.clone();
    tokio::spawn(async move {
        r_clone.run().await.unwrap();
    });

    // Send function call for unknown tool
    tx.send(Ok(ServerEvent::FunctionCallDone {
        event_id: "evt-1".to_string(),
        response_id: "resp-1".to_string(),
        item_id: "item-1".to_string(),
        output_index: 0,
        call_id: "call-1".to_string(),
        name: "unknown_tool".to_string(),
        arguments: json!({}),
    }))
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // No confirmations should have been requested since validation failed on unknown tool
    let reqs = handler.confirmations_requested.lock().unwrap();
    assert_eq!(reqs.len(), 0);
}

#[tokio::test]
async fn test_runner_close_fails_pending_confirmations() {
    let (tx, rx) = tokio::sync::mpsc::channel(10);
    let model = MockModel { session_rx: Mutex::new(Some(rx)) };
    let handler = Arc::new(RecordingEventHandler::default());
    let policy =
        StaticToolConfirmationPolicy { require_confirm: true, hint: Some("Confirm".to_string()) };

    let runner = RealtimeRunner::builder()
        .model(Arc::new(model))
        .event_handler_arc(handler.clone())
        .confirmation_policy(policy)
        .tool(
            ToolDefinition::new("pay_bill"),
            DummyToolHandler { calls: Arc::new(AtomicUsize::new(0)), last_args: Mutex::new(None) },
        )
        .build()
        .unwrap();

    runner.connect().await.unwrap();

    let r = Arc::new(runner);
    let r_clone = r.clone();
    tokio::spawn(async move {
        r_clone.run().await.unwrap();
    });

    tx.send(Ok(ServerEvent::FunctionCallDone {
        event_id: "evt-1".to_string(),
        response_id: "resp-1".to_string(),
        item_id: "item-1".to_string(),
        output_index: 0,
        call_id: "call-1".to_string(),
        name: "pay_bill".to_string(),
        arguments: json!({}),
    }))
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let conf_id = {
        let reqs = handler.confirmations_requested.lock().unwrap();
        reqs[0].confirmation_id
    };

    // Close runner
    r.close().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Resolving after close must fail explicitly
    let res = r
        .resolve_confirmation(&conf_id, ToolConfirmationDecision::Confirmed { payload: None })
        .await;
    assert!(res.is_err());
}
