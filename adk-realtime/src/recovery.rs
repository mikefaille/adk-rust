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

pub(crate) mod supervisor;
pub use supervisor::SessionGeneration;
