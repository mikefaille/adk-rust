//! Provider-neutral managed recovery contract for realtime sessions.
//!
//! This module defines the vocabulary and provider SPI used by the managed
//! [`crate::runner::RealtimeRunner`] lifecycle. The raw [`RealtimeSession`]
//! remains provider-facing and fail-fast; recovery policy, retry orchestration,
//! generation publication, and terminal lifecycle belong to the managed runner.
//!
//! # Managed versus raw
//!
//! ```text
//! RealtimeSession
//!   raw provider I/O
//!   optional recovery capability
//!   no generic retry loop
//!
//! RealtimeRunner
//!   managed lifecycle
//!   generation authority
//!   recovery/resumption serialization
//!   delivery certainty
//!   terminal state
//! ```
//!
//! A provider opts into managed recovery by returning a [`RealtimeRecovery`]
//! implementation from [`RealtimeSession::recovery`]. Sessions that do not opt
//! in remain valid raw sessions; the managed runner simply cannot rebuild them
//! automatically after a recoverable transport failure.
//!
//! # Transaction boundary
//!
//! One call to [`RealtimeRecovery::recover`] is exactly one private candidate
//! attempt. The provider must return only after that candidate reaches its real
//! provider-specific readiness boundary and the current effective configuration
//! has been applied. The managed supervisor owns the outer retry loop, absolute
//! deadline, generation publication, and replacement ordering.
//!
//! Recovery is not replay. Application audio, business commands, and other
//! domain events remain application-owned. [`DeliveryCertainty`] tells the
//! application whether the raw provider session was invoked by a failed managed
//! write; it does not prove provider-side processing.
//!
//! See `adk-realtime/MANAGED_RECOVERY.md` for the full maintenance, testing, and
//! product-claim contract.

use crate::error::Result;
use crate::session::RealtimeSession;
use async_trait::async_trait;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

/// Describes how much provider-level logical continuity survived recovery.
///
/// This is deliberately narrower than "the transport is connected". A ready
/// replacement may be healthy while prior provider conversation history is not
/// preserved.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryContinuity {
    /// Provider-native logical continuity was actually preserved.
    ///
    /// Use this only when the provider confirms that its resume mechanism kept
    /// the logical session/conversation continuity. Do not infer `Resumed` merely
    /// because a new transport connected successfully.
    Resumed,
    /// Transport reconnected cleanly with the current effective configuration.
    ///
    /// Previous conversation state/history is not guaranteed to have survived.
    /// However, a successful `recover(context)` returning `Reconnected` guarantees that
    /// the current effective configuration (`context.config()`) has been successfully
    /// applied and the session is fully ready to accept commands.
    Reconnected,
}

/// Disposition of a recovery cause or attempt failure.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryDisposition {
    /// Recovery should be attempted or retried according to the managed policy.
    Recoverable,
    /// Recovery is impossible or has failed in an unrecoverable way.
    Fatal,
}

/// What the managed runner knows about whether a failed write crossed the raw
/// provider invocation boundary.
///
/// Neither variant guarantees successful remote processing by the provider.
/// They answer a narrower and operationally useful question: did this managed
/// operation invoke the raw session at all?
///
/// This distinction is intended for application retry/buffering policy:
/// `NotAttempted` may be replayable because the provider was not invoked by that
/// operation, while `Indeterminate` must not be blindly replayed when duplicate
/// side effects would be harmful.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryCertainty {
    /// The managed runner rejected the operation before invoking the active
    /// [`RealtimeSession`].
    ///
    /// The provider was not called by this operation, so application-level
    /// buffering or retry can be considered according to domain semantics.
    NotAttempted,
    /// The raw session was invoked but peer acceptance or processing cannot be
    /// established.
    ///
    /// Do not blindly duplicate side-effectful commands. The application must
    /// decide whether replay is semantically safe.
    Indeterminate,
}

/// Provider-neutral reason that triggered a managed recovery episode.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum RecoveryCause {
    /// Read operation failed with the given session error.
    ReadFailed(Arc<crate::error::RealtimeError>),
    /// Write operation failed after the raw session was invoked.
    WriteFailed(Arc<crate::error::RealtimeError>),
    /// Unexpected end-of-file on the connection stream.
    UnexpectedEof,
}

/// Opaque/defaultable policy for scheduling managed recovery attempts.
///
/// The supervisor applies this policy to the whole episode. Provider
/// implementations must not create a second outer retry loop inside
/// [`RealtimeRecovery::recover`].
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

/// Context passed to one provider candidate attempt.
///
/// `deadline` is the absolute boundary inherited from the managed recovery
/// episode. Provider implementations should use it to bound authentication,
/// transport establishment, setup, and readiness waits.
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

    /// Get the active realtime configuration captured for this attempt.
    pub fn config(&self) -> &crate::config::RealtimeConfig {
        self.config
    }

    /// Get the absolute instant by which the recovery attempt must complete.
    pub fn deadline(&self) -> std::time::Instant {
        self.deadline
    }
}

/// A provider candidate that is fully ready for managed publication.
///
/// Constructing this value is a readiness claim: required provider setup has
/// completed and the current effective configuration has been applied. The
/// supervisor may still reject the candidate before publication if the
/// authoritative configuration revision changed while it was being built.
#[derive(Clone)]
pub struct RecoveredSession {
    session: Arc<dyn RealtimeSession>,
    continuity: RecoveryContinuity,
}

impl RecoveredSession {
    /// Create a new recovered session wrapper.
    ///
    /// Call this only after the provider-specific candidate is ready for managed
    /// traffic. Do not use it merely because a socket was opened.
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

/// Provider-facing capability for one managed recovery candidate attempt.
///
/// An individual call to [`recover`](Self::recover) represents exactly one
/// attempt to establish a replacement session. The provider does not own retry
/// loops, backoff, generation publication, or the total recovery policy; those
/// are managed by the supervisor.
///
/// `recover()` returns only after the candidate satisfies the provider-specific
/// readiness boundary (for example, a provider setup-complete signal) and after
/// the current effective configuration (`context.config()`) has been fully
/// applied to the returned session.
///
/// Provider implementations should also refresh attempt-scoped credentials when
/// required and clean up any resources that fail before a `RecoveredSession` is
/// returned.
#[async_trait]
pub trait RealtimeRecovery: Send + Sync {
    /// Classify whether the triggering cause is recoverable or fatal for this provider.
    fn classify(&self, cause: &RecoveryCause) -> RecoveryDisposition;

    /// Classify whether an error returned by a `recover()` attempt is retryable or fatal.
    ///
    /// By default, returns `RecoveryDisposition::Fatal` (fail-closed) so that
    /// any unexpected recovery attempt failures are not blindly retried unless
    /// a provider explicitly opts into them.
    fn classify_attempt_error(&self, _error: &crate::error::RealtimeError) -> RecoveryDisposition {
        RecoveryDisposition::Fatal
    }

    /// Build one ready replacement candidate.
    ///
    /// This method must not publish the candidate or mutate the managed active
    /// generation. Return only after provider setup/readiness is complete.
    async fn recover(&self, context: RecoveryContext<'_>) -> Result<RecoveredSession>;
}

pub(crate) mod supervisor;

/// Integration test barrier for holding managed recovery in `TransportStatus::Recovering`
/// before candidate connection/publication completes.
#[cfg(any(test, feature = "integration"))]
#[derive(Debug, Default)]
pub struct TestRecoveryBarrier {
    recovering_entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[cfg(any(test, feature = "integration"))]
impl TestRecoveryBarrier {
    /// Create a new recovery barrier.
    pub fn new() -> Self {
        Self::default()
    }

    /// Wait until `RecoverySupervisor` has entered `TransportStatus::Recovering`.
    pub async fn wait_until_recovering_entered(&self) {
        self.recovering_entered.notified().await;
    }

    /// Release the held recovery episode, allowing provider candidate connection & publication to proceed.
    pub fn release(&self) {
        self.release.notify_one();
    }

    /// Invoked by `RecoverySupervisor::report_failure` to signal `Recovering` state entry and pause until released.
    pub async fn on_recovering(&self) {
        self.recovering_entered.notify_one();
        self.release.notified().await;
    }
}
