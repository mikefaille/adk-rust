use async_trait::async_trait;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use adk_realtime::audio::AudioChunk;
use adk_realtime::error::{RealtimeError, Result};
use adk_realtime::events::{ClientEvent, ServerEvent, ToolResponse};
use adk_realtime::recovery::{
    FailureReport, RealtimeRecovery, RecoveredSession, RecoveryCause, RecoveryContext,
    RecoveryContinuity, RecoveryDisposition, RecoveryPolicy, RecoverySupervisor,
};
use adk_realtime::session::RealtimeSession;

struct MockSession {
    id: String,
    recovery: Option<Arc<dyn RealtimeRecovery>>,
}

#[async_trait]
impl RealtimeSession for MockSession {
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

    fn events(
        &self,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<ServerEvent>> + Send + '_>> {
        Box::pin(futures::stream::empty())
    }

    async fn close(&self) -> Result<()> {
        Ok(())
    }

    async fn mutate_context(
        &self,
        _config: adk_realtime::config::RealtimeConfig,
    ) -> Result<adk_realtime::session::ContextMutationOutcome> {
        Ok(adk_realtime::session::ContextMutationOutcome::Applied)
    }
}

// ── Test Case 1: Coalescing ───────────────────────────────────────────

struct CoalescingRecovery {
    recover_count: Arc<AtomicUsize>,
}

#[async_trait]
impl RealtimeRecovery for CoalescingRecovery {
    fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
        RecoveryDisposition::Recoverable
    }

    async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
        self.recover_count.fetch_add(1, Ordering::SeqCst);
        // Sleep slightly so that concurrent tasks wait on the mutex
        tokio::time::sleep(Duration::from_millis(50)).await;
        let session = Arc::new(MockSession { id: "gen-1-recovered".to_string(), recovery: None });
        Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
    }
}

#[tokio::test]
async fn test_concurrent_coalescing() {
    let recover_count = Arc::new(AtomicUsize::new(0));
    let mock_rec = Arc::new(CoalescingRecovery { recover_count: Arc::clone(&recover_count) });

    let initial_session = Arc::new(MockSession {
        id: "gen-0".to_string(),
        recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
    });

    let policy = RecoveryPolicy::default()
        .with_max_attempts(NonZeroU32::new(3).unwrap())
        .with_deadline(Duration::from_secs(5));

    let supervisor = Arc::new(RecoverySupervisor::new(
        policy,
        adk_realtime::config::RealtimeConfig::default(),
        initial_session,
    ));

    // Spawn 3 concurrent failure reports
    let mut join_handles = Vec::new();
    for _ in 0..3 {
        let sup_clone = Arc::clone(&supervisor);
        let handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_clone.report_failure(report).await
        });
        join_handles.push(handle);
    }

    let mut results = Vec::new();
    for handle in join_handles {
        results.push(handle.await.unwrap());
    }

    // Verify exactly one recovery episode ran
    assert_eq!(recover_count.load(Ordering::SeqCst), 1);

    // Verify all 3 tasks received the exact same recovered session
    for res in results {
        let session = res.unwrap();
        assert_eq!(session.session_id(), "gen-1-recovered");
    }
}

// ── Test Case 2: Delayed / Stale Failure ───────────────────────────────

struct StaleRecovery {
    recover_count: Arc<AtomicUsize>,
}

#[async_trait]
impl RealtimeRecovery for StaleRecovery {
    fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
        RecoveryDisposition::Recoverable
    }

    async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
        self.recover_count.fetch_add(1, Ordering::SeqCst);
        let session = Arc::new(MockSession { id: "gen-1".to_string(), recovery: None });
        Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
    }
}

#[tokio::test]
async fn test_delayed_stale_failure_report() {
    let recover_count = Arc::new(AtomicUsize::new(0));
    let mock_rec = Arc::new(StaleRecovery { recover_count: Arc::clone(&recover_count) });

    let initial_session = Arc::new(MockSession {
        id: "gen-0".to_string(),
        recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
    });

    let policy = RecoveryPolicy::default()
        .with_max_attempts(NonZeroU32::new(3).unwrap())
        .with_deadline(Duration::from_secs(5));

    let supervisor = RecoverySupervisor::new(
        policy,
        adk_realtime::config::RealtimeConfig::default(),
        initial_session,
    );

    // 1. Recover generation 0 to generation 1
    let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
    let first_res = supervisor.report_failure(report).await.unwrap();
    assert_eq!(first_res.session_id(), "gen-1");
    assert_eq!(recover_count.load(Ordering::SeqCst), 1);

    // 2. Deliver delayed stale failure report for generation 0
    let stale_report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
    let second_res = supervisor.report_failure(stale_report).await.unwrap();

    // Verify it returned gen-1 session immediately with zero additional provider attempts
    assert_eq!(second_res.session_id(), "gen-1");
    assert_eq!(recover_count.load(Ordering::SeqCst), 1);
}

// ── Test Case 3: Timeout Cancellation ──────────────────────────────────

struct TimeoutRecovery;

#[async_trait]
impl RealtimeRecovery for TimeoutRecovery {
    fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
        RecoveryDisposition::Recoverable
    }

    async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
        // Sleep longer than the absolute deadline of 50ms
        tokio::time::sleep(Duration::from_millis(200)).await;
        let session = Arc::new(MockSession { id: "gen-1-too-late".to_string(), recovery: None });
        Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
    }
}

#[tokio::test]
async fn test_genuine_timeout_cancellation() {
    let mock_rec = Arc::new(TimeoutRecovery);

    let initial_session = Arc::new(MockSession {
        id: "gen-0".to_string(),
        recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
    });

    let policy = RecoveryPolicy::default()
        .with_max_attempts(NonZeroU32::new(1).unwrap())
        .with_deadline(Duration::from_millis(50));

    let supervisor = RecoverySupervisor::new(
        policy,
        adk_realtime::config::RealtimeConfig::default(),
        initial_session,
    );

    let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

    let start = tokio::time::Instant::now();
    let res = supervisor.report_failure(report).await;
    let elapsed = start.elapsed();

    // The supervisor must abort with a Timeout error
    assert!(res.is_err());
    let err = match res {
        Err(e) => e,
        _ => unreachable!(),
    };
    match err {
        RealtimeError::Timeout(_) => {}
        other => panic!("expected RealtimeError::Timeout, got {:?}", other),
    }

    // Verify it aborted within the timeout budget
    assert!(elapsed >= Duration::from_millis(50));
    assert!(elapsed < Duration::from_millis(150));
}

// ── Test Case 4: Fatal Cause Classification ───────────────────────────

struct FatalCauseRecovery {
    recover_count: Arc<AtomicUsize>,
}

#[async_trait]
impl RealtimeRecovery for FatalCauseRecovery {
    fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
        RecoveryDisposition::Fatal
    }

    async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
        self.recover_count.fetch_add(1, Ordering::SeqCst);
        let session = Arc::new(MockSession { id: "should-not-reach".to_string(), recovery: None });
        Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
    }
}

#[tokio::test]
async fn test_fatal_cause_classification() {
    let recover_count = Arc::new(AtomicUsize::new(0));
    let mock_rec = Arc::new(FatalCauseRecovery { recover_count: Arc::clone(&recover_count) });

    let initial_session = Arc::new(MockSession {
        id: "gen-0".to_string(),
        recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
    });

    let policy = RecoveryPolicy::default()
        .with_max_attempts(NonZeroU32::new(3).unwrap())
        .with_deadline(Duration::from_secs(5));

    let supervisor = RecoverySupervisor::new(
        policy,
        adk_realtime::config::RealtimeConfig::default(),
        initial_session,
    );

    let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

    let res = supervisor.report_failure(report).await;
    assert!(res.is_err());
    assert_eq!(recover_count.load(Ordering::SeqCst), 0);
}

// ── Test Case 5: Fatal Attempt-Error Classification ───────────────────

struct FatalAttemptRecovery {
    recover_count: Arc<AtomicUsize>,
}

#[async_trait]
impl RealtimeRecovery for FatalAttemptRecovery {
    fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
        RecoveryDisposition::Recoverable
    }

    fn classify_attempt_error(&self, _error: &RealtimeError) -> RecoveryDisposition {
        RecoveryDisposition::Fatal
    }

    async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
        self.recover_count.fetch_add(1, Ordering::SeqCst);
        Err(RealtimeError::ConnectionError("fatal attempt error".to_string()))
    }
}

#[tokio::test]
async fn test_fatal_attempt_error_classification() {
    let recover_count = Arc::new(AtomicUsize::new(0));
    let mock_rec = Arc::new(FatalAttemptRecovery { recover_count: Arc::clone(&recover_count) });

    let initial_session = Arc::new(MockSession {
        id: "gen-0".to_string(),
        recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
    });

    let policy = RecoveryPolicy::default()
        .with_max_attempts(NonZeroU32::new(3).unwrap())
        .with_deadline(Duration::from_secs(5));

    let supervisor = RecoverySupervisor::new(
        policy,
        adk_realtime::config::RealtimeConfig::default(),
        initial_session,
    );

    let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

    let res = supervisor.report_failure(report).await;
    assert!(res.is_err());
    assert_eq!(recover_count.load(Ordering::SeqCst), 1);
}

// ── Test Case 6: Retryable Failure Deterministic Backoff ──────────────

struct DeterministicBackoffRecovery {
    recover_count: Arc<AtomicUsize>,
    attempt_times: Arc<parking_lot::Mutex<Vec<tokio::time::Instant>>>,
}

#[async_trait]
impl RealtimeRecovery for DeterministicBackoffRecovery {
    fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
        RecoveryDisposition::Recoverable
    }

    fn classify_attempt_error(&self, _error: &RealtimeError) -> RecoveryDisposition {
        RecoveryDisposition::Recoverable
    }

    async fn recover(&self, context: RecoveryContext<'_>) -> Result<RecoveredSession> {
        self.recover_count.fetch_add(1, Ordering::SeqCst);
        self.attempt_times.lock().push(tokio::time::Instant::now());

        if context.attempt().get() == 3 {
            let session = Arc::new(MockSession { id: "gen-1-finally".to_string(), recovery: None });
            Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
        } else {
            Err(RealtimeError::ConnectionError("try again".to_string()))
        }
    }
}

#[tokio::test(start_paused = true)]
async fn test_retryable_failure_deterministic_backoff() {
    let recover_count = Arc::new(AtomicUsize::new(0));
    let attempt_times = Arc::new(parking_lot::Mutex::new(Vec::new()));

    let mock_rec = Arc::new(DeterministicBackoffRecovery {
        recover_count: Arc::clone(&recover_count),
        attempt_times: Arc::clone(&attempt_times),
    });

    let initial_session = Arc::new(MockSession {
        id: "gen-0".to_string(),
        recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
    });

    let policy = RecoveryPolicy::default()
        .with_max_attempts(NonZeroU32::new(3).unwrap())
        .with_initial_delay(Duration::from_millis(50))
        .with_max_delay(Duration::from_millis(500))
        .with_deadline(Duration::from_secs(5));

    let supervisor = RecoverySupervisor::new(
        policy,
        adk_realtime::config::RealtimeConfig::default(),
        initial_session,
    );

    let start_instant = tokio::time::Instant::now();

    let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

    let res = supervisor.report_failure(report).await.unwrap();
    assert_eq!(res.session_id(), "gen-1-finally");

    // Check we made exactly 3 attempts
    assert_eq!(recover_count.load(Ordering::SeqCst), 3);

    // Verify timestamps of the attempts
    let times = attempt_times.lock().clone();
    assert_eq!(times.len(), 3);

    let diff1 = times[1].duration_since(times[0]);
    let diff2 = times[2].duration_since(times[1]);

    // Backoffs are:
    // After attempt 1: 50ms * 2^0 = 50ms
    // After attempt 2: 50ms * 2^1 = 100ms
    assert!(diff1 >= Duration::from_millis(50) && diff1 < Duration::from_millis(60));
    assert!(diff2 >= Duration::from_millis(100) && diff2 < Duration::from_millis(110));

    let total_elapsed = tokio::time::Instant::now().duration_since(start_instant);
    assert!(
        total_elapsed >= Duration::from_millis(150) && total_elapsed < Duration::from_millis(170)
    );
}
