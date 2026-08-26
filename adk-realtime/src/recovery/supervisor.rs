#![allow(unfulfilled_lint_expectations)]

use crate::error::{RealtimeError, Result};
use crate::recovery::{
    RecoveryCause, RecoveryContext, RecoveryContinuity, RecoveryDisposition, RecoveryPolicy,
};
use crate::session::RealtimeSession;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

/// Transport status of the supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TransportStatus {
    #[default]
    Uninitialized,
    Healthy,
    Recovering,
    Closed,
    Exhausted,
}

/// A session generation paired with its monotonic ID.
#[derive(Clone)]
pub(crate) struct SessionGeneration {
    pub(crate) id: u64,
    pub(crate) session: Arc<dyn RealtimeSession>,
}

impl std::fmt::Debug for SessionGeneration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionGeneration")
            .field("id", &self.id)
            .field("session_id", &self.session.session_id())
            .finish()
    }
}

#[derive(Clone)]
pub(crate) struct FailureReport {
    pub(crate) generation: u64,
    pub(crate) cause: RecoveryCause,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ConfigSnapshot {
    pub(crate) config: crate::config::RealtimeConfig,
    pub(crate) revision: u64,
}

struct SupervisorState {
    status: TransportStatus,
    generation: Option<SessionGeneration>,
    next_generation_id: u64,
    recovery_epoch: u64,
    exhausted_generation: Option<u64>,
    failed_generation: Option<u64>,
    config: ConfigSnapshot,
}

/// RAII guard ensuring an unpublished candidate session is closed if candidate publication fails or is dropped/cancelled.
struct CandidateCleanupGuard(Option<Arc<dyn RealtimeSession>>);

impl CandidateCleanupGuard {
    fn new(session: Arc<dyn RealtimeSession>) -> Self {
        Self(Some(session))
    }

    fn disarm(&mut self) {
        self.0.take();
    }
}

impl Drop for CandidateCleanupGuard {
    fn drop(&mut self) {
        if let Some(session) = self.0.take() {
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(2), session.close()).await;
            });
        }
    }
}

/// RAII guard ensuring an unowned recovery episode restores supervisor status to Healthy if aborted or cancelled.
struct RecoveryEpisodeGuard {
    state: Arc<tokio::sync::RwLock<SupervisorState>>,
    epoch: u64,
    disarmed: bool,
}

impl RecoveryEpisodeGuard {
    fn new(state: Arc<tokio::sync::RwLock<SupervisorState>>, epoch: u64) -> Self {
        Self { state, epoch, disarmed: false }
    }

    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for RecoveryEpisodeGuard {
    fn drop(&mut self) {
        if !self.disarmed {
            let state_lock = Arc::clone(&self.state);
            let epoch = self.epoch;
            tokio::spawn(async move {
                let mut state = state_lock.write().await;
                state.recover_to_healthy_if_unowned(epoch);
            });
        }
    }
}

enum ReportValidation {
    Valid,
    Stale(Arc<dyn RealtimeSession>),
    Exhausted,
    FutureGeneration(String),
    Closed,
}

impl SupervisorState {
    /// Return the currently active session generation if supervisor is healthy and generation is not write-blocked.
    fn admittable_generation(&self) -> Result<SessionGeneration> {
        match self.status {
            TransportStatus::Closed => Err(RealtimeError::SessionClosed),
            TransportStatus::Exhausted => Err(RealtimeError::provider("session exhausted")),
            TransportStatus::Uninitialized | TransportStatus::Recovering => {
                Err(RealtimeError::NotConnected)
            }
            TransportStatus::Healthy => {
                if let Some(ref current) = self.generation
                    && self.failed_generation != Some(current.id)
                {
                    Ok(current.clone())
                } else {
                    Err(RealtimeError::NotConnected)
                }
            }
        }
    }

    fn mark_failed(&mut self, generation: u64) {
        self.failed_generation = Some(generation);
    }

    fn promote_to_recovering(&mut self) -> u64 {
        self.failed_generation = None;
        self.status = TransportStatus::Recovering;
        self.recovery_epoch = self.recovery_epoch.wrapping_add(1);
        self.recovery_epoch
    }

    fn mark_exhausted(&mut self, generation: u64) {
        self.failed_generation = None;
        self.exhausted_generation = Some(generation);
        self.status = TransportStatus::Exhausted;
    }

    fn recover_to_healthy_if_unowned(&mut self, epoch: u64) {
        if self.status == TransportStatus::Recovering
            && self.recovery_epoch == epoch
            && self.generation.is_some()
        {
            self.status = TransportStatus::Healthy;
        }
    }

    fn validate_report(&self, report_gen: u64) -> ReportValidation {
        if self.status == TransportStatus::Closed {
            return ReportValidation::Closed;
        }
        if let Some(ref current) = self.generation {
            if report_gen < current.id {
                return ReportValidation::Stale(current.session.clone());
            }
            if report_gen > current.id {
                return ReportValidation::FutureGeneration(format!(
                    "invalid future generation report: reported {}, current is {}",
                    report_gen, current.id
                ));
            }
        }
        if let Some(exhausted) = self.exhausted_generation
            && report_gen == exhausted
        {
            return ReportValidation::Exhausted;
        }
        ReportValidation::Valid
    }

    /// Atomically publishes a candidate session, advancing generation and returning the old session for teardown.
    fn try_publish_candidate(
        &mut self,
        new_session: Arc<dyn RealtimeSession>,
        expected_revision: u64,
        originating_gen: Option<u64>,
    ) -> Result<(u64, Option<Arc<dyn RealtimeSession>>)> {
        if self.config.revision != expected_revision {
            return Err(RealtimeError::config("stale config revision"));
        }
        if self.status != TransportStatus::Healthy && self.status != TransportStatus::Recovering {
            return Err(RealtimeError::NotConnected);
        }
        if let (Some(expected), Some(current)) = (originating_gen, &self.generation)
            && current.id != expected
        {
            return Err(RealtimeError::provider(
                "active generation changed during candidate preparation",
            ));
        }

        let next_gen = self.next_generation_id;
        self.next_generation_id = next_gen.saturating_add(1);
        let old_session = self.generation.take().map(|g| g.session);

        self.generation = Some(SessionGeneration { id: next_gen, session: new_session });
        self.status = TransportStatus::Healthy;
        self.failed_generation = None;

        Ok((next_gen, old_session))
    }
}

/// The outcome of a recovery report.
#[derive(Clone)]
pub(crate) enum RecoveryOutcome {
    /// A newly recovered session was successfully established and published.
    Recovered { session: Arc<dyn RealtimeSession>, continuity: RecoveryContinuity },
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
            Self::Recovered { session, continuity } => f
                .debug_struct("Recovered")
                .field("session_id", &session.session_id())
                .field("continuity", continuity)
                .finish(),
            Self::Stale(session) => {
                f.debug_struct("Stale").field("session_id", &session.session_id()).finish()
            }
            Self::Exhausted => f.debug_struct("Exhausted").finish(),
        }
    }
}

/// The private recovery supervisor.
pub(crate) struct RecoverySupervisor {
    policy: RecoveryPolicy,
    state: Arc<tokio::sync::RwLock<SupervisorState>>,
    replacement_lock: Arc<tokio::sync::Mutex<()>>,
    generation_tx: tokio::sync::watch::Sender<u64>,
    #[cfg(any(test, feature = "recovery-test-utils"))]
    test_recovery_barrier:
        Arc<parking_lot::Mutex<Option<Arc<crate::recovery::TestRecoveryBarrier>>>>,
}

impl RecoverySupervisor {
    /// Create a new recovery supervisor.
    pub(crate) fn new(
        policy: RecoveryPolicy,
        initial_config: crate::config::RealtimeConfig,
    ) -> Self {
        let (generation_tx, _) = tokio::sync::watch::channel(0);
        Self {
            policy,
            state: Arc::new(tokio::sync::RwLock::new(SupervisorState {
                status: TransportStatus::Uninitialized,
                generation: None,
                next_generation_id: 0,
                recovery_epoch: 0,
                exhausted_generation: None,
                failed_generation: None,
                config: ConfigSnapshot { config: initial_config, revision: 0 },
            })),
            replacement_lock: Arc::new(tokio::sync::Mutex::new(())),
            generation_tx,
            #[cfg(any(test, feature = "recovery-test-utils"))]
            test_recovery_barrier: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    /// Helper constructor for tests or initial session setup.
    #[cfg(test)]
    pub(crate) fn with_initial_session(
        policy: RecoveryPolicy,
        config: Arc<tokio::sync::RwLock<crate::config::RealtimeConfig>>,
        initial_session: Arc<dyn RealtimeSession>,
    ) -> Self {
        let initial_cfg = match config.try_read() {
            Ok(guard) => guard.clone(),
            Err(_) => crate::config::RealtimeConfig::default(),
        };
        let (generation_tx, _) = tokio::sync::watch::channel(0);
        Self {
            policy,
            state: Arc::new(tokio::sync::RwLock::new(SupervisorState {
                status: TransportStatus::Healthy,
                generation: Some(SessionGeneration { id: 0, session: initial_session }),
                next_generation_id: 1,
                recovery_epoch: 0,
                exhausted_generation: None,
                failed_generation: None,
                config: ConfigSnapshot { config: initial_cfg, revision: 0 },
            })),
            replacement_lock: Arc::new(tokio::sync::Mutex::new(())),
            generation_tx,
            #[cfg(any(test, feature = "recovery-test-utils"))]
            test_recovery_barrier: Arc::new(parking_lot::Mutex::new(None)),
        }
    }

    /// Set an integration test recovery barrier to hold managed recovery in `TransportStatus::Recovering`.
    #[cfg(any(test, feature = "recovery-test-utils"))]
    pub(crate) fn set_recovery_barrier_for_testing(
        &self,
        barrier: Arc<crate::recovery::TestRecoveryBarrier>,
    ) {
        *self.test_recovery_barrier.lock() = Some(barrier);
    }

    async fn read_state_at(
        &self,
        deadline: tokio::time::Instant,
        err_msg: &str,
    ) -> Result<tokio::sync::RwLockReadGuard<'_, SupervisorState>> {
        tokio::time::timeout_at(deadline, self.state.read())
            .await
            .map_err(|_| RealtimeError::Timeout(err_msg.to_string()))
    }

    async fn write_state_at(
        &self,
        deadline: tokio::time::Instant,
        err_msg: &str,
    ) -> Result<tokio::sync::RwLockWriteGuard<'_, SupervisorState>> {
        tokio::time::timeout_at(deadline, self.state.write())
            .await
            .map_err(|_| RealtimeError::Timeout(err_msg.to_string()))
    }

    async fn lock_replacement_at(
        &self,
        deadline: tokio::time::Instant,
        err_msg: &str,
    ) -> Result<tokio::sync::MutexGuard<'_, ()>> {
        tokio::time::timeout_at(deadline, self.replacement_lock.lock())
            .await
            .map_err(|_| RealtimeError::Timeout(err_msg.to_string()))
    }

    /// Install the initial session (generation 0).
    ///
    /// Valid ONLY when supervisor status is strictly `Uninitialized`.
    pub(crate) async fn set_initial_session(
        &self,
        session: Arc<dyn RealtimeSession>,
    ) -> Result<SessionGeneration> {
        let mut state = self.state.write().await;
        if state.status != TransportStatus::Uninitialized {
            return Err(RealtimeError::config(
                "cannot set initial session unless supervisor status is strictly Uninitialized",
            ));
        }
        let gen_id = state.next_generation_id;
        state.next_generation_id = gen_id.saturating_add(1);
        let sg_item = SessionGeneration { id: gen_id, session };
        state.generation = Some(sg_item.clone());
        state.status = TransportStatus::Healthy;
        let _ = self.generation_tx.send(gen_id);
        Ok(sg_item)
    }

    /// Retrieve a watcher for generation change signals.
    pub(crate) fn subscribe_generation(&self) -> tokio::sync::watch::Receiver<u64> {
        self.generation_tx.subscribe()
    }

    /// Check current transport status.
    pub(crate) async fn status(&self) -> TransportStatus {
        self.state.read().await.status
    }

    /// Check if currently connected and healthy.
    pub(crate) async fn is_connected(&self) -> bool {
        self.state
            .read()
            .await
            .admittable_generation()
            .map(|g| g.session.is_connected())
            .unwrap_or(false)
    }

    /// Retrieve current active session snapshot if healthy.
    pub(crate) async fn get_active_generation(&self) -> Result<SessionGeneration> {
        self.state.read().await.admittable_generation()
    }

    /// Admit a write operation before invoking the raw session.
    pub(crate) async fn admit_write(&self) -> Result<SessionGeneration> {
        self.state.read().await.admittable_generation()
    }

    /// Mutate the canonical configuration atomically and return the new snapshot.
    pub(crate) async fn update_config<F>(&self, mutate: F) -> ConfigSnapshot
    where
        F: FnOnce(&mut crate::config::RealtimeConfig),
    {
        let mut state = self.state.write().await;
        mutate(&mut state.config.config);
        state.config.revision = state.config.revision.wrapping_add(1);
        state.config.clone()
    }

    /// Get coherent configuration and revision snapshot.
    pub(crate) async fn get_config(&self) -> ConfigSnapshot {
        self.state.read().await.config.clone()
    }

    /// Atomically publish a newly created replacement session.
    pub(crate) async fn publish_replacement(
        &self,
        new_session: Arc<dyn RealtimeSession>,
        expected_revision: u64,
    ) -> Result<SessionGeneration> {
        let _lock = self.replacement_lock.lock().await;
        let mut candidate_guard = CandidateCleanupGuard::new(new_session.clone());

        let mut state = self.state.write().await;
        if state.status == TransportStatus::Closed || state.status == TransportStatus::Exhausted {
            return Err(RealtimeError::SessionClosed);
        }

        let (next_gen, old_session) = state
            .try_publish_candidate(new_session.clone(), expected_revision, None)
            .map_err(|e| {
                if let RealtimeError::ConfigError(_) = e {
                    tracing::warn!(
                        expected = expected_revision,
                        current = state.config.revision,
                        "rejecting replacement publication due to stale config revision"
                    );
                    RealtimeError::config("stale config revision during publication")
                } else {
                    e
                }
            })?;

        let _ = self.generation_tx.send(next_gen);
        candidate_guard.disarm();

        if let Some(old) = old_session {
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(2), old.close()).await;
            });
        }

        Ok(SessionGeneration { id: next_gen, session: new_session })
    }

    /// Execute an intentional context resumption under the unified replacement lock.
    pub(crate) async fn execute_context_resumption(
        &self,
        model: &crate::model::BoxedModel,
        queued_snapshot: ConfigSnapshot,
    ) -> Result<SessionGeneration> {
        let _lock = self.replacement_lock.lock().await;

        {
            let state = self.state.read().await;
            if state.status == TransportStatus::Closed || state.status == TransportStatus::Exhausted
            {
                return Err(RealtimeError::SessionClosed);
            }
            if state.config.revision != queued_snapshot.revision {
                return Err(RealtimeError::config(
                    "stale config revision prior to resumption connect",
                ));
            }
        }

        tracing::info!(
            "Executing intentional context resumption under supervisor replacement lock."
        );
        let new_session = model.connect(queued_snapshot.config).await?;

        let mut state = self.state.write().await;
        if state.status == TransportStatus::Closed || state.status == TransportStatus::Exhausted {
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(2), new_session.close()).await;
            });
            return Err(RealtimeError::SessionClosed);
        }

        if state.config.revision != queued_snapshot.revision {
            tracing::warn!(
                expected = queued_snapshot.revision,
                current = state.config.revision,
                "rejecting context resumption publication due to stale config revision"
            );
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(2), new_session.close()).await;
            });
            return Err(RealtimeError::config("stale config revision post-resumption connect"));
        }

        let next_gen = state.next_generation_id;
        state.next_generation_id = next_gen.saturating_add(1);
        let old_session = state.generation.take().map(|g| g.session);

        let sg_item = SessionGeneration { id: next_gen, session: Arc::from(new_session) };
        state.generation = Some(sg_item.clone());
        state.status = TransportStatus::Healthy;

        let _ = self.generation_tx.send(next_gen);

        if let Some(old) = old_session {
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(2), old.close()).await;
            });
        }

        Ok(sg_item)
    }

    /// Execute a proactive planned replacement (make-before-break).
    ///
    /// Unlike ordinary failure recovery (`report_failure`), planned replacement keeps
    /// generation N in `TransportStatus::Healthy` and write-admissible via `admit_write()`
    /// while candidate N+1 is being prepared. N+1 is atomically published only after
    /// reaching provider setup readiness (`setupComplete`), and session N is closed only
    /// after N+1 publication. If candidate creation fails while N is still healthy, N remains
    /// authoritative and healthy.
    #[allow(clippy::collapsible_if)]
    pub(crate) async fn execute_planned_replacement(
        &self,
        report_generation: u64,
        cause: RecoveryCause,
    ) -> Result<RecoveryOutcome> {
        let start_time = tokio::time::Instant::now();
        let deadline_duration = self.policy.deadline();
        let deadline_instant = start_time
            .checked_add(deadline_duration)
            .unwrap_or_else(|| start_time + Duration::from_secs(86400 * 365));

        // 1. Check current state and validation
        {
            let state_guard = self
                .read_state_at(
                    deadline_instant,
                    "deadline expired acquiring state read snapshot for planned replacement",
                )
                .await?;

            if state_guard.status != TransportStatus::Healthy {
                tracing::info!(
                    generation = report_generation,
                    status = ?state_guard.status,
                    "planned replacement ignored: supervisor not in Healthy status"
                );
                return Err(RealtimeError::NotConnected);
            }

            match state_guard.validate_report(report_generation) {
                ReportValidation::Closed => return Err(RealtimeError::SessionClosed),
                ReportValidation::Stale(session) => {
                    tracing::info!(
                        report_gen = report_generation,
                        "stale planned replacement report ignored"
                    );
                    return Ok(RecoveryOutcome::Stale(session));
                }
                ReportValidation::FutureGeneration(msg) => {
                    return Err(RealtimeError::provider(msg));
                }
                ReportValidation::Exhausted => {
                    return Ok(RecoveryOutcome::Exhausted);
                }
                ReportValidation::Valid => {
                    if state_guard.generation.is_none() {
                        return Err(RealtimeError::NotConnected);
                    }
                }
            }
        }

        // 2. Lock wait capped by remaining deadline
        let _lock_guard = self
            .lock_replacement_at(deadline_instant, "deadline expired waiting for replacement lock")
            .await?;

        // 3. Double-check state now that we hold the replacement lock.
        // DO NOT transition to Recovering! Stay in Healthy so Generation N remains write-admissible during replacement!
        let (session_to_rotate, effective_config, snapshot_revision) = {
            let state_guard = self
                .read_state_at(
                    deadline_instant,
                    "deadline expired acquiring state read snapshot after replacement lock",
                )
                .await?;

            if state_guard.status != TransportStatus::Healthy {
                return Err(RealtimeError::NotConnected);
            }

            let gen_item = match &state_guard.generation {
                Some(g) => g,
                None => return Err(RealtimeError::NotConnected),
            };

            match state_guard.validate_report(report_generation) {
                ReportValidation::Closed => return Err(RealtimeError::SessionClosed),
                ReportValidation::Stale(session) => {
                    tracing::info!(
                        report_gen = report_generation,
                        "coalesced planned replacement report ignored after lock acquisition"
                    );
                    return Ok(RecoveryOutcome::Stale(session));
                }
                ReportValidation::FutureGeneration(_) => {
                    return Err(RealtimeError::provider(
                        "invalid future generation report after lock acquisition",
                    ));
                }
                ReportValidation::Exhausted => {
                    return Ok(RecoveryOutcome::Exhausted);
                }
                ReportValidation::Valid => {}
            }

            (
                gen_item.session.clone(),
                state_guard.config.config.clone(),
                state_guard.config.revision,
            )
        };

        // 4. Retrieve recovery implementation
        let recovery_impl = match session_to_rotate.recovery() {
            Some(r) => r,
            None => {
                let err = RealtimeError::provider("active session does not support recovery");
                tracing::error!(generation = report_generation, err = %err, "planned replacement recovery not supported");
                return Err(err);
            }
        };

        // 5. Pre-flight cause classification
        if recovery_impl.classify(&cause) == RecoveryDisposition::Fatal {
            let err = RealtimeError::provider(
                "fatal cause detected for planned replacement; performing zero provider attempts",
            );
            tracing::warn!(generation = report_generation, err = %err, "fatal cause classification for planned replacement");
            return Err(err);
        }

        // 6. Build candidate N+1
        tracing::info!(
            generation = report_generation,
            "planned replacement candidate attempt initiated"
        );

        let now = tokio::time::Instant::now();
        if now >= deadline_instant {
            let msg = "planned replacement deadline expired before attempt".to_string();
            tracing::warn!(generation = report_generation, err = %msg, "planned replacement deadline expired");
            return Err(RealtimeError::Timeout(msg));
        }

        let remaining_dur = deadline_instant.saturating_duration_since(now);
        let context_deadline = std::time::Instant::now()
            .checked_add(remaining_dur)
            .unwrap_or_else(std::time::Instant::now);

        let context = RecoveryContext::new(
            NonZeroU32::new(1).unwrap(),
            &cause,
            &effective_config,
            context_deadline,
        );

        let recover_fut = recovery_impl.recover(context);
        let attempt_res = match tokio::time::timeout_at(deadline_instant, recover_fut).await {
            Ok(res) => res,
            Err(_) => {
                let msg =
                    "planned replacement attempt timed out waiting for setupComplete".to_string();
                tracing::warn!(generation = report_generation, err = %msg, "planned replacement attempt timed out");
                return Err(RealtimeError::Timeout(msg));
            }
        };

        match attempt_res {
            Ok(recovered) => {
                tracing::info!(
                    generation = report_generation,
                    continuity = ?recovered.continuity(),
                    "planned replacement candidate ready"
                );

                let new_session = recovered.session();
                let continuity = recovered.continuity();
                let mut candidate_guard = CandidateCleanupGuard::new(new_session.clone());

                let mut state_guard = self
                    .write_state_at(
                        deadline_instant,
                        "planned replacement deadline expired acquiring state write lock for candidate publication",
                    )
                    .await?;

                let (next_gen, old_session) = match state_guard.try_publish_candidate(
                    new_session.clone(),
                    snapshot_revision,
                    Some(report_generation),
                ) {
                    Ok(res) => res,
                    Err(RealtimeError::ProviderError(ref msg))
                        if msg.contains("active generation changed") =>
                    {
                        let current_gen_session = state_guard
                            .generation
                            .as_ref()
                            .map(|g| g.session.clone())
                            .unwrap_or_else(|| new_session.clone());
                        return Ok(RecoveryOutcome::Stale(current_gen_session));
                    }
                    Err(e) => return Err(e),
                };

                let _ = self.generation_tx.send(next_gen);
                candidate_guard.disarm();

                if let Some(old) = old_session {
                    tokio::spawn(async move {
                        let _ = tokio::time::timeout(Duration::from_secs(2), old.close()).await;
                    });
                }

                tracing::info!(
                    generation = next_gen,
                    session_id = %new_session.session_id(),
                    continuity = ?continuity,
                    "planned replacement published"
                );

                Ok(RecoveryOutcome::Recovered { session: new_session, continuity })
            }
            Err(err) => {
                tracing::warn!(
                    generation = report_generation,
                    err = %err,
                    "proactive planned replacement attempt failed; keeping generation N authoritative and healthy"
                );
                Err(err)
            }
        }
    }

    /// Report a failure in the active session.
    #[allow(clippy::collapsible_if)]
    pub(crate) async fn report_failure(&self, report: FailureReport) -> Result<RecoveryOutcome> {
        let start_time = tokio::time::Instant::now();
        let deadline_duration = self.policy.deadline();
        let deadline_instant = start_time
            .checked_add(deadline_duration)
            .unwrap_or_else(|| start_time + Duration::from_secs(86400 * 365));

        // 1. Mark generation failed (durable: not cleared on cancellation or timeout)
        {
            let mut state_guard = self
                .write_state_at(deadline_instant, "deadline expired acquiring state read snapshot")
                .await?;

            match state_guard.validate_report(report.generation) {
                ReportValidation::Closed => return Err(RealtimeError::SessionClosed),
                ReportValidation::Stale(session) => {
                    tracing::info!(
                        report_gen = report.generation,
                        "stale/coalesced failure report ignored"
                    );
                    return Ok(RecoveryOutcome::Stale(session));
                }
                ReportValidation::FutureGeneration(msg) => {
                    tracing::warn!(
                        report_gen = report.generation,
                        err = %msg,
                        "future generation report rejected"
                    );
                    return Err(RealtimeError::provider(msg));
                }
                ReportValidation::Exhausted => {
                    tracing::info!(
                        generation = report.generation,
                        "report for already exhausted generation"
                    );
                    return Ok(RecoveryOutcome::Exhausted);
                }
                ReportValidation::Valid => {
                    state_guard.mark_failed(report.generation);
                    tracing::info!(
                        generation = report.generation,
                        "Active generation marked failed; writes blocked pending recovery completion"
                    );
                }
            }
        }

        // 2. Execute recovery transaction directly
        Self::execute_recovery_transaction_internal(
            report,
            deadline_instant,
            Arc::clone(&self.state),
            Arc::clone(&self.replacement_lock),
            self.generation_tx.clone(),
            self.policy.clone(),
            #[cfg(any(test, feature = "recovery-test-utils"))]
            Arc::clone(&self.test_recovery_barrier),
        )
        .await
    }

    async fn execute_recovery_transaction_internal(
        report: FailureReport,
        deadline_instant: tokio::time::Instant,
        state: Arc<tokio::sync::RwLock<SupervisorState>>,
        replacement_lock: Arc<tokio::sync::Mutex<()>>,
        generation_tx: tokio::sync::watch::Sender<u64>,
        policy: RecoveryPolicy,
        #[cfg(any(test, feature = "recovery-test-utils"))] test_recovery_barrier: Arc<
            parking_lot::Mutex<Option<Arc<crate::recovery::TestRecoveryBarrier>>>,
        >,
    ) -> Result<RecoveryOutcome> {
        // 1. Lock wait capped by remaining deadline
        let _lock_guard = match tokio::time::timeout_at(deadline_instant, replacement_lock.lock())
            .await
        {
            Ok(guard) => guard,
            Err(_) => {
                let err = RealtimeError::Timeout(
                    "deadline expired waiting for recovery lock".to_string(),
                );
                tracing::warn!(generation = report.generation, err = %err, "recovery lock timeout");
                return Err(err);
            }
        };

        // 2. Double-check state now that we hold the lock and transition to Recovering
        let recovery_epoch = {
            let mut state_guard =
                match tokio::time::timeout_at(deadline_instant, state.write()).await {
                    Ok(guard) => guard,
                    Err(_) => {
                        return Err(RealtimeError::Timeout(
                            "deadline expired acquiring state write snapshot".to_string(),
                        ));
                    }
                };

            match state_guard.validate_report(report.generation) {
                ReportValidation::Closed => {
                    return Err(RealtimeError::SessionClosed);
                }
                ReportValidation::Stale(session) => {
                    tracing::info!(
                        report_gen = report.generation,
                        "coalesced failure report ignored after lock acquisition"
                    );
                    return Ok(RecoveryOutcome::Stale(session));
                }
                ReportValidation::FutureGeneration(_) => {
                    return Err(RealtimeError::provider(
                        "invalid future generation report after lock acquisition",
                    ));
                }
                ReportValidation::Exhausted => {
                    tracing::info!(
                        generation = report.generation,
                        "report for already exhausted generation after lock acquisition"
                    );
                    return Ok(RecoveryOutcome::Exhausted);
                }
                ReportValidation::Valid => {}
            }

            state_guard.promote_to_recovering()
        };

        let mut episode_guard = RecoveryEpisodeGuard::new(Arc::clone(&state), recovery_epoch);

        // 3. Determine recovery implementation
        let session_to_recover = {
            let state_guard = match tokio::time::timeout_at(deadline_instant, state.read()).await {
                Ok(guard) => guard,
                Err(_) => {
                    return Err(RealtimeError::Timeout(
                        "deadline expired acquiring state read snapshot".to_string(),
                    ));
                }
            };
            match &state_guard.generation {
                Some(gen_item) => gen_item.session.clone(),
                None => {
                    return Err(RealtimeError::NotConnected);
                }
            }
        };

        let recovery_impl = match session_to_recover.recovery() {
            Some(r) => r,
            None => {
                let err = RealtimeError::provider("active session does not support recovery");
                tracing::error!(generation = report.generation, err = %err, "recovery not supported");
                let mut state_guard = state.write().await;
                state_guard.mark_exhausted(report.generation);
                episode_guard.disarm();
                return Err(err);
            }
        };

        // 4. Pre-flight cause classification
        if recovery_impl.classify(&report.cause) == RecoveryDisposition::Fatal {
            let err = RealtimeError::provider(
                "fatal recovery cause detected; performing zero provider attempts",
            );
            tracing::warn!(generation = report.generation, err = %err, "fatal cause classification");
            let mut state_guard = state.write().await;
            state_guard.mark_exhausted(report.generation);
            episode_guard.disarm();
            return Err(err);
        }

        #[cfg(any(test, feature = "recovery-test-utils"))]
        {
            let maybe_barrier = test_recovery_barrier.lock().take();
            if let Some(barrier) = maybe_barrier {
                barrier.on_recovering().await;
            }
        }

        // 5. Recovery attempt loop
        tracing::info!(generation = report.generation, "recovery episode started");
        enum EpisodeStopReason {
            Exhausted,
            Fatal,
            Timeout(String),
        }

        let mut stop_reason = EpisodeStopReason::Exhausted;
        let mut last_provider_error: Option<RealtimeError> = None;
        let max_attempts = policy.max_attempts().get();
        let mut attempts_launched = 0;

        for attempt_idx in 1..=max_attempts {
            let now = tokio::time::Instant::now();
            if now >= deadline_instant {
                let msg = "recovery deadline expired before attempt".to_string();
                tracing::warn!(generation = report.generation, attempt = attempt_idx, err = %msg, "recovery deadline expired");
                stop_reason = EpisodeStopReason::Timeout(msg);
                break;
            }

            let attempt_nz = NonZeroU32::new(attempt_idx).unwrap();

            let state_guard = match tokio::time::timeout_at(deadline_instant, state.read()).await {
                Ok(guard) => guard,
                Err(_) => {
                    let msg = "recovery deadline expired acquiring state read snapshot".to_string();
                    tracing::warn!(generation = report.generation, attempt = attempt_idx, err = %msg, "recovery deadline expired");
                    stop_reason = EpisodeStopReason::Timeout(msg);
                    break;
                }
            };

            let effective_config = state_guard.config.config.clone();
            let snapshot_revision = state_guard.config.revision;
            drop(state_guard);

            if tokio::time::Instant::now() >= deadline_instant {
                let msg = "recovery deadline expired after acquiring state snapshot".to_string();
                tracing::warn!(generation = report.generation, attempt = attempt_idx, err = %msg, "recovery deadline expired");
                stop_reason = EpisodeStopReason::Timeout(msg);
                break;
            }

            attempts_launched += 1;

            let now_after_config = tokio::time::Instant::now();
            let remaining_dur = deadline_instant.saturating_duration_since(now_after_config);
            let context_deadline = std::time::Instant::now()
                .checked_add(remaining_dur)
                .unwrap_or_else(std::time::Instant::now);

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
            let attempt_res = match tokio::time::timeout_at(deadline_instant, recover_fut).await {
                Ok(res) => res,
                Err(_) => {
                    let msg = "provider recovery attempt timed out".to_string();
                    tracing::warn!(generation = report.generation, attempt = attempt_idx, err = %msg, "recovery attempt timed out");
                    stop_reason = EpisodeStopReason::Timeout(msg);
                    break;
                }
            };

            match attempt_res {
                Ok(recovered) => {
                    tracing::info!(
                        generation = report.generation,
                        attempt = attempt_idx,
                        continuity = ?recovered.continuity(),
                        "successful candidate ready"
                    );

                    let new_session = recovered.session();
                    let continuity = recovered.continuity();
                    let mut candidate_guard = CandidateCleanupGuard::new(new_session.clone());

                    let mut state_guard = match tokio::time::timeout_at(
                        deadline_instant,
                        state.write(),
                    )
                    .await
                    {
                        Ok(guard) => guard,
                        Err(_) => {
                            let msg = "recovery deadline expired acquiring state write lock for candidate publication".to_string();
                            tracing::warn!(generation = report.generation, attempt = attempt_idx, err = %msg, "recovery deadline expired");
                            stop_reason = EpisodeStopReason::Timeout(msg);
                            break;
                        }
                    };

                    let (next_gen, old_session) = match state_guard.try_publish_candidate(
                        new_session.clone(),
                        snapshot_revision,
                        None,
                    ) {
                        Ok(res) => res,
                        Err(RealtimeError::ConfigError(_)) => {
                            tracing::warn!(
                                expected = snapshot_revision,
                                current = state_guard.config.revision,
                                "recovery candidate rejected due to stale config revision during attempt"
                            );
                            last_provider_error =
                                Some(RealtimeError::config("stale config revision"));
                            continue;
                        }
                        Err(e) => {
                            last_provider_error = Some(e);
                            continue;
                        }
                    };

                    let _ = generation_tx.send(next_gen);
                    candidate_guard.disarm();

                    if let Some(old) = old_session {
                        tokio::spawn(async move {
                            let _ = tokio::time::timeout(Duration::from_secs(2), old.close()).await;
                        });
                    }

                    tracing::info!(
                        generation = next_gen,
                        session_id = %new_session.session_id(),
                        continuity = ?continuity,
                        "recovered session published"
                    );

                    episode_guard.disarm();
                    return Ok(RecoveryOutcome::Recovered { session: new_session, continuity });
                }
                Err(err) => {
                    tracing::error!(
                        generation = report.generation,
                        attempt = attempt_idx,
                        err = %err,
                        "recovery attempt failed"
                    );

                    let disposition = recovery_impl.classify_attempt_error(&err);
                    last_provider_error = Some(err);

                    if disposition == RecoveryDisposition::Fatal {
                        tracing::warn!(
                            generation = report.generation,
                            attempt = attempt_idx,
                            "fatal attempt error classification; no retry"
                        );
                        stop_reason = EpisodeStopReason::Fatal;
                        break;
                    }

                    if attempt_idx < max_attempts {
                        let factor = 2u32.checked_pow(attempt_idx - 1).unwrap_or(u32::MAX);
                        let mut backoff = policy.initial_delay().saturating_mul(factor);
                        backoff = backoff.min(policy.max_delay());

                        let now_after_attempt = tokio::time::Instant::now();
                        if now_after_attempt >= deadline_instant {
                            let msg =
                                "recovery deadline expired during backoff calculation".to_string();
                            tracing::warn!(generation = report.generation, err = %msg, "recovery deadline expired");
                            stop_reason = EpisodeStopReason::Timeout(msg);
                            break;
                        }

                        let remaining_after_attempt =
                            deadline_instant.saturating_duration_since(now_after_attempt);
                        backoff = backoff.min(remaining_after_attempt);

                        if !backoff.is_zero() {
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
        }

        let final_err = match stop_reason {
            EpisodeStopReason::Timeout(msg) => {
                let full_msg = if let Some(ref provider_err) = last_provider_error {
                    format!("{}; last provider error: {}", msg, provider_err)
                } else {
                    msg
                };
                RealtimeError::Timeout(full_msg)
            }
            EpisodeStopReason::Fatal => {
                if let Some(ref provider_err) = last_provider_error {
                    RealtimeError::ProviderError(format!(
                        "recovery aborted after {} attempt(s) due to fatal error; last error: {}",
                        attempts_launched, provider_err
                    ))
                } else {
                    RealtimeError::ProviderError(format!(
                        "recovery aborted after {} attempt(s) due to fatal error",
                        attempts_launched
                    ))
                }
            }
            EpisodeStopReason::Exhausted => {
                if let Some(ref provider_err) = last_provider_error {
                    RealtimeError::ProviderError(format!(
                        "recovery exhausted after {} attempt(s); last error: {}",
                        attempts_launched, provider_err
                    ))
                } else {
                    RealtimeError::ProviderError(format!(
                        "recovery exhausted after {} attempt(s)",
                        attempts_launched
                    ))
                }
            }
        };

        tracing::error!(
            generation = report.generation,
            err = %final_err,
            "recovery exhausted or fatal"
        );

        let mut state_guard = state.write().await;
        state_guard.mark_exhausted(report.generation);
        episode_guard.disarm();

        Err(final_err)
    }

    /// Close the session gracefully and mark supervisor as closed.
    pub(crate) async fn close(&self) -> Result<()> {
        let _lock = self.replacement_lock.lock().await;

        let old_session = {
            let mut state = self.state.write().await;
            state.status = TransportStatus::Closed;
            state.generation.take().map(|g| g.session)
        };

        let _ = self.generation_tx.send(u64::MAX);

        if let Some(session) = old_session {
            let _ = tokio::time::timeout(Duration::from_secs(2), session.close()).await;
        }

        Ok(())
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
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

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

        assert_eq!(recover_count.load(Ordering::SeqCst), 1);
        assert_eq!(max_active_recoveries.load(Ordering::SeqCst), 1);

        let mut success_count = 0;
        for res in results {
            match res.unwrap() {
                RecoveryOutcome::Recovered { session, continuity } => {
                    success_count += 1;
                    assert_eq!(session.session_id(), "gen-1-recovered");
                    assert_eq!(continuity, RecoveryContinuity::Resumed);
                }
                RecoveryOutcome::Stale(session) => {
                    success_count += 1;
                    assert_eq!(session.session_id(), "gen-1-recovered");
                }
                RecoveryOutcome::Exhausted => {
                    panic!("Expected Recovered or Stale, got Exhausted");
                }
            }
        }
        assert_eq!(success_count, 3);
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
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res1 = supervisor.report_failure(report).await.unwrap();
        assert!(matches!(res1, RecoveryOutcome::Recovered { .. }));

        {
            let sg_item = supervisor.get_active_generation().await.unwrap();
            assert_eq!(sg_item.id, 1);
        }

        recover_count.store(0, Ordering::SeqCst);

        let delayed_report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res2 = supervisor.report_failure(delayed_report).await.unwrap();

        match res2 {
            RecoveryOutcome::Stale(session) => {
                assert_eq!(session.session_id(), "gen-1-recovered");
            }
            _ => panic!("Expected Stale outcome, got {:?}", res2),
        }
        assert_eq!(recover_count.load(Ordering::SeqCst), 0);

        let sg_item = supervisor.get_active_generation().await.unwrap();
        assert_eq!(sg_item.id, 1);
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
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

        let report = FailureReport { generation: 1, cause: RecoveryCause::UnexpectedEof };

        let res = supervisor.report_failure(report).await;
        assert!(res.is_err());
        assert_eq!(recover_count.load(Ordering::SeqCst), 0);
        let sg_item = supervisor.get_active_generation().await.unwrap();
        assert_eq!(sg_item.id, 0);
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
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

        let report1 = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res1 = supervisor.report_failure(report1).await;
        assert!(res1.is_err());
        assert_eq!(recover_count.load(Ordering::SeqCst), 3);

        let report2 = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res2 = supervisor.report_failure(report2).await;
        assert!(res2.is_ok());
        let outcome = res2.unwrap();
        assert!(matches!(outcome, RecoveryOutcome::Exhausted));
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
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

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
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

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
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

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
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

        let start_instant = tokio::time::Instant::now();

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

        let res = supervisor.report_failure(report).await.unwrap();
        match res {
            RecoveryOutcome::Recovered { session, continuity } => {
                assert_eq!(session.session_id(), "gen-1-finally");
                assert_eq!(continuity, RecoveryContinuity::Resumed);
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

    #[tokio::test]
    async fn test_zero_initial_delay_retries_immediately() {
        let recover_count = Arc::new(AtomicUsize::new(0));

        struct ZeroDelayRecovery {
            recover_count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl RealtimeRecovery for ZeroDelayRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            fn classify_attempt_error(&self, _error: &RealtimeError) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                self.recover_count.fetch_add(1, Ordering::SeqCst);
                if context.attempt().get() == 3 {
                    let session = Arc::new(MockSession {
                        id: "recovered-zero-delay".to_string(),
                        recovery: None,
                    });
                    Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
                } else {
                    Err(RealtimeError::ConnectionError("retry immediately".to_string()))
                }
            }
        }

        let mock_rec = Arc::new(ZeroDelayRecovery { recover_count: Arc::clone(&recover_count) });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_initial_delay(Duration::ZERO)
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

        let res = supervisor.report_failure(report).await.unwrap();
        match res {
            RecoveryOutcome::Recovered { session, .. } => {
                assert_eq!(session.session_id(), "recovered-zero-delay");
            }
            _ => panic!("Expected Recovered outcome"),
        }

        assert_eq!(recover_count.load(Ordering::SeqCst), 3);
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
        let supervisor =
            RecoverySupervisor::with_initial_session(policy, config, initial_session.clone());

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res = supervisor.report_failure(report).await;

        assert!(res.is_err());

        let state_guard = supervisor.state.read().await;
        assert_eq!(state_guard.generation.as_ref().unwrap().id, 0);
        assert_eq!(state_guard.generation.as_ref().unwrap().session.session_id(), "gen-0-active");
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

        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

        supervisor
            .update_config(|cfg| {
                cfg.instruction = Some("mutated-instruction".to_string());
            })
            .await;

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res = supervisor.report_failure(report).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_fatal_error_on_final_allowed_attempt_reports_fatal_not_exhausted() {
        struct FatalOnFinalRecovery {
            recover_count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl RealtimeRecovery for FatalOnFinalRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            fn classify_attempt_error(&self, _error: &RealtimeError) -> RecoveryDisposition {
                if self.recover_count.load(Ordering::SeqCst) == 3 {
                    RecoveryDisposition::Fatal
                } else {
                    RecoveryDisposition::Recoverable
                }
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                let count = self.recover_count.fetch_add(1, Ordering::SeqCst) + 1;
                if count == 3 {
                    Err(RealtimeError::ConnectionError("fatal on 3rd attempt".to_string()))
                } else {
                    Err(RealtimeError::ConnectionError("retryable error".to_string()))
                }
            }
        }

        let recover_count = Arc::new(AtomicUsize::new(0));
        let mock_rec = Arc::new(FatalOnFinalRecovery { recover_count: Arc::clone(&recover_count) });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_initial_delay(Duration::ZERO)
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

        let res = supervisor.report_failure(report).await;
        assert!(res.is_err());
        assert_eq!(recover_count.load(Ordering::SeqCst), 3);

        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("recovery aborted after 3 attempt(s) due to fatal error"),
            "Error message should state fatal abortion after exactly 3 attempts, got: {}",
            err_msg
        );
        assert!(
            err_msg.contains("fatal on 3rd attempt"),
            "Error message should contain last provider error, got: {}",
            err_msg
        );
        assert!(
            !err_msg.contains("recovery exhausted"),
            "Fatal on 3rd attempt should NOT be formatted as ordinary exhaustion, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_planned_replacement_keeps_n_admissible_during_preparation() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let pause_notify = Arc::new(tokio::sync::Notify::new());

        struct PausingRotationRecovery {
            started_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            pause_notify: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl RealtimeRecovery for PausingRotationRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                if let Some(tx) = self.started_tx.lock().take() {
                    let _ = tx.send(());
                }
                self.pause_notify.notified().await;
                let session = Arc::new(MockSession { id: "gen-1-planned".into(), recovery: None });
                Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
            }
        }

        let mock_rec = Arc::new(PausingRotationRecovery {
            started_tx: parking_lot::Mutex::new(Some(started_tx)),
            pause_notify: pause_notify.clone(),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default();
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        let sup_task = Arc::clone(&supervisor);
        let rotation_handle = tokio::spawn(async move {
            let cause = RecoveryCause::PlannedRotation { time_left: Some("10s".into()) };
            sup_task.execute_planned_replacement(0, cause).await
        });

        // Wait until recover() starts building candidate N+1
        started_rx.await.expect("recover should start");

        // Status is STILL Healthy and admit_write() STILL returns Generation 0!
        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
        let admitted = supervisor
            .admit_write()
            .await
            .expect("Generation 0 must remain write-admissible during planned replacement");
        assert_eq!(admitted.id, 0);

        // Release candidate preparation
        pause_notify.notify_one();

        let outcome = rotation_handle.await.unwrap().unwrap();
        match outcome {
            RecoveryOutcome::Recovered { session, .. } => {
                assert_eq!(session.session_id(), "gen-1-planned");
            }
            _ => panic!("Expected Recovered outcome"),
        }

        // Supervisor has now advanced to Generation 1
        let active = supervisor.get_active_generation().await.unwrap();
        assert_eq!(active.id, 1);
        assert_eq!(active.session.session_id(), "gen-1-planned");
    }

    #[tokio::test]
    async fn test_failed_planned_replacement_leaves_n_healthy_and_authoritative() {
        struct FailingRotationRecovery;

        #[async_trait]
        impl RealtimeRecovery for FailingRotationRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                Err(RealtimeError::ConnectionError("proactive connect failed".into()))
            }
        }

        let mock_rec = Arc::new(FailingRotationRecovery);

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default();
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

        let cause = RecoveryCause::PlannedRotation { time_left: None };
        let res = supervisor.execute_planned_replacement(0, cause).await;

        assert!(res.is_err(), "Proactive rotation attempt failed");

        // Generation 0 remains Healthy and authoritative!
        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
        let active = supervisor.get_active_generation().await.unwrap();
        assert_eq!(active.id, 0);
        assert_eq!(active.session.session_id(), "gen-0");
    }

    #[tokio::test]
    async fn test_pre_guard_cancellation_repair() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let pause_recover = Arc::new(tokio::sync::Notify::new());

        struct PausingRecovery {
            started_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            pause_notify: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl RealtimeRecovery for PausingRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                if let Some(tx) = self.started_tx.lock().take() {
                    let _ = tx.send(());
                }
                self.pause_notify.notified().await;
                Err(RealtimeError::ConnectionError("cancelled".to_string()))
            }
        }

        let mock_rec = Arc::new(PausingRecovery {
            started_tx: parking_lot::Mutex::new(Some(started_tx)),
            pause_notify: pause_recover.clone(),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default();
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        let sup_task = Arc::clone(&supervisor);
        let handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_task.report_failure(report).await
        });

        started_rx.await.expect("recover should start and signal");
        assert_eq!(supervisor.status().await, TransportStatus::Recovering);

        // Abort while paused inside recover (after transition to Recovering)
        handle.abort();
        let _ = handle.await;

        // Allow any spawned drop task to acquire lock and run
        let mut restored = false;
        for _ in 0..50 {
            if supervisor.status().await == TransportStatus::Healthy {
                restored = true;
                break;
            }
            tokio::task::yield_now().await;
        }

        assert!(restored, "Status must return to Healthy after cancelled episode");
        assert!(supervisor.admit_write().await.is_ok(), "Old generation becomes usable again");
    }

    #[tokio::test]
    async fn test_post_disarm_terminal_cancellation_repair() {
        let (started_tx, _started_rx) = tokio::sync::oneshot::channel();

        struct ImmediateExhaustRecovery {
            started_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        }

        #[async_trait]
        impl RealtimeRecovery for ImmediateExhaustRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Fatal
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                if let Some(tx) = self.started_tx.lock().take() {
                    let _ = tx.send(());
                }
                Err(RealtimeError::ConnectionError("fatal".to_string()))
            }
        }

        let mock_rec = Arc::new(ImmediateExhaustRecovery {
            started_tx: parking_lot::Mutex::new(Some(started_tx)),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default();
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // Block state write lock so report_failure pauses at terminal state write
        let write_guard = supervisor.state.write().await;

        let sup_task = Arc::clone(&supervisor);
        let handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_task.report_failure(report).await
        });

        // Let task reach write lock wait
        tokio::task::yield_now().await;

        // Abort task while waiting for terminal state publication
        handle.abort();
        let _ = handle.await;

        drop(write_guard);

        let status = supervisor.status().await;
        assert_ne!(
            status,
            TransportStatus::Recovering,
            "Must never remain permanently stuck in Recovering after terminal cancellation"
        );
    }

    #[tokio::test]
    async fn test_cancelled_episode_a_cannot_unlock_episode_b() {
        let (ready_a_tx, ready_a_rx) = tokio::sync::oneshot::channel();
        let (done_a_tx, done_a_rx) = tokio::sync::oneshot::channel();
        let pause_a = Arc::new(tokio::sync::Notify::new());

        struct SignallingRecoveryA {
            ready_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            done_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            pause_notify: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl RealtimeRecovery for SignallingRecoveryA {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                let session = Arc::new(MockSession { id: "gen-1-a".into(), recovery: None });
                if let Some(tx) = self.ready_tx.lock().take() {
                    let _ = tx.send(());
                }
                self.pause_notify.notified().await;
                if let Some(tx) = self.done_tx.lock().take() {
                    let _ = tx.send(());
                }
                Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
            }
        }

        let mock_rec_a = Arc::new(SignallingRecoveryA {
            ready_tx: parking_lot::Mutex::new(Some(ready_a_tx)),
            done_tx: parking_lot::Mutex::new(Some(done_a_tx)),
            pause_notify: pause_a.clone(),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec_a) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default();
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        let sup_task_a = Arc::clone(&supervisor);
        let report_task_a = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_task_a.report_failure(report).await
        });

        ready_a_rx.await.expect("Episode A should yield ready signal");

        let sup_clone = Arc::clone(&supervisor);
        let write_guard = sup_clone.state.write().await;

        pause_a.notify_one();
        done_a_rx.await.expect("Episode A recover must finish");

        report_task_a.abort();
        let _ = report_task_a.await;

        drop(write_guard);

        let (ready_b_tx, ready_b_rx) = tokio::sync::oneshot::channel();
        let pause_b = Arc::new(tokio::sync::Notify::new());

        struct SignallingRecoveryB {
            ready_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            pause_notify: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl RealtimeRecovery for SignallingRecoveryB {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                let session = Arc::new(MockSession { id: "gen-1-b".into(), recovery: None });
                if let Some(tx) = self.ready_tx.lock().take() {
                    let _ = tx.send(());
                }
                self.pause_notify.notified().await;
                Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
            }
        }

        let mock_rec_b = Arc::new(SignallingRecoveryB {
            ready_tx: parking_lot::Mutex::new(Some(ready_b_tx)),
            pause_notify: pause_b.clone(),
        });

        {
            let mut state = supervisor.state.write().await;
            if let Some(ref mut current_gen) = state.generation {
                current_gen.session = Arc::new(MockSession {
                    id: "gen-0".into(),
                    recovery: Some(mock_rec_b as Arc<dyn RealtimeRecovery>),
                });
            }
        }

        let sup_task_b = Arc::clone(&supervisor);
        let report_task_b = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_task_b.report_failure(report).await
        });

        ready_b_rx.await.expect("Episode B should yield ready signal");

        // Explicitly verify episode A's drop task has completed by yielding
        tokio::task::yield_now().await;

        assert_eq!(supervisor.status().await, TransportStatus::Recovering);
        assert!(supervisor.admit_write().await.is_err(), "admit_write remains rejected");

        pause_b.notify_one();
        let outcome_b = report_task_b.await.unwrap().unwrap();

        assert!(matches!(outcome_b, RecoveryOutcome::Recovered { .. }));
        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
    }

    #[tokio::test]
    async fn test_candidate_ready_cancellation_cleans_unpublished_candidate_without_sleeps() {
        let candidate_close_calls = Arc::new(AtomicUsize::new(0));
        let (close_tx, close_rx) = tokio::sync::oneshot::channel();
        let close_tx_slot = Arc::new(parking_lot::Mutex::new(Some(close_tx)));

        struct CandidateSession {
            close_calls: Arc<AtomicUsize>,
            close_tx: Arc<parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
        }

        #[async_trait]
        impl RealtimeSession for CandidateSession {
            fn session_id(&self) -> &str {
                "candidate-session"
            }
            fn is_connected(&self) -> bool {
                true
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
                self.close_calls.fetch_add(1, Ordering::SeqCst);
                if let Some(tx) = self.close_tx.lock().take() {
                    let _ = tx.send(());
                }
                Ok(())
            }
            async fn mutate_context(
                &self,
                _config: crate::config::RealtimeConfig,
            ) -> Result<crate::session::ContextMutationOutcome> {
                Ok(crate::session::ContextMutationOutcome::Applied)
            }
        }

        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let pause_notify = Arc::new(tokio::sync::Notify::new());

        struct ReadySignallingRecovery {
            candidate_close_calls: Arc<AtomicUsize>,
            close_tx: Arc<parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
            ready_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            done_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            pause_notify: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl RealtimeRecovery for ReadySignallingRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                let session = Arc::new(CandidateSession {
                    close_calls: self.candidate_close_calls.clone(),
                    close_tx: self.close_tx.clone(),
                });
                if let Some(tx) = self.ready_tx.lock().take() {
                    let _ = tx.send(());
                }
                self.pause_notify.notified().await;
                if let Some(tx) = self.done_tx.lock().take() {
                    let _ = tx.send(());
                }
                Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
            }
        }

        let mock_rec = Arc::new(ReadySignallingRecovery {
            candidate_close_calls: candidate_close_calls.clone(),
            close_tx: close_tx_slot,
            ready_tx: parking_lot::Mutex::new(Some(ready_tx)),
            done_tx: parking_lot::Mutex::new(Some(done_tx)),
            pause_notify: pause_notify.clone(),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default();
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        let sup_task_clone = Arc::clone(&supervisor);
        let report_task = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_task_clone.report_failure(report).await
        });

        // 1. Wait until recover() prepares candidate and sends ready signal
        ready_rx.await.expect("recover() should yield candidate ready");

        // 2. Acquire supervisor state write lock to block report_failure at candidate publication
        let sup_clone = Arc::clone(&supervisor);
        let write_guard = sup_clone.state.write().await;

        // 3. Unblock recover() so it completes
        pause_notify.notify_one();

        // 4. Wait until recover() finishes returning Ok(RecoveredSession), ensuring CandidateCleanupGuard is constructed in report_failure
        done_rx.await.expect("recover() must complete returning candidate");

        // 5. Abort report_failure task while it is blocked waiting for publication write lock
        report_task.abort();
        let _ = report_task.await;

        // 6. Release supervisor state write lock
        drop(write_guard);

        // 7. Await close_rx: proves CandidateCleanupGuard spawned close() and completed without sleeps
        close_rx.await.expect("CandidateSession::close() must complete on cancellation");

        let sg_item = supervisor.get_active_generation().await.unwrap();
        assert_eq!(sg_item.id, 0);
        assert_eq!(sg_item.session.session_id(), "gen-0");

        assert_eq!(
            candidate_close_calls.load(Ordering::SeqCst),
            1,
            "Candidate session must be closed exactly once when report_failure is cancelled before publication"
        );
    }

    #[tokio::test]
    async fn test_deadline_expiry_during_snapshot_launches_zero_attempts() {
        struct CountingRecovery {
            recover_count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl RealtimeRecovery for CountingRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                self.recover_count.fetch_add(1, Ordering::SeqCst);
                Err(RealtimeError::ConnectionError("should not be called".to_string()))
            }
        }

        let recover_count = Arc::new(AtomicUsize::new(0));
        let mock_rec = Arc::new(CountingRecovery { recover_count: Arc::clone(&recover_count) });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_deadline(Duration::from_millis(50));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        let sup_clone = Arc::clone(&supervisor);
        let write_guard = sup_clone.state.write().await;

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

        let res = sup_clone.report_failure(report).await;

        drop(write_guard);

        assert!(res.is_err());
        let err = res.unwrap_err();
        match err {
            RealtimeError::Timeout(ref msg) => {
                assert!(
                    msg.contains("deadline expired acquiring state read snapshot"),
                    "Expected snapshot timeout message, got: {}",
                    msg
                );
            }
            other => panic!("expected RealtimeError::Timeout, got {:?}", other),
        }

        assert_eq!(
            recover_count.load(Ordering::SeqCst),
            0,
            "Zero provider attempts must be launched"
        );
    }

    #[tokio::test]
    async fn test_timeout_preserves_previous_provider_error() {
        struct SlowTimeoutRecovery {
            recover_count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl RealtimeRecovery for SlowTimeoutRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            fn classify_attempt_error(&self, _error: &RealtimeError) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                let count = self.recover_count.fetch_add(1, Ordering::SeqCst) + 1;
                if count == 1 {
                    Err(RealtimeError::ConnectionError("attempt 1 provider failure".to_string()))
                } else {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    let session = Arc::new(MockSession {
                        id: "should-not-reach".to_string(),
                        recovery: None,
                    });
                    Ok(RecoveredSession::new(session, RecoveryContinuity::Resumed))
                }
            }
        }

        let recover_count = Arc::new(AtomicUsize::new(0));
        let mock_rec = Arc::new(SlowTimeoutRecovery { recover_count: Arc::clone(&recover_count) });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_initial_delay(Duration::ZERO)
            .with_deadline(Duration::from_millis(100));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };

        let res = supervisor.report_failure(report).await;
        assert!(res.is_err());
        let err = res.unwrap_err();

        match err {
            RealtimeError::Timeout(ref msg) => {
                assert!(
                    msg.contains("last provider error: WebSocket connection error: attempt 1 provider failure"),
                    "Timeout message should contain the last provider error, got: {}",
                    msg
                );
            }
            other => panic!("expected RealtimeError::Timeout, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_failure_during_planned_rotation_immediately_blocks_writes_and_publishes_n_plus_1()
    {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let pause_notify = Arc::new(tokio::sync::Notify::new());

        struct PausablePlannedRecovery {
            started_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            pause_notify: Arc<tokio::sync::Notify>,
            replacement_session: Arc<dyn RealtimeSession>,
        }

        #[async_trait]
        impl RealtimeRecovery for PausablePlannedRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                if let Some(tx) = self.started_tx.lock().take() {
                    let _ = tx.send(());
                }
                self.pause_notify.notified().await;
                Ok(RecoveredSession::new(
                    Arc::clone(&self.replacement_session),
                    RecoveryContinuity::Reconnected,
                ))
            }
        }

        let replacement_session = Arc::new(MockSession { id: "gen-1".to_string(), recovery: None });

        let mock_rec = Arc::new(PausablePlannedRecovery {
            started_tx: parking_lot::Mutex::new(Some(started_tx)),
            pause_notify: Arc::clone(&pause_notify),
            replacement_session,
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default();
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
        assert_eq!(supervisor.admit_write().await.unwrap().id, 0);

        // 1. Launch planned replacement in background
        let sup_task = Arc::clone(&supervisor);
        let rotation_handle = tokio::spawn(async move {
            let cause = RecoveryCause::PlannedRotation { time_left: Some("30s".into()) };
            sup_task.execute_planned_replacement(0, cause).await
        });

        // Wait until recover() is actively building candidate N+1
        started_rx.await.expect("candidate preparation must begin");

        // While candidate N+1 is preparing, generation 0 was Healthy.
        // Now simulate generation 0 transport failure!
        let sup_fail_task = Arc::clone(&supervisor);
        let failure_handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_fail_task.report_failure(report).await
        });

        // Deterministically wait for failed_generation to be observed and reject admit_write()
        let mut write_blocked = false;
        for _ in 0..100 {
            if supervisor.admit_write().await.is_err() {
                write_blocked = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            write_blocked,
            "Supervisor must immediately reject writes on generation 0 upon report_failure"
        );
        assert_eq!(
            supervisor.status().await,
            TransportStatus::Healthy,
            "Supervisor status must remain Healthy while waiting for recovery lock (no unowned Recovering state)"
        );

        // Release candidate preparation
        pause_notify.notify_one();

        let rotation_outcome = rotation_handle.await.unwrap().unwrap();
        let failure_outcome = failure_handle.await.unwrap().unwrap();

        // Rotation outcome published generation 1; failure outcome coalesced into generation 1
        assert!(matches!(rotation_outcome, RecoveryOutcome::Recovered { .. }));
        assert!(
            matches!(failure_outcome, RecoveryOutcome::Stale(ref s) if s.session_id() == "gen-1")
        );

        // Supervisor must now be Healthy on Generation 1
        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
        let admitted = supervisor.admit_write().await.expect("Generation 1 must be admittable");
        assert_eq!(admitted.id, 1);
        assert_eq!(admitted.session.session_id(), "gen-1");
    }

    #[tokio::test]
    async fn test_failed_planned_candidate_with_failed_generation_triggers_reactive_recovery() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let pause_notify = Arc::new(tokio::sync::Notify::new());
        let attempt_count = Arc::new(AtomicUsize::new(0));

        struct FailFirstRecovery {
            started_tx: parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            pause_notify: Arc<tokio::sync::Notify>,
            attempt_count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl RealtimeRecovery for FailFirstRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                let n = self.attempt_count.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // Attempt 0: Planned candidate preparation fails
                    if let Some(tx) = self.started_tx.lock().take() {
                        let _ = tx.send(());
                    }
                    self.pause_notify.notified().await;
                    Err(RealtimeError::ConnectionError(
                        "planned candidate network error".to_string(),
                    ))
                } else {
                    // Attempt 1: Reactive recovery succeeds
                    let session =
                        Arc::new(MockSession { id: format!("gen-recovered-{n}"), recovery: None });
                    Ok(RecoveredSession::new(session, RecoveryContinuity::Reconnected))
                }
            }
        }

        let mock_rec = Arc::new(FailFirstRecovery {
            started_tx: parking_lot::Mutex::new(Some(started_tx)),
            pause_notify: Arc::clone(&pause_notify),
            attempt_count: Arc::clone(&attempt_count),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_initial_delay(Duration::ZERO);
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // 1. Launch planned replacement in background
        let sup_task = Arc::clone(&supervisor);
        let rotation_handle = tokio::spawn(async move {
            let cause = RecoveryCause::PlannedRotation { time_left: Some("30s".into()) };
            sup_task.execute_planned_replacement(0, cause).await
        });

        started_rx.await.expect("candidate preparation must begin");

        // 2. Report failure on Generation 0
        let sup_fail_task = Arc::clone(&supervisor);
        let failure_handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_fail_task.report_failure(report).await
        });

        // Wait for failed_generation to block writes
        let mut write_blocked = false;
        for _ in 0..100 {
            if supervisor.admit_write().await.is_err() {
                write_blocked = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(write_blocked);

        // Release planned candidate to fail
        pause_notify.notify_one();

        let rotation_res = rotation_handle.await.unwrap();
        assert!(rotation_res.is_err(), "Planned candidate must report error");

        // report_failure must take over and successfully recover via attempt 1
        let failure_res = failure_handle.await.unwrap().unwrap();
        assert!(matches!(failure_res, RecoveryOutcome::Recovered { .. }));

        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
        let admitted = supervisor.admit_write().await.unwrap();
        assert_eq!(admitted.id, 1);
        assert_eq!(admitted.session.session_id(), "gen-recovered-1");
    }

    #[tokio::test]
    async fn test_failure_reporter_aborted_keeps_generation_failed_until_planned_replacement_succeeds()
     {
        let (lock_acquired_tx, lock_acquired_rx) = tokio::sync::oneshot::channel();
        let release_lock_notify = Arc::new(tokio::sync::Notify::new());

        let initial_session = Arc::new(MockSession { id: "gen-0".to_string(), recovery: None });
        let policy = RecoveryPolicy::default();
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // 1. Manually hold replacement_lock in a separate background task (simulating planned candidate preparation)
        let sup_lock_task = Arc::clone(&supervisor);
        let release_notify_clone = Arc::clone(&release_lock_notify);
        let lock_holder = tokio::spawn(async move {
            let _guard = sup_lock_task.replacement_lock.lock().await;
            let _ = lock_acquired_tx.send(());
            release_notify_clone.notified().await;
        });

        lock_acquired_rx.await.expect("replacement lock acquired");

        // 2. Call report_failure in a spawned task while lock is held
        let sup_fail_task = Arc::clone(&supervisor);
        let failure_task = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_fail_task.report_failure(report).await
        });

        // Wait until generation 0 is marked failed and admit_write() is blocked
        let mut write_blocked = false;
        for _ in 0..100 {
            if supervisor.admit_write().await.is_err() {
                write_blocked = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(write_blocked, "Writes must be blocked while waiting for lock");

        // 3. Abort the failure reporter task BEFORE it acquires the lock
        failure_task.abort();
        let _ = failure_task.await;

        // PROOF: Generation 0 must REMAIN write-blocked! Dropping the caller does NOT resurrect dead generation
        assert!(
            supervisor.admit_write().await.is_err(),
            "Dead generation 0 must remain non-admissible even after caller future is dropped"
        );

        // 4. Release the lock holding task (simulating planned candidate preparation finishing)
        release_lock_notify.notify_one();
        let _ = lock_holder.await;

        // 5. Planned candidate N+1 succeeds and publishes
        let replacement_session =
            Arc::new(MockSession { id: "gen-1-planned".to_string(), recovery: None });
        let published_gen = supervisor.publish_replacement(replacement_session, 0).await.unwrap();
        assert_eq!(published_gen.id, 1);

        // PROOF: Generation 1 is now Healthy and admittable
        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
        let admitted = supervisor.admit_write().await.unwrap();
        assert_eq!(admitted.id, 1);
        assert_eq!(admitted.session.session_id(), "gen-1-planned");
    }

    #[tokio::test]
    async fn test_failure_reporter_aborted_keeps_generation_failed_and_subsequent_recovery_succeeds()
     {
        let (lock_acquired_tx, lock_acquired_rx) = tokio::sync::oneshot::channel();
        let release_lock_notify = Arc::new(tokio::sync::Notify::new());

        let recovered_session =
            Arc::new(MockSession { id: "gen-1-recovered".to_string(), recovery: None });
        let mock_rec = Arc::new(AlwaysSuccessfulRecovery {
            recovered_session,
            continuity: RecoveryContinuity::Reconnected,
        });
        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(mock_rec as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default().with_initial_delay(Duration::ZERO);
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // 1. Manually hold replacement_lock in a background task
        let sup_lock_task = Arc::clone(&supervisor);
        let release_notify_clone = Arc::clone(&release_lock_notify);
        let lock_holder = tokio::spawn(async move {
            let _guard = sup_lock_task.replacement_lock.lock().await;
            let _ = lock_acquired_tx.send(());
            release_notify_clone.notified().await;
        });

        lock_acquired_rx.await.expect("replacement lock acquired");

        // 2. Call report_failure in a spawned task while lock is held
        let sup_fail_task = Arc::clone(&supervisor);
        let failure_task = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_fail_task.report_failure(report).await
        });

        // Wait until generation 0 is marked failed and admit_write() is blocked
        let mut write_blocked = false;
        for _ in 0..100 {
            if supervisor.admit_write().await.is_err() {
                write_blocked = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(write_blocked, "Writes must be blocked while waiting for lock");

        // 3. Abort the failure reporter task BEFORE it acquires the lock
        failure_task.abort();
        let _ = failure_task.await;

        // PROOF: Generation 0 must REMAIN write-blocked!
        assert!(
            supervisor.admit_write().await.is_err(),
            "Dead generation 0 must remain non-admissible even after caller future is dropped"
        );

        // 4. Release the lock and perform recovery on the failed generation
        release_lock_notify.notify_one();
        let _ = lock_holder.await;

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let outcome = supervisor.report_failure(report).await.unwrap();
        assert!(matches!(outcome, RecoveryOutcome::Recovered { .. }));

        // PROOF: Generation 1 is now Healthy and admittable
        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
        let admitted = supervisor.admit_write().await.unwrap();
        assert_eq!(admitted.id, 1);
        assert_eq!(admitted.session.session_id(), "gen-1-recovered");
    }

    #[tokio::test]
    async fn test_failure_reporter_lock_timeout_keeps_generation_failed_and_subsequent_recovery_succeeds()
     {
        let (lock_acquired_tx, lock_acquired_rx) = tokio::sync::oneshot::channel();
        let release_lock_notify = Arc::new(tokio::sync::Notify::new());

        let recovered_session =
            Arc::new(MockSession { id: "gen-1-recovered".to_string(), recovery: None });
        let mock_rec = Arc::new(AlwaysSuccessfulRecovery {
            recovered_session,
            continuity: RecoveryContinuity::Reconnected,
        });
        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(mock_rec.clone() as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_deadline(Duration::from_millis(50))
            .with_initial_delay(Duration::ZERO);
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = Arc::new(RecoverySupervisor::with_initial_session(
            policy,
            config.clone(),
            initial_session,
        ));

        // 1. Hold replacement_lock
        let sup_lock_task = Arc::clone(&supervisor);
        let release_notify_clone = Arc::clone(&release_lock_notify);
        let lock_holder = tokio::spawn(async move {
            let _guard = sup_lock_task.replacement_lock.lock().await;
            let _ = lock_acquired_tx.send(());
            release_notify_clone.notified().await;
        });

        lock_acquired_rx.await.expect("replacement lock acquired");

        // 2. report_failure should timeout waiting for the lock
        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let report_res = supervisor.report_failure(report).await;
        assert!(matches!(report_res, Err(RealtimeError::Timeout(_))));

        // PROOF: Generation 0 remains write-blocked after timeout!
        assert!(
            supervisor.admit_write().await.is_err(),
            "Dead generation 0 must remain non-admissible after caller timeout"
        );

        // 3. Release the lock and perform recovery with extended deadline
        release_lock_notify.notify_one();
        let _ = lock_holder.await;

        let retry_supervisor = RecoverySupervisor::with_initial_session(
            RecoveryPolicy::default().with_initial_delay(Duration::ZERO),
            config,
            Arc::new(MockSession {
                id: "gen-0".to_string(),
                recovery: Some(mock_rec as Arc<dyn RealtimeRecovery>),
            }),
        );
        let retry_report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let outcome = retry_supervisor.report_failure(retry_report).await.unwrap();
        assert!(matches!(outcome, RecoveryOutcome::Recovered { .. }));

        assert_eq!(retry_supervisor.status().await, TransportStatus::Healthy);
        let admitted = retry_supervisor.admit_write().await.unwrap();
        assert_eq!(admitted.id, 1);
        assert_eq!(admitted.session.session_id(), "gen-1-recovered");
    }

    struct AlwaysSuccessfulRecovery {
        recovered_session: Arc<dyn RealtimeSession>,
        continuity: RecoveryContinuity,
    }

    #[async_trait]
    impl RealtimeRecovery for AlwaysSuccessfulRecovery {
        fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
            RecoveryDisposition::Recoverable
        }

        async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
            Ok(RecoveredSession::new(self.recovered_session.clone(), self.continuity))
        }
    }

    #[tokio::test]
    async fn test_recovery_epoch_increments_exactly_once() {
        let recovered_session = Arc::new(MockSession { id: "gen-1".to_string(), recovery: None });
        let mock_rec = Arc::new(AlwaysSuccessfulRecovery {
            recovered_session,
            continuity: RecoveryContinuity::Reconnected,
        });
        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(mock_rec as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default().with_initial_delay(Duration::ZERO);
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

        assert_eq!(supervisor.state.read().await.recovery_epoch, 0);

        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let outcome = supervisor.report_failure(report).await.unwrap();
        assert!(matches!(outcome, RecoveryOutcome::Recovered { .. }));

        // PROOF: Exactly one recovery episode -> recovery_epoch is exactly 1
        assert_eq!(
            supervisor.state.read().await.recovery_epoch,
            1,
            "recovery_epoch must increment exactly once per recovery episode"
        );
    }
}
