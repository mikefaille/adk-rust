use async_trait::async_trait;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use adk_realtime::audio::AudioChunk;
use adk_realtime::error::{RealtimeError, Result};
use adk_realtime::events::{ClientEvent, ServerEvent, ToolResponse};
use adk_realtime::recovery::{
    RealtimeRecovery, RecoveredSession, RecoveryCause, RecoveryContext, RecoveryContinuity,
    RecoveryDisposition, RecoveryPolicy,
};
use adk_realtime::session::RealtimeSession;

/// A completely mock session with no recovery capabilities.
struct MockSessionNone {
    config: adk_realtime::config::RealtimeConfig,
}

#[async_trait]
impl RealtimeSession for MockSessionNone {
    fn config(&self) -> Option<&adk_realtime::config::RealtimeConfig> {
        Some(&self.config)
    }

    fn session_id(&self) -> &str {
        "mock-none"
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

/// A mock recovery implementation to test the trait and dynamic dispatch.
struct MockRecoveryImpl;

#[async_trait]
impl RealtimeRecovery for MockRecoveryImpl {
    fn classify(&self, cause: &RecoveryCause) -> RecoveryDisposition {
        match cause {
            RecoveryCause::UnexpectedEof => RecoveryDisposition::Recoverable,
            RecoveryCause::ProviderReset { code, .. } if *code == 1000 => {
                RecoveryDisposition::Recoverable
            }
            _ => RecoveryDisposition::Fatal,
        }
    }

    fn classify_attempt_error(&self, error: &RealtimeError) -> RecoveryDisposition {
        match error {
            RealtimeError::ConnectionError(msg) if msg.contains("retryable") => {
                RecoveryDisposition::Recoverable
            }
            _ => RecoveryDisposition::Fatal,
        }
    }

    async fn recover(&self, context: RecoveryContext<'_>) -> Result<RecoveredSession> {
        // Validate that context.config() is applied and effective before the session is returned
        let session = Arc::new(MockSessionNone {
            config: context.config().clone(),
        });
        Ok(RecoveredSession::new(session, RecoveryContinuity::Reconnected))
    }
}

/// A mock session that exposes recovery capabilities.
struct MockSessionWithRecovery {
    recovery_impl: MockRecoveryImpl,
}

#[async_trait]
impl RealtimeSession for MockSessionWithRecovery {
    fn session_id(&self) -> &str {
        "mock-with-recovery"
    }

    fn is_connected(&self) -> bool {
        true
    }

    fn recovery(&self) -> Option<&dyn RealtimeRecovery> {
        Some(&self.recovery_impl)
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

#[test]
fn test_none_recovery_capability() {
    let session = MockSessionNone {
        config: adk_realtime::config::RealtimeConfig::default(),
    };
    assert!(session.recovery().is_none());
}

#[test]
fn test_some_recovery_capability_dynamic_dispatch() {
    let session = MockSessionWithRecovery { recovery_impl: MockRecoveryImpl };

    let recovery_ref = session.recovery().expect("Expected recovery capability");

    // Validate classify API behavior
    let cause_eof = RecoveryCause::UnexpectedEof;
    let cause_reset_1000 =
        RecoveryCause::ProviderReset { code: 1000, reason: "Normal closure".to_string() };
    let cause_reset_fatal =
        RecoveryCause::ProviderReset { code: 1008, reason: "Policy violation".to_string() };

    assert_eq!(recovery_ref.classify(&cause_eof), RecoveryDisposition::Recoverable);
    assert_eq!(recovery_ref.classify(&cause_reset_1000), RecoveryDisposition::Recoverable);
    assert_eq!(recovery_ref.classify(&cause_reset_fatal), RecoveryDisposition::Fatal);
}

#[test]
fn test_recovery_attempt_error_classification() {
    let session = MockSessionWithRecovery { recovery_impl: MockRecoveryImpl };
    let recovery_ref = session.recovery().expect("Expected recovery capability");

    let retryable_err = RealtimeError::ConnectionError("retryable connection failure".to_string());
    let fatal_err = RealtimeError::AuthError("unauthorized".to_string());

    assert_eq!(recovery_ref.classify_attempt_error(&retryable_err), RecoveryDisposition::Recoverable);
    assert_eq!(recovery_ref.classify_attempt_error(&fatal_err), RecoveryDisposition::Fatal);
}

#[test]
fn test_recovery_policy_getters_and_setters() {
    let policy = RecoveryPolicy::default();
    assert_eq!(policy.max_attempts(), NonZeroU32::new(3).unwrap());
    assert_eq!(policy.deadline(), Duration::from_secs(5));
    assert_eq!(policy.initial_delay(), Duration::from_millis(50));
    assert_eq!(policy.max_delay(), Duration::from_millis(500));

    let custom_policy = RecoveryPolicy::new()
        .with_max_attempts(NonZeroU32::new(5).unwrap())
        .with_deadline(Duration::from_secs(10))
        .with_initial_delay(Duration::from_millis(100))
        .with_max_delay(Duration::from_millis(1000));

    assert_eq!(custom_policy.max_attempts(), NonZeroU32::new(5).unwrap());
    assert_eq!(custom_policy.deadline(), Duration::from_secs(10));
    assert_eq!(custom_policy.initial_delay(), Duration::from_millis(100));
    assert_eq!(custom_policy.max_delay(), Duration::from_millis(1000));
}

#[tokio::test]
async fn test_recovery_execution_and_recovered_session_fields() {
    let recovery = MockRecoveryImpl;
    let config = adk_realtime::config::RealtimeConfig::default().with_instruction("travel agent");
    let cause = RecoveryCause::UnexpectedEof;
    let context = RecoveryContext::new(
        NonZeroU32::new(1).unwrap(),
        &cause,
        &config,
        std::time::Instant::now() + Duration::from_secs(5),
    );

    let recovered = recovery.recover(context).await.unwrap();
    assert_eq!(recovered.continuity(), RecoveryContinuity::Reconnected);

    let session = recovered.session();
    assert_eq!(session.session_id(), "mock-none");

    // Verify that context config is applied and effective before it is returned
    // This proves the Reconnected success contract: config was applied and effective before readiness is reported.
    let effective_config = session.config().expect("Expected config to be available");
    assert_eq!(effective_config.instruction.as_deref(), Some("travel agent"));
}
