#![allow(unfulfilled_lint_expectations)]

use crate::error::{RealtimeError, Result};
use crate::recovery::{RecoveryCause, RecoveryContext, RecoveryDisposition, RecoveryPolicy};
use crate::session::RealtimeSession;
use std::num::NonZeroU32;
use std::sync::Arc;

#[cfg_attr(not(test), expect(dead_code, reason = "wired by managed-runner integration follow-up"))]
pub(crate) struct SessionGeneration {
    pub(crate) id: u64,
    pub(crate) session: Arc<dyn RealtimeSession>,
}

#[cfg_attr(not(test), expect(dead_code, reason = "wired by managed-runner integration follow-up"))]
pub(crate) struct FailureReport {
    pub(crate) generation: u64,
    pub(crate) cause: RecoveryCause,
}

#[cfg_attr(not(test), expect(dead_code, reason = "wired by managed-runner integration follow-up"))]
struct SupervisorState {
    generation: SessionGeneration,
    exhausted_generation: Option<u64>,
}

/// The outcome of a recovery report.
#[cfg_attr(not(test), expect(dead_code, reason = "wired by managed-runner integration follow-up"))]
#[derive(Clone)]
pub(crate) enum RecoveryOutcome {
    /// A newly recovered session was successfully established and published.
    Recovered(Arc<dyn RealtimeSession>),
    /// A report for a stale/older generation than the currently active one.
    /// It performs zero provider attempts and leaves the newer active session running.
    Stale(Arc<dyn RealtimeSession>),
    /// A report for a generation that was already terminally exhausted / handled.
    /// It performs zero provider attempts and does not return the failed session.
    Exhausted,
}

impl std::fmt::Debug for RecoveryOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Recovered(session) => {
                f.debug_struct("Recovered").field("session_id", &session.session_id()).finish()
            }
            Self::Stale(session) => {
                f.debug_struct("Stale").field("session_id", &session.session_id()).finish()
            }
            Self::Exhausted => f.debug_struct("Exhausted").finish(),
        }
    }
}

/// The private recovery supervisor.
#[cfg_attr(not(test), expect(dead_code, reason = "wired by managed-runner integration follow-up"))]
pub(crate) struct RecoverySupervisor {
    policy: RecoveryPolicy,
    config: Arc<tokio::sync::RwLock<crate::config::RealtimeConfig>>,
    state: tokio::sync::RwLock<SupervisorState>,
    recovery_lock: tokio::sync::Mutex<()>,
}

#[cfg_attr(not(test), expect(dead_code, reason = "wired by managed-runner integration follow-up"))]
impl RecoverySupervisor {
    /// Create a new recovery supervisor.
    pub(crate) fn new(
        policy: RecoveryPolicy,
        config: Arc<tokio::sync::RwLock<crate::config::RealtimeConfig>>,
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
    pub(crate) async fn report_failure(&self, report: FailureReport) -> Result<RecoveryOutcome> {
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
                return Ok(RecoveryOutcome::Stale(state_guard.generation.session.clone()));
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
                        "report for already exhausted generation"
                    );
                    return Ok(RecoveryOutcome::Exhausted);
                }
            }
        }

        // 2. Lock wait capped by remaining deadline
        let remaining = deadline_instant.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let err =
                RealtimeError::Timeout("deadline expired before lock acquisition".to_string());
            tracing::warn!(generation = report.generation, err = %err, "recovery deadline expired");
            return Err(err);
        }

        let lock_guard = match tokio::time::timeout(remaining, self.recovery_lock.lock()).await {
            Ok(guard) => guard,
            Err(_) => {
                let err = RealtimeError::Timeout(
                    "deadline expired waiting for recovery lock".to_string(),
                );
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
                return Ok(RecoveryOutcome::Stale(state_guard.generation.session.clone()));
            }

            if report.generation > state_guard.generation.id {
                return Err(RealtimeError::provider(
                    "invalid future generation report after lock acquisition",
                ));
            }

            if let Some(exhausted) = state_guard.exhausted_generation {
                if report.generation == exhausted {
                    tracing::info!(
                        generation = report.generation,
                        "report for already exhausted generation after lock acquisition"
                    );
                    return Ok(RecoveryOutcome::Exhausted);
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
            let err = RealtimeError::provider(
                "fatal recovery cause detected; performing zero provider attempts",
            );
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
        let mut attempts_made = 0;

        for attempt_idx in 1..=max_attempts {
            attempts_made = attempt_idx;
            let now = tokio::time::Instant::now();
            if now >= deadline_instant {
                let err =
                    RealtimeError::Timeout("recovery deadline expired before attempt".to_string());
                tracing::warn!(generation = report.generation, attempt = attempt_idx, err = %err, "recovery deadline expired");
                final_error = Some(err);
                break;
            }

            let attempt_remaining = deadline_instant.saturating_duration_since(now);
            let attempt_nz = NonZeroU32::new(attempt_idx).unwrap();
            let remaining_dur =
                deadline_instant.saturating_duration_since(tokio::time::Instant::now());
            let context_deadline = std::time::Instant::now()
                .checked_add(remaining_dur)
                .unwrap_or_else(std::time::Instant::now);

            // Clone configuration dynamically before recovery invocation to preserve config authority
            // and release the read lock on self.config during the async call to prevent deadlock.
            let effective_config = self.config.read().await.clone();
            let context = RecoveryContext::new(
                attempt_nz,
                &report.cause,
                &effective_config,
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
                    let err =
                        RealtimeError::Timeout("provider recovery attempt timed out".to_string());
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
                    state_guard.generation =
                        SessionGeneration { id: next_gen, session: new_session.clone() };

                    return Ok(RecoveryOutcome::Recovered(new_session));
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
                        let remaining_after_attempt =
                            deadline_instant.saturating_duration_since(now_after_attempt);
                        backoff = backoff.min(remaining_after_attempt);

                        if backoff.is_zero() {
                            let timeout_err = RealtimeError::Timeout(
                                "recovery deadline expired during backoff calculation".to_string(),
                            );
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
                other => {
                    if attempts_made < max_attempts {
                        RealtimeError::ProviderError(format!(
                            "recovery aborted after {} attempt(s) due to fatal error; last error: {}",
                            attempts_made, other
                        ))
                    } else {
                        RealtimeError::ProviderError(format!(
                            "recovery exhausted after {} attempt(s); last error: {}",
                            attempts_made, other
                        ))
                    }
                }
            }
        } else {
            RealtimeError::ProviderError(format!(
                "recovery exhausted after {} attempt(s)",
                attempts_made
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
    use crate::events::{ClientEvent, ServerEvent, ToolResponse};
    use crate::recovery::{RealtimeRecovery, RecoveredSession, RecoveryContinuity};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
        ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<ServerEvent>> + Send + '_>>
        {
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
        active_recoveries: Arc<AtomicUsize>,
        max_active_recoveries: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl RealtimeRecovery for CoalescingRecovery {
        fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
            RecoveryDisposition::Recoverable
        }

        async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
            self.recover_count.fetch_add(1, Ordering::SeqCst);
            let active = self.active_recoveries.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active_recoveries.fetch_max(active, Ordering::SeqCst);

            tokio::time::sleep(Duration::from_millis(50)).await;

            self.active_recoveries.fetch_sub(1, Ordering::SeqCst);
            let session =
                Arc::new(MockSession { id: "gen-1-recovered".to_string(), recovery: None });
            Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
        }
    }

    #[tokio::test]
    async fn test_concurrent_coalescing() {
        let recover_count = Arc::new(AtomicUsize::new(0));
        let active_recoveries = Arc::new(AtomicUsize::new(0));
        let max_active_recoveries = Arc::new(AtomicUsize::new(0));

        let mock_rec = Arc::new(CoalescingRecovery {
            recover_count: Arc::clone(&recover_count),
            active_recoveries,
            max_active_recoveries: Arc::clone(&max_active_recoveries),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::new(policy, config, initial_session);

        // Spawn 3 concurrent failure reports
        let mut join_handles = Vec::new();
        let sup_arc = Arc::new(supervisor);
        for _ in 0..3 {
            let sup_clone = Arc::clone(&sup_arc);
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

        // Verify maximum simultaneous provider recoveries is exactly 1
        assert_eq!(max_active_recoveries.load(Ordering::SeqCst), 1);

        // Verify 1 task received Recovered, and the others received Stale
        let mut recovered_count = 0;
        let mut stale_count = 0;
        for res in results {
            match res.unwrap() {
                RecoveryOutcome::Recovered(session) => {
                    recovered_count += 1;
                    assert_eq!(session.session_id(), "gen-1-recovered");
                }
                RecoveryOutcome::Stale(session) => {
                    stale_count += 1;
                    assert_eq!(session.session_id(), "gen-1-recovered");
                }
                RecoveryOutcome::Exhausted => {
                    panic!("Expected Recovered or Stale, got Exhausted");
                }
            }
        }
        assert_eq!(recovered_count, 1);
        assert_eq!(stale_count, 2);
    }

    #[tokio::test]
    async fn test_delayed_report_after_publication() {
        let recover_count = Arc::new(AtomicUsize::new(0));
        let active_recoveries = Arc::new(AtomicUsize::new(0));
        let max_active_recoveries = Arc::new(AtomicUsize::new(0));

        let mock_rec = Arc::new(CoalescingRecovery {
            recover_count: Arc::clone(&recover_count),
            active_recoveries,
            max_active_recoveries,
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default();
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::new(policy, config, initial_session);

        // First, successfully recover generation 0 to publish generation 1
        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res1 = supervisor.report_failure(report).await.unwrap();
        assert!(matches!(res1, RecoveryOutcome::Recovered(_)));

        // Verify generation 1 is active
        {
            let state_guard = supervisor.state.read().await;
            assert_eq!(state_guard.generation.id, 1);
        }

        // Reset recover count to track any new attempts
        recover_count.store(0, Ordering::SeqCst);

        // Now, report delayed failure for generation 0
        let delayed_report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res2 = supervisor.report_failure(delayed_report).await.unwrap();

        // Verify it returns Stale with the generation 1 session, performs 0 attempts, and leaves N+1 active
        match res2 {
            RecoveryOutcome::Stale(session) => {
                assert_eq!(session.session_id(), "gen-1-recovered");
            }
            _ => panic!("Expected Stale outcome, got {:?}", res2),
        }
        assert_eq!(recover_count.load(Ordering::SeqCst), 0);

        let state_guard = supervisor.state.read().await;
        assert_eq!(state_guard.generation.id, 1);
    }

    #[tokio::test]
    async fn test_future_generation_rejection() {
        let recover_count = Arc::new(AtomicUsize::new(0));
        let active_recoveries = Arc::new(AtomicUsize::new(0));
        let max_active_recoveries = Arc::new(AtomicUsize::new(0));
        let mock_rec = Arc::new(CoalescingRecovery {
            recover_count: Arc::clone(&recover_count),
            active_recoveries,
            max_active_recoveries,
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default();
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::new(policy, config, initial_session);

        // Report future generation 1 while current is 0
        let report = FailureReport { generation: 1, cause: RecoveryCause::UnexpectedEof };

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

            fn classify_attempt_error(&self, _error: &RealtimeError) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                self.recover_count.fetch_add(1, Ordering::SeqCst);
                Err(RealtimeError::ConnectionError("always fail".to_string()))
            }
        }

        let recover_count = Arc::new(AtomicUsize::new(0));
        let mock_rec =
            Arc::new(ExhaustFailingRecovery { recover_count: Arc::clone(&recover_count) });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::new(policy, config, initial_session);

        // 1. Report N once, which will fail on first attempt since it's fatal
        let report1 = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res1 = supervisor.report_failure(report1).await;
        assert!(res1.is_err());
        assert_eq!(recover_count.load(Ordering::SeqCst), 3);

        // 2. Report N again. This must trigger ZERO new provider attempts and return Ok(RecoveryOutcome::Exhausted)
        // treated as a normal AlreadyHandled outcome.
        let report2 = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res2 = supervisor.report_failure(report2).await;
        assert!(res2.is_ok());
        let outcome = res2.unwrap();
        assert!(matches!(outcome, RecoveryOutcome::Exhausted));
        // Verify recover_count did not increase
        assert_eq!(recover_count.load(Ordering::SeqCst), 3);
    }

    struct CancelTrackingRecovery {
        dropped: Arc<AtomicBool>,
    }

    #[async_trait]
    impl RealtimeRecovery for CancelTrackingRecovery {
        fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
            RecoveryDisposition::Recoverable
        }

        async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
            struct Guard(Arc<AtomicBool>);
            impl Drop for Guard {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _guard = Guard(self.dropped.clone());
            tokio::time::sleep(Duration::from_millis(200)).await;
            let session =
                Arc::new(MockSession { id: "gen-1-too-late".to_string(), recovery: None });
            Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
        }
    }

    #[tokio::test]
    async fn test_genuine_timeout_cancellation_proves_drop() {
        let dropped = Arc::new(AtomicBool::new(false));
        let mock_rec = Arc::new(CancelTrackingRecovery { dropped: Arc::clone(&dropped) });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(1).unwrap())
            .with_deadline(Duration::from_millis(50));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::new(policy, config, initial_session);

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

        // Sleep briefly to make sure any task drop and state propagation completes
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert!(
            dropped.load(Ordering::SeqCst),
            "Pending provider future was not dropped/cancelled!"
        );
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
            let session =
                Arc::new(MockSession { id: "should-not-reach".to_string(), recovery: None });
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

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::new(policy, config, initial_session);

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

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::new(policy, config, initial_session);

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

        let res = supervisor.report_failure(report).await;
        assert!(res.is_err());
        assert_eq!(recover_count.load(Ordering::SeqCst), 1);

        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("aborted after 1 attempt(s) due to fatal error"),
            "Error message did not report exactly 1 attempt! got: {}",
            err_msg
        );
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
                let session =
                    Arc::new(MockSession { id: "gen-1-finally".to_string(), recovery: None });
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

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::new(policy, config, initial_session);

        let start_instant = tokio::time::Instant::now();

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

        let res = supervisor.report_failure(report).await.unwrap();
        match res {
            RecoveryOutcome::Recovered(session) => {
                assert_eq!(session.session_id(), "gen-1-finally");
            }
            _ => panic!("Expected Recovered outcome"),
        }

        assert_eq!(recover_count.load(Ordering::SeqCst), 3);

        let times = attempt_times.lock().clone();
        assert_eq!(times.len(), 3);

        let diff1 = times[1].duration_since(times[0]);
        let diff2 = times[2].duration_since(times[1]);

        assert!(diff1 >= Duration::from_millis(50) && diff1 < Duration::from_millis(60));
        assert!(diff2 >= Duration::from_millis(100) && diff2 < Duration::from_millis(110));

        let total_elapsed = tokio::time::Instant::now().duration_since(start_instant);
        assert!(
            total_elapsed >= Duration::from_millis(150)
                && total_elapsed < Duration::from_millis(170)
        );
    }

    struct AlwaysFailingRecovery;

    #[async_trait]
    impl RealtimeRecovery for AlwaysFailingRecovery {
        fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
            RecoveryDisposition::Recoverable
        }

        async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
            Err(RealtimeError::ConnectionError("always fail".to_string()))
        }
    }

    #[tokio::test]
    async fn test_failed_candidate_leaves_active_unchanged() {
        let mock_rec = Arc::new(AlwaysFailingRecovery);

        let initial_session = Arc::new(MockSession {
            id: "gen-0-active".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(2).unwrap())
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::new(policy, config, initial_session.clone());

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res = supervisor.report_failure(report).await;

        // Verify recovery failed
        assert!(res.is_err());

        // Verify active session and generation remains unchanged
        let state_guard = supervisor.state.read().await;
        assert_eq!(state_guard.generation.id, 0);
        assert_eq!(state_guard.generation.session.session_id(), "gen-0-active");
    }

    struct ConfigVerifyingRecovery {
        expected_instruction: String,
    }

    #[async_trait]
    impl RealtimeRecovery for ConfigVerifyingRecovery {
        fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
            RecoveryDisposition::Recoverable
        }

        async fn recover(&self, context: RecoveryContext<'_>) -> Result<RecoveredSession> {
            assert_eq!(
                context.config().instruction.as_deref(),
                Some(self.expected_instruction.as_str())
            );
            let session = Arc::new(MockSession { id: "recovered".to_string(), recovery: None });
            Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
        }
    }

    #[tokio::test]
    async fn test_recovery_uses_mutated_config() {
        let mock_rec = Arc::new(ConfigVerifyingRecovery {
            expected_instruction: "mutated-instruction".to_string(),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default();
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));

        let supervisor = RecoverySupervisor::new(policy, Arc::clone(&config), initial_session);

        // Mutate the authoritative config before calling report_failure
        {
            let mut writer = config.write().await;
            writer.instruction = Some("mutated-instruction".to_string());
        }

        // Now run recovery and verify it receives the mutated config
        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res = supervisor.report_failure(report).await;
        assert!(res.is_ok());
    }
}
