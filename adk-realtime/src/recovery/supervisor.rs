#![allow(unfulfilled_lint_expectations)]

use crate::error::{RealtimeError, Result};
use crate::recovery::{
    RecoveryCause, RecoveryContext, RecoveryContinuity, RecoveryDisposition, RecoveryPolicy,
};
use crate::session::RealtimeSession;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// A supervisor-owned replacement transaction (planned or reactive).
pub(crate) struct ReplacementTxn {
    pub(crate) id: u64,
    pub(crate) _target_generation_id: u64,
    pub(crate) _cause: RecoveryCause,
    pub(crate) deadline: tokio::time::Instant,
    pub(crate) cancel_token: CancellationToken,
    pub(crate) outcome_tx: tokio::sync::watch::Sender<Option<Result<RecoveryOutcome>>>,
    pub(crate) outcome_rx: tokio::sync::watch::Receiver<Option<Result<RecoveryOutcome>>>,
}

impl ReplacementTxn {
    pub fn new(
        id: u64,
        target_generation_id: u64,
        cause: RecoveryCause,
        deadline: tokio::time::Instant,
    ) -> Self {
        let (outcome_tx, outcome_rx) = tokio::sync::watch::channel(None);
        Self {
            id,
            _target_generation_id: target_generation_id,
            _cause: cause,
            deadline,
            cancel_token: CancellationToken::new(),
            outcome_tx,
            outcome_rx,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalReason {
    ExplicitClose,
    Exhausted,
}

/// Closed algebraic state machine for supervisor transport lifecycle.
pub(crate) enum ManagedState {
    Uninitialized,

    Serving { active: Arc<SessionGeneration>, planned: Option<Arc<ReplacementTxn>> },

    Recovering { failed: Arc<SessionGeneration>, txn: Arc<ReplacementTxn> },

    Terminal { reason: TerminalReason, _last_generation: Option<Arc<SessionGeneration>> },
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
    state: Arc<tokio::sync::RwLock<ManagedState>>,
    config: Arc<tokio::sync::RwLock<ConfigSnapshot>>,
    next_generation_id: Arc<AtomicU64>,
    next_txn_id: Arc<AtomicU64>,
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
            state: Arc::new(tokio::sync::RwLock::new(ManagedState::Uninitialized)),
            config: Arc::new(tokio::sync::RwLock::new(ConfigSnapshot {
                config: initial_config,
                revision: 0,
            })),
            next_generation_id: Arc::new(AtomicU64::new(0)),
            next_txn_id: Arc::new(AtomicU64::new(1)),
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
            state: Arc::new(tokio::sync::RwLock::new(ManagedState::Serving {
                active: gen_0,
                planned: None,
            })),
            config: Arc::new(tokio::sync::RwLock::new(ConfigSnapshot {
                config: initial_cfg,
                revision: 0,
            })),
            next_generation_id: Arc::new(AtomicU64::new(1)),
            next_txn_id: Arc::new(AtomicU64::new(1)),
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
        let mut state = self.state.write().await;
        if !matches!(*state, ManagedState::Uninitialized) {
            return Err(RealtimeError::config(
                "cannot set initial session unless supervisor status is strictly Uninitialized",
            ));
        }
        let gen_id = self.next_generation_id.fetch_add(1, Ordering::SeqCst);
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
        let state = self.state.read().await;
        match &*state {
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
        let state = self.state.read().await;
        match &*state {
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
        let state = self.state.read().await;
        match &*state {
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
        let state = self.state.read().await;
        match &*state {
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
        let mut cfg = self.config.write().await;
        mutate(&mut cfg.config);
        cfg.revision = cfg.revision.wrapping_add(1);
        cfg.clone()
    }

    /// Get coherent configuration and revision snapshot.
    pub(crate) async fn get_config(&self) -> ConfigSnapshot {
        self.config.read().await.clone()
    }

    /// Atomically publish a newly created replacement session.
    pub(crate) async fn publish_replacement(
        &self,
        new_session: Arc<dyn RealtimeSession>,
        expected_revision: u64,
    ) -> Result<SessionGeneration> {
        let current_rev = self.config.read().await.revision;
        if current_rev != expected_revision {
            return Err(RealtimeError::config("stale config revision during publication"));
        }

        let mut candidate_guard = CandidateCleanupGuard::new(new_session.clone());

        let (next_gen, old_session) = {
            let mut state = self.state.write().await;
            match &mut *state {
                ManagedState::Terminal { .. } => return Err(RealtimeError::SessionClosed),
                ManagedState::Uninitialized => return Err(RealtimeError::NotConnected),
                ManagedState::Serving { active, planned } => {
                    if let Some(p) = planned.take() {
                        p.cancel_token.cancel();
                    }
                    let next_gen = self.next_generation_id.fetch_add(1, Ordering::SeqCst);
                    let new_gen = Arc::new(SessionGeneration::new(next_gen, new_session.clone()));
                    let old = active.session.clone();
                    *state = ManagedState::Serving { active: new_gen, planned: None };
                    (next_gen, Some(old))
                }
                ManagedState::Recovering { failed, txn } => {
                    txn.cancel_token.cancel();
                    let next_gen = self.next_generation_id.fetch_add(1, Ordering::SeqCst);
                    let new_gen = Arc::new(SessionGeneration::new(next_gen, new_session.clone()));
                    let old = failed.session.clone();
                    *state = ManagedState::Serving { active: new_gen, planned: None };
                    (next_gen, Some(old))
                }
            }
        };

        candidate_guard.disarm();
        let _ = self.generation_tx.send(next_gen);

        if let Some(old) = old_session {
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(2), old.close()).await;
            });
        }

        Ok(SessionGeneration {
            id: next_gen,
            session: new_session,
            write_gate: WriteGate::new_open(),
        })
    }

    /// Execute an intentional context resumption under the supervisor.
    pub(crate) async fn execute_context_resumption(
        &self,
        model: &crate::model::BoxedModel,
        queued_snapshot: ConfigSnapshot,
    ) -> Result<SessionGeneration> {
        let (current_active, current_rev) = {
            let state = self.state.read().await;
            match &*state {
                ManagedState::Terminal { .. } => return Err(RealtimeError::SessionClosed),
                ManagedState::Uninitialized => return Err(RealtimeError::NotConnected),
                ManagedState::Recovering { .. } => return Err(RealtimeError::NotConnected),
                ManagedState::Serving { active, .. } => {
                    (Arc::clone(active), self.config.read().await.revision)
                }
            }
        };

        if current_rev != queued_snapshot.revision {
            return Err(RealtimeError::config("stale config revision prior to resumption connect"));
        }

        tracing::info!("Executing intentional context resumption under supervisor.");
        let new_session = model.connect(queued_snapshot.config).await?;
        let new_session_arc: Arc<dyn RealtimeSession> = Arc::from(new_session);

        let mut candidate_guard = CandidateCleanupGuard::new(new_session_arc.clone());

        let (next_gen, old_session) = {
            let mut state = self.state.write().await;
            let current_rev_after = self.config.read().await.revision;
            if current_rev_after != queued_snapshot.revision {
                return Err(RealtimeError::config("stale config revision post-resumption connect"));
            }

            match &mut *state {
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
                    let next_gen = self.next_generation_id.fetch_add(1, Ordering::SeqCst);
                    let new_gen =
                        Arc::new(SessionGeneration::new(next_gen, new_session_arc.clone()));
                    let old = active.session.clone();
                    *state = ManagedState::Serving { active: new_gen, planned: None };
                    (next_gen, old)
                }
                _ => return Err(RealtimeError::NotConnected),
            }
        };

        candidate_guard.disarm();
        let _ = self.generation_tx.send(next_gen);

        tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(2), old_session.close()).await;
        });

        Ok(SessionGeneration {
            id: next_gen,
            session: new_session_arc,
            write_gate: WriteGate::new_open(),
        })
    }

    /// Execute a proactive planned replacement (make-before-break).
    pub(crate) async fn execute_planned_replacement(
        &self,
        report_generation: u64,
        cause: RecoveryCause,
    ) -> Result<RecoveryOutcome> {
        let (txn, mut outcome_rx) = {
            let mut state = self.state.write().await;
            match &mut *state {
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
                                    let provider_deadline =
                                        tokio::time::Instant::now() + provider_dur;
                                    let safety_margin = Duration::from_secs(2);
                                    let capped = provider_deadline
                                        .checked_sub(safety_margin)
                                        .unwrap_or(provider_deadline);
                                    policy_deadline.min(capped)
                                } else {
                                    policy_deadline
                                }
                            }
                            _ => policy_deadline,
                        };

                        let next_txn_id = self.next_txn_id.fetch_add(1, Ordering::SeqCst);
                        let txn = Arc::new(ReplacementTxn::new(
                            next_txn_id,
                            active.id + 1,
                            cause.clone(),
                            deadline,
                        ));
                        let rx = txn.outcome_rx.clone();
                        let active_arc = Arc::clone(active);
                        *planned = Some(Arc::clone(&txn));

                        self.spawn_planned_worker(active_arc, Arc::clone(&txn), cause);

                        (txn, rx)
                    }
                }
            }
        };

        Self::await_txn_outcome(&mut outcome_rx, txn.deadline).await
    }

    /// Report a failure in the active session.
    pub(crate) async fn report_failure(&self, report: FailureReport) -> Result<RecoveryOutcome> {
        let (txn, mut outcome_rx) = {
            let mut state = self.state.write().await;
            match &mut *state {
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
                        let txn = Arc::clone(&planned_txn);
                        let rx = txn.outcome_rx.clone();
                        *state = ManagedState::Recovering { failed: active_arc, txn: planned_txn };
                        (txn, rx)
                    } else {
                        let next_txn_id = self.next_txn_id.fetch_add(1, Ordering::SeqCst);
                        let deadline = tokio::time::Instant::now() + self.policy.deadline();
                        let txn = Arc::new(ReplacementTxn::new(
                            next_txn_id,
                            active_arc.id + 1,
                            report.cause.clone(),
                            deadline,
                        ));
                        let rx = txn.outcome_rx.clone();
                        *state = ManagedState::Recovering {
                            failed: Arc::clone(&active_arc),
                            txn: Arc::clone(&txn),
                        };

                        self.spawn_recovery_worker(
                            Arc::clone(&active_arc),
                            Arc::clone(&txn),
                            report.cause.clone(),
                        );

                        (txn, rx)
                    }
                }
            }
        };

        Self::await_txn_outcome(&mut outcome_rx, txn.deadline).await
    }

    async fn await_txn_outcome(
        outcome_rx: &mut tokio::sync::watch::Receiver<Option<Result<RecoveryOutcome>>>,
        deadline: tokio::time::Instant,
    ) -> Result<RecoveryOutcome> {
        loop {
            if let Some(ref outcome) = *outcome_rx.borrow_and_update() {
                return outcome.clone();
            }

            let timeout_res = tokio::time::timeout_at(deadline, outcome_rx.changed()).await;
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

    fn spawn_recovery_worker(
        &self,
        failed_gen: Arc<SessionGeneration>,
        txn: Arc<ReplacementTxn>,
        cause: RecoveryCause,
    ) {
        let state_lock = Arc::clone(&self.state);
        let config_lock = Arc::clone(&self.config);
        let next_gen_atomic = Arc::clone(&self.next_generation_id);
        let generation_tx = self.generation_tx.clone();
        let policy = self.policy.clone();
        #[cfg(any(test, feature = "recovery-test-utils"))]
        let test_recovery_barrier = Arc::clone(&self.test_recovery_barrier);

        tokio::spawn(async move {
            #[cfg(any(test, feature = "recovery-test-utils"))]
            {
                let maybe_barrier = test_recovery_barrier.lock().take();
                if let Some(barrier) = maybe_barrier {
                    barrier.on_recovering().await;
                }
            }

            let recovery_impl = match failed_gen.session.recovery() {
                Some(r) => r,
                None => {
                    let err = RealtimeError::provider("active session does not support recovery");
                    tracing::error!(generation = failed_gen.id, err = %err, "recovery not supported");
                    let mut state = state_lock.write().await;
                    if let ManagedState::Recovering { txn: current_txn, .. } = &*state
                        && current_txn.id == txn.id
                    {
                        *state = ManagedState::Terminal {
                            reason: TerminalReason::Exhausted,
                            _last_generation: Some(Arc::clone(&failed_gen)),
                        };
                    }
                    let _ = txn.outcome_tx.send(Some(Err(err)));
                    return;
                }
            };

            if recovery_impl.classify(&cause) == RecoveryDisposition::Fatal {
                let err = RealtimeError::provider(
                    "fatal recovery cause detected; performing zero provider attempts",
                );
                tracing::warn!(generation = failed_gen.id, err = %err, "fatal cause classification");
                let mut state = state_lock.write().await;
                if let ManagedState::Recovering { txn: current_txn, .. } = &*state
                    && current_txn.id == txn.id
                {
                    *state = ManagedState::Terminal {
                        reason: TerminalReason::Exhausted,
                        _last_generation: Some(Arc::clone(&failed_gen)),
                    };
                }
                let _ = txn.outcome_tx.send(Some(Err(err)));
                return;
            }

            let max_attempts = policy.max_attempts().get();
            let mut attempts_launched = 0;
            let mut last_provider_error: Option<RealtimeError> = None;
            let mut stop_reason = None;

            for attempt_idx in 1..=max_attempts {
                if txn.cancel_token.is_cancelled() {
                    tracing::info!(generation = failed_gen.id, "recovery transaction cancelled");
                    let _ = txn.outcome_tx.send(Some(Err(RealtimeError::SessionClosed)));
                    return;
                }

                let now = tokio::time::Instant::now();
                if now >= txn.deadline {
                    let msg = "recovery deadline expired before attempt".to_string();
                    tracing::warn!(generation = failed_gen.id, attempt = attempt_idx, err = %msg, "recovery deadline expired");
                    stop_reason = Some(RealtimeError::Timeout(msg));
                    break;
                }

                let attempt_nz = NonZeroU32::new(attempt_idx).unwrap();
                let snapshot = config_lock.read().await.clone();

                attempts_launched += 1;
                let remaining_dur =
                    txn.deadline.saturating_duration_since(tokio::time::Instant::now());
                let context_deadline = std::time::Instant::now()
                    .checked_add(remaining_dur)
                    .unwrap_or_else(std::time::Instant::now);

                let context =
                    RecoveryContext::new(attempt_nz, &cause, &snapshot.config, context_deadline);

                tracing::info!(
                    generation = failed_gen.id,
                    attempt = attempt_idx,
                    "provider recovery attempt initiated"
                );
                let recover_fut = recovery_impl.recover(context);

                let attempt_res = tokio::select! {
                    _ = txn.cancel_token.cancelled() => {
                        let _ = txn.outcome_tx.send(Some(Err(RealtimeError::SessionClosed)));
                        return;
                    }
                    res = tokio::time::timeout_at(txn.deadline, recover_fut) => {
                        match res {
                            Ok(r) => r,
                            Err(_) => {
                                let msg = "provider recovery attempt timed out".to_string();
                                stop_reason = Some(RealtimeError::Timeout(msg));
                                break;
                            }
                        }
                    }
                };

                match attempt_res {
                    Ok(recovered) => {
                        let new_session = recovered.session();
                        let continuity = recovered.continuity();
                        let mut candidate_guard = CandidateCleanupGuard::new(new_session.clone());

                        let (next_gen, old_session) = {
                            let mut state = state_lock.write().await;
                            let current_rev = config_lock.read().await.revision;
                            if current_rev != snapshot.revision {
                                tracing::warn!(
                                    "recovery candidate rejected due to stale config revision during attempt"
                                );
                                last_provider_error =
                                    Some(RealtimeError::config("stale config revision"));
                                continue;
                            }

                            match &mut *state {
                                ManagedState::Terminal { .. } => {
                                    return;
                                }
                                ManagedState::Recovering { failed, txn: cur_txn } => {
                                    if cur_txn.id != txn.id {
                                        return;
                                    }
                                    let next_gen = next_gen_atomic.fetch_add(1, Ordering::SeqCst);
                                    let new_gen = Arc::new(SessionGeneration::new(
                                        next_gen,
                                        new_session.clone(),
                                    ));
                                    let old = failed.session.clone();
                                    *state =
                                        ManagedState::Serving { active: new_gen, planned: None };
                                    (next_gen, old)
                                }
                                _ => return,
                            }
                        };

                        candidate_guard.disarm();
                        let _ = generation_tx.send(next_gen);

                        tokio::spawn(async move {
                            let _ =
                                tokio::time::timeout(Duration::from_secs(2), old_session.close())
                                    .await;
                        });

                        let outcome =
                            RecoveryOutcome::Recovered { session: new_session, continuity };
                        let _ = txn.outcome_tx.send(Some(Ok(outcome)));
                        return;
                    }
                    Err(err) => {
                        let disposition = recovery_impl.classify_attempt_error(&err);
                        last_provider_error = Some(err);

                        if disposition == RecoveryDisposition::Fatal {
                            tracing::warn!(
                                generation = failed_gen.id,
                                attempt = attempt_idx,
                                "fatal attempt error classification; no retry"
                            );
                            stop_reason = Some(RealtimeError::ProviderError(format!(
                                "recovery aborted after {} attempt(s) due to fatal error",
                                attempts_launched
                            )));
                            break;
                        }

                        if attempt_idx < max_attempts {
                            let factor = 2u32.checked_pow(attempt_idx - 1).unwrap_or(u32::MAX);
                            let mut backoff = policy
                                .initial_delay()
                                .saturating_mul(factor)
                                .min(policy.max_delay());
                            let now_after = tokio::time::Instant::now();
                            if now_after >= txn.deadline {
                                stop_reason = Some(RealtimeError::Timeout(
                                    "recovery deadline expired during backoff calculation"
                                        .to_string(),
                                ));
                                break;
                            }
                            backoff =
                                backoff.min(txn.deadline.saturating_duration_since(now_after));
                            if !backoff.is_zero() {
                                tokio::select! {
                                    _ = txn.cancel_token.cancelled() => {
                                        let _ = txn.outcome_tx.send(Some(Err(RealtimeError::SessionClosed)));
                                        return;
                                    }
                                    _ = tokio::time::sleep(backoff) => {}
                                }
                            }
                        }
                    }
                }
            }

            let final_err = stop_reason.unwrap_or_else(|| {
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
            });

            {
                let mut state = state_lock.write().await;
                if let ManagedState::Recovering { txn: cur_txn, .. } = &*state
                    && cur_txn.id == txn.id
                {
                    *state = ManagedState::Terminal {
                        reason: TerminalReason::Exhausted,
                        _last_generation: Some(Arc::clone(&failed_gen)),
                    };
                }
            }

            let _ = txn.outcome_tx.send(Some(Err(final_err)));
        });
    }

    fn spawn_planned_worker(
        &self,
        active_gen: Arc<SessionGeneration>,
        txn: Arc<ReplacementTxn>,
        cause: RecoveryCause,
    ) {
        let state_lock = Arc::clone(&self.state);
        let config_lock = Arc::clone(&self.config);
        let next_gen_atomic = Arc::clone(&self.next_generation_id);
        let generation_tx = self.generation_tx.clone();

        tokio::spawn(async move {
            let recovery_impl = match active_gen.session.recovery() {
                Some(r) => r,
                None => {
                    let err = RealtimeError::provider("active session does not support recovery");
                    let _ = txn.outcome_tx.send(Some(Err(err)));
                    return;
                }
            };

            if recovery_impl.classify(&cause) == RecoveryDisposition::Fatal {
                let err = RealtimeError::provider(
                    "fatal cause detected for planned replacement; performing zero provider attempts",
                );
                let _ = txn.outcome_tx.send(Some(Err(err)));
                return;
            }

            let snapshot = config_lock.read().await.clone();
            let remaining_dur = txn.deadline.saturating_duration_since(tokio::time::Instant::now());
            let context_deadline = std::time::Instant::now()
                .checked_add(remaining_dur)
                .unwrap_or_else(std::time::Instant::now);

            let context = RecoveryContext::new(
                NonZeroU32::new(1).unwrap(),
                &cause,
                &snapshot.config,
                context_deadline,
            );

            let recover_fut = recovery_impl.recover(context);
            let attempt_res = tokio::select! {
                _ = txn.cancel_token.cancelled() => {
                    let _ = txn.outcome_tx.send(Some(Err(RealtimeError::SessionClosed)));
                    return;
                }
                res = tokio::time::timeout_at(txn.deadline, recover_fut) => {
                    match res {
                        Ok(r) => r,
                        Err(_) => {
                            let msg = "planned replacement attempt timed out waiting for setupComplete".to_string();
                            let _ = txn.outcome_tx.send(Some(Err(RealtimeError::Timeout(msg))));
                            return;
                        }
                    }
                }
            };

            match attempt_res {
                Ok(recovered) => {
                    let new_session = recovered.session();
                    let continuity = recovered.continuity();
                    let mut candidate_guard = CandidateCleanupGuard::new(new_session.clone());

                    let (next_gen, old_session) = {
                        let mut state = state_lock.write().await;
                        let current_rev = config_lock.read().await.revision;
                        if current_rev != snapshot.revision {
                            let _ = txn
                                .outcome_tx
                                .send(Some(Err(RealtimeError::config("stale config revision"))));
                            return;
                        }

                        match &mut *state {
                            ManagedState::Terminal { .. } => {
                                return;
                            }
                            ManagedState::Serving { active, planned } => {
                                if let Some(p) = planned
                                    && p.id != txn.id
                                {
                                    return;
                                }
                                if active.id != active_gen.id {
                                    let current_session = active.session.clone();
                                    let _ = txn
                                        .outcome_tx
                                        .send(Some(Ok(RecoveryOutcome::Stale(current_session))));
                                    return;
                                }
                                let next_gen = next_gen_atomic.fetch_add(1, Ordering::SeqCst);
                                let new_gen =
                                    Arc::new(SessionGeneration::new(next_gen, new_session.clone()));
                                let old = active.session.clone();
                                *state = ManagedState::Serving { active: new_gen, planned: None };
                                (next_gen, old)
                            }
                            ManagedState::Recovering { failed, txn: cur_txn } => {
                                if cur_txn.id != txn.id {
                                    return;
                                }
                                let next_gen = next_gen_atomic.fetch_add(1, Ordering::SeqCst);
                                let new_gen =
                                    Arc::new(SessionGeneration::new(next_gen, new_session.clone()));
                                let old = failed.session.clone();
                                *state = ManagedState::Serving { active: new_gen, planned: None };
                                (next_gen, old)
                            }
                            _ => return,
                        }
                    };

                    candidate_guard.disarm();
                    let _ = generation_tx.send(next_gen);

                    tokio::spawn(async move {
                        let _ =
                            tokio::time::timeout(Duration::from_secs(2), old_session.close()).await;
                    });

                    let outcome = RecoveryOutcome::Recovered { session: new_session, continuity };
                    let _ = txn.outcome_tx.send(Some(Ok(outcome)));
                }
                Err(err) => {
                    {
                        let mut state = state_lock.write().await;
                        if let ManagedState::Serving { planned, .. } = &mut *state
                            && let Some(p) = planned
                            && p.id == txn.id
                        {
                            *planned = None;
                        }
                    }
                    let _ = txn.outcome_tx.send(Some(Err(err)));
                }
            }
        });
    }

    /// Close the session gracefully and mark supervisor as closed.
    pub(crate) async fn close(&self) -> Result<()> {
        let (old_session, old_planned) = {
            let mut state = self.state.write().await;
            match std::mem::replace(
                &mut *state,
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

    #[tokio::test]
    async fn test_monotonic_failure_prevents_write_resurrection() {
        let mock_rec = Arc::new(CoalescingRecovery {
            recover_count: Arc::new(AtomicUsize::new(0)),
            active_recoveries: Arc::new(AtomicUsize::new(0)),
            max_active_recoveries: Arc::new(AtomicUsize::new(0)),
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

        // Wait until supervisor enters Recovering
        while supervisor.status().await != TransportStatus::Recovering {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Abort the caller future mid-recovery
        reporter_handle.abort();
        let _ = reporter_handle.await;

        // Invariant 1: Generation 0 write gate is CLOSED forever and cannot be resurrected
        let write_res = supervisor.admit_write().await;
        if let Ok(gen_item) = write_res {
            assert_eq!(gen_item.id, 1, "resurrected generation 0 is illegal");
        }
    }

    #[tokio::test]
    async fn test_caller_drop_does_not_strand_recovery() {
        let mock_rec = Arc::new(CoalescingRecovery {
            recover_count: Arc::new(AtomicUsize::new(0)),
            active_recoveries: Arc::new(AtomicUsize::new(0)),
            max_active_recoveries: Arc::new(AtomicUsize::new(0)),
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

        while supervisor.status().await != TransportStatus::Recovering {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        reporter_handle.abort();
        let _ = reporter_handle.await;

        // Invariant 2: Supervisor-owned recovery continues in background and publishes generation 1
        let timeout_res = tokio::time::timeout(Duration::from_secs(2), gen_rx.changed()).await;
        assert!(timeout_res.is_ok(), "supervisor background recovery must not strand");
        assert_eq!(*gen_rx.borrow(), 1);
        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
    }

    #[tokio::test]
    async fn test_authority_snapshot_during_recovery() {
        let mock_rec = Arc::new(CoalescingRecovery {
            recover_count: Arc::new(AtomicUsize::new(0)),
            active_recoveries: Arc::new(AtomicUsize::new(0)),
            max_active_recoveries: Arc::new(AtomicUsize::new(0)),
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

        while supervisor.status().await != TransportStatus::Recovering {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Invariant 3: get_active_generation returns generation 0 so readers do not hit synthetic EOF
        let auth_gen = supervisor.get_active_generation().await.unwrap();
        assert_eq!(auth_gen.id, 0);
        assert_eq!(auth_gen.session.session_id(), "gen-0");

        // But writes are properly rejected
        assert!(supervisor.admit_write().await.is_err());
    }

    #[tokio::test]
    async fn test_planned_rotation_seamless_promotion() {
        let mock_rec = Arc::new(CoalescingRecovery {
            recover_count: Arc::new(AtomicUsize::new(0)),
            active_recoveries: Arc::new(AtomicUsize::new(0)),
            max_active_recoveries: Arc::new(AtomicUsize::new(0)),
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
        let sup_clone = Arc::clone(&supervisor);
        let planned_handle = tokio::spawn(async move {
            sup_clone
                .execute_planned_replacement(
                    0,
                    RecoveryCause::PlannedRotation { time_left: Some("30s".into()) },
                )
                .await
        });

        tokio::time::sleep(Duration::from_millis(10)).await;

        // Active generation fails while planned replacement is in-flight:
        // Report failure promotes the planned replacement to recovery transaction!
        let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
        let fail_res = supervisor.report_failure(report).await.unwrap();
        match fail_res {
            RecoveryOutcome::Recovered { session, .. } => {
                assert_eq!(session.session_id(), "gen-1-recovered");
            }
            _ => panic!("Expected Recovered"),
        }

        let planned_res = planned_handle.await.unwrap().unwrap();
        match planned_res {
            RecoveryOutcome::Recovered { session, .. } => {
                assert_eq!(session.session_id(), "gen-1-recovered");
            }
            _ => panic!("Expected Recovered"),
        }

        assert_eq!(supervisor.status().await, TransportStatus::Healthy);
    }

    #[tokio::test]
    async fn test_supreme_close_cancels_candidate() {
        let mock_rec = Arc::new(CoalescingRecovery {
            recover_count: Arc::new(AtomicUsize::new(0)),
            active_recoveries: Arc::new(AtomicUsize::new(0)),
            max_active_recoveries: Arc::new(AtomicUsize::new(0)),
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

        // Close immediately
        sup_arc.close().await.unwrap();

        // Invariant: Status is Closed, and no candidate can publish
        assert_eq!(sup_arc.status().await, TransportStatus::Closed);
        assert!(!sup_arc.is_connected().await);
        let _ = planned_handle.await;
    }
}
