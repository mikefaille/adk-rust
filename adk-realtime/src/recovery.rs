//! Provider-neutral recovery architecture contract.
//!
//! This module defines the types and traits required to handle connection recovery
//! across different realtime session providers.

use crate::error::Result;
use crate::session::RealtimeSession;
use async_trait::async_trait;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

/// Recovery continuity indicates whether the logical provider-level session history
/// was preserved or if a clean reconnect occurred.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryContinuity {
    /// Provider-native logical continuity was actually preserved.
    Resumed,
    /// Transport reconnected cleanly.
    ///
    /// Previous conversation state/history is not guaranteed to have survived.
    /// However, a successful `recover(context)` returning `Reconnected` guarantees that
    /// the current effective configuration (`context.config()`) has been successfully
    /// applied and the session is fully ready to accept commands.
    Reconnected,
}

/// Disposition of a recovery attempt based on the recovery cause.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryDisposition {
    /// Recovery should be attempted.
    Recoverable,
    /// Recovery is impossible or has failed in an unrecoverable way.
    Fatal,
}

/// Delivery certainty indicates what is known about whether a payload was sent.
///
/// Neither variant guarantees successful remote processing by the provider; they merely
/// bound when retry or buffering is safe vs when it might cause duplicate execution.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryCertainty {
    /// The managed runner rejected the operation before invoking the active RealtimeSession;
    /// application-level retry/buffering is safe.
    NotAttempted,
    /// The raw session was invoked but peer acceptance cannot be established; do not
    /// blindly duplicate side-effectful commands.
    Indeterminate,
}

/// The provider-neutral cause that triggered the recovery attempt.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum RecoveryCause {
    /// Read operation failed with the given session error.
    ReadFailed(Arc<crate::error::RealtimeError>),
    /// Write operation failed with the given session error.
    WriteFailed(Arc<crate::error::RealtimeError>),
    /// Unexpected end-of-file on the connection stream.
    UnexpectedEof,
}

/// Opaque/defaultable policy for scheduling recovery attempts.
#[derive(Debug, Clone)]
pub struct RecoveryPolicy {
    max_attempts: NonZeroU32,
    deadline: Duration,
    initial_delay: Duration,
    max_delay: Duration,
}

impl Default for RecoveryPolicy {
    /// Recommended realtime defaults:
    /// - max_attempts = 3
    /// - total deadline around 5s
    /// - initial_delay = 50ms
    /// - max_delay = 500ms
    /// - deterministic/no jitter by default
    fn default() -> Self {
        Self {
            max_attempts: NonZeroU32::new(3).unwrap(),
            deadline: Duration::from_secs(5),
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_millis(500),
        }
    }
}

impl RecoveryPolicy {
    /// Create a new default policy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the maximum number of recovery attempts.
    pub fn max_attempts(&self) -> NonZeroU32 {
        self.max_attempts
    }

    /// Set the maximum number of recovery attempts.
    pub fn with_max_attempts(mut self, max_attempts: NonZeroU32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    /// Get the maximum total duration allowed for recovery.
    pub fn deadline(&self) -> Duration {
        self.deadline
    }

    /// Set the maximum total duration allowed for recovery.
    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// Get the initial delay between recovery attempts.
    pub fn initial_delay(&self) -> Duration {
        self.initial_delay
    }

    /// Set the initial delay between recovery attempts.
    pub fn with_initial_delay(mut self, initial_delay: Duration) -> Self {
        self.initial_delay = initial_delay;
        self
    }

    /// Get the maximum delay between recovery attempts.
    pub fn max_delay(&self) -> Duration {
        self.max_delay
    }

    /// Set the maximum delay between recovery attempts.
    pub fn with_max_delay(mut self, max_delay: Duration) -> Self {
        self.max_delay = max_delay;
        self
    }
}

/// Context passed to each individual provider recovery attempt.
#[derive(Debug, Clone)]
pub struct RecoveryContext<'a> {
    attempt: NonZeroU32,
    cause: &'a RecoveryCause,
    config: &'a crate::config::RealtimeConfig,
    deadline: std::time::Instant,
}

impl<'a> RecoveryContext<'a> {
    /// Create a new recovery context.
    pub fn new(
        attempt: NonZeroU32,
        cause: &'a RecoveryCause,
        config: &'a crate::config::RealtimeConfig,
        deadline: std::time::Instant,
    ) -> Self {
        Self { attempt, cause, config, deadline }
    }

    /// Get the current attempt number.
    pub fn attempt(&self) -> NonZeroU32 {
        self.attempt
    }

    /// Get the cause of the recovery attempt.
    pub fn cause(&self) -> &RecoveryCause {
        self.cause
    }

    /// Get the active realtime configuration.
    pub fn config(&self) -> &crate::config::RealtimeConfig {
        self.config
    }

    /// Get the absolute instant by which the recovery process must complete.
    pub fn deadline(&self) -> std::time::Instant {
        self.deadline
    }
}

/// The successful outcome of a single provider recovery attempt.
#[derive(Clone)]
pub struct RecoveredSession {
    session: Arc<dyn RealtimeSession>,
    continuity: RecoveryContinuity,
}

impl RecoveredSession {
    /// Create a new recovered session wrapper.
    pub fn new(session: Arc<dyn RealtimeSession>, continuity: RecoveryContinuity) -> Self {
        Self { session, continuity }
    }

    /// Get the recovered session.
    pub fn session(&self) -> Arc<dyn RealtimeSession> {
        Arc::clone(&self.session)
    }

    /// Get the recovery continuity.
    pub fn continuity(&self) -> RecoveryContinuity {
        self.continuity
    }
}

impl std::fmt::Debug for RecoveredSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoveredSession").field("continuity", &self.continuity).finish()
    }
}

/// Provider-facing recovery capability interface.
///
/// An individual call to `recover()` represents exactly one attempt to establish
/// a fresh session. The provider does not own retry loops or the total recovery policy;
/// those are managed by the supervisor.
///
/// `recover()` returns only after the candidate satisfies the provider-specific readiness boundary
/// (e.g. `setupComplete` for Gemini Live) and after the current effective configuration
/// (`context.config()`) has been fully applied to the returned session.
#[async_trait]
pub trait RealtimeRecovery: Send + Sync {
    /// Classify whether the given cause is recoverable or fatal for this provider.
    fn classify(&self, cause: &RecoveryCause) -> RecoveryDisposition;

    /// Classify whether an error returned by a `recover()` attempt is retryable or fatal.
    ///
    /// By default, returns `RecoveryDisposition::Fatal` (fail-closed) so that
    /// any unexpected recovery attempt failures are not blindly retried unless
    /// a provider explicitly opts into them.
    fn classify_attempt_error(&self, _error: &crate::error::RealtimeError) -> RecoveryDisposition {
        RecoveryDisposition::Fatal
    }

    /// Attempt to recover the session once.
    async fn recover(&self, context: RecoveryContext<'_>) -> Result<RecoveredSession>;
}

/// Helper function to clone standard RealtimeErrors since they don't implement Clone natively.
#[allow(dead_code)]
fn clone_realtime_error(err: &crate::error::RealtimeError) -> crate::error::RealtimeError {
    match err {
        crate::error::RealtimeError::ConnectionError(s) => {
            crate::error::RealtimeError::ConnectionError(s.clone())
        }
        crate::error::RealtimeError::MessageError(s) => {
            crate::error::RealtimeError::MessageError(s.clone())
        }
        crate::error::RealtimeError::Protocol(s) => {
            crate::error::RealtimeError::Protocol(s.clone())
        }
        crate::error::RealtimeError::AuthError(s) => {
            crate::error::RealtimeError::AuthError(s.clone())
        }
        crate::error::RealtimeError::NotConnected => crate::error::RealtimeError::NotConnected,
        crate::error::RealtimeError::SessionClosed => crate::error::RealtimeError::SessionClosed,
        crate::error::RealtimeError::ConfigError(s) => {
            crate::error::RealtimeError::ConfigError(s.clone())
        }
        crate::error::RealtimeError::AudioFormatError(s) => {
            crate::error::RealtimeError::AudioFormatError(s.clone())
        }
        crate::error::RealtimeError::ToolError(s) => {
            crate::error::RealtimeError::ToolError(s.clone())
        }
        crate::error::RealtimeError::ServerError { code, message } => {
            crate::error::RealtimeError::ServerError {
                code: code.clone(),
                message: message.clone(),
            }
        }
        crate::error::RealtimeError::Timeout(s) => crate::error::RealtimeError::Timeout(s.clone()),
        crate::error::RealtimeError::SerializationError(e) => {
            crate::error::RealtimeError::protocol(format!("serialization error: {}", e))
        }
        crate::error::RealtimeError::ProviderError(s) => {
            crate::error::RealtimeError::ProviderError(s.clone())
        }
        crate::error::RealtimeError::IoError(e) => {
            crate::error::RealtimeError::IoError(std::io::Error::new(e.kind(), e.to_string()))
        }
        crate::error::RealtimeError::OpusCodecError(s) => {
            crate::error::RealtimeError::OpusCodecError(s.clone())
        }
        crate::error::RealtimeError::WebRTCError(s) => {
            crate::error::RealtimeError::WebRTCError(s.clone())
        }
        crate::error::RealtimeError::LiveKitError(s) => {
            crate::error::RealtimeError::LiveKitError(s.clone())
        }
        #[cfg(feature = "livekit")]
        crate::error::RealtimeError::LiveKitNativeError(e) => {
            crate::error::RealtimeError::LiveKitError(format!("native livekit error: {:?}", e))
        }
    }
}

/// Active session generation tracking the unique generation index and active session reference.
#[allow(dead_code)]
#[derive(Clone)]
pub struct SessionGeneration {
    pub id: u64,
    pub session: Arc<dyn RealtimeSession>,
}

/// Trigger report of a session failure.
#[allow(dead_code)]
#[derive(Clone)]
pub struct FailureReport {
    pub generation: u64,
    pub cause: RecoveryCause,
}

type CachedRecoveryResult =
    std::result::Result<Arc<dyn RealtimeSession>, Arc<crate::error::RealtimeError>>;

#[allow(dead_code)]
struct SupervisorState {
    generation: SessionGeneration,
    last_recovery_result: Option<(u64, CachedRecoveryResult)>,
}

/// The private recovery supervisor.
#[allow(dead_code)]
pub struct RecoverySupervisor {
    policy: RecoveryPolicy,
    config: crate::config::RealtimeConfig,
    state: tokio::sync::RwLock<SupervisorState>,
    recovery_lock: tokio::sync::Mutex<()>,
}

#[allow(dead_code)]
impl RecoverySupervisor {
    /// Create a new recovery supervisor.
    pub fn new(
        policy: RecoveryPolicy,
        config: crate::config::RealtimeConfig,
        initial_session: Arc<dyn RealtimeSession>,
    ) -> Self {
        Self {
            policy,
            config,
            state: tokio::sync::RwLock::new(SupervisorState {
                generation: SessionGeneration { id: 0, session: initial_session },
                last_recovery_result: None,
            }),
            recovery_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Report a failure in the active session.
    #[allow(clippy::collapsible_if)]
    pub async fn report_failure(&self, report: FailureReport) -> Result<Arc<dyn RealtimeSession>> {
        let start_time = tokio::time::Instant::now();
        let deadline_duration = self.policy.deadline();
        let deadline_instant = start_time.checked_add(deadline_duration).unwrap_or(start_time);

        // 1. Quick check of state
        {
            let state_guard = self.state.read().await;
            if state_guard.generation.id > report.generation {
                // Stale report, some other thread already advanced the generation.
                tracing::info!(
                    report_gen = report.generation,
                    current_gen = state_guard.generation.id,
                    "stale failure report ignored"
                );
                return Ok(state_guard.generation.session.clone());
            }

            if let Some((cached_gen, ref cached_res)) = state_guard.last_recovery_result {
                if cached_gen == report.generation {
                    tracing::info!(
                        generation = report.generation,
                        "returning cached recovery result"
                    );
                    return cached_res
                        .as_ref()
                        .map(|s| s.clone())
                        .map_err(|e| clone_realtime_error(e));
                }
            }
        }

        // 2. Lock wait capped by remaining deadline
        let remaining = deadline_instant.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let err = crate::error::RealtimeError::Timeout(
                "deadline expired before lock acquisition".to_string(),
            );
            tracing::warn!(generation = report.generation, err = %err, "recovery deadline expired");
            return Err(err);
        }

        let lock_guard = match tokio::time::timeout(remaining, self.recovery_lock.lock()).await {
            Ok(guard) => guard,
            Err(_) => {
                let err = crate::error::RealtimeError::Timeout(
                    "deadline expired waiting for recovery lock".to_string(),
                );
                tracing::warn!(generation = report.generation, err = %err, "recovery deadline expired");
                return Err(err);
            }
        };

        // 3. Double-check state now that we hold the lock
        {
            let state_guard = self.state.read().await;
            if state_guard.generation.id > report.generation {
                tracing::info!(
                    report_gen = report.generation,
                    current_gen = state_guard.generation.id,
                    "coalesced failure report ignored"
                );
                return Ok(state_guard.generation.session.clone());
            }

            if let Some((cached_gen, ref cached_res)) = state_guard.last_recovery_result {
                if cached_gen == report.generation {
                    tracing::info!(
                        generation = report.generation,
                        "returning cached recovery result after lock acquisition"
                    );
                    return cached_res
                        .as_ref()
                        .map(|s| s.clone())
                        .map_err(|e| clone_realtime_error(e));
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
                let err = crate::error::RealtimeError::provider(
                    "active session does not support recovery",
                );
                tracing::error!(generation = report.generation, err = %err, "recovery not supported");
                // Cache the error so other waiters don't re-run this
                let mut state_guard = self.state.write().await;
                state_guard.last_recovery_result =
                    Some((report.generation, Err(Arc::new(clone_realtime_error(&err)))));
                return Err(err);
            }
        };

        // 5. Pre-flight cause classification
        if recovery_impl.classify(&report.cause) == RecoveryDisposition::Fatal {
            let err = crate::error::RealtimeError::provider(
                "fatal recovery cause detected; performing zero provider attempts",
            );
            tracing::warn!(generation = report.generation, err = %err, "fatal cause classification");
            let mut state_guard = self.state.write().await;
            state_guard.last_recovery_result =
                Some((report.generation, Err(Arc::new(clone_realtime_error(&err)))));
            return Err(err);
        }

        // 6. Recovery attempt loop
        tracing::info!(generation = report.generation, "recovery episode started");
        let mut final_error: Option<crate::error::RealtimeError> = None;
        let max_attempts = self.policy.max_attempts().get();

        for attempt_idx in 1..=max_attempts {
            let now = tokio::time::Instant::now();
            if now >= deadline_instant {
                let err = crate::error::RealtimeError::Timeout(
                    "recovery deadline expired before attempt".to_string(),
                );
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
            let context =
                RecoveryContext::new(attempt_nz, &report.cause, &self.config, context_deadline);

            tracing::info!(
                generation = report.generation,
                attempt = attempt_idx,
                "provider recovery attempt initiated"
            );

            let recover_fut = recovery_impl.recover(context);
            let attempt_res = match tokio::time::timeout(attempt_remaining, recover_fut).await {
                Ok(res) => res,
                Err(_) => {
                    let err = crate::error::RealtimeError::Timeout(
                        "provider recovery attempt timed out".to_string(),
                    );
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
                    let next_gen = report.generation.saturating_add(1);

                    let mut state_guard = self.state.write().await;
                    state_guard.generation =
                        SessionGeneration { id: next_gen, session: new_session.clone() };
                    state_guard.last_recovery_result =
                        Some((report.generation, Ok(new_session.clone())));

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
                        let remaining_after_attempt =
                            deadline_instant.saturating_duration_since(now_after_attempt);
                        backoff = backoff.min(remaining_after_attempt);

                        if backoff.is_zero() {
                            let timeout_err = crate::error::RealtimeError::Timeout(
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
                crate::error::RealtimeError::Timeout(_) => err,
                other => crate::error::RealtimeError::ProviderError(format!(
                    "recovery exhausted after {} attempts; last error: {}",
                    max_attempts, other
                )),
            }
        } else {
            crate::error::RealtimeError::ProviderError(format!(
                "recovery exhausted after {} attempts",
                max_attempts
            ))
        };

        tracing::error!(
            generation = report.generation,
            err = %final_err,
            "recovery exhausted or fatal"
        );

        let mut state_guard = self.state.write().await;
        state_guard.last_recovery_result =
            Some((report.generation, Err(Arc::new(clone_realtime_error(&final_err)))));

        drop(lock_guard); // Release lock explicitly
        Err(final_err)
    }
}
