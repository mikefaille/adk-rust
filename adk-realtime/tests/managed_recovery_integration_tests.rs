use adk_realtime::audio::{AudioChunk, AudioFormat};
use adk_realtime::config::{RealtimeConfig, SessionUpdateConfig};
use adk_realtime::error::{RealtimeError, Result};
use adk_realtime::events::{ClientEvent, ServerEvent, ToolResponse};
use adk_realtime::model::RealtimeModel;
use adk_realtime::recovery::{
    DeliveryCertainty, RealtimeRecovery, RecoveredSession, RecoveryCause, RecoveryContinuity,
    RecoveryDisposition,
};
use adk_realtime::runner::RealtimeRunner;
use adk_realtime::session::{BoxedSession, ContextMutationOutcome, RealtimeSession};
use async_trait::async_trait;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[derive(Default)]
struct TestCounters {
    raw_sends: AtomicUsize,
    close_calls: AtomicUsize,
}

#[derive(Clone)]
struct MockRecoverySession {
    id: String,
    counters: Arc<TestCounters>,
    recovery: Option<Arc<dyn RealtimeRecovery>>,
    close_hangs: bool,
    send_fails: bool,
    mutate_fails: bool,
    close_tx: Option<Arc<parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>>,
}

impl MockRecoverySession {
    fn new(id: &str, counters: Arc<TestCounters>) -> Self {
        Self {
            id: id.to_string(),
            counters,
            recovery: None,
            close_hangs: false,
            send_fails: false,
            mutate_fails: false,
            close_tx: None,
        }
    }

    fn new_with_close_signal(
        id: &str,
        counters: Arc<TestCounters>,
        close_tx: Arc<parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    ) -> Self {
        Self {
            id: id.to_string(),
            counters,
            recovery: None,
            close_hangs: false,
            send_fails: false,
            mutate_fails: false,
            close_tx: Some(close_tx),
        }
    }

    fn with_recovery(
        id: &str,
        counters: Arc<TestCounters>,
        recovery: Arc<dyn RealtimeRecovery>,
    ) -> Self {
        Self {
            id: id.to_string(),
            counters,
            recovery: Some(recovery),
            close_hangs: false,
            send_fails: false,
            mutate_fails: false,
            close_tx: None,
        }
    }
}

#[async_trait]
impl RealtimeSession for MockRecoverySession {
    fn session_id(&self) -> &str {
        &self.id
    }

    fn is_connected(&self) -> bool {
        true
    }

    fn recovery(&self) -> Option<&dyn RealtimeRecovery> {
        self.recovery.as_ref().map(|r| r.as_ref())
    }

    async fn send_audio(&self, _audio: &AudioChunk) -> Result<()> {
        self.counters.raw_sends.fetch_add(1, Ordering::SeqCst);
        if self.send_fails {
            return Err(RealtimeError::connection("simulated send_audio write failure"));
        }
        Ok(())
    }

    async fn send_audio_base64(&self, _audio: &str) -> Result<()> {
        self.counters.raw_sends.fetch_add(1, Ordering::SeqCst);
        if self.send_fails {
            return Err(RealtimeError::connection("simulated send_audio_base64 write failure"));
        }
        Ok(())
    }

    async fn send_text(&self, _text: &str) -> Result<()> {
        self.counters.raw_sends.fetch_add(1, Ordering::SeqCst);
        if self.send_fails {
            return Err(RealtimeError::connection("simulated send_text write failure"));
        }
        Ok(())
    }

    async fn send_tool_response(&self, _response: ToolResponse) -> Result<()> {
        self.counters.raw_sends.fetch_add(1, Ordering::SeqCst);
        if self.send_fails {
            return Err(RealtimeError::connection("simulated send_tool_response write failure"));
        }
        Ok(())
    }

    async fn commit_audio(&self) -> Result<()> {
        self.counters.raw_sends.fetch_add(1, Ordering::SeqCst);
        if self.send_fails {
            return Err(RealtimeError::connection("simulated commit_audio write failure"));
        }
        Ok(())
    }

    async fn clear_audio(&self) -> Result<()> {
        Ok(())
    }

    async fn create_response(&self) -> Result<()> {
        self.counters.raw_sends.fetch_add(1, Ordering::SeqCst);
        if self.send_fails {
            return Err(RealtimeError::connection("simulated create_response write failure"));
        }
        Ok(())
    }

    async fn interrupt(&self) -> Result<()> {
        self.counters.raw_sends.fetch_add(1, Ordering::SeqCst);
        if self.send_fails {
            return Err(RealtimeError::connection("simulated interrupt write failure"));
        }
        Ok(())
    }

    async fn send_event(&self, _event: ClientEvent) -> Result<()> {
        self.counters.raw_sends.fetch_add(1, Ordering::SeqCst);
        if self.send_fails {
            return Err(RealtimeError::connection("simulated send_event write failure"));
        }
        Ok(())
    }

    async fn next_event(&self) -> Option<Result<ServerEvent>> {
        std::future::pending().await
    }

    fn events(&self) -> Pin<Box<dyn futures::Stream<Item = Result<ServerEvent>> + Send + '_>> {
        Box::pin(futures::stream::empty())
    }

    async fn close(&self) -> Result<()> {
        self.counters.close_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(ref slot) = self.close_tx
            && let Some(tx) = slot.lock().take()
        {
            let _ = tx.send(());
        }
        if self.close_hangs {
            std::future::pending::<()>().await;
        }
        Ok(())
    }

    async fn mutate_context(&self, config: RealtimeConfig) -> Result<ContextMutationOutcome> {
        self.counters.raw_sends.fetch_add(1, Ordering::SeqCst);
        if self.mutate_fails {
            return Err(RealtimeError::connection("simulated mutate_context write failure"));
        }
        Ok(ContextMutationOutcome::RequiresResumption(Box::new(config)))
    }
}

struct DummyModel;

#[async_trait]
impl RealtimeModel for DummyModel {
    fn provider(&self) -> &str {
        "dummy"
    }
    fn model_id(&self) -> &str {
        "dummy"
    }
    fn supported_input_formats(&self) -> Vec<AudioFormat> {
        vec![]
    }
    fn supported_output_formats(&self) -> Vec<AudioFormat> {
        vec![]
    }
    fn available_voices(&self) -> Vec<&str> {
        vec![]
    }
    async fn connect(&self, _config: RealtimeConfig) -> Result<BoxedSession> {
        let counters = Arc::new(TestCounters::default());
        Ok(Box::new(MockRecoverySession::new("dummy-connected", counters)))
    }
}

#[tokio::test]
async fn test_single_active_session_authority_and_no_reset() {
    let runner = RealtimeRunner::builder().model(Arc::new(DummyModel)).build().unwrap();
    let counters = Arc::new(TestCounters::default());
    let session = Arc::new(MockRecoverySession::new("gen-0", counters.clone()));

    // Initial installation succeeds for generation 0
    let gen0_id = runner.set_initial_session(session.clone()).await.unwrap();
    assert_eq!(gen0_id, 0);

    // Re-setting initial session when generation 0 exists MUST fail and NOT reset live generation to 0
    let res = runner.set_initial_session(session).await;
    assert!(res.is_err(), "set_initial_session must reject when active generation exists");

    assert_eq!(runner.session_id().await.as_deref(), Some("gen-0"));
}

#[tokio::test]
async fn test_write_delivery_certainty_not_attempted_vs_indeterminate() {
    let runner = RealtimeRunner::builder().model(Arc::new(DummyModel)).build().unwrap();
    let counters = Arc::new(TestCounters::default());

    // 1. Generic non-write error has NO delivery certainty
    let generic_err = RealtimeError::config("generic error");
    assert_eq!(generic_err.delivery_certainty(), None);

    // 2. Managed admission failure before raw session invocation -> WriteFailed(NotAttempted) with 0 raw calls
    let err = runner.send_text("hello").await.unwrap_err();
    assert_eq!(err.delivery_certainty(), Some(DeliveryCertainty::NotAttempted));
    assert_eq!(counters.raw_sends.load(Ordering::SeqCst), 0);

    // 3. Post-invocation write failure -> WriteFailed(Indeterminate) with 1 raw call
    let mut failing_session = MockRecoverySession::new("gen-0-failing", counters.clone());
    failing_session.send_fails = true;
    let _ = runner.set_initial_session(Arc::new(failing_session)).await.unwrap();

    let err2 = runner.send_text("hello").await.unwrap_err();
    assert_eq!(err2.delivery_certainty(), Some(DeliveryCertainty::Indeterminate));
    assert_eq!(counters.raw_sends.load(Ordering::SeqCst), 1);
}

struct RecoveringProvider {
    attempts: Arc<AtomicUsize>,
    recovered_session: Arc<dyn RealtimeSession>,
    continuity: RecoveryContinuity,
}

#[async_trait]
impl RealtimeRecovery for RecoveringProvider {
    fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
        RecoveryDisposition::Recoverable
    }

    async fn recover(
        &self,
        _context: adk_realtime::recovery::RecoveryContext<'_>,
    ) -> Result<RecoveredSession> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Ok(RecoveredSession::new(self.recovered_session.clone(), self.continuity))
    }
}

#[tokio::test]
async fn test_recovery_admission_rejects_concurrent_writes_as_not_attempted() {
    let counters = Arc::new(TestCounters::default());
    let gen1_counters = Arc::new(TestCounters::default());

    let (active_tx, active_rx) = tokio::sync::oneshot::channel();
    let unblock_notify = Arc::new(tokio::sync::Notify::new());

    struct ActiveRecoveryProvider {
        recovered_session: Arc<dyn RealtimeSession>,
        active_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        unblock_notify: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl RealtimeRecovery for ActiveRecoveryProvider {
        fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
            RecoveryDisposition::Recoverable
        }

        async fn recover(
            &self,
            _context: adk_realtime::recovery::RecoveryContext<'_>,
        ) -> Result<RecoveredSession> {
            if let Some(tx) = self.active_tx.lock().take() {
                let _ = tx.send(());
            }
            self.unblock_notify.notified().await;
            Ok(RecoveredSession::new(
                self.recovered_session.clone(),
                RecoveryContinuity::Reconnected,
            ))
        }
    }

    let gen1_session = Arc::new(MockRecoverySession::new("gen-1-recovered", gen1_counters.clone()));
    let recovery_provider = Arc::new(ActiveRecoveryProvider {
        recovered_session: gen1_session,
        active_tx: parking_lot::Mutex::new(Some(active_tx)),
        unblock_notify: unblock_notify.clone(),
    });

    let mut gen0_session =
        MockRecoverySession::with_recovery("gen-0", counters.clone(), recovery_provider);
    gen0_session.send_fails = true;

    let runner = Arc::new(RealtimeRunner::builder().model(Arc::new(DummyModel)).build().unwrap());
    let _ = runner.set_initial_session(Arc::new(gen0_session)).await.unwrap();

    let runner_clone = runner.clone();
    let write1_task = tokio::spawn(async move { runner_clone.send_text("write 1 on gen 0").await });

    active_rx.await.expect("recovery episode must become active");

    let write2_err = runner.send_text("write 2 during recovery").await.unwrap_err();
    assert_eq!(write2_err.delivery_certainty(), Some(DeliveryCertainty::NotAttempted));

    assert_eq!(counters.raw_sends.load(Ordering::SeqCst), 1);

    unblock_notify.notify_one();
    let _ = write1_task.await;

    assert_eq!(runner.session_id().await.as_deref(), Some("gen-1-recovered"));

    runner.send_text("write 3 on gen 1").await.unwrap();
    assert_eq!(gen1_counters.raw_sends.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_raw_write_failure_triggers_supervisor_recovery() {
    let runner = RealtimeRunner::builder().model(Arc::new(DummyModel)).build().unwrap();
    let counters = Arc::new(TestCounters::default());

    let rec_attempts = Arc::new(AtomicUsize::new(0));
    let replacement_session =
        Arc::new(MockRecoverySession::new("gen-1-replacement", counters.clone()));
    let recovery_provider = Arc::new(RecoveringProvider {
        attempts: rec_attempts.clone(),
        recovered_session: replacement_session,
        continuity: RecoveryContinuity::Reconnected,
    });

    let mut initial_session =
        MockRecoverySession::with_recovery("gen-0-initial", counters.clone(), recovery_provider);
    initial_session.send_fails = true;

    let _ = runner.set_initial_session(Arc::new(initial_session)).await.unwrap();

    let err = runner.send_text("hello").await.unwrap_err();
    assert_eq!(err.delivery_certainty(), Some(DeliveryCertainty::Indeterminate));
    assert_eq!(rec_attempts.load(Ordering::SeqCst), 1);

    assert_eq!(runner.session_id().await.as_deref(), Some("gen-1-replacement"));
}

#[tokio::test]
async fn test_mutate_context_failure_triggers_write_failed_recovery() {
    let runner = RealtimeRunner::builder().model(Arc::new(DummyModel)).build().unwrap();
    let counters = Arc::new(TestCounters::default());

    let rec_attempts = Arc::new(AtomicUsize::new(0));
    let replacement_session =
        Arc::new(MockRecoverySession::new("gen-1-context-resumed", counters.clone()));
    let recovery_provider = Arc::new(RecoveringProvider {
        attempts: rec_attempts.clone(),
        recovered_session: replacement_session,
        continuity: RecoveryContinuity::Resumed,
    });

    let mut initial_session = MockRecoverySession::with_recovery(
        "gen-0-mutate-failing",
        counters.clone(),
        recovery_provider,
    );
    initial_session.mutate_fails = true;

    let _ = runner.set_initial_session(Arc::new(initial_session)).await.unwrap();

    let update = SessionUpdateConfig(RealtimeConfig::default().with_instruction("new instruction"));
    let err = runner.update_session(update).await.unwrap_err();

    assert_eq!(err.delivery_certainty(), Some(DeliveryCertainty::Indeterminate));
    assert_eq!(rec_attempts.load(Ordering::SeqCst), 1);

    assert_eq!(runner.session_id().await.as_deref(), Some("gen-1-context-resumed"));
}

#[derive(Clone)]
struct HangingCloseSession {
    counters: Arc<TestCounters>,
    events: Arc<parking_lot::Mutex<Vec<ServerEvent>>>,
}

#[async_trait]
impl RealtimeSession for HangingCloseSession {
    fn session_id(&self) -> &str {
        "same-session-id"
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
        let ev = self.events.lock().pop();
        if let Some(ev) = ev { Some(Ok(ev)) } else { std::future::pending().await }
    }

    fn events(&self) -> Pin<Box<dyn futures::Stream<Item = Result<ServerEvent>> + Send + '_>> {
        Box::pin(futures::stream::empty())
    }

    async fn close(&self) -> Result<()> {
        self.counters.close_calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending::<()>().await;
        Ok(())
    }

    async fn mutate_context(&self, config: RealtimeConfig) -> Result<ContextMutationOutcome> {
        Ok(ContextMutationOutcome::RequiresResumption(Box::new(config)))
    }
}

#[tokio::test]
async fn test_generation_watcher_wakeup_even_if_close_hangs_and_same_session_id() {
    let counters = Arc::new(TestCounters::default());

    let gen1_session = Arc::new(HangingCloseSession {
        counters: counters.clone(),
        events: Arc::new(parking_lot::Mutex::new(vec![ServerEvent::TextDelta {
            event_id: "evt".into(),
            response_id: "resp".into(),
            item_id: "item".into(),
            output_index: 0,
            content_index: 0,
            delta: "from-gen-1".into(),
        }])),
    });

    let update = SessionUpdateConfig(RealtimeConfig::default().with_instruction("new instruction"));

    struct CustomModel(Arc<HangingCloseSession>);
    #[async_trait]
    impl RealtimeModel for CustomModel {
        fn provider(&self) -> &str {
            "c"
        }
        fn model_id(&self) -> &str {
            "c"
        }
        fn supported_input_formats(&self) -> Vec<AudioFormat> {
            vec![]
        }
        fn supported_output_formats(&self) -> Vec<AudioFormat> {
            vec![]
        }
        fn available_voices(&self) -> Vec<&str> {
            vec![]
        }
        async fn connect(&self, _config: RealtimeConfig) -> Result<BoxedSession> {
            Ok(Box::new((*self.0).clone()) as BoxedSession)
        }
    }

    let custom_runner =
        RealtimeRunner::builder().model(Arc::new(CustomModel(gen1_session))).build().unwrap();
    let _ = custom_runner
        .set_initial_session(Arc::new(HangingCloseSession {
            counters: counters.clone(),
            events: Arc::new(parking_lot::Mutex::new(vec![])),
        }))
        .await
        .unwrap();

    let custom_runner = Arc::new(custom_runner);
    let custom_runner_clone = custom_runner.clone();

    let pull_task = tokio::spawn(async move { custom_runner_clone.next_event().await });

    tokio::time::sleep(Duration::from_millis(50)).await;

    custom_runner.update_session(update).await.unwrap();

    let pulled_event = tokio::time::timeout(Duration::from_secs(2), pull_task)
        .await
        .expect("next_event must wake immediately on generation change without waiting for hanging close")
        .unwrap()
        .unwrap()
        .unwrap();

    match pulled_event {
        ServerEvent::TextDelta { delta, .. } => assert_eq!(delta, "from-gen-1"),
        other => panic!("expected TextDelta from gen 1, got {:?}", other),
    }
}

#[tokio::test]
async fn test_bridge_message_write_failure_returns_indeterminate_write_failed() {
    let counters = Arc::new(TestCounters::default());

    let mut resumed_session = MockRecoverySession::new("gen-1-resumed", counters.clone());
    resumed_session.send_fails = true;
    let resumed_session = Arc::new(resumed_session);

    struct ResumptionModel(Arc<MockRecoverySession>);
    #[async_trait]
    impl RealtimeModel for ResumptionModel {
        fn provider(&self) -> &str {
            "res"
        }
        fn model_id(&self) -> &str {
            "res"
        }
        fn supported_input_formats(&self) -> Vec<AudioFormat> {
            vec![]
        }
        fn supported_output_formats(&self) -> Vec<AudioFormat> {
            vec![]
        }
        fn available_voices(&self) -> Vec<&str> {
            vec![]
        }
        async fn connect(&self, _config: RealtimeConfig) -> Result<BoxedSession> {
            Ok(Box::new((*self.0).clone()) as BoxedSession)
        }
    }

    let runner = RealtimeRunner::builder()
        .model(Arc::new(ResumptionModel(resumed_session)))
        .build()
        .unwrap();
    let initial_session = Arc::new(MockRecoverySession::new("gen-0", counters.clone()));
    let _ = runner.set_initial_session(initial_session).await.unwrap();

    let update = SessionUpdateConfig(RealtimeConfig::default().with_instruction("new instruction"));
    let err =
        runner.update_session_with_bridge(update, Some("bridge message".into())).await.unwrap_err();

    assert_eq!(err.delivery_certainty(), Some(DeliveryCertainty::Indeterminate));
}

#[tokio::test]
async fn test_cancellation_after_candidate_ready_cleans_unpublished_candidate() {
    let counters = Arc::new(TestCounters::default());
    let candidate_counters = Arc::new(TestCounters::default());
    let (candidate_close_tx, candidate_close_rx) = tokio::sync::oneshot::channel();
    let candidate_close_tx_slot = Arc::new(parking_lot::Mutex::new(Some(candidate_close_tx)));

    let candidate_session = Arc::new(MockRecoverySession::new_with_close_signal(
        "candidate",
        candidate_counters.clone(),
        candidate_close_tx_slot,
    ));

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let pause_notify = Arc::new(tokio::sync::Notify::new());

    struct SynchronizedCandidateRecovery {
        candidate: Arc<MockRecoverySession>,
        ready_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        done_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        pause_notify: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl RealtimeRecovery for SynchronizedCandidateRecovery {
        fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
            RecoveryDisposition::Recoverable
        }

        async fn recover(
            &self,
            _context: adk_realtime::recovery::RecoveryContext<'_>,
        ) -> Result<RecoveredSession> {
            if let Some(tx) = self.ready_tx.lock().take() {
                let _ = tx.send(());
            }
            self.pause_notify.notified().await;
            if let Some(tx) = self.done_tx.lock().take() {
                let _ = tx.send(());
            }
            Ok(RecoveredSession::new(self.candidate.clone(), RecoveryContinuity::Resumed))
        }
    }

    let mock_rec = Arc::new(SynchronizedCandidateRecovery {
        candidate: candidate_session,
        ready_tx: parking_lot::Mutex::new(Some(ready_tx)),
        done_tx: parking_lot::Mutex::new(Some(done_tx)),
        pause_notify: pause_notify.clone(),
    });

    let mut initial_session = MockRecoverySession::with_recovery("gen-0", counters, mock_rec);
    initial_session.send_fails = true;

    let runner = Arc::new(RealtimeRunner::builder().model(Arc::new(DummyModel)).build().unwrap());
    let _ = runner.set_initial_session(Arc::new(initial_session)).await.unwrap();

    let runner_clone = runner.clone();
    let recovery_task = tokio::spawn(async move {
        let _ = runner_clone.send_text("trigger recovery").await;
    });

    ready_rx.await.expect("recover() should reach candidate-ready");

    let runner_for_lock = runner.clone();
    let release_lock = Arc::new(tokio::sync::Notify::new());
    let release_lock_clone = release_lock.clone();

    let lock_task = tokio::spawn(async move {
        runner_for_lock.prepend_instruction_context("blocking lock").await;
        release_lock_clone.notified().await;
    });

    pause_notify.notify_one();
    done_rx.await.expect("recover() must complete returning candidate");

    recovery_task.abort();
    let _ = recovery_task.await;

    release_lock.notify_one();
    let _ = lock_task.await;

    candidate_close_rx.await.expect("CandidateSession::close() must complete on cancellation");

    assert_eq!(runner.session_id().await.as_deref(), Some("gen-0"));
    assert_eq!(
        candidate_counters.close_calls.load(Ordering::SeqCst),
        1,
        "Unpublished candidate session must be closed on cancellation"
    );
}

#[tokio::test]
async fn test_recovered_tool_failure_under_cancellation_runner_keeps_runner_alive() {
    let counters = Arc::new(TestCounters::default());

    let rec_attempts = Arc::new(AtomicUsize::new(0));
    let gen1_session = Arc::new(MockRecoverySession::new("gen-1-recovered", counters.clone()));
    let recovery_provider = Arc::new(RecoveringProvider {
        attempts: rec_attempts.clone(),
        recovered_session: gen1_session,
        continuity: RecoveryContinuity::Reconnected,
    });

    let mut gen0_session =
        MockRecoverySession::with_recovery("gen-0", counters.clone(), recovery_provider);
    gen0_session.send_fails = true;

    let runner = Arc::new(RealtimeRunner::builder().model(Arc::new(DummyModel)).build().unwrap());
    let _ = runner.set_initial_session(Arc::new(gen0_session)).await.unwrap();

    let cancel_token = tokio_util::sync::CancellationToken::new();
    let cancel_token_clone = cancel_token.clone();

    let runner_clone = runner.clone();
    let run_task =
        tokio::spawn(async move { runner_clone.run_with_cancellation(cancel_token_clone).await });

    let err = runner.send_text("trigger tool write failure").await.unwrap_err();
    assert_eq!(err.delivery_certainty(), Some(DeliveryCertainty::Indeterminate));

    assert_eq!(runner.session_id().await.as_deref(), Some("gen-1-recovered"));

    tokio::time::sleep(Duration::from_millis(20)).await;

    assert!(
        !run_task.is_finished(),
        "run_with_cancellation must stay alive after recovered tool write failure"
    );

    cancel_token.cancel();
    let res = run_task.await.unwrap();
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_explicit_close_leaves_no_admittable_session() {
    let runner = RealtimeRunner::builder().model(Arc::new(DummyModel)).build().unwrap();
    let counters = Arc::new(TestCounters::default());
    let session = Arc::new(MockRecoverySession::new("gen-0", counters.clone()));

    let _ = runner.set_initial_session(session).await.unwrap();
    assert!(runner.is_connected().await);

    runner.close().await.unwrap();

    assert!(!runner.is_connected().await);

    let err = runner.send_text("after close").await.unwrap_err();
    assert_eq!(err.delivery_certainty(), Some(DeliveryCertainty::NotAttempted));
    assert_eq!(counters.raw_sends.load(Ordering::SeqCst), 0);
}
