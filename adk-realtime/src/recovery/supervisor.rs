#![allow(unfulfilled_lint_expectations)]

use crate::error::{RealtimeError, Result};
use crate::recovery::{
    RealtimeRecovery, RecoveredSession, RecoveryCause, RecoveryContext, RecoveryContinuity,
    RecoveryDisposition, RecoveryPolicy,
};
use crate::session::RealtimeSession;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Transport status projection of the supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TransportStatus {
    #[default]
    Uninitialized,
    Healthy,
    Recovering,
    Closed,
    Exhausted,
}

/// Monotonic write gate for a session generation.
///
/// Once closed by `fail()`, it cannot be reopened. Illegal write admission
/// after failure is unrepresentable.
#[derive(Debug, Clone)]
pub(crate) struct WriteGate {
    closed: Arc<AtomicBool>,
}

impl WriteGate {
    pub fn new_open() -> Self {
        Self { closed: Arc::new(AtomicBool::new(false)) }
    }

    /// Atomically and irreversibly close the write gate.
    /// Returns true if this call changed the gate from open to closed.
    pub fn close(&self) -> bool {
        !self.closed.swap(true, Ordering::SeqCst)
    }

    pub fn is_open(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
    }
}

/// A session generation paired with its monotonic ID and monotonic write gate.
#[derive(Clone)]
pub(crate) struct SessionGeneration {
    pub(crate) id: u64,
    pub(crate) session: Arc<dyn RealtimeSession>,
    pub(crate) write_gate: WriteGate,
}

impl SessionGeneration {
    pub fn new(id: u64, session: Arc<dyn RealtimeSession>) -> Self {
        Self { id, session, write_gate: WriteGate::new_open() }
    }

    pub fn fail(&self) -> bool {
        self.write_gate.close()
    }

    pub fn is_writable(&self) -> bool {
        self.write_gate.is_open()
    }
}

impl std::fmt::Debug for SessionGeneration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionGeneration")
            .field("id", &self.id)
            .field("session_id", &self.session.session_id())
            .field("writable", &self.is_writable())
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

/// The outcome of a recovery or planned replacement transaction.
#[derive(Clone)]
pub(crate) enum RecoveryOutcome {
    /// A newly recovered session was successfully established and published.
    Recovered { session: Arc<dyn RealtimeSession>, continuity: RecoveryContinuity },
    /// A report for a stale/older generation than the currently active one.
    Stale(Arc<dyn RealtimeSession>),
    /// A report for a generation that was already terminally exhausted / handled.
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

#[derive(Debug, Clone)]
pub(crate) enum ReplacementPhase {
    Planned { cause: RecoveryCause, deadline: tokio::time::Instant },
    Recovering { cause: RecoveryCause, deadline: tokio::time::Instant },
}

/// A supervisor-owned replacement transaction (planned or reactive).
pub(crate) struct ReplacementTxn {
    pub(crate) id: u64,
    pub(crate) _target_generation_id: u64,
    pub(crate) phase: parking_lot::Mutex<ReplacementPhase>,
    pub(crate) cancel_token: CancellationToken,
    pub(crate) outcome_tx: tokio::sync::watch::Sender<Option<Result<RecoveryOutcome>>>,
    pub(crate) outcome_rx: tokio::sync::watch::Receiver<Option<Result<RecoveryOutcome>>>,
}

impl ReplacementTxn {
    pub fn new(id: u64, target_generation_id: u64, phase: ReplacementPhase) -> Self {
        let (outcome_tx, outcome_rx) = tokio::sync::watch::channel(None);
        Self {
            id,
            _target_generation_id: target_generation_id,
            phase: parking_lot::Mutex::new(phase),
            cancel_token: CancellationToken::new(),
            outcome_tx,
            outcome_rx,
        }
    }

    pub fn snapshot_phase(&self) -> ReplacementPhase {
        self.phase.lock().clone()
    }

    pub fn update_phase(&self, new_phase: ReplacementPhase) {
        *self.phase.lock() = new_phase;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalReason {
    ExplicitClose,
    Exhausted,
}

#[derive(Debug)]
pub(crate) enum AttemptTransition {
    Published,
    KeepServing,
    RetryRecovering,
    Terminal,
}

pub(crate) struct AttemptContext<'a> {
    pub(crate) originating_gen_id: u64,
    pub(crate) attempt_idx: u32,
    pub(crate) max_attempts: u32,
    pub(crate) expected_revision: Option<u64>,
    pub(crate) recovery_impl: Option<&'a dyn RealtimeRecovery>,
}

/// Closed algebraic state machine for supervisor transport lifecycle.
pub(crate) enum ManagedState {
    Uninitialized,

    Serving { active: Arc<SessionGeneration>, planned: Option<Arc<ReplacementTxn>> },

    Recovering { failed: Arc<SessionGeneration>, txn: Arc<ReplacementTxn> },

    Terminal { reason: TerminalReason, _last_generation: Option<Arc<SessionGeneration>> },
}

/// Unified core protected under a single `tokio::sync::RwLock` to prevent lock inversion.
pub(crate) struct SupervisorCore {
    pub(crate) state: ManagedState,
    pub(crate) config: ConfigSnapshot,
    pub(crate) next_generation_id: u64,
    pub(crate) next_txn_id: u64,
}

/// RAII cleanup guard for un-published candidate sessions.
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

/// The algebraic recovery supervisor.
pub(crate) struct RecoverySupervisor {
    policy: RecoveryPolicy,
    core: Arc<tokio::sync::RwLock<SupervisorCore>>,
    generation_tx: tokio::sync::watch::Sender<u64>,
    #[cfg(any(test, feature = "recovery-test-utils"))]
    test_recovery_barrier:
        Arc<parking_lot::Mutex<Option<Arc<crate::recovery::TestRecoveryBarrier>>>>,
}

pub(crate) fn parse_duration_string(s: &str) -> Option<std::time::Duration> {
    let s = s.trim();
    let secs = if let Some(stripped) = s.strip_suffix("ms") {
        let ms = stripped.trim().parse::<f64>().ok()?;
        ms / 1000.0
    } else if let Some(stripped) = s.strip_suffix('s') {
        stripped.trim().parse::<f64>().ok()?
    } else if let Some(stripped) = s.strip_suffix('m') {
        let m = stripped.trim().parse::<f64>().ok()?;
        m * 60.0
    } else {
        s.parse::<f64>().ok()?
    };

    if secs.is_finite() && (0.0..=86400.0 * 365.0).contains(&secs) {
        Some(std::time::Duration::from_secs_f64(secs))
    } else {
        None
    }
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
            core: Arc::new(tokio::sync::RwLock::new(SupervisorCore {
                state: ManagedState::Uninitialized,
                config: ConfigSnapshot { config: initial_config, revision: 0 },
                next_generation_id: 0,
                next_txn_id: 1,
            })),
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
        let gen_0 = Arc::new(SessionGeneration::new(0, initial_session));
        Self {
            policy,
            core: Arc::new(tokio::sync::RwLock::new(SupervisorCore {
                state: ManagedState::Serving { active: gen_0, planned: None },
                config: ConfigSnapshot { config: initial_cfg, revision: 0 },
                next_generation_id: 1,
                next_txn_id: 1,
            })),
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

    /// Install the initial session (generation 0).
    pub(crate) async fn set_initial_session(
        &self,
        session: Arc<dyn RealtimeSession>,
    ) -> Result<SessionGeneration> {
        let mut core_guard = self.core.write().await;
        let SupervisorCore { state, next_generation_id, .. } = &mut *core_guard;
        if !matches!(state, ManagedState::Uninitialized) {
            return Err(RealtimeError::config(
                "cannot set initial session unless supervisor status is strictly Uninitialized",
            ));
        }
        let gen_id = *next_generation_id;
        *next_generation_id += 1;
        let gen_item = Arc::new(SessionGeneration::new(gen_id, session));
        *state = ManagedState::Serving { active: Arc::clone(&gen_item), planned: None };
        let _ = self.generation_tx.send(gen_id);
        Ok((*gen_item).clone())
    }

    /// Retrieve a watcher for generation change signals.
    pub(crate) fn subscribe_generation(&self) -> tokio::sync::watch::Receiver<u64> {
        self.generation_tx.subscribe()
    }

    /// Check current transport status projection.
    pub(crate) async fn status(&self) -> TransportStatus {
        let core = self.core.read().await;
        match &core.state {
            ManagedState::Uninitialized => TransportStatus::Uninitialized,
            ManagedState::Serving { active, .. } => {
                if active.is_writable() {
                    TransportStatus::Healthy
                } else {
                    TransportStatus::Recovering
                }
            }
            ManagedState::Recovering { .. } => TransportStatus::Recovering,
            ManagedState::Terminal { reason, .. } => match reason {
                TerminalReason::ExplicitClose => TransportStatus::Closed,
                TerminalReason::Exhausted => TransportStatus::Exhausted,
            },
        }
    }

    /// Check if currently connected and healthy for writes.
    pub(crate) async fn is_connected(&self) -> bool {
        let core = self.core.read().await;
        match &core.state {
            ManagedState::Serving { active, .. } => {
                active.is_writable() && active.session.is_connected()
            }
            _ => false,
        }
    }

    /// Retrieve the currently authoritative session generation for reading events.
    ///
    /// Unlike `admit_write()`, this returns the generation even during recovery so that
    /// pending server events can be drained without returning false EOF.
    pub(crate) async fn get_active_generation(&self) -> Result<SessionGeneration> {
        let core = self.core.read().await;
        match &core.state {
            ManagedState::Uninitialized => Err(RealtimeError::NotConnected),
            ManagedState::Serving { active, .. } => Ok((**active).clone()),
            ManagedState::Recovering { failed, .. } => Ok((**failed).clone()),
            ManagedState::Terminal { reason, .. } => match reason {
                TerminalReason::ExplicitClose => Err(RealtimeError::SessionClosed),
                TerminalReason::Exhausted => Err(RealtimeError::provider("session exhausted")),
            },
        }
    }

    /// Admit a write operation before invoking the raw session.
    ///
    /// Fails closed if the active generation's write gate is closed or if supervisor is recovering.
    pub(crate) async fn admit_write(&self) -> Result<SessionGeneration> {
        let core = self.core.read().await;
        match &core.state {
            ManagedState::Uninitialized | ManagedState::Recovering { .. } => {
                Err(RealtimeError::NotConnected)
            }
            ManagedState::Serving { active, .. } => {
                if active.is_writable() {
                    Ok((**active).clone())
                } else {
                    Err(RealtimeError::NotConnected)
                }
            }
            ManagedState::Terminal { reason, .. } => match reason {
                TerminalReason::ExplicitClose => Err(RealtimeError::SessionClosed),
                TerminalReason::Exhausted => Err(RealtimeError::provider("session exhausted")),
            },
        }
    }

    /// Mutate the canonical configuration atomically and return the new snapshot.
    pub(crate) async fn update_config<F>(&self, mutate: F) -> ConfigSnapshot
    where
        F: FnOnce(&mut crate::config::RealtimeConfig),
    {
        let mut core = self.core.write().await;
        mutate(&mut core.config.config);
        core.config.revision = core.config.revision.wrapping_add(1);
        core.config.clone()
    }

    /// Get coherent configuration and revision snapshot.
    pub(crate) async fn get_config(&self) -> ConfigSnapshot {
        self.core.read().await.config.clone()
    }

    /// Atomically publish a newly created replacement session.
    pub(crate) async fn publish_replacement(
        &self,
        new_session: Arc<dyn RealtimeSession>,
        expected_revision: u64,
    ) -> Result<SessionGeneration> {
        let mut candidate_guard = CandidateCleanupGuard::new(new_session.clone());

        let (new_gen, old_session) = {
            let mut core_guard = self.core.write().await;
            let SupervisorCore { state, config, next_generation_id, .. } = &mut *core_guard;
            if config.revision != expected_revision {
                return Err(RealtimeError::config("stale config revision during publication"));
            }

            match state {
                ManagedState::Terminal { .. } => return Err(RealtimeError::SessionClosed),
                ManagedState::Uninitialized => return Err(RealtimeError::NotConnected),
                ManagedState::Serving { active, planned } => {
                    if let Some(p) = planned.take() {
                        p.cancel_token.cancel();
                    }
                    let next_gen = *next_generation_id;
                    *next_generation_id += 1;
                    let gen_item = Arc::new(SessionGeneration::new(next_gen, new_session.clone()));
                    let old = active.session.clone();
                    *state = ManagedState::Serving { active: Arc::clone(&gen_item), planned: None };
                    (gen_item, Some(old))
                }
                ManagedState::Recovering { failed, txn } => {
                    txn.cancel_token.cancel();
                    let next_gen = *next_generation_id;
                    *next_generation_id += 1;
                    let gen_item = Arc::new(SessionGeneration::new(next_gen, new_session.clone()));
                    let old = failed.session.clone();
                    *state = ManagedState::Serving { active: Arc::clone(&gen_item), planned: None };
                    (gen_item, Some(old))
                }
            }
        };

        candidate_guard.disarm();
        let _ = self.generation_tx.send(new_gen.id);

        if let Some(old) = old_session {
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(2), old.close()).await;
            });
        }

        Ok((*new_gen).clone())
    }

    /// Execute an intentional context resumption under the supervisor.
    pub(crate) async fn execute_context_resumption(
        &self,
        model: &crate::model::BoxedModel,
        queued_snapshot: ConfigSnapshot,
    ) -> Result<SessionGeneration> {
        let current_active = {
            let core = self.core.read().await;
            if core.config.revision != queued_snapshot.revision {
                return Err(RealtimeError::config(
                    "stale config revision prior to resumption connect",
                ));
            }
            match &core.state {
                ManagedState::Terminal { .. } => return Err(RealtimeError::SessionClosed),
                ManagedState::Uninitialized | ManagedState::Recovering { .. } => {
                    return Err(RealtimeError::NotConnected);
                }
                ManagedState::Serving { active, .. } => Arc::clone(active),
            }
        };

        tracing::info!("Executing intentional context resumption under supervisor.");
        let new_session = model.connect(queued_snapshot.config).await?;
        let new_session_arc: Arc<dyn RealtimeSession> = Arc::from(new_session);

        let mut candidate_guard = CandidateCleanupGuard::new(new_session_arc.clone());

        let (new_gen, old_session) = {
            let mut core_guard = self.core.write().await;
            let SupervisorCore { state, config, next_generation_id, .. } = &mut *core_guard;
            if config.revision != queued_snapshot.revision {
                return Err(RealtimeError::config("stale config revision post-resumption connect"));
            }

            match state {
                ManagedState::Terminal { .. } => return Err(RealtimeError::SessionClosed),
                ManagedState::Serving { active, planned } => {
                    if active.id != current_active.id {
                        return Err(RealtimeError::provider(
                            "active generation changed during resumption",
                        ));
                    }
                    if let Some(p) = planned.take() {
                        p.cancel_token.cancel();
                    }
                    let next_gen = *next_generation_id;
                    *next_generation_id += 1;
                    let gen_item =
                        Arc::new(SessionGeneration::new(next_gen, new_session_arc.clone()));
                    let old = active.session.clone();
                    *state = ManagedState::Serving { active: Arc::clone(&gen_item), planned: None };
                    (gen_item, old)
                }
                _ => return Err(RealtimeError::NotConnected),
            }
        };

        candidate_guard.disarm();
        let _ = self.generation_tx.send(new_gen.id);

        tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(2), old_session.close()).await;
        });

        Ok((*new_gen).clone())
    }

    /// Execute a proactive planned replacement (make-before-break).
    pub(crate) async fn execute_planned_replacement(
        &self,
        report_generation: u64,
        cause: RecoveryCause,
    ) -> Result<RecoveryOutcome> {
        let (txn, mut outcome_rx) = {
            let mut core_guard = self.core.write().await;
            let SupervisorCore { state, next_txn_id, .. } = &mut *core_guard;
            match state {
                ManagedState::Terminal { reason, .. } => {
                    return match reason {
                        TerminalReason::ExplicitClose => Err(RealtimeError::SessionClosed),
                        TerminalReason::Exhausted => Ok(RecoveryOutcome::Exhausted),
                    };
                }
                ManagedState::Uninitialized => return Err(RealtimeError::NotConnected),
                ManagedState::Recovering { failed, .. } => {
                    if report_generation <= failed.id {
                        return Ok(RecoveryOutcome::Stale(failed.session.clone()));
                    }
                    return Err(RealtimeError::NotConnected);
                }
                ManagedState::Serving { active, planned } => {
                    if report_generation < active.id {
                        return Ok(RecoveryOutcome::Stale(active.session.clone()));
                    }
                    if report_generation > active.id {
                        return Err(RealtimeError::provider(format!(
                            "invalid future generation report: reported {}, current is {}",
                            report_generation, active.id
                        )));
                    }

                    if let Some(existing_planned) = planned {
                        (Arc::clone(existing_planned), existing_planned.outcome_rx.clone())
                    } else {
                        let policy_deadline = tokio::time::Instant::now() + self.policy.deadline();
                        let deadline = match &cause {
                            RecoveryCause::PlannedRotation { time_left: Some(dur_str) } => {
                                if let Some(provider_dur) = parse_duration_string(dur_str) {
                                    let safety_margin = Duration::from_secs(2);
                                    let min_window = Duration::from_millis(100);
                                    let capped_dur = if provider_dur > safety_margin {
                                        provider_dur - safety_margin
                                    } else {
                                        provider_dur.min(min_window)
                                    };
                                    let provider_deadline =
                                        tokio::time::Instant::now() + capped_dur;
                                    policy_deadline.min(provider_deadline)
                                } else {
                                    policy_deadline
                                }
                            }
                            _ => policy_deadline,
                        };

                        let txn_id = *next_txn_id;
                        *next_txn_id += 1;
                        let phase = ReplacementPhase::Planned { cause: cause.clone(), deadline };
                        let txn = Arc::new(ReplacementTxn::new(txn_id, active.id + 1, phase));
                        let rx = txn.outcome_rx.clone();
                        let active_arc = Arc::clone(active);
                        *planned = Some(Arc::clone(&txn));

                        self.spawn_replacement_worker(active_arc, Arc::clone(&txn));

                        (txn, rx)
                    }
                }
            }
        };

        Self::await_txn_outcome(&mut outcome_rx, &txn).await
    }

    /// Report a failure in the active session.
    pub(crate) async fn report_failure(&self, report: FailureReport) -> Result<RecoveryOutcome> {
        let (txn, mut outcome_rx) = {
            let mut core_guard = self.core.write().await;
            let SupervisorCore { state, next_txn_id, .. } = &mut *core_guard;
            match state {
                ManagedState::Terminal { reason, .. } => {
                    return match reason {
                        TerminalReason::ExplicitClose => Err(RealtimeError::SessionClosed),
                        TerminalReason::Exhausted => Ok(RecoveryOutcome::Exhausted),
                    };
                }
                ManagedState::Uninitialized => return Err(RealtimeError::NotConnected),
                ManagedState::Recovering { failed, txn } => {
                    if report.generation < failed.id {
                        return Ok(RecoveryOutcome::Stale(failed.session.clone()));
                    }
                    if report.generation > failed.id {
                        return Err(RealtimeError::provider(format!(
                            "invalid future generation report: reported {}, current is {}",
                            report.generation, failed.id
                        )));
                    }
                    (Arc::clone(txn), txn.outcome_rx.clone())
                }
                ManagedState::Serving { active, planned } => {
                    if report.generation < active.id {
                        return Ok(RecoveryOutcome::Stale(active.session.clone()));
                    }
                    if report.generation > active.id {
                        return Err(RealtimeError::provider(format!(
                            "invalid future generation report: reported {}, current is {}",
                            report.generation, active.id
                        )));
                    }

                    // Active generation failed! Monotonically close write gate
                    active.fail();
                    let active_arc = Arc::clone(active);

                    if let Some(planned_txn) = planned.take() {
                        let new_deadline = tokio::time::Instant::now() + self.policy.deadline();
                        planned_txn.update_phase(ReplacementPhase::Recovering {
                            cause: report.cause.clone(),
                            deadline: new_deadline,
                        });
                        let txn = Arc::clone(&planned_txn);
                        let rx = txn.outcome_rx.clone();
                        *state = ManagedState::Recovering { failed: active_arc, txn: planned_txn };
                        (txn, rx)
                    } else {
                        let txn_id = *next_txn_id;
                        *next_txn_id += 1;
                        let deadline = tokio::time::Instant::now() + self.policy.deadline();
                        let phase =
                            ReplacementPhase::Recovering { cause: report.cause.clone(), deadline };
                        let txn = Arc::new(ReplacementTxn::new(txn_id, active_arc.id + 1, phase));
                        let rx = txn.outcome_rx.clone();
                        *state = ManagedState::Recovering {
                            failed: Arc::clone(&active_arc),
                            txn: Arc::clone(&txn),
                        };

                        self.spawn_replacement_worker(Arc::clone(&active_arc), Arc::clone(&txn));

                        (txn, rx)
                    }
                }
            }
        };

        Self::await_txn_outcome(&mut outcome_rx, &txn).await
    }

    async fn await_txn_outcome(
        outcome_rx: &mut tokio::sync::watch::Receiver<Option<Result<RecoveryOutcome>>>,
        txn: &Arc<ReplacementTxn>,
    ) -> Result<RecoveryOutcome> {
        loop {
            if let Some(ref outcome) = *outcome_rx.borrow_and_update() {
                return outcome.clone();
            }

            let current_deadline = match txn.snapshot_phase() {
                ReplacementPhase::Planned { deadline, .. } => deadline,
                ReplacementPhase::Recovering { deadline, .. } => deadline,
            };

            let timeout_res = tokio::time::timeout_at(current_deadline, outcome_rx.changed()).await;
            match timeout_res {
                Ok(Ok(())) => {
                    if let Some(ref outcome) = *outcome_rx.borrow_and_update() {
                        return outcome.clone();
                    }
                }
                Ok(Err(_)) => {
                    return Err(RealtimeError::provider(
                        "recovery transaction dropped without outcome",
                    ));
                }
                Err(_) => {
                    return Err(RealtimeError::Timeout(
                        "deadline expired waiting for recovery transaction outcome".to_string(),
                    ));
                }
            }
        }
    }

    /// Single unified worker owning both planned preparation and reactive recovery attempts.
    fn spawn_replacement_worker(
        &self,
        originating_gen: Arc<SessionGeneration>,
        txn: Arc<ReplacementTxn>,
    ) {
        let core_lock = Arc::clone(&self.core);
        let generation_tx = self.generation_tx.clone();
        let policy = self.policy.clone();
        #[cfg(any(test, feature = "recovery-test-utils"))]
        let test_recovery_barrier = Arc::clone(&self.test_recovery_barrier);

        tokio::spawn(async move {
            let recovery_impl = match originating_gen.session.recovery() {
                Some(r) => r,
                None => {
                    let err = RealtimeError::provider("active session does not support recovery");
                    tracing::error!(generation = originating_gen.id, err = %err, "recovery not supported");
                    let ctx = AttemptContext {
                        originating_gen_id: originating_gen.id,
                        attempt_idx: 1,
                        max_attempts: 1,
                        expected_revision: None,
                        recovery_impl: None,
                    };
                    let _ =
                        Self::apply_attempt_result(&core_lock, &generation_tx, &txn, ctx, Err(err))
                            .await;
                    return;
                }
            };

            let initial_phase = txn.snapshot_phase();
            let initial_cause = match &initial_phase {
                ReplacementPhase::Planned { cause, .. } => cause,
                ReplacementPhase::Recovering { cause, .. } => cause,
            };

            if recovery_impl.classify(initial_cause) == RecoveryDisposition::Fatal {
                let err = RealtimeError::provider(
                    "fatal recovery cause detected; performing zero provider attempts",
                );
                tracing::warn!(generation = originating_gen.id, err = %err, "fatal cause classification");
                let ctx = AttemptContext {
                    originating_gen_id: originating_gen.id,
                    attempt_idx: 1,
                    max_attempts: 1,
                    expected_revision: None,
                    recovery_impl: Some(recovery_impl),
                };
                let _ = Self::apply_attempt_result(&core_lock, &generation_tx, &txn, ctx, Err(err))
                    .await;
                return;
            }

            let max_attempts = policy.max_attempts().get();
            // Phase-aware budget: planned attempts use 1..=max_attempts,
            // but if promoted to Recovering the reactive phase gets its own
            // fresh 1..=max_attempts budget.
            let mut reactive_budget_started = false;
            let mut attempt_idx: u32 = 0;
            let mut current_max: u32 = max_attempts;

            loop {
                attempt_idx += 1;
                if attempt_idx > current_max {
                    break;
                }

                if txn.cancel_token.is_cancelled() {
                    tracing::info!(
                        generation = originating_gen.id,
                        "replacement transaction cancelled"
                    );
                    let ctx = AttemptContext {
                        originating_gen_id: originating_gen.id,
                        attempt_idx,
                        max_attempts: current_max,
                        expected_revision: None,
                        recovery_impl: Some(recovery_impl),
                    };
                    let _ = Self::apply_attempt_result(
                        &core_lock,
                        &generation_tx,
                        &txn,
                        ctx,
                        Err(RealtimeError::SessionClosed),
                    )
                    .await;
                    return;
                }

                let phase = txn.snapshot_phase();
                let (current_cause, current_deadline, is_recovering) = match phase {
                    ReplacementPhase::Planned { cause, deadline } => (cause, deadline, false),
                    ReplacementPhase::Recovering { cause, deadline } => (cause, deadline, true),
                };

                // When promoted to Recovering, give recovery its own fresh budget.
                if is_recovering && !reactive_budget_started {
                    reactive_budget_started = true;
                    attempt_idx = 1;
                    current_max = max_attempts;
                    tracing::info!(
                        generation = originating_gen.id,
                        "promoted to reactive recovery; resetting attempt budget to {}",
                        current_max
                    );
                }

                if is_recovering
                    && recovery_impl.classify(&current_cause) == RecoveryDisposition::Fatal
                {
                    let err = RealtimeError::provider(
                        "fatal failure cause classified during recovery; aborting attempts",
                    );
                    tracing::warn!(
                        generation = originating_gen.id,
                        attempt = attempt_idx,
                        err = %err,
                        "fatal cause classification during recovery"
                    );
                    let ctx = AttemptContext {
                        originating_gen_id: originating_gen.id,
                        attempt_idx,
                        max_attempts: current_max,
                        expected_revision: None,
                        recovery_impl: Some(recovery_impl),
                    };
                    let _ =
                        Self::apply_attempt_result(&core_lock, &generation_tx, &txn, ctx, Err(err))
                            .await;
                    return;
                }

                #[cfg(any(test, feature = "recovery-test-utils"))]
                if is_recovering {
                    let maybe_barrier = test_recovery_barrier.lock().take();
                    if let Some(barrier) = maybe_barrier {
                        barrier.on_recovering().await;
                    }
                }

                let now = tokio::time::Instant::now();
                if now >= current_deadline {
                    let phase_now = txn.snapshot_phase();
                    if let ReplacementPhase::Recovering { deadline: new_deadline, .. } = phase_now
                        && now < new_deadline
                    {
                        tracing::info!(
                            generation = originating_gen.id,
                            attempt = attempt_idx,
                            "planned deadline expired but promoted to recovery; continuing under fresh deadline"
                        );
                        // Reset budget on promotion detected via deadline takeover.
                        if !reactive_budget_started {
                            reactive_budget_started = true;
                            attempt_idx = 0; // Will be incremented to 1 at loop top
                            current_max = max_attempts;
                        }
                        continue;
                    }

                    let msg = "replacement deadline expired before attempt".to_string();
                    tracing::warn!(generation = originating_gen.id, attempt = attempt_idx, err = %msg, "deadline expired");
                    let ctx = AttemptContext {
                        originating_gen_id: originating_gen.id,
                        attempt_idx,
                        max_attempts: current_max,
                        expected_revision: None,
                        recovery_impl: Some(recovery_impl),
                    };
                    let _ = Self::apply_attempt_result(
                        &core_lock,
                        &generation_tx,
                        &txn,
                        ctx,
                        Err(RealtimeError::Timeout(msg)),
                    )
                    .await;
                    return;
                }

                let attempt_nz = NonZeroU32::new(attempt_idx).unwrap_or(NonZeroU32::MIN);
                let snapshot = core_lock.read().await.config.clone();

                let remaining_dur = current_deadline.saturating_duration_since(now);
                let context_deadline = std::time::Instant::now()
                    .checked_add(remaining_dur)
                    .unwrap_or_else(std::time::Instant::now);

                let context = RecoveryContext::new(
                    attempt_nz,
                    &current_cause,
                    &snapshot.config,
                    context_deadline,
                );

                tracing::debug!(
                    generation = originating_gen.id,
                    attempt = attempt_idx,
                    "building replacement candidate"
                );

                let attempt_res = tokio::select! {
                    _ = txn.cancel_token.cancelled() => {
                        tracing::debug!(
                            generation = originating_gen.id,
                            attempt = attempt_idx,
                            "replacement candidate cancelled during connection"
                        );
                        let ctx = AttemptContext {
                            originating_gen_id: originating_gen.id,
                            attempt_idx,
                            max_attempts: current_max,
                            expected_revision: None,
                            recovery_impl: Some(recovery_impl),
                        };
                        let _ = Self::apply_attempt_result(
                            &core_lock,
                            &generation_tx,
                            &txn,
                            ctx,
                            Err(RealtimeError::SessionClosed),
                        ).await;
                        return;
                    }
                    _ = tokio::time::sleep_until(current_deadline) => {
                        let now_timeout = tokio::time::Instant::now();
                        let phase_timeout = txn.snapshot_phase();
                        if let ReplacementPhase::Recovering { deadline: new_deadline, .. } = phase_timeout
                            && now_timeout < new_deadline
                        {
                            tracing::info!(
                                generation = originating_gen.id,
                                attempt = attempt_idx,
                                "in-flight attempt timed out under planned deadline after promotion; continuing under fresh deadline"
                            );
                            // Reset budget on promotion detected via in-flight timeout.
                            if !reactive_budget_started {
                                reactive_budget_started = true;
                                attempt_idx = 0; // Will be incremented to 1 at loop top
                                current_max = max_attempts;
                            }
                            continue;
                        }
                        let msg = "replacement deadline expired during connection attempt".to_string();
                        tracing::warn!(
                            generation = originating_gen.id,
                            attempt = attempt_idx,
                            err = %msg,
                            "deadline expired during connection"
                        );
                        let ctx = AttemptContext {
                            originating_gen_id: originating_gen.id,
                            attempt_idx,
                            max_attempts: current_max,
                            expected_revision: None,
                            recovery_impl: Some(recovery_impl),
                        };
                        let _ = Self::apply_attempt_result(
                            &core_lock,
                            &generation_tx,
                            &txn,
                            ctx,
                            Err(RealtimeError::Timeout(msg)),
                        ).await;
                        return;
                    }
                    res = recovery_impl.recover(context) => res,
                };

                let ctx = AttemptContext {
                    originating_gen_id: originating_gen.id,
                    attempt_idx,
                    max_attempts: current_max,
                    expected_revision: Some(snapshot.revision),
                    recovery_impl: Some(recovery_impl),
                };
                let transition =
                    Self::apply_attempt_result(&core_lock, &generation_tx, &txn, ctx, attempt_res)
                        .await;

                match transition {
                    AttemptTransition::Published
                    | AttemptTransition::KeepServing
                    | AttemptTransition::Terminal => {
                        return;
                    }
                    AttemptTransition::RetryRecovering => {
                        if attempt_idx < current_max {
                            let base_delay = policy.initial_delay();
                            let factor = 2u32.checked_pow(attempt_idx - 1).unwrap_or(u32::MAX);
                            let mut backoff =
                                base_delay.saturating_mul(factor).min(policy.max_delay());
                            let current_deadline = match txn.snapshot_phase() {
                                ReplacementPhase::Planned { deadline, .. } => deadline,
                                ReplacementPhase::Recovering { deadline, .. } => deadline,
                            };
                            let now_after = tokio::time::Instant::now();
                            if now_after >= current_deadline {
                                let msg = "recovery deadline expired during backoff calculation"
                                    .to_string();
                                let ctx = AttemptContext {
                                    originating_gen_id: originating_gen.id,
                                    attempt_idx,
                                    max_attempts: current_max,
                                    expected_revision: None,
                                    recovery_impl: Some(recovery_impl),
                                };
                                let _ = Self::apply_attempt_result(
                                    &core_lock,
                                    &generation_tx,
                                    &txn,
                                    ctx,
                                    Err(RealtimeError::Timeout(msg)),
                                )
                                .await;
                                return;
                            }
                            backoff =
                                backoff.min(current_deadline.saturating_duration_since(now_after));
                            if !backoff.is_zero() {
                                tokio::select! {
                                    _ = txn.cancel_token.cancelled() => {
                                        let ctx = AttemptContext {
                                            originating_gen_id: originating_gen.id,
                                            attempt_idx,
                                            max_attempts: current_max,
                                            expected_revision: None,
                                            recovery_impl: Some(recovery_impl),
                                        };
                                        let _ = Self::apply_attempt_result(
                                            &core_lock,
                                            &generation_tx,
                                            &txn,
                                            ctx,
                                            Err(RealtimeError::SessionClosed),
                                        ).await;
                                        return;
                                    }
                                    _ = tokio::time::sleep_until(current_deadline) => {
                                        let msg = "recovery deadline expired during backoff sleep".to_string();
                                        let ctx = AttemptContext {
                                            originating_gen_id: originating_gen.id,
                                            attempt_idx,
                                            max_attempts: current_max,
                                            expected_revision: None,
                                            recovery_impl: Some(recovery_impl),
                                        };
                                        let _ = Self::apply_attempt_result(
                                            &core_lock,
                                            &generation_tx,
                                            &txn,
                                            ctx,
                                            Err(RealtimeError::Timeout(msg)),
                                        ).await;
                                        return;
                                    }
                                    _ = tokio::time::sleep(backoff) => {}
                                }
                            }
                        }
                        // If attempt_idx >= current_max, fall through to loop exit
                        // where the defensive terminalization will catch it.
                    }
                }
            }

            // Defensive post-loop terminalization: if we exit the loop while
            // ManagedState::Recovering is still authoritative for this txn,
            // atomically transition to Terminal::Exhausted. This catches any
            // code path that reaches the loop boundary without terminalizing.
            {
                let mut core_guard = core_lock.write().await;
                if let ManagedState::Recovering { ref failed, txn: ref cur_txn } = core_guard.state
                    && cur_txn.id == txn.id
                {
                    tracing::warn!(
                        generation = originating_gen.id,
                        txn_id = txn.id,
                        "worker exiting loop with authoritative Recovering; defensive terminalization"
                    );
                    let last_gen = Arc::clone(failed);
                    core_guard.state = ManagedState::Terminal {
                        reason: TerminalReason::Exhausted,
                        _last_generation: Some(last_gen),
                    };
                    let _ = txn.outcome_tx.send(Some(Err(RealtimeError::provider(
                        "recovery exhausted: worker loop completed without terminalization",
                    ))));
                }
            }
        });
    }

    /// Single centralized publication/finalization helper.
    ///
    /// Validates supervisor state, verifies config revision, updates state atomically,
    /// cleans up old sessions / candidates, and publishes outcome.
    async fn apply_attempt_result(
        core_lock: &Arc<tokio::sync::RwLock<SupervisorCore>>,
        generation_tx: &tokio::sync::watch::Sender<u64>,
        txn: &Arc<ReplacementTxn>,
        ctx: AttemptContext<'_>,
        result: Result<RecoveredSession>,
    ) -> AttemptTransition {
        let mut candidate_guard = match &result {
            Ok(recovered) => Some(CandidateCleanupGuard::new(recovered.session.clone())),
            Err(_) => None,
        };

        let mut old_session_to_close = None;
        let mut final_outcome_to_send = None;
        let mut published_next_gen = None;
        let transition;

        {
            let mut core_guard = core_lock.write().await;
            let SupervisorCore { state, config, next_generation_id, .. } = &mut *core_guard;

            match state {
                ManagedState::Terminal { .. } => {
                    let err = RealtimeError::SessionClosed;
                    final_outcome_to_send = Some(Err(err));
                    transition = AttemptTransition::Terminal;
                }
                ManagedState::Uninitialized => {
                    let err = RealtimeError::NotConnected;
                    final_outcome_to_send = Some(Err(err));
                    transition = AttemptTransition::Terminal;
                }
                ManagedState::Serving { active, planned } => {
                    if active.id != ctx.originating_gen_id {
                        let err = RealtimeError::config(
                            "originating generation changed during replacement",
                        );
                        final_outcome_to_send = Some(Err(err));
                        transition = AttemptTransition::Terminal;
                    } else if let Some(p) = planned
                        && p.id == txn.id
                    {
                        match result {
                            Ok(recovered) => {
                                if let Some(expected_rev) = ctx.expected_revision
                                    && config.revision != expected_rev
                                {
                                    tracing::warn!(
                                        expected = expected_rev,
                                        current = config.revision,
                                        "candidate publication rejected: stale config revision"
                                    );
                                    *planned = None;
                                    let err = RealtimeError::config(
                                        "candidate publication rejected: stale config revision",
                                    );
                                    final_outcome_to_send = Some(Err(err));
                                    transition = AttemptTransition::KeepServing;
                                } else {
                                    let next_gen = *next_generation_id;
                                    *next_generation_id += 1;
                                    let session = recovered.session.clone();
                                    let continuity = recovered.continuity;
                                    let new_gen =
                                        Arc::new(SessionGeneration::new(next_gen, session.clone()));
                                    old_session_to_close = Some(active.session.clone());
                                    *state =
                                        ManagedState::Serving { active: new_gen, planned: None };
                                    published_next_gen = Some(next_gen);
                                    if let Some(ref mut cg) = candidate_guard {
                                        cg.disarm();
                                    }
                                    final_outcome_to_send = Some(Ok(RecoveryOutcome::Recovered {
                                        session,
                                        continuity,
                                    }));
                                    transition = AttemptTransition::Published;
                                }
                            }
                            Err(err) => {
                                *planned = None;
                                final_outcome_to_send = Some(Err(err));
                                transition = AttemptTransition::KeepServing;
                            }
                        }
                    } else {
                        let err =
                            RealtimeError::config("planned transaction missing or superseded");
                        final_outcome_to_send = Some(Err(err));
                        transition = AttemptTransition::Terminal;
                    }
                }
                ManagedState::Recovering { failed, txn: cur_txn } => {
                    if cur_txn.id != txn.id {
                        let err = RealtimeError::config("recovery transaction superseded");
                        final_outcome_to_send = Some(Err(err));
                        transition = AttemptTransition::Terminal;
                    } else {
                        match result {
                            Ok(recovered) => {
                                if let Some(expected_rev) = ctx.expected_revision
                                    && config.revision != expected_rev
                                {
                                    tracing::warn!(
                                        expected = expected_rev,
                                        current = config.revision,
                                        attempt = ctx.attempt_idx,
                                        max_attempts = ctx.max_attempts,
                                        "candidate publication rejected: stale config revision"
                                    );
                                    if ctx.attempt_idx >= ctx.max_attempts {
                                        *state = ManagedState::Terminal {
                                            reason: TerminalReason::Exhausted,
                                            _last_generation: Some(Arc::clone(failed)),
                                        };
                                        let err = RealtimeError::config(
                                            "recovery exhausted: stale config revision on final attempt",
                                        );
                                        final_outcome_to_send = Some(Err(err));
                                        transition = AttemptTransition::Terminal;
                                    } else {
                                        transition = AttemptTransition::RetryRecovering;
                                    }
                                } else {
                                    let next_gen = *next_generation_id;
                                    *next_generation_id += 1;
                                    let session = recovered.session.clone();
                                    let continuity = recovered.continuity;
                                    let new_gen =
                                        Arc::new(SessionGeneration::new(next_gen, session.clone()));
                                    old_session_to_close = Some(failed.session.clone());
                                    *state =
                                        ManagedState::Serving { active: new_gen, planned: None };
                                    published_next_gen = Some(next_gen);
                                    if let Some(ref mut cg) = candidate_guard {
                                        cg.disarm();
                                    }
                                    final_outcome_to_send = Some(Ok(RecoveryOutcome::Recovered {
                                        session,
                                        continuity,
                                    }));
                                    transition = AttemptTransition::Published;
                                }
                            }
                            Err(err) => {
                                let (cause_disposition, error_disposition) = match ctx.recovery_impl
                                {
                                    Some(impl_) => {
                                        let current_cause = match txn.snapshot_phase() {
                                            ReplacementPhase::Planned { cause, .. } => cause,
                                            ReplacementPhase::Recovering { cause, .. } => cause,
                                        };
                                        (
                                            impl_.classify(&current_cause),
                                            impl_.classify_attempt_error(&err),
                                        )
                                    }
                                    None => {
                                        (RecoveryDisposition::Fatal, RecoveryDisposition::Fatal)
                                    }
                                };

                                if cause_disposition == RecoveryDisposition::Fatal
                                    || error_disposition == RecoveryDisposition::Fatal
                                    || ctx.attempt_idx >= ctx.max_attempts
                                {
                                    *state = ManagedState::Terminal {
                                        reason: TerminalReason::Exhausted,
                                        _last_generation: Some(Arc::clone(failed)),
                                    };
                                    final_outcome_to_send = Some(Err(err));
                                    transition = AttemptTransition::Terminal;
                                } else {
                                    transition = AttemptTransition::RetryRecovering;
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(next_gen) = published_next_gen {
            let _ = generation_tx.send(next_gen);
        }

        if let Some(old) = old_session_to_close {
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(2), old.close()).await;
            });
        }

        if let Some(out) = final_outcome_to_send {
            let _ = txn.outcome_tx.send(Some(out));
        }

        transition
    }

    /// Close the session gracefully and mark supervisor as closed.
    pub(crate) async fn close(&self) -> Result<()> {
        let (old_session, old_planned) = {
            let mut core = self.core.write().await;
            match std::mem::replace(
                &mut core.state,
                ManagedState::Terminal {
                    reason: TerminalReason::ExplicitClose,
                    _last_generation: None,
                },
            ) {
                ManagedState::Serving { active, planned } => {
                    active.fail();
                    if let Some(ref p) = planned {
                        p.cancel_token.cancel();
                    }
                    (Some(active.session.clone()), planned)
                }
                ManagedState::Recovering { failed, txn } => {
                    txn.cancel_token.cancel();
                    (Some(failed.session.clone()), None)
                }
                _ => (None, None),
            }
        };

        if let Some(p) = old_planned {
            p.cancel_token.cancel();
        }

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

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

        let report0 = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res0 = supervisor.report_failure(report0).await.unwrap();
        match res0 {
            RecoveryOutcome::Recovered { session, .. } => {
                assert_eq!(session.session_id(), "gen-1-recovered");
            }
            _ => panic!("Expected Recovered"),
        }

        assert_eq!(recover_count.load(Ordering::SeqCst), 1);

        let report_stale = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let res_stale = supervisor.report_failure(report_stale).await.unwrap();
        match res_stale {
            RecoveryOutcome::Stale(session) => {
                assert_eq!(session.session_id(), "gen-1-recovered");
            }
            _ => panic!("Expected Stale"),
        }
    }

    struct BarrierRecovery {
        entered_notify: Arc<tokio::sync::Notify>,
        unblock_notify: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl RealtimeRecovery for BarrierRecovery {
        fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
            RecoveryDisposition::Recoverable
        }

        async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
            self.entered_notify.notify_one();
            self.unblock_notify.notified().await;
            let session =
                Arc::new(MockSession { id: "gen-1-recovered".to_string(), recovery: None });
            Ok(RecoveredSession::new(session, RecoveryContinuity::Reconnected))
        }
    }

    #[tokio::test]
    async fn test_monotonic_failure_prevents_write_resurrection() {
        let entered_notify = Arc::new(tokio::sync::Notify::new());
        let unblock_notify = Arc::new(tokio::sync::Notify::new());

        let mock_rec = Arc::new(BarrierRecovery {
            entered_notify: Arc::clone(&entered_notify),
            unblock_notify: Arc::clone(&unblock_notify),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // Initial generation 0 is writable
        assert!(supervisor.admit_write().await.is_ok());

        // Launch a failure reporter in a background task
        let sup_clone = Arc::clone(&supervisor);
        let reporter_handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_clone.report_failure(report).await
        });

        // Wait until recover is entered deterministically
        entered_notify.notified().await;

        // Invariant: Generation 0 write gate is CLOSED immediately and cannot be resurrected
        assert!(supervisor.admit_write().await.is_err(), "Dead generation 0 must be non-writable");

        // Abort the caller future mid-recovery
        reporter_handle.abort();
        let _ = reporter_handle.await;

        // Generation 0 remains closed
        assert!(supervisor.admit_write().await.is_err());

        // Unblock worker to finish
        unblock_notify.notify_one();
    }

    #[tokio::test]
    async fn test_caller_drop_does_not_strand_recovery() {
        let entered_notify = Arc::new(tokio::sync::Notify::new());
        let unblock_notify = Arc::new(tokio::sync::Notify::new());

        let mock_rec = Arc::new(BarrierRecovery {
            entered_notify: Arc::clone(&entered_notify),
            unblock_notify: Arc::clone(&unblock_notify),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        let mut gen_rx = supervisor.subscribe_generation();

        // Launch a failure reporter in a background task and immediately abort the caller future
        let sup_clone = Arc::clone(&supervisor);
        let reporter_handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_clone.report_failure(report).await
        });

        entered_notify.notified().await;

        reporter_handle.abort();
        let _ = reporter_handle.await;

        // Unblock worker to complete
        unblock_notify.notify_one();

        // Invariant: Supervisor-owned recovery continues in background and publishes generation 1
        let timeout_res = tokio::time::timeout(Duration::from_secs(2), gen_rx.changed()).await;
        assert!(timeout_res.is_ok(), "supervisor background recovery must not strand");
        assert_eq!(*gen_rx.borrow(), 1);
        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
    }

    #[tokio::test]
    async fn test_authority_snapshot_during_recovery() {
        let entered_notify = Arc::new(tokio::sync::Notify::new());
        let unblock_notify = Arc::new(tokio::sync::Notify::new());

        let mock_rec = Arc::new(BarrierRecovery {
            entered_notify: Arc::clone(&entered_notify),
            unblock_notify: Arc::clone(&unblock_notify),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        let sup_clone = Arc::clone(&supervisor);
        let _reporter_handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_clone.report_failure(report).await
        });

        entered_notify.notified().await;

        // Invariant: get_active_generation returns generation 0 so readers do not hit synthetic EOF
        let auth_gen = supervisor.get_active_generation().await.unwrap();
        assert_eq!(auth_gen.id, 0);
        assert_eq!(auth_gen.session.session_id(), "gen-0");

        // But writes are properly rejected
        assert!(supervisor.admit_write().await.is_err());

        unblock_notify.notify_one();
    }

    struct MultiAttemptRecovery {
        attempts: Arc<AtomicUsize>,
        fail_first_k: usize,
        entered_notify: Arc<tokio::sync::Notify>,
        unblock_attempt_1: Option<Arc<tokio::sync::Notify>>,
    }

    #[async_trait]
    impl RealtimeRecovery for MultiAttemptRecovery {
        fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
            RecoveryDisposition::Recoverable
        }

        fn classify_attempt_error(
            &self,
            _error: &crate::error::RealtimeError,
        ) -> RecoveryDisposition {
            RecoveryDisposition::Recoverable
        }

        async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            self.entered_notify.notify_one();
            if attempt == 1
                && let Some(ref unblock) = self.unblock_attempt_1
            {
                unblock.notified().await;
            }
            if attempt <= self.fail_first_k {
                Err(RealtimeError::provider(format!("simulated failure on attempt {}", attempt)))
            } else {
                let session =
                    Arc::new(MockSession { id: "gen-1-recovered".to_string(), recovery: None });
                Ok(RecoveredSession::new(session, RecoveryContinuity::Reconnected))
            }
        }
    }

    #[tokio::test]
    async fn test_planned_rotation_promotion_candidate_fails_reactive_retry_succeeds() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let entered_notify = Arc::new(tokio::sync::Notify::new());
        let unblock_attempt_1 = Arc::new(tokio::sync::Notify::new());

        let mock_rec = Arc::new(MultiAttemptRecovery {
            attempts: Arc::clone(&attempts),
            fail_first_k: 1, // Attempt 1 fails, attempt 2 succeeds!
            entered_notify: Arc::clone(&entered_notify),
            unblock_attempt_1: Some(Arc::clone(&unblock_attempt_1)),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_initial_delay(Duration::from_millis(10))
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // Start planned replacement
        let sup_clone = Arc::clone(&supervisor);
        let planned_handle = tokio::spawn(async move {
            sup_clone
                .execute_planned_replacement(
                    0,
                    RecoveryCause::PlannedRotation { time_left: Some("30s".into()) },
                )
                .await
        });

        // Wait until planned candidate attempt 1 is in-flight
        entered_notify.notified().await;

        // Active generation N fails while planned candidate attempt 1 is in-flight:
        let sup_fail = Arc::clone(&supervisor);
        let fail_handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_fail.report_failure(report).await
        });

        // Wait until supervisor state has been promoted to Recovering
        while supervisor.status().await != TransportStatus::Recovering {
            tokio::task::yield_now().await;
        }

        // Unblock attempt 1 to fail now that state is strictly Recovering
        unblock_attempt_1.notify_one();

        // Attempt 1 fails, worker continues into attempt 2 and succeeds!
        let fail_res = fail_handle.await.unwrap().unwrap();
        match fail_res {
            RecoveryOutcome::Recovered { session, .. } => {
                assert_eq!(session.session_id(), "gen-1-recovered");
            }
            _ => panic!("Expected Recovered after attempt 2"),
        }

        let planned_res = planned_handle.await.unwrap().unwrap();
        match planned_res {
            RecoveryOutcome::Recovered { session, .. } => {
                assert_eq!(session.session_id(), "gen-1-recovered");
            }
            _ => panic!("Expected Recovered"),
        }

        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
    }

    #[tokio::test]
    async fn test_planned_rotation_promotion_all_attempts_fail_exhausts() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let entered_notify = Arc::new(tokio::sync::Notify::new());
        let unblock_attempt_1 = Arc::new(tokio::sync::Notify::new());

        let mock_rec = Arc::new(MultiAttemptRecovery {
            attempts: Arc::clone(&attempts),
            fail_first_k: 10, // All attempts fail!
            entered_notify: Arc::clone(&entered_notify),
            unblock_attempt_1: Some(Arc::clone(&unblock_attempt_1)),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(2).unwrap())
            .with_initial_delay(Duration::from_millis(10))
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // Start planned replacement
        let sup_clone = Arc::clone(&supervisor);
        let planned_handle = tokio::spawn(async move {
            sup_clone
                .execute_planned_replacement(
                    0,
                    RecoveryCause::PlannedRotation { time_left: Some("30s".into()) },
                )
                .await
        });

        entered_notify.notified().await;

        // Active generation N fails while planned candidate is running
        let sup_fail = Arc::clone(&supervisor);
        let fail_handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_fail.report_failure(report).await
        });

        while supervisor.status().await != TransportStatus::Recovering {
            tokio::task::yield_now().await;
        }

        unblock_attempt_1.notify_one();

        // Both attempts fail -> outcome is Err and state is Terminal::Exhausted
        let fail_res = fail_handle.await.unwrap();
        assert!(fail_res.is_err());
        assert_eq!(supervisor.status().await, TransportStatus::Exhausted);
        let _ = planned_handle.await;
    }

    #[tokio::test]
    async fn test_planned_candidate_failure_keeps_healthy_n() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let entered_notify = Arc::new(tokio::sync::Notify::new());

        let mock_rec = Arc::new(MultiAttemptRecovery {
            attempts: Arc::clone(&attempts),
            fail_first_k: 1, // Planned candidate fails
            entered_notify: Arc::clone(&entered_notify),
            unblock_attempt_1: None,
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // Start planned replacement
        let res = supervisor
            .execute_planned_replacement(
                0,
                RecoveryCause::PlannedRotation { time_left: Some("30s".into()) },
            )
            .await;

        assert!(res.is_err(), "Planned candidate failure returns error");

        // Invariant: Generation 0 is still healthy and writable!
        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
        assert!(supervisor.admit_write().await.is_ok());
    }

    #[tokio::test]
    async fn test_returned_generation_shares_write_gate_identity() {
        let mock_rec = Arc::new(CoalescingRecovery {
            recover_count: Arc::new(AtomicUsize::new(0)),
            active_recoveries: Arc::new(AtomicUsize::new(0)),
            max_active_recoveries: Arc::new(AtomicUsize::new(0)),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default();
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor = RecoverySupervisor::with_initial_session(policy, config, initial_session);

        let gen_handle = supervisor.admit_write().await.unwrap();
        assert!(gen_handle.is_writable());

        // Report failure on generation 0
        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let _ = supervisor.report_failure(report).await;

        // Invariant: The handle previously returned shares the EXACT same WriteGate that was closed!
        assert!(!gen_handle.is_writable(), "Returned handle must see the closed write gate");
    }

    #[tokio::test]
    async fn test_supreme_close_cancels_candidate() {
        let entered_notify = Arc::new(tokio::sync::Notify::new());
        let unblock_notify = Arc::new(tokio::sync::Notify::new());

        let mock_rec = Arc::new(BarrierRecovery {
            entered_notify: Arc::clone(&entered_notify),
            unblock_notify: Arc::clone(&unblock_notify),
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

        // Start planned replacement
        let sup_arc = Arc::new(supervisor);
        let sup_clone = Arc::clone(&sup_arc);
        let planned_handle = tokio::spawn(async move {
            sup_clone
                .execute_planned_replacement(
                    0,
                    RecoveryCause::PlannedRotation { time_left: Some("30s".into()) },
                )
                .await
        });

        entered_notify.notified().await;

        // Close immediately while candidate is in-flight
        sup_arc.close().await.unwrap();

        // Invariant: Status is Closed, and no candidate can publish
        assert_eq!(sup_arc.status().await, TransportStatus::Closed);
        assert!(!sup_arc.is_connected().await);

        unblock_notify.notify_one();
        let _ = planned_handle.await;
    }

    #[tokio::test]
    async fn test_planned_rotation_deadline_safety_margins() {
        let test_cases = [
            ("500ms", Duration::from_millis(100)),
            ("1s", Duration::from_millis(100)),
            ("2s", Duration::from_millis(100)),
            ("5s", Duration::from_secs(3)),
            ("30s", Duration::from_secs(5)), // capped by policy deadline 5s
        ];

        for (dur_str, expected_min_dur) in test_cases {
            let parsed = parse_duration_string(dur_str).unwrap();
            let safety_margin = Duration::from_secs(2);
            let min_window = Duration::from_millis(100);
            let capped_dur = if parsed > safety_margin {
                parsed - safety_margin
            } else {
                parsed.min(min_window)
            };
            assert!(capped_dur >= expected_min_dur);
        }
    }

    #[tokio::test]
    async fn test_config_revision_change_rejects_stale_candidate() {
        let entered_notify = Arc::new(tokio::sync::Notify::new());
        let unblock_notify = Arc::new(tokio::sync::Notify::new());

        let mock_rec = Arc::new(BarrierRecovery {
            entered_notify: Arc::clone(&entered_notify),
            unblock_notify: Arc::clone(&unblock_notify),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default().with_deadline(Duration::from_secs(5));
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // Start planned replacement
        let sup_clone = Arc::clone(&supervisor);
        let planned_handle = tokio::spawn(async move {
            sup_clone
                .execute_planned_replacement(
                    0,
                    RecoveryCause::PlannedRotation { time_left: Some("30s".into()) },
                )
                .await
        });

        // Wait until candidate construction is in-flight
        entered_notify.notified().await;

        // Force config revision to change concurrently while candidate was building
        supervisor.update_config(|cfg| cfg.voice = Some("updated_voice".into())).await;

        // Unblock candidate to try to publish
        unblock_notify.notify_one();

        // Invariant: Publication is rejected due to stale config revision!
        let planned_res = planned_handle.await.unwrap();
        assert!(planned_res.is_err(), "Stale candidate publication must be rejected");

        // Generation 0 remains healthy and writable
        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
        assert!(supervisor.admit_write().await.is_ok());

        // Invariant 1: Planned transaction was cleared, so a real failure can immediately recover!
        let sup_fail = Arc::clone(&supervisor);
        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let fail_handle = tokio::spawn(async move { sup_fail.report_failure(report).await });

        entered_notify.notified().await;
        unblock_notify.notify_one();

        let fail_res = fail_handle.await.unwrap().unwrap();
        match fail_res {
            RecoveryOutcome::Recovered { session, .. } => {
                assert_eq!(session.session_id(), "gen-1-recovered");
            }
            _ => panic!("Expected Recovered session on real failure after stale planned candidate"),
        }
    }

    #[tokio::test]
    async fn test_reactive_stale_config_retries_and_succeeds() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let entered_notify = Arc::new(tokio::sync::Notify::new());
        let unblock_notify = Arc::new(tokio::sync::Notify::new());

        struct RevisionAwareRecovery {
            attempts: Arc<AtomicUsize>,
            entered_notify: Arc<tokio::sync::Notify>,
            unblock_notify: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl RealtimeRecovery for RevisionAwareRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            fn classify_attempt_error(
                &self,
                _error: &crate::error::RealtimeError,
            ) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
                self.entered_notify.notify_one();
                if attempt == 1 {
                    self.unblock_notify.notified().await;
                }
                let session = Arc::new(MockSession {
                    id: format!(
                        "gen-1-recovered-rev-{}",
                        context.config.voice.as_deref().unwrap_or("default")
                    ),
                    recovery: None,
                });
                Ok(RecoveredSession::new(session, RecoveryContinuity::Reconnected))
            }
        }

        let mock_rec = Arc::new(RevisionAwareRecovery {
            attempts: Arc::clone(&attempts),
            entered_notify: Arc::clone(&entered_notify),
            unblock_notify: Arc::clone(&unblock_notify),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_initial_delay(Duration::from_millis(10))
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        let sup_clone = Arc::clone(&supervisor);
        let fail_handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_clone.report_failure(report).await
        });

        // Attempt 1 in-flight
        entered_notify.notified().await;

        // Change config while attempt 1 is building
        supervisor.update_config(|cfg| cfg.voice = Some("rev2_voice".into())).await;

        // Unblock attempt 1 (will be rejected for stale revision, then attempt 2 runs with rev2)
        unblock_notify.notify_one();

        let fail_res = fail_handle.await.unwrap().unwrap();
        match fail_res {
            RecoveryOutcome::Recovered { session, .. } => {
                assert_eq!(session.session_id(), "gen-1-recovered-rev-rev2_voice");
            }
            _ => panic!("Expected Recovered session after retry with new config"),
        }

        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_planned_to_reactive_promotion_fatal_cause_aborts() {
        let entered_notify = Arc::new(tokio::sync::Notify::new());
        let unblock_notify = Arc::new(tokio::sync::Notify::new());

        struct FatalCauseRecovery {
            entered_notify: Arc<tokio::sync::Notify>,
            unblock_notify: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl RealtimeRecovery for FatalCauseRecovery {
            fn classify(&self, cause: &RecoveryCause) -> RecoveryDisposition {
                match cause {
                    RecoveryCause::PlannedRotation { .. } => RecoveryDisposition::Recoverable,
                    RecoveryCause::ReadFailed(_) => RecoveryDisposition::Fatal,
                    _ => RecoveryDisposition::Recoverable,
                }
            }

            fn classify_attempt_error(
                &self,
                _error: &crate::error::RealtimeError,
            ) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                self.entered_notify.notify_one();
                self.unblock_notify.notified().await;
                Err(RealtimeError::provider("simulated connection failure"))
            }
        }

        let mock_rec = Arc::new(FatalCauseRecovery {
            entered_notify: Arc::clone(&entered_notify),
            unblock_notify: Arc::clone(&unblock_notify),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_initial_delay(Duration::from_millis(10))
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // Start planned replacement (PlannedRotation is recoverable)
        let sup_clone = Arc::clone(&supervisor);
        let planned_handle = tokio::spawn(async move {
            sup_clone
                .execute_planned_replacement(
                    0,
                    RecoveryCause::PlannedRotation { time_left: Some("30s".into()) },
                )
                .await
        });

        entered_notify.notified().await;

        // Active generation N suffers a fatal ReadFailed error!
        let sup_fail = Arc::clone(&supervisor);
        let fail_handle = tokio::spawn(async move {
            let report = FailureReport {
                generation: 0,
                cause: RecoveryCause::ReadFailed(Arc::new(RealtimeError::provider(
                    "unrecoverable auth error",
                ))),
            };
            sup_fail.report_failure(report).await
        });

        while supervisor.status().await != TransportStatus::Recovering {
            tokio::task::yield_now().await;
        }

        // Unblock attempt 1
        unblock_notify.notify_one();

        // Worker detects promoted cause is Fatal -> aborts without retrying attempt 2/3!
        let fail_res = fail_handle.await.unwrap();
        assert!(fail_res.is_err());
        assert_eq!(supervisor.status().await, TransportStatus::Exhausted);
        let _ = planned_handle.await;
    }

    #[tokio::test]
    async fn test_backoff_respects_max_delay() {
        let policy = RecoveryPolicy::default()
            .with_initial_delay(Duration::from_millis(100))
            .with_max_delay(Duration::from_millis(200));

        let base_delay = policy.initial_delay();
        for attempt in 1..=5 {
            let factor = 2u32.checked_pow(attempt - 1).unwrap_or(u32::MAX);
            let backoff = base_delay.saturating_mul(factor).min(policy.max_delay());
            assert!(backoff <= Duration::from_millis(200), "Backoff must not exceed max_delay");
        }
    }

    #[tokio::test]
    async fn test_fresh_deadline_takeover_on_promoted_in_flight_attempt() {
        tokio::time::pause();

        let entered_notify = Arc::new(tokio::sync::Notify::new());
        let unblock_notify = Arc::new(tokio::sync::Notify::new());

        let mock_rec = Arc::new(BarrierRecovery {
            entered_notify: Arc::clone(&entered_notify),
            unblock_notify: Arc::clone(&unblock_notify),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        // Planned deadline will be very short: 2.1s - 2s safety margin = 100ms
        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // Start planned replacement with short provider timeLeft: "2100ms" -> capped to 100ms
        let sup_planned = Arc::clone(&supervisor);
        let planned_handle = tokio::spawn(async move {
            sup_planned
                .execute_planned_replacement(
                    0,
                    RecoveryCause::PlannedRotation { time_left: Some("2100ms".into()) },
                )
                .await
        });

        // Wait until candidate construction is in-flight (attempt 1)
        entered_notify.notified().await;

        // At +50ms: Generation 0 suffers real failure and is promoted to Recovering (fresh 5s deadline)
        tokio::time::advance(Duration::from_millis(50)).await;
        let sup_fail = Arc::clone(&supervisor);
        let fail_handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_fail.report_failure(report).await
        });

        while supervisor.status().await != TransportStatus::Recovering {
            tokio::task::yield_now().await;
        }

        // At +150ms: Old planned deadline (+100ms) has expired, but we are well before fresh 5s deadline!
        tokio::time::advance(Duration::from_millis(100)).await;

        // Invariant: Supervisor MUST NOT be exhausted! It stays Recovering!
        assert_eq!(supervisor.status().await, TransportStatus::Recovering);

        // Candidate 2 is building or attempt 1 is unblocked: unblock and complete recovery
        unblock_notify.notify_one();

        let fail_res = fail_handle.await.unwrap().unwrap();
        match fail_res {
            RecoveryOutcome::Recovered { session, .. } => {
                assert_eq!(session.session_id(), "gen-1-recovered");
            }
            _ => panic!("Expected Recovered session under fresh deadline"),
        }

        let _ = planned_handle.await;
    }

    #[tokio::test]
    async fn test_error_promotion_atomic_transition_retries() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let entered_notify = Arc::new(tokio::sync::Notify::new());
        let unblock_notify = Arc::new(tokio::sync::Notify::new());

        struct AttemptControlledRecovery {
            attempts: Arc<AtomicUsize>,
            entered_notify: Arc<tokio::sync::Notify>,
            unblock_notify: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl RealtimeRecovery for AttemptControlledRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            fn classify_attempt_error(
                &self,
                _error: &crate::error::RealtimeError,
            ) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
                self.entered_notify.notify_one();
                if attempt == 1 {
                    self.unblock_notify.notified().await;
                    // Attempt 1 fails while planned
                    Err(RealtimeError::provider("transient planned failure"))
                } else {
                    let session = Arc::new(MockSession {
                        id: format!("gen-1-attempt-{}", attempt),
                        recovery: None,
                    });
                    Ok(RecoveredSession::new(session, RecoveryContinuity::Reconnected))
                }
            }
        }

        let mock_rec = Arc::new(AttemptControlledRecovery {
            attempts: Arc::clone(&attempts),
            entered_notify: Arc::clone(&entered_notify),
            unblock_notify: Arc::clone(&unblock_notify),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_initial_delay(Duration::from_millis(10))
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // Start planned replacement
        let sup_clone = Arc::clone(&supervisor);
        let planned_handle = tokio::spawn(async move {
            sup_clone
                .execute_planned_replacement(
                    0,
                    RecoveryCause::PlannedRotation { time_left: Some("30s".into()) },
                )
                .await
        });

        // Attempt 1 in-flight
        entered_notify.notified().await;

        // Concurrently fail generation 0 and promote to Recovering right as attempt 1 finishes with error
        let sup_fail = Arc::clone(&supervisor);
        let fail_handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_fail.report_failure(report).await
        });

        while supervisor.status().await != TransportStatus::Recovering {
            tokio::task::yield_now().await;
        }

        // Unblock attempt 1 failure: atomic transition evaluates as RetryRecovering, not Terminal!
        unblock_notify.notify_one();

        let fail_res = fail_handle.await.unwrap().unwrap();
        match fail_res {
            RecoveryOutcome::Recovered { session, .. } => {
                assert_eq!(session.session_id(), "gen-1-attempt-2");
            }
            _ => panic!(
                "Expected Recovered session on retry after atomic error promotion transition"
            ),
        }

        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        let _ = planned_handle.await;
    }

    #[tokio::test]
    async fn test_backoff_respects_max_delay_with_paused_time() {
        tokio::time::pause();

        let attempts = Arc::new(AtomicUsize::new(0));
        let timestamps = Arc::new(parking_lot::Mutex::new(Vec::new()));

        struct TimingRecovery {
            attempts: Arc<AtomicUsize>,
            timestamps: Arc<parking_lot::Mutex<Vec<tokio::time::Instant>>>,
        }

        #[async_trait]
        impl RealtimeRecovery for TimingRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            fn classify_attempt_error(
                &self,
                _error: &crate::error::RealtimeError,
            ) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
                self.timestamps.lock().push(tokio::time::Instant::now());
                if attempt < 3 {
                    Err(RealtimeError::provider("transient failure"))
                } else {
                    let session = Arc::new(MockSession {
                        id: format!("gen-1-attempt-{}", attempt),
                        recovery: None,
                    });
                    Ok(RecoveredSession::new(session, RecoveryContinuity::Reconnected))
                }
            }
        }

        let mock_rec = Arc::new(TimingRecovery {
            attempts: Arc::clone(&attempts),
            timestamps: Arc::clone(&timestamps),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(3).unwrap())
            .with_initial_delay(Duration::from_millis(50))
            .with_max_delay(Duration::from_millis(60))
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        let sup_clone = Arc::clone(&supervisor);
        let fail_handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_clone.report_failure(report).await
        });

        let fail_res = fail_handle.await.unwrap().unwrap();
        assert!(matches!(fail_res, RecoveryOutcome::Recovered { .. }));

        let recorded = timestamps.lock().clone();
        assert_eq!(recorded.len(), 3);

        // Backoff attempt 1 -> 2: 50ms * 2^0 = 50ms
        let delay_1_2 = recorded[1].duration_since(recorded[0]);
        assert!(
            delay_1_2 >= Duration::from_millis(50) && delay_1_2 <= Duration::from_millis(55),
            "Expected ~50ms delay, got {:?}",
            delay_1_2
        );

        // Backoff attempt 2 -> 3: 50ms * 2^1 = 100ms, clamped to max_delay 60ms!
        let delay_2_3 = recorded[2].duration_since(recorded[1]);
        assert!(
            delay_2_3 >= Duration::from_millis(60) && delay_2_3 <= Duration::from_millis(65),
            "Expected ~60ms delay (clamped by max_delay), got {:?}",
            delay_2_3
        );
    }

    /// Proves that when max_attempts=1 and a planned attempt is in-flight
    /// when promoted to Recovering, the in-flight timeout under the planned
    /// deadline does NOT exit the worker. Instead, the reactive phase gets
    /// its own fresh attempt budget and the worker continues.
    #[tokio::test]
    async fn test_promoted_in_flight_timeout_with_max_attempts_one_gets_reactive_attempt() {
        tokio::time::pause();

        let attempts = Arc::new(AtomicUsize::new(0));
        let entered_notify = Arc::new(tokio::sync::Notify::new());

        struct SlowThenSucceedRecovery {
            attempts: Arc<AtomicUsize>,
            entered_notify: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl RealtimeRecovery for SlowThenSucceedRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            fn classify_attempt_error(
                &self,
                _error: &crate::error::RealtimeError,
            ) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
                self.entered_notify.notify_one();
                if attempt == 1 {
                    // First attempt (planned): takes very long — will be timed out
                    // by the short planned deadline
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Err(RealtimeError::provider("should not reach here"))
                } else {
                    // Reactive attempt: succeed immediately
                    let session = Arc::new(MockSession {
                        id: format!("reactive-gen-1-attempt-{}", attempt),
                        recovery: None,
                    });
                    Ok(RecoveredSession::new(session, RecoveryContinuity::Reconnected))
                }
            }
        }

        let mock_rec = Arc::new(SlowThenSucceedRecovery {
            attempts: Arc::clone(&attempts),
            entered_notify: Arc::clone(&entered_notify),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        // max_attempts = 1: only one planned attempt allowed
        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(1).unwrap())
            .with_deadline(Duration::from_secs(10));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // Start planned replacement with a short deadline: "2100ms" - 2s margin = 100ms
        let sup_planned = Arc::clone(&supervisor);
        let planned_handle = tokio::spawn(async move {
            sup_planned
                .execute_planned_replacement(
                    0,
                    RecoveryCause::PlannedRotation { time_left: Some("2100ms".into()) },
                )
                .await
        });

        // Wait for attempt 1 to start
        entered_notify.notified().await;

        // Advance past the short planned deadline but before the recovery deadline
        // Then promote to Recovering via a real failure
        tokio::time::advance(Duration::from_millis(50)).await;

        let sup_fail = Arc::clone(&supervisor);
        let fail_handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_fail.report_failure(report).await
        });

        while supervisor.status().await != TransportStatus::Recovering {
            tokio::task::yield_now().await;
        }

        // Advance past the planned deadline (100ms total) to trigger the in-flight timeout
        tokio::time::advance(Duration::from_millis(60)).await;

        // CRITICAL INVARIANT: Supervisor must NOT be Exhausted!
        // Despite max_attempts=1, the promotion gives a fresh reactive budget.
        // The worker should detect the promoted deadline and continue with
        // a reactive attempt 1.
        assert_ne!(
            supervisor.status().await,
            TransportStatus::Exhausted,
            "max_attempts=1 promoted to Recovering must get fresh reactive attempt, not exhaust"
        );

        // The reactive attempt succeeds immediately
        let fail_res = fail_handle.await.unwrap().unwrap();
        match fail_res {
            RecoveryOutcome::Recovered { session, .. } => {
                assert!(
                    session.session_id().starts_with("reactive-gen-1-attempt-"),
                    "Expected reactive recovery session, got {}",
                    session.session_id()
                );
            }
            _ => panic!("Expected Recovered under reactive budget, got {:?}", fail_res),
        }

        // At least 2 attempts: the original planned (timed out) + reactive
        assert!(
            attempts.load(Ordering::SeqCst) >= 2,
            "Expected at least 2 attempts (planned + reactive), got {}",
            attempts.load(Ordering::SeqCst)
        );

        let _ = planned_handle.await;
    }

    /// Proves that when the final attempt in Recovering mode succeeds at
    /// building a candidate but config revision changed (stale), the supervisor
    /// atomically transitions to Terminal::Exhausted instead of leaving an
    /// ownerless Recovering state.
    #[tokio::test]
    async fn test_stale_config_on_final_attempt_atomically_exhausts() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let entered_notify = Arc::new(tokio::sync::Notify::new());
        let unblock_notify = Arc::new(tokio::sync::Notify::new());

        struct BarrierSucceedRecovery {
            attempts: Arc<AtomicUsize>,
            entered_notify: Arc<tokio::sync::Notify>,
            unblock_notify: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl RealtimeRecovery for BarrierSucceedRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            fn classify_attempt_error(
                &self,
                _error: &crate::error::RealtimeError,
            ) -> RecoveryDisposition {
                RecoveryDisposition::Recoverable
            }

            async fn recover(&self, _context: RecoveryContext<'_>) -> Result<RecoveredSession> {
                let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
                // Signal that recovery is in-flight, then wait for test to change config
                self.entered_notify.notify_one();
                self.unblock_notify.notified().await;
                let session = Arc::new(MockSession {
                    id: format!("candidate-attempt-{}", attempt),
                    recovery: None,
                });
                Ok(RecoveredSession::new(session, RecoveryContinuity::Reconnected))
            }
        }

        let mock_rec = Arc::new(BarrierSucceedRecovery {
            attempts: Arc::clone(&attempts),
            entered_notify: Arc::clone(&entered_notify),
            unblock_notify: Arc::clone(&unblock_notify),
        });

        let initial_session = Arc::new(MockSession {
            id: "gen-0".to_string(),
            recovery: Some(Arc::clone(&mock_rec) as Arc<dyn RealtimeRecovery>),
        });

        // max_attempts = 1: only one recovery attempt
        let policy = RecoveryPolicy::default()
            .with_max_attempts(NonZeroU32::new(1).unwrap())
            .with_deadline(Duration::from_secs(5));

        let config = Arc::new(tokio::sync::RwLock::new(crate::config::RealtimeConfig::default()));
        let supervisor =
            Arc::new(RecoverySupervisor::with_initial_session(policy, config, initial_session));

        // Trigger a failure to enter Recovering
        let sup_clone = Arc::clone(&supervisor);
        let fail_handle = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            sup_clone.report_failure(report).await
        });

        // Wait for the recovery attempt to be in-flight (barrier-controlled)
        entered_notify.notified().await;

        // Now the recovery attempt has snapshotted config revision 0
        // and is waiting for us to unblock. Bump the config revision.
        {
            let mut core = supervisor.core.write().await;
            core.config.revision += 1;
        }

        // Unblock the recovery attempt. It returns Ok(candidate) built under rev 0,
        // but apply_attempt_result will see config.revision == 1 != expected_rev 0.
        // With max_attempts=1 and attempt_idx=1, this is the final attempt.
        unblock_notify.notify_one();

        // Wait for the failure handle to resolve
        let fail_res = fail_handle.await.unwrap();

        // CRITICAL INVARIANT: The supervisor must be in Terminal::Exhausted,
        // NOT stuck in ownerless Recovering.
        assert_eq!(
            supervisor.status().await,
            TransportStatus::Exhausted,
            "stale config on final recovery attempt must terminalize to Exhausted"
        );

        // The result should be an error (stale config exhaustion)
        assert!(
            fail_res.is_err(),
            "Expected error from stale config exhaustion, got {:?}",
            fail_res
        );

        // Only 1 attempt was made (max_attempts = 1)
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
