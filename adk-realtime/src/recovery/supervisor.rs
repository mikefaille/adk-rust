use crate::error::{RealtimeError, Result};
use crate::session::RealtimeSession;
use crate::recovery::{
    RecoveryCause, RecoveryDisposition, RecoveryContext, RecoveryPolicy,
};
use std::num::NonZeroU32;
use std::sync::Arc;

#[allow(dead_code)]
pub(crate) struct SessionGeneration {
    pub(crate) id: u64,
    pub(crate) session: Arc<dyn RealtimeSession>,
}

#[allow(dead_code)]
pub(crate) struct FailureReport {
    pub(crate) generation: u64,
    pub(crate) cause: RecoveryCause,
}

#[allow(dead_code)]
struct SupervisorState {
    generation: SessionGeneration,
    exhausted_generation: Option<u64>,
}

/// The private recovery supervisor.
#[allow(dead_code)]
pub(crate) struct RecoverySupervisor {
    policy: RecoveryPolicy,
    config: crate::config::RealtimeConfig,
    state: tokio::sync::RwLock<SupervisorState>,
    recovery_lock: tokio::sync::Mutex<()>,
}

#[allow(dead_code)]
impl RecoverySupervisor {
    /// Create a new recovery supervisor.
    pub(crate) fn new(
        policy: RecoveryPolicy,
        config: crate::config::RealtimeConfig,
        initial_session: Arc<dyn RealtimeSession>,
    ) -> Self {
        Self {
            policy,
            config,
            state: tokio::sync::RwLock::new(SupervisorState {
                generation: SessionGeneration { id: 0, session: initial_session },
                exhausted_generation: None,
            }),
            recovery_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Report a failure in the active session.
    #[allow(clippy::collapsible_if)]
    pub(crate) async fn report_failure(&self, report: FailureReport) -> Result<Arc<dyn RealtimeSession>> {
        let start_time = tokio::time::Instant::now();
        let deadline_duration = self.policy.deadline();
        let deadline_instant = start_time.checked_add(deadline_duration).unwrap_or(start_time);

        // 1. Check current state and validation
        {
            let state_guard = self.state.read().await;

            // Rule A: report < current  -> stale/coalesced, no recovery, return active
            if report.generation < state_guard.generation.id {
                tracing::info!(
                    report_gen = report.generation,
                    current_gen = state_guard.generation.id,
                    "stale/coalesced failure report ignored"
                );
                return Ok(state_guard.generation.session.clone());
            }

            // Rule B: report > current  -> invalid future generation, zero attempts, reject
            if report.generation > state_guard.generation.id {
                let err = RealtimeError::provider(format!(
                    "invalid future generation report: reported {}, current is {}",
                    report.generation, state_guard.generation.id
                ));
                tracing::warn!(
                    report_gen = report.generation,
                    current_gen = state_guard.generation.id,
                    err = %err,
                    "future generation report rejected"
                );
                return Err(err);
            }

            // Rule C: terminal duplicates of exhausted generation -> AlreadyHandled outcome
            if let Some(exhausted) = state_guard.exhausted_generation {
                if report.generation == exhausted {
                    tracing::info!(
                        generation = report.generation,
                        "report for already exhausted generation; returning active session"
                    );
                    return Ok(state_guard.generation.session.clone());
                }
            }
        }

        // 2. Lock wait capped by remaining deadline
        let remaining = deadline_instant.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let err = RealtimeError::Timeout("deadline expired before lock acquisition".to_string());
            tracing::warn!(generation = report.generation, err = %err, "recovery deadline expired");
            return Err(err);
        }

        let lock_guard = match tokio::time::timeout(remaining, self.recovery_lock.lock()).await {
            Ok(guard) => guard,
            Err(_) => {
                let err = RealtimeError::Timeout("deadline expired waiting for recovery lock".to_string());
                tracing::warn!(generation = report.generation, err = %err, "recovery deadline expired");
                return Err(err);
            }
        };

        // 3. Double-check state now that we hold the lock
        {
            let state_guard = self.state.read().await;

            if report.generation < state_guard.generation.id {
                tracing::info!(
                    report_gen = report.generation,
                    current_gen = state_guard.generation.id,
                    "coalesced failure report ignored after lock acquisition"
                );
                return Ok(state_guard.generation.session.clone());
            }

            if report.generation > state_guard.generation.id {
                return Err(RealtimeError::provider("invalid future generation report after lock acquisition"));
            }

            if let Some(exhausted) = state_guard.exhausted_generation {
                if report.generation == exhausted {
                    tracing::info!(
                        generation = report.generation,
                        "report for already exhausted generation after lock acquisition"
                    );
                    return Ok(state_guard.generation.session.clone());
                }
            }
        }

        // 4. Determine recovery implementation
        let session_to_recover = {
            let state_guard = self.state.read().await;
            state_guard.generation.session.clone()
        };

        let recovery_impl = match session_to_recover.recovery() {
            Some(r) => r,
            None => {
                let err = RealtimeError::provider("active session does not support recovery");
                tracing::error!(generation = report.generation, err = %err, "recovery not supported");
                // Mark as terminally exhausted so subsequent reports are coalesced
                let mut state_guard = self.state.write().await;
                state_guard.exhausted_generation = Some(report.generation);
                return Err(err);
            }
        };

        // 5. Pre-flight cause classification
        if recovery_impl.classify(&report.cause) == RecoveryDisposition::Fatal {
            let err = RealtimeError::provider("fatal recovery cause detected; performing zero provider attempts");
            tracing::warn!(generation = report.generation, err = %err, "fatal cause classification");
            // Mark as terminally exhausted so subsequent reports are coalesced
            let mut state_guard = self.state.write().await;
            state_guard.exhausted_generation = Some(report.generation);
            return Err(err);
        }

        // 6. Recovery attempt loop
        tracing::info!(generation = report.generation, "recovery episode started");
        let mut final_error: Option<RealtimeError> = None;
        let max_attempts = self.policy.max_attempts().get();

        for attempt_idx in 1..=max_attempts {
            let now = tokio::time::Instant::now();
            if now >= deadline_instant {
                let err = RealtimeError::Timeout("recovery deadline expired before attempt".to_string());
                tracing::warn!(generation = report.generation, attempt = attempt_idx, err = %err, "recovery deadline expired");
                final_error = Some(err);
                break;
            }

            let attempt_remaining = deadline_instant.saturating_duration_since(now);
            let attempt_nz = NonZeroU32::new(attempt_idx).unwrap();
            let remaining_dur = deadline_instant.saturating_duration_since(tokio::time::Instant::now());
            let context_deadline = std::time::Instant::now().checked_add(remaining_dur).unwrap_or_else(std::time::Instant::now);
            let context = RecoveryContext::new(
                attempt_nz,
                &report.cause,
                &self.config,
                context_deadline,
            );

            tracing::info!(
                generation = report.generation,
                attempt = attempt_idx,
                "provider recovery attempt initiated"
            );

            let recover_fut = recovery_impl.recover(context);
            let attempt_res = match tokio::time::timeout(attempt_remaining, recover_fut).await {
                Ok(res) => res,
                Err(_) => {
                    let err = RealtimeError::Timeout("provider recovery attempt timed out".to_string());
                    tracing::warn!(generation = report.generation, attempt = attempt_idx, err = %err, "recovery attempt timed out");
                    final_error = Some(err);
                    break;
                }
            };

            match attempt_res {
                Ok(recovered) => {
                    tracing::info!(
                        generation = report.generation,
                        attempt = attempt_idx,
                        continuity = ?recovered.continuity(),
                        "successful candidate published"
                    );

                    let new_session = recovered.session();

                    let mut state_guard = self.state.write().await;
                    // Atomically derive next generation ID from write-guarded active state
                    let next_gen = state_guard.generation.id.saturating_add(1);
                    state_guard.generation = SessionGeneration {
                        id: next_gen,
                        session: new_session.clone(),
                    };

                    return Ok(new_session);
                }
                Err(err) => {
                    tracing::error!(
                        generation = report.generation,
                        attempt = attempt_idx,
                        err = %err,
                        "recovery attempt failed"
                    );

                    let disposition = recovery_impl.classify_attempt_error(&err);
                    final_error = Some(err);

                    if disposition == RecoveryDisposition::Fatal {
                        tracing::warn!(
                            generation = report.generation,
                            attempt = attempt_idx,
                            "fatal attempt error classification; no retry"
                        );
                        break;
                    }

                    if attempt_idx < max_attempts {
                        // Apply deterministic backoff
                        let factor = 2u32.checked_pow(attempt_idx - 1).unwrap_or(u32::MAX);
                        let mut backoff = self.policy.initial_delay().saturating_mul(factor);
                        backoff = backoff.min(self.policy.max_delay());

                        let now_after_attempt = tokio::time::Instant::now();
                        let remaining_after_attempt = deadline_instant.saturating_duration_since(now_after_attempt);
                        backoff = backoff.min(remaining_after_attempt);

                        if backoff.is_zero() {
                            let timeout_err = RealtimeError::Timeout("recovery deadline expired during backoff calculation".to_string());
                            tracing::warn!(generation = report.generation, err = %timeout_err, "recovery deadline expired");
                            final_error = Some(timeout_err);
                            break;
                        }

                        tracing::info!(
                            generation = report.generation,
                            attempt = attempt_idx,
                            delay_ms = backoff.as_millis(),
                            "backing off before next attempt"
                        );
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }

        // 7. Exhaustion / final failure
        let final_err = if let Some(err) = final_error {
            match err {
                RealtimeError::Timeout(_) => err,
                other => RealtimeError::ProviderError(format!(
                    "recovery exhausted after {} attempts; last error: {}",
                    max_attempts, other
                )),
            }
        } else {
            RealtimeError::ProviderError(format!(
                "recovery exhausted after {} attempts",
                max_attempts
            ))
        };

        tracing::error!(
            generation = report.generation,
            err = %final_err,
            "recovery exhausted or fatal"
        );

        // Mark as terminally exhausted so subsequent reports are coalesced as normal outcomes
        let mut state_guard = self.state.write().await;
        state_guard.exhausted_generation = Some(report.generation);

        drop(lock_guard); // Release lock explicitly
        Err(final_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioChunk;
    use crate::recovery::{RecoveryContinuity, RecoveredSession, RealtimeRecovery};
    use crate::events::{ClientEvent, ServerEvent, ToolResponse};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

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
            _config: crate::config::RealtimeConfig,
        ) -> Result<crate::session::ContextMutationOutcome> {
            Ok(crate::session::ContextMutationOutcome::Applied)
        }
    }

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
            tokio::time::sleep(Duration::from_millis(50)).await;
            let session = Arc::new(MockSession {
                id: "gen-1-recovered".to_string(),
                recovery: None,
            });
            Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
        }
    }

    #[tokio::test]
    async fn test_concurrent_coalescing() {
        let recover_count = Arc::new(AtomicUsize::new(0));
        let mock_rec = Arc::new(CoalescingRecovery {
            recover_count: Arc::clone(&recover_count),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_deadline(Duration::from_secs(5));

        let supervisor = RecoverySupervisor::new(
            policy,
            crate::config::RealtimeConfig::default(),
            initial_session,
        );

        // Spawn 3 concurrent failure reports
        let mut join_handles = Vec::new();
        let sup_arc = Arc::new(supervisor);
        for _ in 0..3 {
            let sup_clone = Arc::clone(&sup_arc);
            let handle = tokio::spawn(async move {
                let report = FailureReport {
                    generation: 0,
                    cause: RecoveryCause::UnexpectedEof,
                };
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

    #[tokio::test]
    async fn test_future_generation_rejection() {
        let recover_count = Arc::new(AtomicUsize::new(0));
        let mock_rec = Arc::new(CoalescingRecovery {
            recover_count: Arc::clone(&recover_count),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default();
        let supervisor = RecoverySupervisor::new(
            policy,
            crate::config::RealtimeConfig::default(),
            initial_session,
        );

        // Report future generation 1 while current is 0
        let report = FailureReport {
            generation: 1,
            cause: RecoveryCause::UnexpectedEof,
        };

        let res = supervisor.report_failure(report).await;
        // Verify it failed/rejected
        assert!(res.is_err());
        // Verify 0 provider attempts made
        assert_eq!(recover_count.load(Ordering::SeqCst), 0);
        // Verify current generation remains 0
        let state_guard = supervisor.state.read().await;
        assert_eq!(state_guard.generation.id, 0);
    }

    #[tokio::test]
    async fn test_duplicate_exhausted_handled_report() {
        struct ExhaustFailingRecovery {
            recover_count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl RealtimeRecovery for ExhaustFailingRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                self.recover_count.fetch_add(1, Ordering::SeqCst);
                Err(RealtimeError::ConnectionError("always fail".to_string()))
            }
        }

        let recover_count = Arc::new(AtomicUsize::new(0));
        let mock_rec = Arc::new(ExhaustFailingRecovery {
            recover_count: Arc::clone(&recover_count),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_deadline(Duration::from_secs(5));

        let supervisor = RecoverySupervisor::new(
            policy,
            crate::config::RealtimeConfig::default(),
            initial_session,
        );

        // 1. Report N once, which will fail on first attempt since it's fatal
        let report1 = FailureReport {
            generation: 0,
            cause: RecoveryCause::UnexpectedEof,
        };
        let res1 = supervisor.report_failure(report1).await;
        assert!(res1.is_err());
        assert_eq!(recover_count.load(Ordering::SeqCst), 1);

        // 2. Report N again. This must trigger ZERO new provider attempts and return Ok(active_session)
        // treated as a normal AlreadyHandled outcome.
        let report2 = FailureReport {
            generation: 0,
            cause: RecoveryCause::UnexpectedEof,
        };
        let res2 = supervisor.report_failure(report2).await;
        assert!(res2.is_ok());
        let returned_session = res2.unwrap();
        assert_eq!(returned_session.session_id(), "gen-0");
        // Verify recover_count did not increase
        assert_eq!(recover_count.load(Ordering::SeqCst), 1);
    }

    struct TimeoutRecovery;

    #[async_trait]
    impl RealtimeRecovery for TimeoutRecovery {
        fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
            RecoveryDisposition::Recoverable
        }

        async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
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
            crate::config::RealtimeConfig::default(),
            initial_session,
        );

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

        let start = tokio::time::Instant::now();
        let res = supervisor.report_failure(report).await;
        let elapsed = start.elapsed();

        assert!(res.is_err());
        let err = match res {
            Err(e) => e,
            _ => unreachable!(),
        };
        match err {
            RealtimeError::Timeout(_) => {}
            other => panic!("expected RealtimeError::Timeout, got {:?}", other),
        }

        assert!(elapsed >= Duration::from_millis(50));
        assert!(elapsed < Duration::from_millis(150));
    }

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
            crate::config::RealtimeConfig::default(),
            initial_session,
        );

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

        let res = supervisor.report_failure(report).await;
        assert!(res.is_err());
        assert_eq!(recover_count.load(Ordering::SeqCst), 0);
    }

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
            crate::config::RealtimeConfig::default(),
            initial_session,
        );

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

        let res = supervisor.report_failure(report).await;
        assert!(res.is_err());
        assert_eq!(recover_count.load(Ordering::SeqCst), 1);
    }

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
            crate::config::RealtimeConfig::default(),
            initial_session,
        );

        let start_instant = tokio::time::Instant::now();

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

        let res = supervisor.report_failure(report).await.unwrap();
        assert_eq!(res.session_id(), "gen-1-finally");

        assert_eq!(recover_count.load(Ordering::SeqCst), 3);

        let times = attempt_times.lock().clone();
        assert_eq!(times.len(), 3);

        let diff1 = times[1].duration_since(times[0]);
        let diff2 = times[2].duration_since(times[1]);

        assert!(diff1 >= Duration::from_millis(50) && diff1 < Duration::from_millis(60));
        assert!(diff2 >= Duration::from_millis(100) && diff2 < Duration::from_millis(110));

        let total_elapsed = tokio::time::Instant::now().duration_since(start_instant);
        assert!(
            total_elapsed >= Duration::from_millis(150) && total_elapsed < Duration::from_millis(170)
        );
    }
}
