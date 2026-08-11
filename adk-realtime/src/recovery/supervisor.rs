use crate::error::{RealtimeError, Result};
use crate::recovery::{
    RecoveredSession, RecoveryCause, RecoveryContext, RecoveryDisposition, RecoveryPolicy,
};
use crate::session::RealtimeSession;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{sleep, Instant};

/// A generic orchestrator for realtime session recovery.
///
/// It coordinates bounded retries, applies exponential backoff based on a policy,
/// enforces an absolute deadline, and orchestrates exactly one active attempt at a time.
pub struct RecoverySupervisor {
    policy: RecoveryPolicy,
    active_session_ref: Arc<RwLock<Option<Arc<dyn RealtimeSession>>>>,
    generation: AtomicU64,
}

impl RecoverySupervisor {
    /// Creates a new `RecoverySupervisor`.
    pub fn new(
        policy: RecoveryPolicy,
        active_session_ref: Arc<RwLock<Option<Arc<dyn RealtimeSession>>>>,
    ) -> Self {
        Self { policy, active_session_ref, generation: AtomicU64::new(0) }
    }

    /// Attempts to recover the session based on the provided cause.
    pub async fn recover(
        &self,
        cause: &RecoveryCause,
        config: &crate::config::RealtimeConfig,
    ) -> Result<RecoveredSession> {
        let (session, my_generation) = {
            let guard = self.active_session_ref.read().await;
            let current_gen = self.generation.load(Ordering::Acquire);
            if let Some(ref session) = *guard {
                (Arc::clone(session), current_gen)
            } else {
                return Err(RealtimeError::connection("No active session to recover."));
            }
        };

        let recovery = session.recovery().ok_or_else(|| {
            RealtimeError::connection("Session does not support recovery capability.")
        })?;

        let disposition = recovery.classify(cause);
        if disposition == RecoveryDisposition::Fatal {
            return Err(RealtimeError::connection("Recovery cause is fatal; aborting."));
        }

        let max_attempts = self.policy.max_attempts().get();
        let deadline = Instant::now() + self.policy.deadline();
        let mut current_delay = self.policy.initial_delay();

        for attempt in 1..=max_attempts {
            let attempt_nz = NonZeroU32::new(attempt).unwrap();
            let context = RecoveryContext::new(attempt_nz, cause, config, deadline.into());

            if Instant::now() >= deadline {
                break;
            }

            match recovery.recover(context).await {
                Ok(recovered_session) => {
                    // Update active session only if the generation hasn't changed
                    let mut guard = self.active_session_ref.write().await;
                    let current_gen = self.generation.load(Ordering::Acquire);
                    if current_gen == my_generation {
                        *guard = Some(recovered_session.session());
                        self.generation.fetch_add(1, Ordering::Release);
                        return Ok(recovered_session);
                    } else {
                        return Err(RealtimeError::connection("Recovery attempt is stale."));
                    }
                }
                Err(e) => {
                    tracing::error!("Recovery attempt {} failed: {}", attempt, e);
                    if attempt == max_attempts {
                        return Err(RealtimeError::connection(format!(
                            "Max recovery attempts ({}) exhausted.",
                            max_attempts
                        )));
                    }

                    if Instant::now() >= deadline {
                        break;
                    }

                    // Check if generation changed during delay
                    if self.generation.load(Ordering::Acquire) != my_generation {
                        return Err(RealtimeError::connection("Recovery attempt is stale."));
                    }

                    // Apply delay and exponential backoff
                    sleep(current_delay).await;
                    current_delay = std::cmp::min(current_delay * 2, self.policy.max_delay());

                    // Check if generation changed during delay
                    if self.generation.load(Ordering::Acquire) != my_generation {
                        return Err(RealtimeError::connection("Recovery attempt is stale."));
                    }
                }
            }
        }

        Err(RealtimeError::connection("Recovery deadline exceeded."))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::recovery::{
        RealtimeRecovery, RecoveryContinuity, RecoveryDisposition,
    };
    use crate::error::RealtimeError;
    use crate::events::{ClientEvent, ServerEvent, ToolResponse};
    use crate::audio::AudioChunk;
    use crate::session::ContextMutationOutcome;
    use async_trait::async_trait;
    use std::pin::Pin;
    use futures::Stream;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[derive(Clone)]
    pub(crate) struct MockSession {
        id: String,
        recovery: Option<Arc<dyn RealtimeRecovery>>,
    }

    #[async_trait]
    impl RealtimeSession for MockSession {
        fn session_id(&self) -> &str { &self.id }
        fn is_connected(&self) -> bool { true }
        fn recovery(&self) -> Option<&dyn RealtimeRecovery> {
            self.recovery.as_deref()
        }
        async fn send_audio(&self, _audio: &AudioChunk) -> Result<()> { Ok(()) }
        async fn send_audio_base64(&self, _audio_base64: &str) -> Result<()> { Ok(()) }
        async fn send_text(&self, _text: &str) -> Result<()> { Ok(()) }
        async fn send_tool_response(&self, _response: ToolResponse) -> Result<()> { Ok(()) }
        async fn commit_audio(&self) -> Result<()> { Ok(()) }
        async fn clear_audio(&self) -> Result<()> { Ok(()) }
        async fn create_response(&self) -> Result<()> { Ok(()) }
        async fn interrupt(&self) -> Result<()> { Ok(()) }
        async fn send_event(&self, _event: ClientEvent) -> Result<()> { Ok(()) }
        async fn next_event(&self) -> Option<Result<ServerEvent>> { None }
        fn events(&self) -> Pin<Box<dyn Stream<Item = Result<ServerEvent>> + Send + '_>> {
            Box::pin(futures::stream::empty())
        }
        async fn close(&self) -> Result<()> { Ok(()) }
        async fn mutate_context(
            &self,
            _config: crate::config::RealtimeConfig,
        ) -> Result<ContextMutationOutcome> {
            Ok(ContextMutationOutcome::Applied)
        }
    }

    pub(crate) struct MockRecovery {
        disposition: RecoveryDisposition,
        recover_result: Arc<tokio::sync::Mutex<Box<dyn FnMut() -> Result<RecoveredSession> + Send + Sync>>>,
        attempts: Arc<AtomicU32>,
    }

    #[async_trait]
    impl RealtimeRecovery for MockRecovery {
        fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
            self.disposition
        }

        async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            let mut guard = self.recover_result.lock().await;
            guard()
        }
    }

    #[tokio::test]
    async fn unsupported_recovery_fails_explicitly() {
        let active_session = Arc::new(RwLock::new(Some(Arc::new(MockSession {
            id: "test".to_string(),
            recovery: None,
        }) as Arc<dyn RealtimeSession>)));

        let supervisor = RecoverySupervisor::new(RecoveryPolicy::default(), active_session);

        let err = supervisor.recover(&RecoveryCause::UnexpectedEof, &Default::default()).await.unwrap_err();
        assert!(err.to_string().contains("does not support recovery capability"));
    }

    #[tokio::test]
    async fn fatal_classification_performs_zero_attempts() {
        let attempts = Arc::new(AtomicU32::new(0));
        let active_session = Arc::new(RwLock::new(Some(Arc::new(MockSession {
            id: "test".to_string(),
            recovery: Some(Arc::new(MockRecovery {
                disposition: RecoveryDisposition::Fatal,
                recover_result: Arc::new(tokio::sync::Mutex::new(Box::new(|| {
                    Err(RealtimeError::connection("failed"))
                }))),
                attempts: Arc::clone(&attempts),
            })),
        }) as Arc<dyn RealtimeSession>)));

        let supervisor = RecoverySupervisor::new(RecoveryPolicy::default(), active_session);

        let err = supervisor.recover(&RecoveryCause::UnexpectedEof, &Default::default()).await.unwrap_err();
        assert!(err.to_string().contains("fatal; aborting"));
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn recoverable_classification_invokes_recovery() {
        let attempts = Arc::new(AtomicU32::new(0));
        let active_session = Arc::new(RwLock::new(Some(Arc::new(MockSession {
            id: "test".to_string(),
            recovery: Some(Arc::new(MockRecovery {
                disposition: RecoveryDisposition::Recoverable,
                recover_result: Arc::new(tokio::sync::Mutex::new(Box::new(|| {
                    Ok(RecoveredSession::new(Arc::new(MockSession {
                        id: "new".to_string(),
                        recovery: None,
                    }), RecoveryContinuity::Reconnected))
                }))),
                attempts: Arc::clone(&attempts),
            })),
        }) as Arc<dyn RealtimeSession>)));

        let supervisor = RecoverySupervisor::new(RecoveryPolicy::default(), active_session.clone());

        let result = supervisor.recover(&RecoveryCause::UnexpectedEof, &Default::default()).await;
        assert!(result.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);

        let active = active_session.read().await;
        assert_eq!(active.as_ref().unwrap().session_id(), "new");
    }

    #[tokio::test]
    async fn max_attempts_exhaustion() {
        let attempts = Arc::new(AtomicU32::new(0));
        let active_session = Arc::new(RwLock::new(Some(Arc::new(MockSession {
            id: "test".to_string(),
            recovery: Some(Arc::new(MockRecovery {
                disposition: RecoveryDisposition::Recoverable,
                recover_result: Arc::new(tokio::sync::Mutex::new(Box::new(|| {
                    Err(RealtimeError::connection("failed"))
                }))),
                attempts: Arc::clone(&attempts),
            })),
        }) as Arc<dyn RealtimeSession>)));

        let policy = RecoveryPolicy::new()
            .with_max_attempts(NonZeroU32::new(2).unwrap())
            .with_initial_delay(tokio::time::Duration::from_millis(1))
            .with_max_delay(tokio::time::Duration::from_millis(1));

        let supervisor = RecoverySupervisor::new(policy, active_session.clone());

        let err = supervisor.recover(&RecoveryCause::UnexpectedEof, &Default::default()).await.unwrap_err();
        assert!(err.to_string().contains("exhausted"));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);

        let active = active_session.read().await;
        assert_eq!(active.as_ref().unwrap().session_id(), "test");
    }

    #[tokio::test]
    async fn failure_then_success_publishes_successful() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let active_session = Arc::new(RwLock::new(Some(Arc::new(MockSession {
            id: "test".to_string(),
            recovery: Some(Arc::new(MockRecovery {
                disposition: RecoveryDisposition::Recoverable,
                recover_result: Arc::new(tokio::sync::Mutex::new(Box::new(move || {
                    if attempts_clone.load(Ordering::SeqCst) == 1 {
                        Err(RealtimeError::connection("failed"))
                    } else {
                        Ok(RecoveredSession::new(Arc::new(MockSession {
                            id: "new".to_string(),
                            recovery: None,
                        }), RecoveryContinuity::Reconnected))
                    }
                }))),
                attempts: Arc::clone(&attempts),
            })),
        }) as Arc<dyn RealtimeSession>)));

        let policy = RecoveryPolicy::new()
            .with_initial_delay(tokio::time::Duration::from_millis(1))
            .with_max_delay(tokio::time::Duration::from_millis(1));

        let supervisor = RecoverySupervisor::new(policy, active_session.clone());

        let result = supervisor.recover(&RecoveryCause::UnexpectedEof, &Default::default()).await;
        assert!(result.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);

        let active = active_session.read().await;
        assert_eq!(active.as_ref().unwrap().session_id(), "new");
    }

    #[tokio::test]
    async fn absolute_deadline_stops_recovery() {
        let attempts = Arc::new(AtomicU32::new(0));
        let active_session = Arc::new(RwLock::new(Some(Arc::new(MockSession {
            id: "test".to_string(),
            recovery: Some(Arc::new(MockRecovery {
                disposition: RecoveryDisposition::Recoverable,
                recover_result: Arc::new(tokio::sync::Mutex::new(Box::new(|| {
                    Err(RealtimeError::connection("failed"))
                }))),
                attempts: Arc::clone(&attempts),
            })),
        }) as Arc<dyn RealtimeSession>)));

        let policy = RecoveryPolicy::new()
            .with_max_attempts(NonZeroU32::new(100).unwrap())
            .with_deadline(tokio::time::Duration::from_millis(50))
            .with_initial_delay(tokio::time::Duration::from_millis(30))
            .with_max_delay(tokio::time::Duration::from_millis(30));

        let supervisor = RecoverySupervisor::new(policy, active_session.clone());

        let err = supervisor.recover(&RecoveryCause::UnexpectedEof, &Default::default()).await.unwrap_err();
        assert!(err.to_string().contains("deadline"));
        // Should only be able to make 2-3 attempts before hitting 50ms total, definitely not 100
        assert!(attempts.load(Ordering::SeqCst) < 10);
    }

    #[tokio::test]
    async fn stale_attempts_cannot_overwrite_newer_session() {
        let active_session = Arc::new(RwLock::new(Some(Arc::new(MockSession {
            id: "test".to_string(),
            recovery: Some(Arc::new(MockRecovery {
                disposition: RecoveryDisposition::Recoverable,
                recover_result: Arc::new(tokio::sync::Mutex::new(Box::new(|| {
                    Err(RealtimeError::connection("failed"))
                }))),
                attempts: Arc::new(AtomicU32::new(0)),
            })),
        }) as Arc<dyn RealtimeSession>)));

        let supervisor = RecoverySupervisor::new(RecoveryPolicy::default(), active_session.clone());

        let supervisor = Arc::new(supervisor);
        let supervisor_clone = supervisor.clone();

        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
            supervisor_clone.generation.fetch_add(1, Ordering::SeqCst);
        });

        let err = supervisor.recover(&RecoveryCause::UnexpectedEof, &Default::default()).await.unwrap_err();
        assert!(err.to_string().contains("stale"));

        let active = active_session.read().await;
        assert_eq!(active.as_ref().unwrap().session_id(), "test");
    }
}
