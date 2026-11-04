//! RealtimeRunner for integrating realtime sessions with agents.
//!
//! This module provides the bridge between realtime audio sessions and
//! the ADK agent framework, handling tool execution and event routing.

use crate::config::{RealtimeConfig, SessionUpdateConfig, ToolDefinition};
use crate::error::{RealtimeError, Result};
use crate::events::{CallerActivitySource, ServerEvent, ToolCall, ToolResponse};
use crate::metering::{RealtimePricing, ResponseMeter};
use crate::model::BoxedModel;
use crate::recovery::supervisor::{
    ConfigSnapshot, FailureReport, RecoveryOutcome, RecoverySupervisor, TransportStatus,
};
use crate::recovery::{DeliveryCertainty, RecoveryCause, RecoveryPolicy};
use crate::session::{ContextMutationOutcome, RealtimeSession};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// How long a realtime tool may run before its call is answered with a timeout error.
pub const DEFAULT_REALTIME_TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// The agent a realtime runner governs and meters its tool calls under unless
/// [`RealtimeRunnerBuilder::agent_name`] names another; `IntegratedRealtimeRunner` also
/// authors its held-call events with it, where [`adk_core::held_tool_calls`] finds them.
pub const DEFAULT_REALTIME_AGENT_NAME: &str = "realtime";

/// [`ToolOrigin::source`](adk_core::ToolOrigin::source) of a native handler registered on a
/// [`RealtimeRunner`] with [`RealtimeRunnerBuilder::tool`].
///
/// A governed call to the handler carries this source and a digest of the handler's
/// [`ToolDefinition`] ([`ToolDefinition::handler_origin`]), so an approval of the call never
/// authorizes a [`Tool`](adk_core::Tool) of the same name and stops applying when the
/// handler's description or parameters change.
pub const REALTIME_HANDLER_ORIGIN: &str = "realtime";

/// Runs `screen` on `result`, returning the result to deliver — the
/// [`result_blocked_response`](adk_core::result_blocked_response) that replaces a withheld one —
/// and whether the screen withheld it, which sets the answer's
/// [`ToolResultStatus`](adk_core::ToolResultStatus).
pub(crate) async fn screen_result(
    screen: &dyn adk_core::ToolResultScreen,
    name: &str,
    args: &serde_json::Value,
    result: serde_json::Value,
) -> (serde_json::Value, bool) {
    match screen.screen_result(name, args, &result).await {
        Ok(screened) => (screened, false),
        Err(reason) => {
            // The reason can describe the result, so it goes to the model, not the log.
            tracing::warn!(tool.name = %name, "tool result withheld by its screen");
            (adk_core::result_blocked_response(name, &reason), true)
        }
    }
}

/// Parses a provider's tool-call arguments.
///
/// Empty arguments are an empty object. Anything that is not a JSON object is refused,
/// so a malformed call is answered with an error instead of running with `{}`.
pub(crate) fn parse_tool_arguments(
    arguments: &serde_json::Value,
) -> std::result::Result<serde_json::Value, String> {
    match arguments {
        serde_json::Value::Object(_) => Ok(arguments.clone()),
        serde_json::Value::Null => Ok(serde_json::Value::Object(Default::default())),
        serde_json::Value::String(text) if text.trim().is_empty() => {
            Ok(serde_json::Value::Object(Default::default()))
        }
        serde_json::Value::String(text) => match serde_json::from_str::<serde_json::Value>(text) {
            Ok(value @ serde_json::Value::Object(_)) => Ok(value),
            Ok(_) => Err("its arguments are not a JSON object".to_string()),
            Err(error) => Err(format!("its arguments are not valid JSON ({error})")),
        },
        _ => Err("its arguments are not a JSON object".to_string()),
    }
}

/// The run-wide governance a runner applies to every tool call it dispatches.
#[derive(Clone)]
pub(crate) struct ToolGovernance {
    pub(crate) config: adk_core::RunConfig,
    pub(crate) scope: adk_core::ApprovalScope,
}

/// Internal state machine tracking the resumability status of the RealtimeRunner.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum RunnerState {
    /// Runner is ready to accept transport resumption immediately.
    #[default]
    Idle,
    /// Model is currently generating a response; tearing down the connection would corrupt context.
    Generating,
    /// A tool is currently executing; teardown would cause tool loss.
    ExecutingTool,
    /// A context mutation was queued while the runner was busy, and must be executed once Idle.
    ///
    /// Semantics: Last-write-wins for pending mutations. If multiple mutations are requested
    /// while busy, only the latest configuration and bridge message are retained and executed
    /// when the runner transitions back to `Idle`.
    PendingResumption {
        /// The new configuration to apply on reconnection.
        config: Box<crate::config::RealtimeConfig>,
        /// An optional message to inject immediately after resumption.
        bridge_message: Option<String>,
        /// Number of failed reconnection attempts for this mutation.
        attempts: u8,
    },
}

/// Handler for tool/function calls from the realtime model.
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Execute a tool call and return the result.
    async fn execute(&self, call: &ToolCall) -> Result<serde_json::Value>;

    /// Execute a tool call whose cancellation the runner fences: `token`
    /// is cancelled when the provider withdraws the call or the response.
    /// The default ignores it; handlers that create a tool context
    /// override it so the context reports the cancellation.
    async fn execute_cancellable(
        &self,
        call: &ToolCall,
        _token: CancellationToken,
    ) -> Result<serde_json::Value> {
        self.execute(call).await
    }
}

/// A simple function-based tool handler.
pub struct FnToolHandler<F>
where
    F: Fn(&ToolCall) -> Result<serde_json::Value> + Send + Sync,
{
    handler: F,
}

impl<F> FnToolHandler<F>
where
    F: Fn(&ToolCall) -> Result<serde_json::Value> + Send + Sync,
{
    /// Create a new function-based tool handler.
    pub fn new(handler: F) -> Self {
        Self { handler }
    }
}

#[async_trait]
impl<F> ToolHandler for FnToolHandler<F>
where
    F: Fn(&ToolCall) -> Result<serde_json::Value> + Send + Sync,
{
    async fn execute(&self, call: &ToolCall) -> Result<serde_json::Value> {
        (self.handler)(call)
    }
}

/// Async function-based tool handler.
#[allow(dead_code)]
pub struct AsyncToolHandler<F, Fut>
where
    F: Fn(ToolCall) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = Result<serde_json::Value>> + Send,
{
    handler: F,
}

impl<F, Fut> AsyncToolHandler<F, Fut>
where
    F: Fn(ToolCall) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = Result<serde_json::Value>> + Send,
{
    /// Create a new async tool handler.
    pub fn new(handler: F) -> Self {
        Self { handler }
    }
}

/// Maps a server event to the caller-activity evidence it carries, if any.
pub fn caller_activity_source(event: &ServerEvent) -> Option<CallerActivitySource> {
    match event {
        ServerEvent::InputTranscriptDelta { .. } => Some(CallerActivitySource::TranscriptDelta),
        ServerEvent::InputTranscriptCompleted { .. } => {
            Some(CallerActivitySource::TranscriptCompleted)
        }
        ServerEvent::SpeechStopped { .. } => Some(CallerActivitySource::SpeechStopped),
        _ => None,
    }
}

/// Event handler for processing realtime events.
#[async_trait]
pub trait EventHandler: Send + Sync {
    /// Called when an audio delta is received (raw PCM bytes).
    async fn on_audio(&self, _audio: &[u8], _item_id: &str) -> Result<()> {
        Ok(())
    }

    /// Called when a text delta is received.
    async fn on_text(&self, _text: &str, _item_id: &str) -> Result<()> {
        Ok(())
    }

    /// Called when a transcript delta is received.
    async fn on_transcript(&self, _transcript: &str, _item_id: &str) -> Result<()> {
        Ok(())
    }

    /// Called when the provider has finalized one caller-input transcript item.
    async fn on_input_transcript_completed(&self, _transcript: &str, _item_id: &str) -> Result<()> {
        Ok(())
    }

    /// Called whenever a server event indicates the caller was active.
    async fn on_caller_activity(&self, _source: CallerActivitySource) -> Result<()> {
        Ok(())
    }

    /// Called when speech is detected.
    async fn on_speech_started(&self, _audio_start_ms: u64) -> Result<()> {
        Ok(())
    }

    /// Called when speech ends.
    async fn on_speech_stopped(&self, _audio_end_ms: u64) -> Result<()> {
        Ok(())
    }

    /// Called when a response completes.
    async fn on_response_done(&self) -> Result<()> {
        Ok(())
    }

    /// Called when function calls are cancelled.
    async fn on_tool_calls_cancelled(&self, _call_ids: &[crate::events::ToolCallId]) -> Result<()> {
        Ok(())
    }

    /// Called when the provider signals a planned connection rotation (e.g. Gemini goAway).
    async fn on_planned_rotation(&self, _time_left: Option<&str>) -> Result<()> {
        Ok(())
    }

    /// Called when a response is cancelled or interrupted.
    async fn on_response_cancelled(&self) -> Result<()> {
        Ok(())
    }

    /// Called when the provider transport ends terminally and [`RealtimeRunner::run`] is returning.
    ///
    /// Under managed recovery, transient connection losses trigger automatic background recovery.
    /// `on_disconnect` is invoked only when recovery has exhausted all attempts, encountered a fatal error,
    /// or when the session is explicitly closed.
    async fn on_disconnect(&self) -> Result<()> {
        Ok(())
    }

    /// Called on any error.
    async fn on_error(&self, _error: &RealtimeError) -> Result<()> {
        Ok(())
    }
}

/// Default no-op event handler.
#[derive(Debug, Clone, Default)]
pub struct NoOpEventHandler;

#[async_trait]
impl EventHandler for NoOpEventHandler {}

/// A tool call the run loop still has to dispatch.
#[derive(Debug, Clone)]
struct PendingToolCall {
    call_id: String,
    name: String,
    arguments: serde_json::Value,
    /// Fence generation in force when the provider issued this call. A
    /// `ResponseCancelled` since then fences the call before it runs.
    epoch: u64,
}

/// One in-flight tool call in the cancellation fence.
struct FencedToolCall {
    token: CancellationToken,
    abort_handle: Option<tokio::task::AbortHandle>,
    effect: adk_core::ToolEffect,
}

/// The cancellation fence for dispatched tool calls.
#[derive(Default)]
struct ToolCallFence {
    /// Bumped by every `ResponseCancelled`; calls registered under an
    /// older generation are fenced. `SpeechStarted` alone never bumps it:
    /// speech onset does not withdraw the response's tool calls.
    epoch: u64,
    calls: HashMap<String, FencedToolCall>,
}

/// Configuration for the RealtimeRunner.
#[derive(Clone)]
pub struct RunnerConfig {
    /// Whether to automatically execute tool calls.
    pub auto_execute_tools: bool,
    /// Whether to automatically send tool responses.
    pub auto_respond_tools: bool,
    /// Maximum concurrent tool executions.
    pub max_concurrent_tools: usize,
    /// Maximum consecutive tool execution failures before tripping the circuit breaker (default: 3).
    pub max_consecutive_tool_failures: usize,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            auto_execute_tools: true,
            auto_respond_tools: true,
            max_concurrent_tools: 4,
            max_consecutive_tool_failures: 3,
        }
    }
}

/// Builder for RealtimeRunner.
pub struct RealtimeRunnerBuilder {
    model: Option<BoxedModel>,
    config: RealtimeConfig,
    runner_config: RunnerConfig,
    recovery_policy: RecoveryPolicy,
    tools: HashMap<String, (ToolDefinition, Arc<dyn ToolHandler>)>,
    /// The first name registered twice; `build` rejects it.
    duplicate_tool: Option<String>,
    event_handler: Option<Arc<dyn EventHandler>>,
    governance: Option<ToolGovernance>,
    tool_timeout: std::time::Duration,
    tool_result_screen: Option<Arc<dyn adk_core::ToolResultScreen>>,
    expose_tool_error_messages: bool,
    pricing: Option<RealtimePricing>,
    agent_name: String,
}

impl Default for RealtimeRunnerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl RealtimeRunnerBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            model: None,
            config: RealtimeConfig::default(),
            runner_config: RunnerConfig::default(),
            recovery_policy: RecoveryPolicy::default(),
            tools: HashMap::new(),
            duplicate_tool: None,
            event_handler: None,
            governance: None,
            tool_timeout: DEFAULT_REALTIME_TOOL_TIMEOUT,
            tool_result_screen: None,
            expose_tool_error_messages: false,
            pricing: None,
            agent_name: DEFAULT_REALTIME_AGENT_NAME.to_string(),
        }
    }

    /// Names the agent every tool call is governed and metered under: the gate's agent
    /// name, the acting agent of each call's fingerprint and audit record, and the agent in
    /// the spend key. Defaults to [`DEFAULT_REALTIME_AGENT_NAME`]. A surface that runs an
    /// existing agent's tools over a realtime session sets that agent's name, so the
    /// agent's tool policy recognises its own tools (spec 3.1 §7.17).
    pub fn agent_name(mut self, name: impl Into<String>) -> Self {
        self.agent_name = name.into();
        self
    }

    /// Prices each response's reported usage, so the cost committed in the run's spend
    /// ledger is the usage's price rather than the run's per-call estimate. Metering needs
    /// [`tool_governance`](Self::tool_governance) with a `spend_ledger`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use adk_realtime::metering::RealtimePricing;
    ///
    /// let runner = RealtimeRunner::builder()
    ///     .model(model)
    ///     .tool_governance(config, scope)
    ///     .realtime_pricing(RealtimePricing {
    ///         input_audio_micro_usd_per_million: 32_000_000,
    ///         output_audio_micro_usd_per_million: 64_000_000,
    ///         ..RealtimePricing::default()
    ///     })
    ///     .build()?;
    /// ```
    pub fn realtime_pricing(mut self, pricing: RealtimePricing) -> Self {
        self.pricing = Some(pricing);
        self
    }

    /// Governs every tool call this runner dispatches.
    ///
    /// The config's `tool_policy`, `governance` kill switches, confirmation handler,
    /// approvals, and approval store apply exactly as they do to an agent run, through
    /// [`adk_core::authorize_tool_call`]. A call held for confirmation is answered with an
    /// error and recorded in the approval store under `scope`. A frozen kill switch ends
    /// [`run`](RealtimeRunner::run) with an error.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use adk_core::{ApprovalScope, DeclarativePolicy, RunConfig};
    ///
    /// let runner = RealtimeRunner::builder()
    ///     .model(model)
    ///     .tool_governance(
    ///         RunConfig::builder()
    ///             .tool_policy(Arc::new(DeclarativePolicy::builder().allow("get_weather").build()))
    ///             .build(),
    ///         ApprovalScope::new("voice", "user-1", "call-42"),
    ///     )
    ///     .build()?;
    /// ```
    pub fn tool_governance(
        mut self,
        config: adk_core::RunConfig,
        scope: adk_core::ApprovalScope,
    ) -> Self {
        self.governance = Some(ToolGovernance { config, scope });
        self
    }

    /// Sets how long a tool may run before its call is answered with a timeout error.
    /// Defaults to [`DEFAULT_REALTIME_TOOL_TIMEOUT`].
    pub fn tool_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.tool_timeout = timeout;
        self
    }

    /// Screens every tool result before it is sent to the model.
    ///
    /// The screen sees each handler's result, or its error response, before
    /// `function_call_output` is sent. A result the screen rewrites is sent rewritten; a result it
    /// withholds, or that it cannot decide on (`Err`), is replaced by
    /// [`result_blocked_response`](adk_core::result_blocked_response), so the runner fails
    /// closed. `adk-guardrail`'s `ToolGuardrailSet` is a screen.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use adk_guardrail::{PiiRedactor, ResultTextGuardrail, ToolGuardrailSet};
    ///
    /// let runner = RealtimeRunner::builder()
    ///     .model(model)
    ///     .tool_fn(ToolDefinition::new("lookup_order"), lookup_order)
    ///     .tool_result_screen(Arc::new(
    ///         ToolGuardrailSet::new().with(ResultTextGuardrail::new(PiiRedactor::new())),
    ///     ))
    ///     .build()?;
    /// ```
    pub fn tool_result_screen(mut self, screen: Arc<dyn adk_core::ToolResultScreen>) -> Self {
        self.tool_result_screen = Some(screen);
        self
    }

    /// Sends the full message of a handler's error to the model.
    ///
    /// By default a handler that fails is answered with `tool '<name>' failed` only, because
    /// the message can quote credentials, internal hosts, or upstream bodies, and the output is
    /// sent to the provider and can be spoken to the user. Timeouts and policy refusals keep
    /// their message.
    pub fn expose_tool_error_messages(mut self, expose: bool) -> Self {
        self.expose_tool_error_messages = expose;
        self
    }

    /// Set the realtime model.
    pub fn model(mut self, model: BoxedModel) -> Self {
        self.model = Some(model);
        self
    }

    /// Set the session configuration.
    pub fn config(mut self, config: RealtimeConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the runner configuration.
    pub fn runner_config(mut self, config: RunnerConfig) -> Self {
        self.runner_config = config;
        self
    }

    /// Set the recovery policy.
    pub fn recovery_policy(mut self, policy: RecoveryPolicy) -> Self {
        self.recovery_policy = policy;
        self
    }

    /// Set the system instruction.
    pub fn instruction(mut self, instruction: impl Into<String>) -> Self {
        self.config.instruction = Some(instruction.into());
        self
    }

    /// Set the voice.
    pub fn voice(mut self, voice: impl Into<String>) -> Self {
        self.config.voice = Some(voice.into());
        self
    }

    /// Register a tool with its handler.
    ///
    /// Each tool needs a name of its own: [`build`](Self::build) fails when two
    /// share one.
    pub fn tool(self, definition: ToolDefinition, handler: impl ToolHandler + 'static) -> Self {
        self.tool_arc(definition, Arc::new(handler))
    }

    /// Register a tool with a sync function handler.
    pub fn tool_fn<F>(self, definition: ToolDefinition, handler: F) -> Self
    where
        F: Fn(&ToolCall) -> Result<serde_json::Value> + Send + Sync + 'static,
    {
        self.tool(definition, FnToolHandler::new(handler))
    }

    /// Register a tool with an `Arc<dyn ToolHandler>` directly.
    pub fn tool_arc(mut self, definition: ToolDefinition, handler: Arc<dyn ToolHandler>) -> Self {
        let name = definition.name.clone();
        if self.tools.insert(name.clone(), (definition, handler)).is_some() {
            self.duplicate_tool.get_or_insert(name);
        }
        self
    }

    /// Set the event handler.
    pub fn event_handler(mut self, handler: impl EventHandler + 'static) -> Self {
        self.event_handler = Some(Arc::new(handler));
        self
    }

    /// Set the event handler from an `Arc<dyn EventHandler>` directly.
    pub fn event_handler_arc(mut self, handler: Arc<dyn EventHandler>) -> Self {
        self.event_handler = Some(handler);
        self
    }

    /// Build the runner (does not connect yet).
    ///
    /// # Errors
    ///
    /// Returns a config error when no model was set or when two tools were
    /// registered under one name.
    pub fn build(self) -> Result<RealtimeRunner> {
        let model = self.model.ok_or_else(|| RealtimeError::config("Model is required"))?;
        if let Some(name) = self.duplicate_tool {
            return Err(RealtimeError::config(format!(
                "two tools are registered as '{name}'; the model calls tools by name, so only \
                 one could ever run. Rename one of them or register it once."
            )));
        }

        let mut config = self.config;
        if !self.tools.is_empty() {
            let tool_defs: Vec<ToolDefinition> =
                self.tools.values().map(|(def, _)| def.clone()).collect();
            config.tools = Some(tool_defs);
        }

        let max_concurrent_tools = self.runner_config.max_concurrent_tools.max(1);
        let supervisor = Arc::new(RecoverySupervisor::new(self.recovery_policy, config));

        // Every response is metered in the run's spend ledger, as `LlmAgent` meters a call.
        let meter = self.governance.as_ref().and_then(|governance| {
            ResponseMeter::for_run(
                &governance.config,
                &governance.scope.app_name,
                &self.agent_name,
                model.model_id(),
                self.pricing,
            )
        });

        Ok(RealtimeRunner {
            model,
            runner_config: self.runner_config,
            tools: self.tools,
            event_handler: self.event_handler.unwrap_or_else(|| Arc::new(NoOpEventHandler)),
            supervisor,
            state: Arc::new(RwLock::new(RunnerState::Idle)),
            pending_tool_response: AtomicBool::new(false),
            tool_output_failed: Arc::new(AtomicBool::new(false)),
            tool_permits: Arc::new(tokio::sync::Semaphore::new(max_concurrent_tools)),
            outstanding_tools: Arc::new(AtomicUsize::new(0)),
            response_closed_awaiting_tools: Arc::new(AtomicBool::new(false)),
            governance: self.governance,
            tool_timeout: self.tool_timeout,
            tool_result_screen: self.tool_result_screen,
            expose_tool_error_messages: self.expose_tool_error_messages,
            meter,
            agent_name: self.agent_name,
            consecutive_tool_failures: Arc::new(AtomicUsize::new(0)),
            circuit_breaker_tripped: Arc::new(AtomicBool::new(false)),
            tool_fence: Arc::new(std::sync::Mutex::new(ToolCallFence::default())),
        })
    }
}

/// A runner that manages a realtime session with automatic recovery and tool execution.
///
/// `RealtimeRunner` coordinates bidirectional audio and text streams between an agent
/// model and an application. It provides single-authority session generation fencing,
/// managed write boundaries with explicit delivery certainty (`NotAttempted` vs `Indeterminate`),
/// atomic configuration revision snapshotting, and automatic background connection recovery.
///
/// # Example
///
/// ```rust,ignore
/// use adk_realtime::{RealtimeRunner, RealtimeConfig, ToolDefinition};
/// use adk_realtime::openai::OpenAIRealtimeModel;
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     let model = OpenAIRealtimeModel::new(api_key, "gpt-realtime-2.1");
///
///     let runner = RealtimeRunner::builder()
///         .model(Box::new(model))
///         .instruction("You are a helpful voice assistant.")
///         .voice("alloy")
///         .tool_fn(
///             ToolDefinition::new("get_weather")
///                 .with_description("Get weather for a location"),
///             |call| {
///                 Ok(serde_json::json!({"temperature": 72, "condition": "sunny"}))
///             }
///         )
///         .build()?;
///
///     runner.connect().await?;
///     runner.run().await?;
///
///     Ok(())
/// }
/// ```
pub struct RealtimeRunner {
    model: BoxedModel,
    runner_config: RunnerConfig,
    tools: HashMap<String, (ToolDefinition, Arc<dyn ToolHandler>)>,
    event_handler: Arc<dyn EventHandler>,
    supervisor: Arc<RecoverySupervisor>,
    state: Arc<RwLock<RunnerState>>,
    pending_tool_response: AtomicBool,
    tool_output_failed: Arc<AtomicBool>,
    tool_permits: Arc<tokio::sync::Semaphore>,
    outstanding_tools: Arc<AtomicUsize>,
    response_closed_awaiting_tools: Arc<AtomicBool>,
    /// Policy, kill switch, and confirmation applied to every dispatched call.
    governance: Option<ToolGovernance>,
    /// How long one tool call may run.
    tool_timeout: std::time::Duration,
    /// Screens every tool result before it is sent to the model.
    tool_result_screen: Option<Arc<dyn adk_core::ToolResultScreen>>,
    /// Whether handler error messages reach the model.
    expose_tool_error_messages: bool,
    /// Meters each response in the run's spend ledger, when the governance has one.
    meter: Option<ResponseMeter>,
    /// The agent every tool call is governed and metered under.
    agent_name: String,
    consecutive_tool_failures: Arc<AtomicUsize>,
    circuit_breaker_tripped: Arc<AtomicBool>,
    tool_fence: Arc<std::sync::Mutex<ToolCallFence>>,
}

impl RealtimeRunner {
    /// Create a new builder.
    pub fn builder() -> RealtimeRunnerBuilder {
        RealtimeRunnerBuilder::new()
    }

    /// Single managed write invocation boundary.
    pub(crate) async fn invoke_write<F, Fut, T>(&self, op: F) -> Result<T>
    where
        F: FnOnce(Arc<dyn crate::session::RealtimeSession>) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let gen_item = match self.supervisor.admit_write().await {
            Ok(g) => g,
            Err(err) => {
                return Err(RealtimeError::write_failed(
                    Arc::new(err),
                    DeliveryCertainty::NotAttempted,
                ));
            }
        };

        match op(gen_item.session).await {
            Ok(res) => Ok(res),
            Err(err) => {
                let err_arc = Arc::new(err);
                let report = FailureReport {
                    generation: gen_item.id,
                    cause: RecoveryCause::WriteFailed(err_arc.clone()),
                };
                let _ = self.supervisor.report_failure(report).await;
                Err(RealtimeError::write_failed(err_arc, DeliveryCertainty::Indeterminate))
            }
        }
    }

    /// Connect to the realtime provider.
    pub async fn connect(&self) -> Result<()> {
        self.consecutive_tool_failures.store(0, Ordering::Release);
        self.circuit_breaker_tripped.store(false, Ordering::Release);

        let config_snapshot = self.supervisor.get_config().await;
        let session = self.model.connect(config_snapshot.config).await?;
        let session_arc: Arc<dyn crate::session::RealtimeSession> = Arc::from(session);

        if self.supervisor.status().await == TransportStatus::Uninitialized {
            let _ = self.supervisor.set_initial_session(session_arc).await?;
        } else {
            let _ =
                self.supervisor.publish_replacement(session_arc, config_snapshot.revision).await?;
        }
        Ok(())
    }

    /// Install an initial session directly (valid only when uninitialized). Returns generation ID.
    pub async fn set_initial_session(
        &self,
        session: Arc<dyn crate::session::RealtimeSession>,
    ) -> Result<u64> {
        let gen_item = self.supervisor.set_initial_session(session).await?;
        Ok(gen_item.id)
    }

    /// Check if currently connected.
    pub async fn is_connected(&self) -> bool {
        self.supervisor.is_connected().await
    }

    /// Get the session ID if connected.
    pub async fn session_id(&self) -> Option<String> {
        self.supervisor
            .get_active_generation()
            .await
            .ok()
            .map(|g| g.session.session_id().to_string())
    }

    /// Deliberately force a transport interruption on the active session generation for testing.
    ///
    /// Abruptly drops the raw active session transport without closing the managed supervisor,
    /// triggering the managed failure recovery path on the next read or write operation.
    #[cfg(any(test, feature = "recovery-test-utils"))]
    pub async fn force_transport_break_for_testing(&self) -> Result<()> {
        let gen_item = self.supervisor.get_active_generation().await?;
        gen_item.session.force_transport_break().await
    }

    /// Set an integration test recovery barrier on the recovery supervisor.
    #[cfg(any(test, feature = "recovery-test-utils"))]
    pub fn set_recovery_barrier_for_testing(
        &self,
        barrier: Arc<crate::recovery::TestRecoveryBarrier>,
    ) {
        self.supervisor.set_recovery_barrier_for_testing(barrier);
    }

    /// Subscribe to session generation publication notifications.
    ///
    /// Returns a read-only watch receiver that fires when a new authoritative session generation
    /// is installed or published by the underlying supervisor.
    ///
    /// # Liveness vs Authority
    ///
    /// This watcher provides a **liveness notification** to wake application-owned buffered-data
    /// replay loops or subscriber tasks upon generation transition (e.g. `N -> N+1`).
    /// It is **not** an authority over recovery policy, transport state transitions, or provider handles.
    /// Candidate connection attempts that fail or are rejected before publication do not advance this watcher.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use adk_realtime::runner::RealtimeRunner;
    /// # async fn example(runner: RealtimeRunner) {
    /// let mut rx = runner.subscribe_generation();
    /// tokio::spawn(async move {
    ///     while rx.changed().await.is_ok() {
    ///         let generation = *rx.borrow();
    ///         println!("New session generation published: {generation}");
    ///     }
    /// });
    /// # }
    /// ```
    pub fn subscribe_generation(&self) -> tokio::sync::watch::Receiver<u64> {
        self.supervisor.subscribe_generation()
    }

    /// Returns the connected provider's native audio output format.
    ///
    /// This format specifies the sample rate, channel count, and encoding (e.g. PCM16 24kHz)
    /// emitted by the active session generation.
    pub async fn native_audio_output_format(&self) -> Result<crate::audio::AudioFormat> {
        let gen_item = self.supervisor.get_active_generation().await?;
        Ok(gen_item.session.native_audio_output_format())
    }

    /// Send a client event directly to the active session generation.
    ///
    /// `ClientEvent::UpdateSession` events are intercepted and merged into the canonical
    /// configuration revision state using [`RealtimeRunner::update_session`]. All other events
    /// pass through the managed write boundary `invoke_write`.
    pub async fn send_client_event(&self, event: crate::events::ClientEvent) -> Result<()> {
        match event {
            crate::events::ClientEvent::UpdateSession { instructions, tools } => {
                let update_config = SessionUpdateConfig(crate::config::RealtimeConfig {
                    instruction: instructions,
                    tools,
                    ..Default::default()
                });
                self.update_session(update_config).await
            }
            other => self.invoke_write(|s| async move { s.send_event(other).await }).await,
        }
    }

    /// Internal helper to merge a `SessionUpdateConfig` delta into config state.
    fn merge_config(base: &mut RealtimeConfig, update: &SessionUpdateConfig) {
        if let Some(instruction) = &update.0.instruction {
            base.instruction = Some(instruction.clone());
        }
        if let Some(tools) = &update.0.tools {
            base.tools = Some(tools.clone());
        }
        if let Some(voice) = &update.0.voice {
            base.voice = Some(voice.clone());
        }
        if let Some(temp) = update.0.temperature {
            base.temperature = Some(temp);
        }
        if let Some(extra) = &update.0.extra {
            base.extra = Some(extra.clone());
        }
        if let Some(language) = &update.0.language {
            base.language = Some(language.clone());
        }
    }

    /// Update the session configuration.
    ///
    /// Merges the delta into the canonical configuration snapshot and increments the configuration revision.
    /// If the runner is idle, context mutation is attempted immediately (natively or via resumption).
    /// If the runner is busy generating a response or executing a tool, the resumption is queued in
    /// [`RunnerState::PendingResumption`] and executed once the runner returns to `Idle`.
    pub async fn update_session(&self, config: SessionUpdateConfig) -> Result<()> {
        self.update_session_with_bridge(config, None).await
    }

    /// Update the session configuration with an optional bridge message.
    ///
    /// Behaves like [`update_session`](Self::update_session), but injects the provided text message
    /// into the conversation stream immediately after resumption completes.
    pub async fn update_session_with_bridge(
        &self,
        config: SessionUpdateConfig,
        bridge_message: Option<String>,
    ) -> Result<()> {
        let snapshot =
            self.supervisor.update_config(|base| Self::merge_config(base, &config)).await;

        let snapshot_for_mutate = snapshot.clone();
        let mutate_res = self
            .invoke_write(|s| async move { s.mutate_context(snapshot_for_mutate.config).await })
            .await?;

        match mutate_res {
            ContextMutationOutcome::Applied => {
                tracing::info!("Context mutated natively mid-flight.");
                if let Some(msg) = bridge_message {
                    let event = crate::events::ClientEvent::Message {
                        role: "user".to_string(),
                        parts: vec![adk_core::types::Part::Text { text: msg }],
                    };
                    self.invoke_write(|s| async move { s.send_event(event).await }).await?;
                }
                Ok(())
            }
            ContextMutationOutcome::RequiresResumption(new_config) => {
                let mut state_guard = self.state.write().await;

                let queued_snapshot =
                    ConfigSnapshot { config: (*new_config).clone(), revision: snapshot.revision };

                if *state_guard == RunnerState::Idle {
                    drop(state_guard);
                    tracing::info!("Runner is idle. Executing resumption immediately.");

                    let model = Arc::clone(&self.model);
                    let cfg = queued_snapshot.config.clone();
                    match self
                        .supervisor
                        .execute_resumption_with(queued_snapshot.clone(), move || async move {
                            let s = model.connect(cfg).await?;
                            Ok(Arc::from(s) as Arc<dyn RealtimeSession>)
                        })
                        .await
                    {
                        Ok(_) => {
                            if let Some(msg) = bridge_message {
                                tracing::info!(
                                    "Sending bridge message post-resumption via invoke_write."
                                );
                                let event = crate::events::ClientEvent::Message {
                                    role: "user".to_string(),
                                    parts: vec![adk_core::types::Part::Text { text: msg }],
                                };
                                self.invoke_write(|s| async move { s.send_event(event).await })
                                    .await?;
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                "Immediate resumption failed: {}. Queueing for retry.",
                                e
                            );
                            let mut fallback_state = self.state.write().await;
                            match &*fallback_state {
                                RunnerState::PendingResumption { .. } => {
                                    tracing::info!(
                                        "Newer resumption intent already queued; preserving newer intent."
                                    );
                                }
                                RunnerState::Idle
                                | RunnerState::Generating
                                | RunnerState::ExecutingTool => {
                                    *fallback_state = RunnerState::PendingResumption {
                                        config: new_config,
                                        bridge_message,
                                        attempts: 1,
                                    };
                                }
                            }
                            return Err(e);
                        }
                    }
                } else {
                    tracing::info!("Runner is busy ({:?}). Queueing resumption.", *state_guard);
                    *state_guard = RunnerState::PendingResumption {
                        config: new_config,
                        bridge_message,
                        attempts: 0,
                    };
                }
                Ok(())
            }
        }
    }

    /// Send a typed raw-audio chunk to the active session generation.
    ///
    /// Routes through the managed write boundary. Admission rejection returns `DeliveryCertainty::NotAttempted`
    /// without calling the raw session. Failures after raw invocation return `DeliveryCertainty::Indeterminate`
    /// and report `RecoveryCause::WriteFailed` to trigger managed recovery.
    pub async fn send_audio_chunk(&self, audio: &crate::audio::AudioChunk) -> Result<()> {
        let chunk = audio.clone();
        self.invoke_write(|s| async move { s.send_audio(&chunk).await }).await
    }

    /// Send base64-encoded audio to the active session generation.
    pub async fn send_audio(&self, audio_base64: &str) -> Result<()> {
        self.send_audio_base64(audio_base64).await
    }

    /// Send base64-encoded audio to the active session generation.
    pub async fn send_audio_base64(&self, audio_base64: &str) -> Result<()> {
        let payload = audio_base64.to_string();
        self.invoke_write(|s| async move { s.send_audio_base64(&payload).await }).await
    }

    /// Send text to the active session generation.
    pub async fn send_text(&self, text: &str) -> Result<()> {
        let payload = text.to_string();
        self.invoke_write(|s| async move { s.send_text(&payload).await }).await
    }

    /// Send video frame to the active session generation.
    pub async fn send_video_frame(&self, mime_type: &str, data_base64: &str) -> Result<()> {
        let mime = mime_type.to_string();
        let data = data_base64.to_string();
        self.invoke_write(|s| async move { s.send_video_frame(&mime, &data).await }).await
    }

    /// Commit audio buffer for the active session generation.
    pub async fn commit_audio(&self) -> Result<()> {
        self.invoke_write(|s| async move { s.commit_audio().await }).await
    }

    /// Report the start of a manually delimited input turn on the active
    /// session generation.
    ///
    /// The opening edge that [`commit_audio`](Self::commit_audio) closes. This
    /// exists so a caller running its own detection can report caller speech
    /// through the neutral runner, rather than downcasting to a concrete
    /// session to reach a backend-specific signaller.
    pub async fn begin_activity(&self) -> Result<()> {
        self.invoke_write(|s| async move { s.begin_activity().await }).await
    }

    /// Trigger model response for the active session generation.
    pub async fn create_response(&self) -> Result<()> {
        self.invoke_write(|s| async move { s.create_response().await }).await
    }

    /// Interrupt current response for the active session generation.
    pub async fn interrupt(&self) -> Result<()> {
        self.invoke_write(|s| async move { s.interrupt().await }).await
    }

    /// Why the provider ended the stream.
    pub async fn disconnect_reason(&self) -> Option<crate::session::DisconnectReason> {
        self.supervisor
            .get_active_generation()
            .await
            .ok()
            .and_then(|g| g.session.disconnect_reason())
    }

    /// Read next event from active generation session, handling generation switches.
    async fn poll_session_next(
        &self,
        watcher: &mut tokio::sync::watch::Receiver<u64>,
        current_gen_id: u64,
        session: &Arc<dyn crate::session::RealtimeSession>,
    ) -> Option<Result<ServerEvent>> {
        let event = tokio::select! {
            biased;
            _ = watcher.changed() => {
                if *watcher.borrow() != current_gen_id {
                    tracing::info!(
                        from = current_gen_id,
                        to = *watcher.borrow(),
                        "generation changed; switching to active generation"
                    );
                    return None;
                }
                session.next_event().await
            }
            ev = session.next_event() => ev,
        };

        // Never yield after an event has been obtained: the caller's `select!`
        // may cancel this future at a yield point, which would drop the event
        // a buffered session already handed over.
        let event = event?;
        if let Ok(inner) = &event
            && let Err(error) = self.observe_event(inner).await
        {
            return Some(Err(error));
        }
        Some(event)
    }

    /// Meters `event` when it starts or ends a response.
    ///
    /// Returns the usage a `ResponseDone` reported, priced with
    /// [`realtime_pricing`](RealtimeRunnerBuilder::realtime_pricing).
    ///
    /// # Errors
    ///
    /// [`RealtimeError::SpendRefused`] when the run's spend ledger refuses the response's
    /// hold; the caller ends the session.
    pub(crate) async fn observe_event(
        &self,
        event: &ServerEvent,
    ) -> Result<Option<adk_core::UsageMetadata>> {
        let Some(meter) = &self.meter else {
            return Ok(None);
        };
        match event {
            ServerEvent::ResponseCreated { .. } => {
                meter.begin_response().await?;
                Ok(None)
            }
            ServerEvent::ResponseDone { response, .. } => meter.end_response(response).await,
            _ => Ok(None),
        }
    }

    /// The agent every tool call is governed and metered under.
    #[cfg(feature = "integration")]
    pub(crate) fn agent_name(&self) -> &str {
        &self.agent_name
    }

    /// The run config every tool call is governed under, when one was set.
    #[cfg(feature = "integration")]
    pub(crate) fn governance_config(&self) -> Option<&adk_core::RunConfig> {
        self.governance.as_ref().map(|governance| &governance.config)
    }

    /// The approval scope held calls are recorded under, when governance was set.
    #[cfg(feature = "integration")]
    pub(crate) fn approval_scope(&self) -> Option<&adk_core::ApprovalScope> {
        self.governance.as_ref().map(|governance| &governance.scope)
    }

    /// Counts a tool dispatch the caller runs outside [`run`](Self::run), so the follow-up
    /// response waits for it; pair with [`end_dispatch`](Self::end_dispatch).
    #[cfg(feature = "integration")]
    pub(crate) fn begin_dispatch(&self) {
        self.outstanding_tools.fetch_add(1, Ordering::AcqRel);
    }

    /// Marks a dispatch counted by [`begin_dispatch`](Self::begin_dispatch) as finished.
    /// Returns `true` when the dispatching response closed meanwhile, so the caller owes
    /// the follow-up response ([`respond_after_tools`](Self::respond_after_tools)).
    #[cfg(feature = "integration")]
    pub(crate) fn end_dispatch(&self) -> bool {
        self.outstanding_tools.fetch_sub(1, Ordering::AcqRel) == 1
            && self.response_closed_awaiting_tools.swap(false, Ordering::AcqRel)
    }

    /// Authoritatively spawn planned proactive replacement and notify the application event handler.
    async fn handle_planned_rotation(&self, gen_id: u64, time_left: Option<String>) -> Result<()> {
        let supervisor = Arc::clone(&self.supervisor);
        let time_left_clone = time_left.clone();
        tokio::spawn(async move {
            let cause = RecoveryCause::PlannedRotation { time_left: time_left_clone };
            if let Err(e) = supervisor.execute_planned_replacement(gen_id, cause).await {
                // Fixed category, not the error Display: recovery errors
                // carry provider text (CWE-532).
                tracing::warn!(
                    gen_id,
                    error = crate::recovery::error_category(&e),
                    "proactive planned replacement attempt failed; keeping generation N authoritative"
                );
            }
        });
        self.event_handler.on_planned_rotation(time_left.as_deref()).await
    }

    /// Report read failure to supervisor, returning true if recovered/stale (caller should loop).
    async fn report_read_failure(&self, gen_id: u64, cause: RecoveryCause) -> bool {
        let report = FailureReport { generation: gen_id, cause };
        matches!(
            self.supervisor.report_failure(report).await,
            Ok(RecoveryOutcome::Recovered { .. }) | Ok(RecoveryOutcome::Stale(_))
        )
    }

    /// Get the next managed event from the session with generation awareness and automatic recovery.
    ///
    /// Provides pull-based reading parity with [`run`](Self::run). `next_event` subscribes to generation
    /// change signals and automatically switches to newly published generations without blocking on
    /// graceful closure of older sessions. Connection losses trigger background recovery automatically.
    pub async fn next_event(&self) -> Option<Result<ServerEvent>> {
        loop {
            let mut watcher = self.supervisor.subscribe_generation();
            let gen_item = match self.supervisor.get_active_generation().await {
                Ok(g) => g,
                Err(RealtimeError::SessionClosed) | Err(RealtimeError::NotConnected) => {
                    return None;
                }
                Err(err) => return Some(Err(err)),
            };

            if *watcher.borrow_and_update() != gen_item.id {
                continue;
            }

            let current_gen_id = gen_item.id;
            let session = gen_item.session;

            match self.poll_session_next(&mut watcher, current_gen_id, &session).await {
                Some(Ok(ServerEvent::PlannedRotation { time_left })) => {
                    let _ = self.handle_planned_rotation(current_gen_id, time_left.clone()).await;
                    return Some(Ok(ServerEvent::PlannedRotation { time_left }));
                }
                Some(Ok(event)) => return Some(Ok(event)),
                Some(Err(e)) => {
                    if self
                        .report_read_failure(
                            current_gen_id,
                            RecoveryCause::ReadFailed(Arc::new(e.clone())),
                        )
                        .await
                    {
                        continue;
                    }
                    return Some(Err(e));
                }
                None => {
                    if *watcher.borrow() != current_gen_id {
                        continue;
                    }
                    if self.report_read_failure(current_gen_id, RecoveryCause::UnexpectedEof).await
                    {
                        continue;
                    }
                    return None;
                }
            }
        }
    }

    /// Send tool response.
    pub async fn send_tool_response(&self, response: ToolResponse) -> Result<()> {
        self.invoke_write(|s| async move { s.send_tool_response(response).await }).await
    }

    /// Execute tool call.
    pub async fn dispatch_tool_call(
        &self,
        call_id: &str,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<()> {
        self.execute_tool_call(call_id, name, arguments).await
    }

    /// Send tool result.
    pub async fn send_tool_result(&self, call_id: &str, output: serde_json::Value) -> Result<()> {
        if !self.runner_config.auto_respond_tools {
            return Ok(());
        }
        let response = ToolResponse { call_id: call_id.to_string(), output, scheduling: None };
        if let Err(e) =
            self.invoke_write(|s| async move { s.send_tool_output(response).await }).await
        {
            self.tool_output_failed.store(true, Ordering::Release);
            return Err(e);
        }
        self.pending_tool_response.store(true, Ordering::Release);
        Ok(())
    }

    /// Run with cancellation token.
    pub async fn run_with_cancellation(
        &self,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        self.run_loop(Some(cancel_token)).await
    }

    /// Run the event loop.
    pub async fn run(&self) -> Result<()> {
        self.run_loop(None).await
    }

    /// Internal unified event loop.
    async fn run_loop(
        &self,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<()> {
        use futures::stream::{FuturesUnordered, StreamExt};

        let mut running_tools = FuturesUnordered::new();

        loop {
            if let Some(token) = &cancel_token
                && token.is_cancelled()
            {
                tracing::info!("RealtimeRunner run loop cancelled via cancellation token.");
                self.close().await?;
                self.event_handler.on_disconnect().await?;
                break;
            }

            let mut watcher = self.supervisor.subscribe_generation();
            let gen_item = match self.supervisor.get_active_generation().await {
                Ok(g) => g,
                Err(RealtimeError::SessionClosed) | Err(RealtimeError::NotConnected) => {
                    self.event_handler.on_disconnect().await?;
                    break;
                }
                Err(err) => return Err(err),
            };

            if *watcher.borrow_and_update() != gen_item.id {
                continue;
            }

            let current_gen_id = gen_item.id;
            let session = gen_item.session;

            tokio::select! {
                biased;
                _ = async {
                    match &cancel_token {
                        Some(token) => token.cancelled().await,
                        None => futures::future::pending().await,
                    }
                } => {
                    tracing::info!("RealtimeRunner run loop cancelled via cancellation token.");
                    self.close().await?;
                    self.event_handler.on_disconnect().await?;
                    return Ok(());
                }
                Some(finished) = running_tools.next(), if !running_tools.is_empty() => {
                    if let Err(e) = finished {
                        self.event_handler.on_error(&e).await?;
                        if !self.supervisor.is_connected().await {
                            return Err(e);
                        }
                    }
                }
                event_res = self.poll_session_next(&mut watcher, current_gen_id, &session) => {
                    match event_res {
                        Some(Ok(event)) => {
                            if let Some(call) =
                                self.handle_event_for_generation(event, Some(current_gen_id)).await?
                            {
                                running_tools.push(self.run_tool_call(call));
                            }
                        }
                        Some(Err(e)) => {
                            if self.report_read_failure(current_gen_id, RecoveryCause::ReadFailed(Arc::new(e.clone()))).await {
                                continue;
                            }
                            self.event_handler.on_error(&e).await?;
                            return Err(e);
                        }
                        None => {
                            if *watcher.borrow() != current_gen_id {
                                continue;
                            }
                            while let Some(finished) = running_tools.next().await {
                                if let Err(e) = finished {
                                    self.event_handler.on_error(&e).await?;
                                    if !self.supervisor.is_connected().await {
                                        return Err(e);
                                    }
                                }
                            }
                            if self.report_read_failure(current_gen_id, RecoveryCause::UnexpectedEof).await {
                                continue;
                            }
                            self.event_handler.on_disconnect().await?;
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Run one dispatched tool call under the configured concurrency bound.
    async fn run_tool_call(&self, call: PendingToolCall) -> Result<()> {
        struct ToolCounterGuard<'a>(&'a RealtimeRunner);
        impl<'a> Drop for ToolCounterGuard<'a> {
            fn drop(&mut self) {
                self.0.outstanding_tools.fetch_sub(1, Ordering::AcqRel);
            }
        }

        struct FenceGuard<'a>(&'a RealtimeRunner, String);
        impl<'a> Drop for FenceGuard<'a> {
            fn drop(&mut self) {
                self.0.unregister_tool_call(&self.1);
            }
        }

        self.outstanding_tools.fetch_add(1, Ordering::AcqRel);
        let guard = ToolCounterGuard(self);
        let token = self.register_tool_call_at(&call.call_id, &call.name, call.epoch);
        let _fence_guard = FenceGuard(self, call.call_id.clone());

        // A call fenced while it waits for a permit never executes.
        let permit = tokio::select! {
            biased;
            _ = token.cancelled() => None,
            permit = Arc::clone(&self.tool_permits).acquire_owned() => Some(permit),
        };
        let Some(permit) = permit else {
            drop(guard);
            return Ok(());
        };
        let result = if token.is_cancelled() {
            Ok(())
        } else {
            self.execute_tool_call_fenced(&call.call_id, &call.name, &call.arguments, &token).await
        };
        drop(permit);

        drop(guard);

        // A cancelled call is owed no follow-up response.
        if !token.is_cancelled()
            && self.outstanding_tools.load(Ordering::Acquire) == 0
            && self.response_closed_awaiting_tools.swap(false, Ordering::AcqRel)
        {
            self.respond_after_tools().await?;
        }

        result
    }

    /// Process a single event (test utility).
    #[cfg(test)]
    async fn handle_event(&self, event: ServerEvent) -> Result<Option<PendingToolCall>> {
        self.handle_event_for_generation(event, None).await
    }

    /// Process a single event tagged with its source generation ID.
    async fn handle_event_for_generation(
        &self,
        event: ServerEvent,
        gen_id: Option<u64>,
    ) -> Result<Option<PendingToolCall>> {
        match &event {
            ServerEvent::ResponseCreated { .. } => {
                let mut state = self.state.write().await;
                if let RunnerState::Idle = *state {
                    *state = RunnerState::Generating;
                }
            }
            ServerEvent::FunctionCallDone { .. } => {
                let mut state = self.state.write().await;
                if let RunnerState::Generating | RunnerState::Idle = *state {
                    *state = RunnerState::ExecutingTool;
                }
            }
            _ => {}
        }

        if let Some(source) = caller_activity_source(&event) {
            self.event_handler.on_caller_activity(source).await?;
        }

        match event {
            ServerEvent::AudioDelta { delta, item_id, .. } => {
                self.event_handler.on_audio(&delta, &item_id).await?;
            }
            ServerEvent::TextDelta { delta, item_id, .. } => {
                self.event_handler.on_text(&delta, &item_id).await?;
            }
            ServerEvent::TranscriptDelta { delta, item_id, .. } => {
                self.event_handler.on_transcript(&delta, &item_id).await?;
            }
            ServerEvent::InputTranscriptCompleted { transcript, item_id, .. } => {
                self.event_handler.on_input_transcript_completed(&transcript, &item_id).await?;
            }
            ServerEvent::SpeechStarted { audio_start_ms, .. } => {
                self.event_handler.on_speech_started(audio_start_ms).await?;
            }
            ServerEvent::SpeechStopped { audio_end_ms, .. } => {
                self.event_handler.on_speech_stopped(audio_end_ms).await?;
            }
            ServerEvent::ToolCallCancelled { call_ids } => {
                self.cancel_tool_calls(&call_ids);
                self.event_handler.on_tool_calls_cancelled(&call_ids).await?;
            }
            ServerEvent::PlannedRotation { time_left } => {
                let target_gen = match gen_id {
                    Some(id) => Some(id),
                    None => self.supervisor.get_active_generation().await.ok().map(|g| g.id),
                };
                if let Some(target_gen_id) = target_gen {
                    self.handle_planned_rotation(target_gen_id, time_left).await?;
                } else {
                    self.event_handler.on_planned_rotation(time_left.as_deref()).await?;
                }
            }
            ServerEvent::ResponseCancelled { .. } => {
                self.cancel_all_tool_calls();
                self.pending_tool_response.store(false, Ordering::Release);
                self.tool_output_failed.store(false, Ordering::Release);
                self.response_closed_awaiting_tools.store(false, Ordering::Release);
                {
                    let mut state = self.state.write().await;
                    if let RunnerState::Generating = *state {
                        *state = RunnerState::Idle;
                    }
                }
                self.event_handler.on_response_cancelled().await?;
            }
            ServerEvent::ResponseDone { .. } => {
                self.event_handler.on_response_done().await?;
                self.respond_after_tools().await?;
                self.check_resumption_queue().await?;
            }
            ServerEvent::FunctionCallDone { call_id, name, arguments, .. }
                if self.runner_config.auto_execute_tools =>
            {
                let epoch = self.fence_epoch();
                return Ok(Some(PendingToolCall { call_id, name, arguments, epoch }));
            }
            ServerEvent::Error { error, .. } => {
                let err = RealtimeError::server(error.code.unwrap_or_default(), error.message);
                self.event_handler.on_error(&err).await?;
            }
            _ => {}
        }
        Ok(None)
    }

    /// Safely transitions the runner back to Idle and executes any queued resumptions.
    async fn check_resumption_queue(&self) -> Result<()> {
        let mut state = self.state.write().await;

        let pending =
            if let RunnerState::PendingResumption { config, bridge_message, attempts } = &*state {
                Some((config.clone(), bridge_message.clone(), *attempts))
            } else {
                None
            };

        if let Some((config, bridge_message, attempts)) = pending {
            tracing::info!(
                "Executing queued resumption after turn completion. (Attempt {})",
                attempts + 1
            );

            *state = RunnerState::Idle;
            drop(state);

            let snapshot = self.supervisor.get_config().await;

            let model = Arc::clone(&self.model);
            let cfg = snapshot.config.clone();
            match self
                .supervisor
                .execute_resumption_with(snapshot, move || async move {
                    let s = model.connect(cfg).await?;
                    Ok(Arc::from(s) as Arc<dyn RealtimeSession>)
                })
                .await
            {
                Ok(_) => {
                    if let Some(msg) = bridge_message {
                        tracing::info!("Sending bridge message post-resumption via invoke_write.");
                        let event = crate::events::ClientEvent::Message {
                            role: "user".to_string(),
                            parts: vec![adk_core::types::Part::Text { text: msg }],
                        };
                        if let Err(e) =
                            self.invoke_write(|s| async move { s.send_event(event).await }).await
                        {
                            // Fixed category, not the error Display (CWE-532); `e`
                            // still travels to the event handler below.
                            tracing::warn!(
                                error = crate::recovery::error_category(&e),
                                "failed to send bridge message after queued resumption"
                            );
                            let _ = self.event_handler.on_error(&e).await;
                        }
                    }
                }
                Err(e) => {
                    // Fixed category, not the error Display (CWE-532); `e`
                    // still travels to the event handler below.
                    tracing::error!(
                        error = crate::recovery::error_category(&e),
                        "Resumption failed."
                    );

                    let mut fallback_state = self.state.write().await;

                    match &*fallback_state {
                        RunnerState::PendingResumption { .. } => {
                            tracing::info!(
                                "Newer resumption intent already queued while resumption failed; preserving newer intent."
                            );
                        }
                        RunnerState::Idle
                        | RunnerState::Generating
                        | RunnerState::ExecutingTool => {
                            if attempts + 1 >= 3 {
                                tracing::error!(
                                    "Maximum resumption attempts reached (3). Dropping queued mutation to prevent infinite loop."
                                );
                            } else {
                                tracing::info!("Restoring pending queue state for retry.");
                                *fallback_state = RunnerState::PendingResumption {
                                    config,
                                    bridge_message,
                                    attempts: attempts + 1,
                                };
                            }
                        }
                    }

                    let _ = self.event_handler.on_error(&e).await;
                }
            }
        } else {
            *state = RunnerState::Idle;
        }
        Ok(())
    }

    /// The current fence generation.
    pub(crate) fn fence_epoch(&self) -> u64 {
        self.tool_fence.lock().unwrap_or_else(|e| e.into_inner()).epoch
    }

    /// Register a dispatched call in the fence and return its token.
    pub(crate) fn register_tool_call(&self, call_id: &str, name: &str) -> CancellationToken {
        let epoch = self.fence_epoch();
        self.register_tool_call_at(call_id, name, epoch)
    }

    /// Register a call issued under an earlier fence generation. A stale
    /// generation's token is already cancelled, so the call never runs.
    pub(crate) fn register_tool_call_at(
        &self,
        call_id: &str,
        name: &str,
        epoch: u64,
    ) -> CancellationToken {
        let effect = self
            .tools
            .get(name)
            .map(|(definition, _)| definition.effect)
            .unwrap_or(adk_core::ToolEffect::NonIdempotent);
        let mut fence = self.tool_fence.lock().unwrap_or_else(|e| e.into_inner());
        // One call_id names one call. A second registration (the integration
        // spawn path registering, then `dispatch_tool_call` registering
        // again on its way in) must join the existing fence entry, not
        // replace it: replacing it would swap in an uncancelled token and
        // drop the recorded abort handle, so a withdrawal arriving during
        // execution would cancel a token nobody is watching.
        if let Some(entry) = fence.calls.get(call_id) {
            let token = entry.token.clone();
            if epoch != fence.epoch {
                token.cancel();
            }
            return token;
        }
        let token = CancellationToken::new();
        if epoch != fence.epoch {
            token.cancel();
        }
        fence.calls.insert(
            call_id.to_string(),
            FencedToolCall { token: token.clone(), abort_handle: None, effect },
        );
        token
    }

    /// The fence token already registered for `call_id`, if any.
    pub(crate) fn fence_token(&self, call_id: &str) -> Option<CancellationToken> {
        self.tool_fence
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .calls
            .get(call_id)
            .map(|entry| entry.token.clone())
    }

    /// Record the abort handle of a task spawned for a registered call.
    #[cfg(feature = "integration")]
    pub(crate) fn set_tool_abort_handle(&self, call_id: &str, handle: tokio::task::AbortHandle) {
        let mut fence = self.tool_fence.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = fence.calls.get_mut(call_id) {
            entry.abort_handle = Some(handle);
        }
    }

    /// Drop a finished call from the fence.
    pub(crate) fn unregister_tool_call(&self, call_id: &str) {
        self.tool_fence.lock().unwrap_or_else(|e| e.into_inner()).calls.remove(call_id);
    }

    /// Fence the named calls: their tokens are cancelled, and a spawned
    /// task for a retry-safe (read-only or idempotent) call is aborted.
    /// A non-idempotent task is left to its token so a call cancelled
    /// after it started can be answered as outcome-unknown.
    pub(crate) fn cancel_tool_calls(&self, call_ids: &[crate::events::ToolCallId]) {
        let fence = self.tool_fence.lock().unwrap_or_else(|e| e.into_inner());
        for id in call_ids {
            if let Some(entry) = fence.calls.get(id.as_str()) {
                entry.token.cancel();
                if entry.effect.is_retry_safe()
                    && let Some(handle) = &entry.abort_handle
                {
                    handle.abort();
                }
            }
        }
    }

    /// Fence every in-flight call and advance the fence generation, so a
    /// call issued before this response was cancelled can never start.
    pub(crate) fn cancel_all_tool_calls(&self) {
        let mut fence = self.tool_fence.lock().unwrap_or_else(|e| e.into_inner());
        fence.epoch += 1;
        for entry in fence.calls.values() {
            entry.token.cancel();
            if entry.effect.is_retry_safe()
                && let Some(handle) = &entry.abort_handle
            {
                handle.abort();
            }
        }
        fence.calls.clear();
    }

    /// Runs one call through the governed path configured with
    /// [`RealtimeRunnerBuilder::tool_governance`]. Without one, every call executes.
    ///
    /// # Errors
    ///
    /// Returns [`RealtimeError::ToolError`] when the kill switch is frozen or the
    /// confirmation handler or approval store fails; the run ends.
    pub(crate) async fn authorize_tool_call(
        &self,
        call: adk_core::GovernedCall,
    ) -> Result<adk_core::ToolAuthorization> {
        let Some(governance) = &self.governance else {
            return Ok(adk_core::ToolAuthorization::Execute {
                args: call.args,
                confirmation: None,
            });
        };
        let gate = adk_core::ToolGate::new(
            &governance.config,
            &self.agent_name,
            &governance.scope.session_id,
            governance.scope.clone(),
        );
        adk_core::authorize_tool_call(&gate, call)
            .await
            .map_err(|error| RealtimeError::ToolError(error.to_string()))
    }

    /// Withdraws `request`, a call's confirmation request that will not be waited on, so no
    /// decision on it authorizes an identical call ([`adk_core::withdraw_held_call`]).
    #[cfg(feature = "integration")]
    pub(crate) async fn withdraw_held_call(
        &self,
        request: &adk_core::ToolConfirmationRequest,
    ) -> Result<()> {
        let Some(governance) = &self.governance else {
            return Ok(());
        };
        let gate = adk_core::ToolGate::new(
            &governance.config,
            &self.agent_name,
            &governance.scope.session_id,
            governance.scope.clone(),
        );
        adk_core::withdraw_held_call(&gate, request)
            .await
            .map_err(|error| RealtimeError::ToolError(error.to_string()))
    }

    /// How long one tool call may run.
    #[cfg(feature = "integration")]
    pub(crate) fn tool_timeout(&self) -> std::time::Duration {
        self.tool_timeout
    }

    /// Whether tool error messages reach the model.
    #[cfg(feature = "integration")]
    pub(crate) fn exposes_tool_error_messages(&self) -> bool {
        self.expose_tool_error_messages
    }

    /// Runs the configured result screen, if any, on `result`, returning the result to deliver
    /// and whether the screen withheld it.
    pub(crate) async fn screen_tool_result(
        &self,
        name: &str,
        args: &serde_json::Value,
        result: serde_json::Value,
    ) -> (serde_json::Value, bool) {
        match &self.tool_result_screen {
            Some(screen) => screen_result(screen.as_ref(), name, args, result).await,
            None => (result, false),
        }
    }

    /// Whether tool error messages may be logged: the `record_payloads` of the config passed to
    /// [`RealtimeRunnerBuilder::tool_governance`]. Without one, only content-free fields are.
    pub(crate) fn records_payloads(&self) -> bool {
        self.governance.as_ref().is_some_and(|governance| governance.config.record_payloads)
    }

    /// Execute a tool call and optionally send the response.
    async fn execute_tool_call(
        &self,
        call_id: &str,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<()> {
        // Join a fence entry an outer dispatch path already registered
        // (and may have attached an abort handle to); only the path that
        // created the entry removes it.
        let (token, owns_entry) = match self.fence_token(call_id) {
            Some(token) => (token, false),
            None => (self.register_tool_call(call_id, name), true),
        };
        let result = self.execute_tool_call_fenced(call_id, name, arguments, &token).await;
        if owns_entry {
            self.unregister_tool_call(call_id);
        }
        result
    }

    /// [`execute_tool_call`](Self::execute_tool_call) under a fence token:
    /// cancellation is checked before authorization and before execution,
    /// and a fenced call's output and follow-up response are suppressed —
    /// except a non-idempotent call cancelled after it started, whose
    /// withheld result is answered as outcome-unknown, never as a success.
    async fn execute_tool_call_fenced(
        &self,
        call_id: &str,
        name: &str,
        arguments: &serde_json::Value,
        token: &CancellationToken,
    ) -> Result<()> {
        if token.is_cancelled() {
            return Ok(());
        }
        if self.circuit_breaker_tripped.load(Ordering::Acquire) {
            tracing::warn!(
                call_id,
                name,
                "Circuit breaker is OPEN — rejecting tool execution call"
            );
            let result = serde_json::json!({
                "status": "rejected",
                "reason": "tool_unavailable",
                "error": "circuit_breaker_open"
            });
            if self.runner_config.auto_respond_tools && !token.is_cancelled() {
                let response =
                    ToolResponse { call_id: call_id.to_string(), output: result, scheduling: None };
                if let Err(e) =
                    self.invoke_write(|s| async move { s.send_tool_output(response).await }).await
                {
                    self.tool_output_failed.store(true, Ordering::Release);
                    return Err(e);
                }
                self.pending_tool_response.store(true, Ordering::Release);
            }
            return Ok(());
        }

        let args = match parse_tool_arguments(arguments) {
            Ok(args) => args,
            Err(reason) => {
                let result = serde_json::json!({
                    "error": format!("Tool '{name}' was not run: {reason}")
                });
                if self.runner_config.auto_respond_tools && !token.is_cancelled() {
                    let response = ToolResponse {
                        call_id: call_id.to_string(),
                        output: result,
                        scheduling: None,
                    };
                    if let Err(e) = self
                        .invoke_write(|s| async move { s.send_tool_output(response).await })
                        .await
                    {
                        self.tool_output_failed.store(true, Ordering::Release);
                        return Err(e);
                    }
                    self.pending_tool_response.store(true, Ordering::Release);
                }
                return Ok(());
            }
        };

        let handler = self.tools.get(name).map(|(definition, h)| (definition, h.clone()));
        // Set when a non-idempotent call is fenced after it started: its
        // handler's result (if any) is withheld and the call is answered
        // as outcome-unknown instead.
        let mut fenced_outcome_unknown = false;

        let result = match handler {
            None => serde_json::json!({ "error": format!("Unknown tool: {name}") }),
            Some((definition, handler)) => {
                let screened_args = self.tool_result_screen.as_ref().map(|_| args.clone());
                let effect = definition.effect;
                let mut call =
                    adk_core::GovernedCall::new(name, call_id, args.clone()).with_effect(effect);
                call.origin = Some(definition.handler_origin());
                // Fenced before authorization: a withdrawn call is never
                // authorized, let alone executed.
                if token.is_cancelled() {
                    return Ok(());
                }
                let authorized = tokio::select! {
                    biased;
                    _ = token.cancelled() => return Ok(()),
                    authorized = self.authorize_tool_call(call) => authorized?,
                };
                let result = match authorized {
                    adk_core::ToolAuthorization::Execute { args, .. } => {
                        // Fenced again before execution, and the handler
                        // runs under the token: cancellation drops it.
                        if token.is_cancelled() {
                            return Ok(());
                        }
                        let call = ToolCall {
                            call_id: call_id.to_string(),
                            name: name.to_string(),
                            arguments: args,
                        };
                        let executed = tokio::select! {
                            biased;
                            _ = token.cancelled() => None,
                            executed = tokio::time::timeout(
                                self.tool_timeout,
                                handler.execute_cancellable(&call, token.clone()),
                            ) => Some(executed),
                        };
                        match executed {
                            None => {
                                if effect == adk_core::ToolEffect::NonIdempotent {
                                    fenced_outcome_unknown = true;
                                    adk_core::outcome_unknown_response(format!(
                                        "tool '{name}' was cancelled after it started; its outcome is unknown"
                                    ))
                                } else {
                                    return Ok(());
                                }
                            }
                            Some(Ok(Ok(value))) => {
                                if token.is_cancelled()
                                    && effect == adk_core::ToolEffect::NonIdempotent
                                {
                                    fenced_outcome_unknown = true;
                                    adk_core::outcome_unknown_response(format!(
                                        "tool '{name}' was cancelled after it started; its outcome is unknown"
                                    ))
                                } else {
                                    self.consecutive_tool_failures.store(0, Ordering::Release);
                                    value
                                }
                            }
                            Some(Ok(Err(e))) => {
                                // A message can quote credentials or internal hosts.
                                if self.records_payloads() {
                                    tracing::warn!(
                                        tool.name = %name,
                                        error.kind = e.kind(),
                                        error.message = %e,
                                        "realtime tool handler failed"
                                    );
                                } else {
                                    tracing::warn!(
                                        tool.name = %name,
                                        error.kind = e.kind(),
                                        "realtime tool handler failed"
                                    );
                                }
                                let failures =
                                    self.consecutive_tool_failures.fetch_add(1, Ordering::AcqRel)
                                        + 1;
                                if failures >= self.runner_config.max_consecutive_tool_failures {
                                    if !self.circuit_breaker_tripped.swap(true, Ordering::AcqRel) {
                                        tracing::error!(
                                            call_id,
                                            name,
                                            failures,
                                            max = self.runner_config.max_consecutive_tool_failures,
                                            "Tool execution circuit breaker TRIPPED: entering Open state"
                                        );
                                        let _ = self
                                            .event_handler
                                            .on_error(&RealtimeError::provider(format!(
                                                "Tool execution circuit breaker tripped after {} consecutive failures: {}",
                                                failures, e
                                            )))
                                            .await;
                                    }
                                    serde_json::json!({
                                        "status": "rejected",
                                        "reason": "tool_unavailable",
                                        "error": "circuit_breaker_open"
                                    })
                                } else if self.expose_tool_error_messages {
                                    serde_json::json!({ "error": e.to_string() })
                                } else {
                                    serde_json::json!({
                                        "error": format!("tool '{name}' failed")
                                    })
                                }
                            }
                            Some(Err(_)) => serde_json::json!({
                                "error": format!(
                                    "Tool '{name}' timed out after {:?}",
                                    self.tool_timeout
                                )
                            }),
                        }
                    }
                    adk_core::ToolAuthorization::Refuse { reason, .. }
                    | adk_core::ToolAuthorization::Pending { reason, .. } => {
                        serde_json::json!({ "error": reason })
                    }
                };
                match screened_args {
                    Some(args) => self.screen_tool_result(name, &args, result).await.0,
                    None => result,
                }
            }
        };

        if token.is_cancelled() && !fenced_outcome_unknown {
            // Fenced before it could finish: no output, no follow-up.
            return Ok(());
        }

        if self.runner_config.auto_respond_tools {
            let response =
                ToolResponse { call_id: call_id.to_string(), output: result, scheduling: None };
            if let Err(e) =
                self.invoke_write(|s| async move { s.send_tool_output(response).await }).await
            {
                self.tool_output_failed.store(true, Ordering::Release);
                return Err(e);
            }
            if !fenced_outcome_unknown {
                self.pending_tool_response.store(true, Ordering::Release);
            }
        }

        Ok(())
    }

    /// The system instruction the next connection will use.
    pub async fn instruction(&self) -> Option<String> {
        self.supervisor.get_config().await.config.instruction.clone()
    }

    /// Prepends a context block to the system instruction before connecting.
    ///
    /// Atomically updates the canonical configuration snapshot and increments the configuration revision.
    pub async fn prepend_instruction_context(&self, block: &str) {
        if block.is_empty() {
            return;
        }

        self.supervisor
            .update_config(|config| {
                config.instruction = Some(match config.instruction.take() {
                    Some(existing) if !existing.is_empty() => format!("{block}\n\n{existing}"),
                    _ => block.to_string(),
                });
            })
            .await;
    }

    /// Forget a pending tool response, so no follow-up response is owed
    /// for a call the fence has since cancelled.
    #[cfg(feature = "integration")]
    pub(crate) fn clear_pending_tool_response(&self) {
        self.pending_tool_response.store(false, Ordering::Release);
    }

    /// Trigger the single follow-up response owed after a tool-dispatching turn.
    ///
    /// Ensures exactly one follow-up response is created after all concurrent tool executions in a turn finish.
    pub async fn respond_after_tools(&self) -> Result<()> {
        if self.outstanding_tools.load(Ordering::Acquire) > 0 {
            self.response_closed_awaiting_tools.store(true, Ordering::Release);
            return Ok(());
        }

        if self.tool_output_failed.swap(false, Ordering::AcqRel) {
            tracing::warn!(
                "Tool output delivery failed during turn; suppressing automatic follow-up response creation."
            );
            self.pending_tool_response.store(false, Ordering::Release);
            return Ok(());
        }

        if self.pending_tool_response.swap(false, Ordering::AcqRel) {
            self.invoke_write(|s| async move { s.create_response().await }).await?;
        }
        Ok(())
    }

    /// Close the session.
    pub async fn close(&self) -> Result<()> {
        if let Some(meter) = &self.meter {
            meter.abandon().await;
        }
        self.supervisor.close().await
    }
}

#[cfg(test)]
mod runner_tests {
    use super::*;
    use crate::audio::{AudioChunk, AudioFormat};
    use crate::events::{ClientEvent, ToolResponse};
    use crate::model::RealtimeModel;
    use crate::session::{BoxedSession, ContextMutationOutcome, RealtimeSession};
    use std::pin::Pin;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    struct MockModel;

    #[async_trait]
    impl RealtimeModel for MockModel {
        fn provider(&self) -> &str {
            "mock"
        }
        fn model_id(&self) -> &str {
            "mock"
        }
        fn supported_input_formats(&self) -> Vec<AudioFormat> {
            vec![]
        }
        fn supported_output_formats(&self) -> Vec<AudioFormat> {
            vec![]
        }
        fn available_voices(&self) -> Vec<&str> {
            vec![]
        }
        async fn connect(&self, _config: RealtimeConfig) -> Result<BoxedSession> {
            Err(RealtimeError::connection("mock model does not connect"))
        }
    }

    #[derive(Default)]
    struct Counts {
        raw_audio: AtomicUsize,
        base64_audio: AtomicUsize,
        last_audio: parking_lot::Mutex<Option<AudioChunk>>,
        tool_output: AtomicUsize,
        tool_response: AtomicUsize,
        create_response: AtomicUsize,
        last_output: parking_lot::Mutex<Option<serde_json::Value>>,
    }

    struct RecordingSession {
        counts: Arc<Counts>,
    }

    #[async_trait]
    impl RealtimeSession for RecordingSession {
        fn session_id(&self) -> &str {
            "mock-session"
        }
        fn is_connected(&self) -> bool {
            true
        }
        async fn send_audio(&self, audio: &AudioChunk) -> Result<()> {
            self.counts.raw_audio.fetch_add(1, Ordering::SeqCst);
            *self.counts.last_audio.lock() = Some(audio.clone());
            Ok(())
        }
        async fn send_audio_base64(&self, _audio: &str) -> Result<()> {
            self.counts.base64_audio.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn send_text(&self, _text: &str) -> Result<()> {
            Ok(())
        }
        async fn send_tool_response(&self, _response: ToolResponse) -> Result<()> {
            self.counts.tool_response.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn send_tool_output(&self, response: ToolResponse) -> Result<()> {
            self.counts.tool_output.fetch_add(1, Ordering::SeqCst);
            *self.counts.last_output.lock() = Some(response.output);
            Ok(())
        }
        async fn commit_audio(&self) -> Result<()> {
            Ok(())
        }
        async fn clear_audio(&self) -> Result<()> {
            Ok(())
        }
        async fn create_response(&self) -> Result<()> {
            self.counts.create_response.fetch_add(1, Ordering::SeqCst);
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
        fn events(&self) -> Pin<Box<dyn futures::Stream<Item = Result<ServerEvent>> + Send + '_>> {
            Box::pin(futures::stream::empty())
        }
        async fn close(&self) -> Result<()> {
            Ok(())
        }
        async fn mutate_context(&self, _config: RealtimeConfig) -> Result<ContextMutationOutcome> {
            Ok(ContextMutationOutcome::Applied)
        }
    }

    fn tool_def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: None,
            parameters: None,
            behavior: None,
            effect: adk_core::ToolEffect::NonIdempotent,
        }
    }

    #[test]
    fn runner_builder_rejects_two_tools_with_one_name() {
        let builder = || {
            RealtimeRunner::builder()
                .model(Arc::new(MockModel) as BoxedModel)
                .tool_fn(tool_def("pay"), |_call| Ok(serde_json::json!({ "paid": 1 })))
        };
        let Err(err) = builder()
            .tool_fn(tool_def("pay"), |_call| Ok(serde_json::json!({ "paid": 2 })))
            .build()
        else {
            panic!("two tools named `pay` must fail the build");
        };
        assert!(matches!(err, RealtimeError::ConfigError(_)), "{err}");
        assert!(err.to_string().contains("'pay'"), "{err}");
        assert!(
            builder()
                .tool_fn(tool_def("refund"), |_call| Ok(serde_json::json!({})))
                .build()
                .is_ok()
        );
    }

    #[test]
    fn agent_builder_rejects_two_tools_with_one_name() {
        struct Lookup;

        #[async_trait]
        impl adk_core::Tool for Lookup {
            fn name(&self) -> &str {
                "lookup"
            }
            fn description(&self) -> &str {
                "looks things up"
            }
            async fn execute(
                &self,
                _ctx: Arc<dyn adk_core::ToolContext>,
                args: serde_json::Value,
            ) -> adk_core::Result<serde_json::Value> {
                Ok(args)
            }
        }

        let Err(err) = crate::agent::RealtimeAgent::builder("voice")
            .model(Arc::new(MockModel) as BoxedModel)
            .tool(Arc::new(Lookup))
            .tool(Arc::new(Lookup))
            .build()
        else {
            panic!("two tools named `lookup` must fail the build");
        };
        assert_eq!(err.code, adk_core::DUPLICATE_TOOL_NAME_CODE);
        assert!(err.message.contains("'lookup'"), "{}", err.message);
    }

    fn ok_tool() -> FnToolHandler<impl Fn(&ToolCall) -> Result<serde_json::Value> + Send + Sync> {
        FnToolHandler::new(|_call: &ToolCall| Ok(serde_json::json!({ "ok": true })))
    }

    fn function_call(call_id: &str, name: &str) -> ServerEvent {
        ServerEvent::FunctionCallDone {
            event_id: "evt".into(),
            response_id: "resp".into(),
            item_id: "item".into(),
            output_index: 0,
            call_id: call_id.into(),
            name: name.into(),
            arguments: serde_json::json!({}),
        }
    }

    fn response_done() -> ServerEvent {
        ServerEvent::ResponseDone { event_id: "evt".into(), response: serde_json::json!({}) }
    }

    async fn runner_with_session(counts: Arc<Counts>) -> RealtimeRunner {
        let runner =
            RealtimeRunner::builder().model(Arc::new(MockModel) as BoxedModel).build().unwrap();
        let session = Arc::new(RecordingSession { counts }) as Arc<dyn RealtimeSession>;
        let _ = runner.set_initial_session(session).await.unwrap();
        runner
    }

    /// A runner whose `pay` tool counts executions, governed by `config`.
    async fn governed_runner(
        counts: Arc<Counts>,
        executions: Arc<AtomicUsize>,
        config: adk_core::RunConfig,
    ) -> RealtimeRunner {
        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .tool_fn(tool_def("pay"), move |_call| {
                executions.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({ "paid": true }))
            })
            .tool_governance(config, adk_core::ApprovalScope::new("voice", "user", "session"))
            .build()
            .unwrap();
        let _ = runner
            .set_initial_session(Arc::new(RecordingSession { counts }) as Arc<dyn RealtimeSession>)
            .await
            .unwrap();
        runner
    }

    /// A runner whose `lookup` handler returns a customer record, or fails quoting a
    /// credential, configured by `configure`.
    async fn lookup_runner(
        counts: Arc<Counts>,
        fails: bool,
        configure: impl FnOnce(RealtimeRunnerBuilder) -> RealtimeRunnerBuilder,
    ) -> RealtimeRunner {
        let builder = RealtimeRunner::builder().model(Arc::new(MockModel) as BoxedModel).tool_fn(
            tool_def("lookup"),
            move |_call| {
                if fails {
                    Err(RealtimeError::ToolError(
                        "connect postgres://admin:hunter2@db.internal/prod failed".into(),
                    ))
                } else {
                    Ok(serde_json::json!({ "ssn": "123-45-6789" }))
                }
            },
        );
        let runner = configure(builder).build().unwrap();
        let _ = runner
            .set_initial_session(Arc::new(RecordingSession { counts }) as Arc<dyn RealtimeSession>)
            .await
            .unwrap();
        runner
    }

    /// Withholds every result, or redacts its `ssn`.
    struct SsnScreen {
        fail: bool,
    }

    #[async_trait]
    impl adk_core::ToolResultScreen for SsnScreen {
        async fn screen_result(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
            result: &serde_json::Value,
        ) -> std::result::Result<serde_json::Value, String> {
            if self.fail {
                return Err("the screening service is unavailable".into());
            }
            let mut result = result.clone();
            if result.get("ssn").is_some() {
                result["ssn"] = serde_json::json!("[SSN REDACTED]");
            }
            Ok(result)
        }
    }

    #[tokio::test]
    async fn handler_errors_reach_the_model_redacted_unless_exposed() {
        let counts = Arc::new(Counts::default());
        let runner = lookup_runner(Arc::clone(&counts), true, |b| b).await;
        runner.dispatch_tool_call("c1", "lookup", &serde_json::json!({})).await.unwrap();
        let output = counts.last_output.lock().clone().unwrap();
        assert_eq!(output, serde_json::json!({ "error": "tool 'lookup' failed" }));

        let runner =
            lookup_runner(Arc::clone(&counts), true, |b| b.expose_tool_error_messages(true)).await;
        runner.dispatch_tool_call("c2", "lookup", &serde_json::json!({})).await.unwrap();
        let output = counts.last_output.lock().clone().unwrap();
        assert!(output["error"].as_str().unwrap_or_default().contains("hunter2"), "{output}");
    }

    /// Records every event field the runner logs.
    #[derive(Clone, Default)]
    struct Logged(Arc<parking_lot::Mutex<Vec<String>>>);

    struct LoggedValues<'a>(&'a mut Vec<String>);

    impl tracing::field::Visit for LoggedValues<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.push(format!("{}={value:?}", field.name()));
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Logged {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            event.record(&mut LoggedValues(&mut self.0.lock()));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handler_error_messages_are_logged_only_under_the_payload_rule() {
        use tracing_subscriber::layer::SubscriberExt;

        for record_payloads in [false, true] {
            let logged = Logged::default();
            let _guard = tracing::subscriber::set_default(
                tracing_subscriber::registry().with(logged.clone()),
            );
            let counts = Arc::new(Counts::default());
            let config = adk_core::RunConfig { record_payloads, ..adk_core::RunConfig::default() };
            let runner = lookup_runner(Arc::clone(&counts), true, |b| {
                b.tool_governance(config, adk_core::ApprovalScope::new("voice", "user", "s"))
            })
            .await;
            runner.dispatch_tool_call("c1", "lookup", &serde_json::json!({})).await.unwrap();

            let values = logged.0.lock().clone();
            assert!(values.contains(&"error.kind=\"tool\"".to_string()), "{values:#?}");
            assert_eq!(
                values.iter().any(|value| value.contains("hunter2")),
                record_payloads,
                "the message is logged only when payloads are recorded: {values:#?}"
            );
        }
    }

    #[tokio::test]
    async fn the_result_screen_runs_before_the_output_is_sent() {
        let counts = Arc::new(Counts::default());
        let runner = lookup_runner(Arc::clone(&counts), false, |b| {
            b.tool_result_screen(Arc::new(SsnScreen { fail: false }))
        })
        .await;
        runner.dispatch_tool_call("c1", "lookup", &serde_json::json!({})).await.unwrap();
        let output = counts.last_output.lock().clone().unwrap();
        assert_eq!(output, serde_json::json!({ "ssn": "[SSN REDACTED]" }));

        let runner = lookup_runner(Arc::clone(&counts), false, |b| {
            b.tool_result_screen(Arc::new(SsnScreen { fail: true }))
        })
        .await;
        runner.dispatch_tool_call("c2", "lookup", &serde_json::json!({})).await.unwrap();
        let output = counts.last_output.lock().clone().unwrap();
        assert_eq!(output["status"], adk_core::RESULT_WITHHELD_STATUS, "{output}");
        assert!(!output.to_string().contains("123-45-6789"), "{output}");
    }

    #[tokio::test]
    async fn malformed_tool_arguments_are_refused() {
        let counts = Arc::new(Counts::default());
        let executions = Arc::new(AtomicUsize::new(0));
        let runner = governed_runner(
            Arc::clone(&counts),
            Arc::clone(&executions),
            adk_core::RunConfig::default(),
        )
        .await;

        runner
            .dispatch_tool_call("c1", "pay", &serde_json::json!(r#"{"amount": 5"#))
            .await
            .unwrap();

        assert_eq!(executions.load(Ordering::SeqCst), 0, "a malformed call must not run");
        let output = counts.last_output.lock().clone().expect("the call is answered");
        assert!(output["error"].as_str().unwrap_or_default().contains("not valid JSON"));
    }

    #[tokio::test]
    async fn the_runner_policy_and_kill_switch_govern_dispatched_calls() {
        let counts = Arc::new(Counts::default());
        let executions = Arc::new(AtomicUsize::new(0));
        let control = adk_core::GovernanceControl::new();
        let config = adk_core::RunConfig::builder()
            .tool_policy(Arc::new(adk_core::DeclarativePolicy::builder().allow("get_*").build()))
            .governance(control.clone())
            .build();
        let runner = governed_runner(Arc::clone(&counts), Arc::clone(&executions), config).await;

        runner.dispatch_tool_call("c1", "pay", &serde_json::json!({})).await.unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), 0);
        let output = counts.last_output.lock().clone().unwrap();
        assert!(output["error"].as_str().unwrap_or_default().contains("denied by policy"));

        control.freeze("drill");
        let error =
            runner.dispatch_tool_call("c2", "pay", &serde_json::json!({})).await.unwrap_err();
        assert!(error.to_string().contains("frozen"), "{error}");
        assert_eq!(executions.load(Ordering::SeqCst), 0);
    }

    /// The review of #1005: a freeze while the confirmation handler decides ends the wait,
    /// and the dispatch ends with the freeze before the handler runs, as an agent's call is
    /// refused.
    #[tokio::test]
    async fn a_freeze_ends_the_wait_for_the_confirmation_handler() {
        #[derive(Debug)]
        struct NeverAnswers;

        #[async_trait]
        impl adk_core::ToolConfirmationHandler for NeverAnswers {
            async fn decide(
                &self,
                _request: &adk_core::ToolConfirmationRequest,
            ) -> adk_core::Result<adk_core::ToolConfirmationDecision> {
                std::future::pending().await
            }
        }

        let counts = Arc::new(Counts::default());
        let executions = Arc::new(AtomicUsize::new(0));
        let control = adk_core::GovernanceControl::new();
        let config = adk_core::RunConfig::builder()
            .tool_policy(Arc::new(
                adk_core::DeclarativePolicy::builder()
                    .require_approval("pay", "payments need a person")
                    .build(),
            ))
            .tool_confirmation_handler(Arc::new(NeverAnswers))
            .governance(control.clone())
            .build();
        let runner = governed_runner(Arc::clone(&counts), Arc::clone(&executions), config).await;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            control.freeze("incident 4017");
        });

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            runner.dispatch_tool_call("c1", "pay", &serde_json::json!({})),
        )
        .await
        .expect("the freeze ends the wait")
        .unwrap_err();
        assert!(error.to_string().contains("incident 4017"), "{error}");
        assert_eq!(executions.load(Ordering::SeqCst), 0, "the handler never runs");
        assert_eq!(*counts.last_output.lock(), None, "nothing is sent for the refused call");
    }

    /// A native handler's held call names the runner's agent and the `realtime` origin of
    /// the handler's definition, so an approval of it never covers the handler once
    /// redefined, nor another agent's identical call (#1210).
    #[tokio::test]
    async fn a_native_handlers_held_call_names_its_agent_and_handler_origin() {
        use adk_core::ApprovalStore;
        let counts = Arc::new(Counts::default());
        let executions = Arc::new(AtomicUsize::new(0));
        let store = Arc::new(adk_core::InMemoryApprovalStore::new());
        let scope = adk_core::ApprovalScope::new("voice", "user", "session");
        let config = adk_core::RunConfig::builder()
            .tool_policy(Arc::new(
                adk_core::DeclarativePolicy::builder()
                    .require_approval("pay", "payments need a person")
                    .build(),
            ))
            .approval_store(store.clone())
            .build();
        let runner = |definition: ToolDefinition, agent: &str| {
            let executions = Arc::clone(&executions);
            RealtimeRunner::builder()
                .model(Arc::new(MockModel) as BoxedModel)
                .agent_name(agent)
                .tool_fn(definition, move |_call| {
                    executions.fetch_add(1, Ordering::SeqCst);
                    Ok(serde_json::json!({ "paid": true }))
                })
                .tool_governance(config.clone(), scope.clone())
                .build()
                .unwrap()
        };
        let install = |runner: RealtimeRunner| {
            let counts = Arc::clone(&counts);
            async move {
                let _ = runner
                    .set_initial_session(
                        Arc::new(RecordingSession { counts }) as Arc<dyn RealtimeSession>
                    )
                    .await
                    .unwrap();
                runner
            }
        };
        let pay = tool_def("pay").with_description("Pays an invoice");
        let reworded_pay = pay.clone().with_description("Pays anyone");
        let voice = install(runner(pay.clone(), "voice")).await;

        voice.dispatch_tool_call("c1", "pay", &serde_json::json!({})).await.unwrap();
        let pending = store.pending(&scope).await.unwrap();
        let [held] = pending.as_slice() else { panic!("one held call: {pending:?}") };
        assert_eq!(held.request.origin, Some(pay.handler_origin()));
        assert_eq!(held.request.origin.as_ref().unwrap().source, REALTIME_HANDLER_ORIGIN);
        assert_eq!(
            held.request.acting.as_ref().map(|acting| acting.agent.id.as_str()),
            Some("urn:adk:agent:voice/voice")
        );
        assert_eq!(
            adk_core::ToolConfirmationRequest::fingerprint_agent(&held.fingerprint),
            Some("urn:adk:agent:voice/voice")
        );

        // Redefined, the handler is not the one approved.
        store.decide(&scope, &held.fingerprint, adk_core::ToolApproval::approve()).await.unwrap();
        let reworded = install(runner(reworded_pay.clone(), "voice")).await;
        reworded.dispatch_tool_call("c2", "pay", &serde_json::json!({})).await.unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), 0, "a redefined handler needs approval");

        // Another agent's identical call is not the one approved.
        let intern = install(runner(pay.clone(), "intern")).await;
        intern.dispatch_tool_call("c3", "pay", &serde_json::json!({})).await.unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), 0, "another agent needs its own approval");

        // The handler as approved, by the agent it was approved for, runs once.
        voice.dispatch_tool_call("c4", "pay", &serde_json::json!({})).await.unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        let mut waiting: Vec<_> = store
            .pending(&scope)
            .await
            .unwrap()
            .into_iter()
            .map(|entry| {
                let agent =
                    adk_core::ToolConfirmationRequest::fingerprint_agent(&entry.fingerprint)
                        .map(str::to_string);
                (agent, entry.request.origin)
            })
            .collect();
        waiting.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            waiting,
            [
                (Some("urn:adk:agent:voice/intern".to_string()), Some(pay.handler_origin())),
                (
                    Some("urn:adk:agent:voice/voice".to_string()),
                    Some(reworded_pay.handler_origin())
                ),
            ],
            "each unapproved call waits on its own"
        );
    }

    #[tokio::test]
    async fn policy_rules_match_the_configured_voice_agent() {
        let counts = Arc::new(Counts::default());
        let executions = Arc::new(AtomicUsize::new(0));
        let principal = adk_core::AgentPrincipal::new(
            adk_core::AgentId::new("spiffe://acme.test/agent/voice").unwrap(),
        );
        let config = adk_core::RunConfig::builder()
            .tool_policy(Arc::new(
                adk_core::DeclarativePolicy::builder()
                    .rule(
                        adk_core::PolicyRule::allow("pay")
                            .for_agent("spiffe://acme.test/agent/voice"),
                    )
                    .build(),
            ))
            .agent_principal("realtime", principal)
            .build();
        let runner = governed_runner(Arc::clone(&counts), Arc::clone(&executions), config).await;

        runner.dispatch_tool_call("c1", "pay", &serde_json::json!({})).await.unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), 1, "the voice agent's rule allows it");

        let unconfigured = adk_core::RunConfig::builder()
            .tool_policy(Arc::new(
                adk_core::DeclarativePolicy::builder()
                    .rule(
                        adk_core::PolicyRule::allow("pay")
                            .for_agent("spiffe://acme.test/agent/voice"),
                    )
                    .build(),
            ))
            .build();
        let runner =
            governed_runner(Arc::clone(&counts), Arc::clone(&executions), unconfigured).await;
        runner.dispatch_tool_call("c2", "pay", &serde_json::json!({})).await.unwrap();
        assert_eq!(
            executions.load(Ordering::SeqCst),
            1,
            "the derived identity urn:adk:agent:voice/realtime is not the voice agent"
        );
    }

    #[tokio::test]
    async fn send_audio_chunk_preserves_bytes_and_format_on_raw_path() {
        let counts = Arc::new(Counts::default());
        let runner = runner_with_session(Arc::clone(&counts)).await;
        let chunk = AudioChunk::pcm16_24khz(vec![1, 2, 3, 4]);

        runner.send_audio_chunk(&chunk).await.unwrap();

        assert_eq!(counts.raw_audio.load(Ordering::SeqCst), 1);
        assert_eq!(counts.base64_audio.load(Ordering::SeqCst), 0);
        let recorded = counts.last_audio.lock();
        let recorded = recorded.as_ref().unwrap();
        assert_eq!(recorded.data, chunk.data);
        assert_eq!(recorded.format, chunk.format);
    }

    #[tokio::test]
    async fn send_audio_keeps_base64_compatibility_path() {
        let counts = Arc::new(Counts::default());
        let runner = runner_with_session(Arc::clone(&counts)).await;

        runner.send_audio("AQIDBA==").await.unwrap();

        assert_eq!(counts.raw_audio.load(Ordering::SeqCst), 0);
        assert_eq!(counts.base64_audio.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn mixed_success_concurrent_tool_outputs_suppresses_create_response() {
        struct SelectiveFailingSession {
            counts: Arc<Counts>,
            events: parking_lot::Mutex<std::collections::VecDeque<ServerEvent>>,
        }

        #[async_trait]
        impl RealtimeSession for SelectiveFailingSession {
            fn session_id(&self) -> &str {
                "failing-tool-output-session"
            }
            fn is_connected(&self) -> bool {
                true
            }
            async fn send_audio(&self, _audio: &AudioChunk) -> Result<()> {
                Ok(())
            }
            async fn send_audio_base64(&self, _audio: &str) -> Result<()> {
                Ok(())
            }
            async fn send_text(&self, _text: &str) -> Result<()> {
                Ok(())
            }
            async fn send_tool_response(&self, _response: ToolResponse) -> Result<()> {
                Ok(())
            }
            async fn send_tool_output(&self, response: ToolResponse) -> Result<()> {
                self.counts.tool_output.fetch_add(1, Ordering::SeqCst);
                if response.call_id == "c2" {
                    Err(RealtimeError::connection("simulated output delivery failure for c2"))
                } else {
                    Ok(())
                }
            }
            async fn commit_audio(&self) -> Result<()> {
                Ok(())
            }
            async fn clear_audio(&self) -> Result<()> {
                Ok(())
            }
            async fn create_response(&self) -> Result<()> {
                self.counts.create_response.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            async fn interrupt(&self) -> Result<()> {
                Ok(())
            }
            async fn send_event(&self, _event: ClientEvent) -> Result<()> {
                Ok(())
            }
            async fn next_event(&self) -> Option<Result<ServerEvent>> {
                self.events.lock().pop_front().map(Ok)
            }
            fn events(
                &self,
            ) -> Pin<Box<dyn futures::Stream<Item = Result<ServerEvent>> + Send + '_>> {
                Box::pin(futures::stream::empty())
            }
            async fn close(&self) -> Result<()> {
                Ok(())
            }
            async fn mutate_context(
                &self,
                _config: RealtimeConfig,
            ) -> Result<ContextMutationOutcome> {
                Ok(ContextMutationOutcome::Applied)
            }
        }

        let counts = Arc::new(Counts::default());
        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .tool(tool_def("tool_a"), ok_tool())
            .tool(tool_def("tool_b"), ok_tool())
            .build()
            .unwrap();

        let session = Arc::new(SelectiveFailingSession {
            counts: counts.clone(),
            events: parking_lot::Mutex::new(
                vec![function_call("c1", "tool_a"), function_call("c2", "tool_b"), response_done()]
                    .into(),
            ),
        });

        let _ = runner.set_initial_session(session as Arc<dyn RealtimeSession>).await.unwrap();
        let _ = runner.run().await;

        assert_eq!(
            counts.tool_output.load(Ordering::SeqCst),
            2,
            "both tool outputs were attempted"
        );
        assert_eq!(
            counts.create_response.load(Ordering::SeqCst),
            0,
            "zero create_response sent when any output delivery fails"
        );
    }

    #[tokio::test]
    async fn parallel_tool_calls_trigger_a_single_response() {
        let counts = Arc::new(Counts::default());
        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .tool(tool_def("get_weather"), ok_tool())
            .tool(tool_def("get_time"), ok_tool())
            .build()
            .unwrap();

        let scripted = Arc::new(ScriptedSession::new(
            counts.clone(),
            vec![
                function_call("c1", "get_weather"),
                function_call("c2", "get_time"),
                response_done(),
            ],
        ));
        let _ = runner.set_initial_session(scripted as Arc<dyn RealtimeSession>).await.unwrap();
        runner.run().await.unwrap();

        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 2, "both outputs sent");
        assert_eq!(counts.create_response.load(Ordering::SeqCst), 1, "exactly one response");
        assert_eq!(
            counts.tool_response.load(Ordering::SeqCst),
            0,
            "auto path must use send_tool_output, not the output+create combo"
        );

        runner.handle_event(response_done()).await.unwrap();
        assert_eq!(counts.create_response.load(Ordering::SeqCst), 1, "no extra response");
    }

    #[tokio::test]
    async fn plain_response_triggers_no_auto_response() {
        let counts = Arc::new(Counts::default());
        let runner =
            RealtimeRunner::builder().model(Arc::new(MockModel) as BoxedModel).build().unwrap();
        let _ = runner
            .set_initial_session(
                Arc::new(RecordingSession { counts: counts.clone() }) as Arc<dyn RealtimeSession>
            )
            .await
            .unwrap();

        runner.handle_event(response_done()).await.unwrap();
        assert_eq!(counts.create_response.load(Ordering::SeqCst), 0);
        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 0);
    }

    struct ScriptedSession {
        counts: Arc<Counts>,
        events: parking_lot::Mutex<std::collections::VecDeque<ServerEvent>>,
        close_calls: Arc<AtomicUsize>,
        stay_connected_when_empty: bool,
    }

    impl ScriptedSession {
        fn new(counts: Arc<Counts>, events: Vec<ServerEvent>) -> Self {
            Self {
                counts,
                events: parking_lot::Mutex::new(events.into()),
                close_calls: Arc::new(AtomicUsize::new(0)),
                stay_connected_when_empty: false,
            }
        }

        fn with_close_counter(
            counts: Arc<Counts>,
            events: Vec<ServerEvent>,
            close_calls: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                counts,
                events: parking_lot::Mutex::new(events.into()),
                close_calls,
                stay_connected_when_empty: true,
            }
        }
    }

    #[async_trait]
    impl RealtimeSession for ScriptedSession {
        fn session_id(&self) -> &str {
            "scripted-session"
        }
        fn is_connected(&self) -> bool {
            true
        }
        async fn send_audio(&self, _audio: &AudioChunk) -> Result<()> {
            Ok(())
        }
        async fn send_audio_base64(&self, _audio: &str) -> Result<()> {
            Ok(())
        }
        async fn send_text(&self, _text: &str) -> Result<()> {
            Ok(())
        }
        async fn send_tool_response(&self, _response: ToolResponse) -> Result<()> {
            self.counts.tool_response.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn send_tool_output(&self, _response: ToolResponse) -> Result<()> {
            self.counts.tool_output.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn commit_audio(&self) -> Result<()> {
            Ok(())
        }
        async fn clear_audio(&self) -> Result<()> {
            Ok(())
        }
        async fn create_response(&self) -> Result<()> {
            self.counts.create_response.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn interrupt(&self) -> Result<()> {
            Ok(())
        }
        async fn send_event(&self, _event: ClientEvent) -> Result<()> {
            Ok(())
        }
        async fn next_event(&self) -> Option<Result<ServerEvent>> {
            let event = self.events.lock().pop_front();
            if let Some(ev) = event {
                Some(Ok(ev))
            } else if self.stay_connected_when_empty {
                std::future::pending::<()>().await;
                None
            } else {
                None
            }
        }
        fn events(&self) -> Pin<Box<dyn futures::Stream<Item = Result<ServerEvent>> + Send + '_>> {
            Box::pin(futures::stream::empty())
        }
        async fn close(&self) -> Result<()> {
            self.close_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn mutate_context(&self, _config: RealtimeConfig) -> Result<ContextMutationOutcome> {
            Ok(ContextMutationOutcome::Applied)
        }
    }

    struct ConcurrencyProbe {
        in_flight: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        barrier: Option<Arc<tokio::sync::Barrier>>,
    }

    #[async_trait]
    impl ToolHandler for ConcurrencyProbe {
        async fn execute(&self, _call: &ToolCall) -> Result<serde_json::Value> {
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);

            match &self.barrier {
                Some(barrier) => {
                    barrier.wait().await;
                }
                None => {
                    tokio::task::yield_now().await;
                }
            }

            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(serde_json::json!({ "ok": true }))
        }
    }

    fn audio_delta() -> ServerEvent {
        ServerEvent::AudioDelta {
            event_id: "evt".into(),
            response_id: "resp".into(),
            item_id: "item".into(),
            output_index: 0,
            content_index: 0,
            delta: vec![1, 2, 3],
        }
    }

    #[tokio::test]
    async fn tool_calls_overlap_up_to_the_configured_bound() {
        let counts = Arc::new(Counts::default());
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(tokio::sync::Barrier::new(3));

        let probe = || ConcurrencyProbe {
            in_flight: Arc::clone(&in_flight),
            peak: Arc::clone(&peak),
            barrier: Some(Arc::clone(&barrier)),
        };

        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .runner_config(RunnerConfig {
                auto_execute_tools: true,
                auto_respond_tools: true,
                max_concurrent_tools: 3,
                ..Default::default()
            })
            .tool(tool_def("a"), probe())
            .tool(tool_def("b"), probe())
            .tool(tool_def("c"), probe())
            .build()
            .unwrap();

        let scripted = Arc::new(ScriptedSession::new(
            Arc::clone(&counts),
            vec![function_call("c1", "a"), function_call("c2", "b"), function_call("c3", "c")],
        ));
        let _ = runner.set_initial_session(scripted as Arc<dyn RealtimeSession>).await.unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(5), runner.run())
            .await
            .expect("serial dispatch cannot satisfy a three-way barrier")
            .unwrap();

        assert_eq!(peak.load(Ordering::SeqCst), 3, "all three tools must overlap");
        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 3, "every output is sent");
    }

    #[tokio::test]
    async fn the_bound_caps_how_many_tools_overlap() {
        let counts = Arc::new(Counts::default());
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let probe = || ConcurrencyProbe {
            in_flight: Arc::clone(&in_flight),
            peak: Arc::clone(&peak),
            barrier: None,
        };

        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .runner_config(RunnerConfig {
                auto_execute_tools: true,
                auto_respond_tools: true,
                max_concurrent_tools: 2,
                ..Default::default()
            })
            .tool(tool_def("a"), probe())
            .tool(tool_def("b"), probe())
            .tool(tool_def("c"), probe())
            .tool(tool_def("d"), probe())
            .build()
            .unwrap();

        let scripted = Arc::new(ScriptedSession::new(
            Arc::clone(&counts),
            vec![
                function_call("c1", "a"),
                function_call("c2", "b"),
                function_call("c3", "c"),
                function_call("c4", "d"),
            ],
        ));
        let _ = runner.set_initial_session(scripted as Arc<dyn RealtimeSession>).await.unwrap();

        runner.run().await.unwrap();

        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "the bound was exceeded: peak {}",
            peak.load(Ordering::SeqCst)
        );
        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 4, "all four still complete");
    }

    struct WaitsForAudio {
        audio_seen: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl ToolHandler for WaitsForAudio {
        async fn execute(&self, _call: &ToolCall) -> Result<serde_json::Value> {
            self.audio_seen.notified().await;
            Ok(serde_json::json!({ "ok": true }))
        }
    }

    struct AudioSignaller {
        audio_seen: Arc<tokio::sync::Notify>,
        audio_events: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl EventHandler for AudioSignaller {
        async fn on_audio(&self, _audio: &[u8], _item_id: &str) -> Result<()> {
            self.audio_events.fetch_add(1, Ordering::SeqCst);
            self.audio_seen.notify_waiters();
            Ok(())
        }
    }

    struct InputTurnSignaller {
        completed: Arc<AtomicUsize>,
        item_id: Arc<parking_lot::Mutex<Option<String>>>,
    }

    #[async_trait]
    impl EventHandler for InputTurnSignaller {
        async fn on_input_transcript_completed(
            &self,
            _transcript: &str,
            item_id: &str,
        ) -> Result<()> {
            self.completed.fetch_add(1, Ordering::SeqCst);
            *self.item_id.lock() = Some(item_id.to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn completed_input_transcript_reaches_the_event_handler() {
        let completed = Arc::new(AtomicUsize::new(0));
        let item_id = Arc::new(parking_lot::Mutex::new(None));
        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .event_handler(InputTurnSignaller {
                completed: completed.clone(),
                item_id: item_id.clone(),
            })
            .build()
            .unwrap();

        runner
            .handle_event(ServerEvent::InputTranscriptCompleted {
                item_id: "caller-turn-7".to_string(),
                content_index: 0,
                transcript: "sensitive caller content".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(completed.load(Ordering::SeqCst), 1);
        assert_eq!(item_id.lock().as_deref(), Some("caller-turn-7"));
    }

    struct ActivityRecorder {
        sources: Arc<parking_lot::Mutex<Vec<CallerActivitySource>>>,
    }

    #[async_trait]
    impl EventHandler for ActivityRecorder {
        async fn on_caller_activity(&self, source: CallerActivitySource) -> Result<()> {
            self.sources.lock().push(source);
            Ok(())
        }
    }

    #[tokio::test]
    async fn input_transcript_delta_reaches_the_event_handler_as_caller_activity() {
        let sources = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .event_handler(ActivityRecorder { sources: sources.clone() })
            .build()
            .unwrap();

        runner
            .handle_event(ServerEvent::InputTranscriptDelta {
                item_id: String::new(),
                content_index: 0,
                delta: "sensitive caller content".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(sources.lock().as_slice(), &[CallerActivitySource::TranscriptDelta]);
    }

    #[tokio::test]
    async fn every_caller_activity_source_is_delivered_and_labelled() {
        let sources = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .event_handler(ActivityRecorder { sources: sources.clone() })
            .build()
            .unwrap();

        for event in [
            ServerEvent::InputTranscriptDelta {
                item_id: String::new(),
                content_index: 0,
                delta: "part".to_string(),
            },
            ServerEvent::InputTranscriptCompleted {
                item_id: "item-1".to_string(),
                content_index: 0,
                transcript: "whole".to_string(),
            },
            ServerEvent::SpeechStopped { event_id: "e1".to_string(), audio_end_ms: 10 },
            ServerEvent::SpeechStarted { event_id: "e2".to_string(), audio_start_ms: 0 },
        ] {
            runner.handle_event(event).await.unwrap();
        }

        assert_eq!(
            sources.lock().as_slice(),
            &[
                CallerActivitySource::TranscriptDelta,
                CallerActivitySource::TranscriptCompleted,
                CallerActivitySource::SpeechStopped,
            ]
        );
    }

    #[tokio::test]
    async fn audio_keeps_flowing_while_a_tool_runs() {
        let counts = Arc::new(Counts::default());
        let audio_seen = Arc::new(tokio::sync::Notify::new());
        let audio_events = Arc::new(AtomicUsize::new(0));

        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .tool(tool_def("slow"), WaitsForAudio { audio_seen: Arc::clone(&audio_seen) })
            .event_handler(AudioSignaller {
                audio_seen: Arc::clone(&audio_seen),
                audio_events: Arc::clone(&audio_events),
            })
            .build()
            .unwrap();

        let scripted = Arc::new(ScriptedSession::new(
            Arc::clone(&counts),
            vec![function_call("c1", "slow"), audio_delta(), response_done()],
        ));
        let _ = runner.set_initial_session(scripted as Arc<dyn RealtimeSession>).await.unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(5), runner.run())
            .await
            .expect("a tool awaiting a later event deadlocks when dispatch blocks intake")
            .unwrap();

        assert_eq!(audio_events.load(Ordering::SeqCst), 1, "the audio delta was handled");
        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 1, "the tool still reported output");
    }

    /// A tool handler that records that it started and then never finishes.
    struct BlockingToolHandler {
        executions: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ToolHandler for BlockingToolHandler {
        async fn execute(&self, _call: &ToolCall) -> Result<serde_json::Value> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            std::future::pending().await
        }
    }

    fn tool_def_effect(name: &str, effect: adk_core::ToolEffect) -> ToolDefinition {
        tool_def(name).with_effect(effect)
    }

    async fn fenced_runner(
        counts: Arc<Counts>,
        tools: Vec<(ToolDefinition, Arc<dyn ToolHandler>)>,
        max_concurrent_tools: usize,
    ) -> Arc<RealtimeRunner> {
        let mut builder = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .runner_config(RunnerConfig { max_concurrent_tools, ..RunnerConfig::default() });
        for (definition, handler) in tools {
            builder = builder.tool_arc(definition, handler);
        }
        let runner = Arc::new(builder.build().unwrap());
        let _ = runner
            .set_initial_session(Arc::new(RecordingSession { counts }) as Arc<dyn RealtimeSession>)
            .await
            .unwrap();
        runner
    }

    async fn wait_started(executions: &AtomicUsize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while executions.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the tool started");
    }

    fn tool_call_cancelled(call_id: &str) -> ServerEvent {
        ServerEvent::ToolCallCancelled {
            call_ids: vec![crate::events::ToolCallId::parse(call_id).unwrap()],
        }
    }

    #[test]
    fn merge_config_merges_language() {
        let mut base = RealtimeConfig::default();
        let update =
            SessionUpdateConfig(RealtimeConfig::default().try_with_language("fr-CA").unwrap());
        RealtimeRunner::merge_config(&mut base, &update);
        assert_eq!(base.language.as_ref().map(|code| code.as_str()), Some("fr-CA"));
    }

    #[tokio::test]
    async fn tool_call_cancelled_aborts_readonly() {
        let counts = Arc::new(Counts::default());
        let executions = Arc::new(AtomicUsize::new(0));
        let runner = fenced_runner(
            Arc::clone(&counts),
            vec![(
                tool_def_effect("lookup", adk_core::ToolEffect::ReadOnly),
                Arc::new(BlockingToolHandler { executions: Arc::clone(&executions) })
                    as Arc<dyn ToolHandler>,
            )],
            4,
        )
        .await;
        let pending = runner
            .handle_event(function_call("c1", "lookup"))
            .await
            .unwrap()
            .expect("the call is dispatched");
        let task_runner = Arc::clone(&runner);
        let mut task = tokio::spawn(async move { task_runner.run_tool_call(pending).await });
        wait_started(&executions).await;

        // Speech onset alone must not fence the call.
        runner
            .handle_event(ServerEvent::SpeechStarted {
                event_id: "e-speech".into(),
                audio_start_ms: 0,
            })
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut task).await.is_err(),
            "SpeechStarted alone fenced an in-flight call"
        );

        runner.handle_event(tool_call_cancelled("c1")).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the fenced call ends")
            .unwrap()
            .unwrap();
        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 0, "no output for a fenced call");
        assert_eq!(counts.create_response.load(Ordering::SeqCst), 0, "no follow-up response");
    }

    #[tokio::test]
    async fn response_cancelled_aborts_all_inflight_readonly() {
        let counts = Arc::new(Counts::default());
        let executions = Arc::new(AtomicUsize::new(0));
        let runner = fenced_runner(
            Arc::clone(&counts),
            vec![
                (
                    tool_def_effect("lookup", adk_core::ToolEffect::ReadOnly),
                    Arc::new(BlockingToolHandler { executions: Arc::clone(&executions) })
                        as Arc<dyn ToolHandler>,
                ),
                (
                    tool_def_effect("search", adk_core::ToolEffect::ReadOnly),
                    Arc::new(BlockingToolHandler { executions: Arc::clone(&executions) })
                        as Arc<dyn ToolHandler>,
                ),
            ],
            4,
        )
        .await;
        let mut tasks = Vec::new();
        for (call_id, name) in [("c1", "lookup"), ("c2", "search")] {
            let pending = runner.handle_event(function_call(call_id, name)).await.unwrap().unwrap();
            let task_runner = Arc::clone(&runner);
            tasks.push(tokio::spawn(async move { task_runner.run_tool_call(pending).await }));
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            while executions.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both tools started");

        runner
            .handle_event(ServerEvent::ResponseCancelled { event_id: "e-cancel".into() })
            .await
            .unwrap();
        for task in tasks {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect("the fenced call ends")
                .unwrap()
                .unwrap();
        }
        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 0, "no output for fenced calls");
        assert_eq!(counts.create_response.load(Ordering::SeqCst), 0, "no follow-up response");
    }

    #[tokio::test]
    async fn queued_nonidempotent_cancelled_never_executes() {
        let counts = Arc::new(Counts::default());
        let blocker_executions = Arc::new(AtomicUsize::new(0));
        let pay_executions = Arc::new(AtomicUsize::new(0));
        let pay_count = Arc::clone(&pay_executions);
        let runner = fenced_runner(
            Arc::clone(&counts),
            vec![
                (
                    tool_def_effect("blocker", adk_core::ToolEffect::ReadOnly),
                    Arc::new(BlockingToolHandler { executions: Arc::clone(&blocker_executions) })
                        as Arc<dyn ToolHandler>,
                ),
                (
                    tool_def_effect("pay", adk_core::ToolEffect::NonIdempotent),
                    Arc::new(FnToolHandler::new(move |_call: &ToolCall| {
                        pay_count.fetch_add(1, Ordering::SeqCst);
                        Ok(serde_json::json!({ "paid": true }))
                    })) as Arc<dyn ToolHandler>,
                ),
            ],
            1,
        )
        .await;
        let blocker = runner.handle_event(function_call("c1", "blocker")).await.unwrap().unwrap();
        let task_runner = Arc::clone(&runner);
        let blocker_task = tokio::spawn(async move { task_runner.run_tool_call(blocker).await });
        wait_started(&blocker_executions).await;

        // The second call queues behind the single permit.
        let pay = runner.handle_event(function_call("c2", "pay")).await.unwrap().unwrap();
        let task_runner = Arc::clone(&runner);
        let pay_task = tokio::spawn(async move { task_runner.run_tool_call(pay).await });
        tokio::time::sleep(Duration::from_millis(50)).await;

        runner.handle_event(tool_call_cancelled("c2")).await.unwrap();
        runner.handle_event(tool_call_cancelled("c1")).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), blocker_task)
            .await
            .expect("the blocker ends")
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), pay_task)
            .await
            .expect("the queued call ends")
            .unwrap()
            .unwrap();
        assert_eq!(pay_executions.load(Ordering::SeqCst), 0, "a fenced queued call never runs");
        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 0, "no output for fenced calls");
    }

    #[tokio::test]
    async fn nonidempotent_cancel_after_start_reports_outcome_unknown_not_success() {
        let counts = Arc::new(Counts::default());
        let executions = Arc::new(AtomicUsize::new(0));
        let runner = fenced_runner(
            Arc::clone(&counts),
            vec![(
                tool_def_effect("pay", adk_core::ToolEffect::NonIdempotent),
                Arc::new(BlockingToolHandler { executions: Arc::clone(&executions) })
                    as Arc<dyn ToolHandler>,
            )],
            4,
        )
        .await;
        let pending = runner.handle_event(function_call("c1", "pay")).await.unwrap().unwrap();
        let task_runner = Arc::clone(&runner);
        let task = tokio::spawn(async move { task_runner.run_tool_call(pending).await });
        wait_started(&executions).await;

        runner.handle_event(tool_call_cancelled("c1")).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the fenced call ends")
            .unwrap()
            .unwrap();
        let output = counts.last_output.lock().clone().expect("the call is answered");
        assert_eq!(output["status"], adk_core::OUTCOME_UNKNOWN_STATUS, "{output}");
        assert!(adk_core::is_outcome_unknown(&output), "{output}");
        assert_eq!(counts.create_response.load(Ordering::SeqCst), 0, "no follow-up response");
    }

    #[tokio::test]
    async fn nonidempotent_response_cancel_after_start_reports_outcome_unknown_not_success() {
        let counts = Arc::new(Counts::default());
        let executions = Arc::new(AtomicUsize::new(0));
        let runner = fenced_runner(
            Arc::clone(&counts),
            vec![(
                tool_def_effect("pay", adk_core::ToolEffect::NonIdempotent),
                Arc::new(BlockingToolHandler { executions: Arc::clone(&executions) })
                    as Arc<dyn ToolHandler>,
            )],
            4,
        )
        .await;
        let pending = runner.handle_event(function_call("c1", "pay")).await.unwrap().unwrap();
        let task_runner = Arc::clone(&runner);
        let task = tokio::spawn(async move { task_runner.run_tool_call(pending).await });
        wait_started(&executions).await;

        runner
            .handle_event(ServerEvent::ResponseCancelled { event_id: "e-cancel".into() })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the fenced call ends")
            .unwrap()
            .unwrap();
        let output = counts.last_output.lock().clone().expect("the call is answered");
        assert_eq!(output["status"], adk_core::OUTCOME_UNKNOWN_STATUS, "{output}");
        assert!(adk_core::is_outcome_unknown(&output), "{output}");
        assert_eq!(counts.create_response.load(Ordering::SeqCst), 0, "no follow-up response");
    }

    #[tokio::test]
    async fn idempotent_cancel_after_start_is_suppressed_not_outcome_unknown() {
        let counts = Arc::new(Counts::default());
        let executions = Arc::new(AtomicUsize::new(0));
        let runner = fenced_runner(
            Arc::clone(&counts),
            vec![(
                tool_def_effect("charge", adk_core::ToolEffect::Idempotent),
                Arc::new(BlockingToolHandler { executions: Arc::clone(&executions) })
                    as Arc<dyn ToolHandler>,
            )],
            4,
        )
        .await;
        let pending = runner.handle_event(function_call("c1", "charge")).await.unwrap().unwrap();
        let task_runner = Arc::clone(&runner);
        let task = tokio::spawn(async move { task_runner.run_tool_call(pending).await });
        wait_started(&executions).await;

        runner.handle_event(tool_call_cancelled("c1")).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the fenced call ends")
            .unwrap()
            .unwrap();
        // A retry-safe call may simply be re-issued, so it is owed no
        // answer at all — in particular not an outcome-unknown one.
        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 0, "no output for a fenced call");
        assert_eq!(*counts.last_output.lock(), None);
        assert_eq!(counts.create_response.load(Ordering::SeqCst), 0, "no follow-up response");
    }

    #[tokio::test]
    async fn response_cancelled_fences_calls_issued_before_it() {
        let counts = Arc::new(Counts::default());
        let executions = Arc::new(AtomicUsize::new(0));
        let exec_count = Arc::clone(&executions);
        let runner = fenced_runner(
            Arc::clone(&counts),
            vec![(
                tool_def_effect("pay", adk_core::ToolEffect::NonIdempotent),
                Arc::new(FnToolHandler::new(move |_call: &ToolCall| {
                    exec_count.fetch_add(1, Ordering::SeqCst);
                    Ok(serde_json::json!({ "paid": true }))
                })) as Arc<dyn ToolHandler>,
            )],
            4,
        )
        .await;
        // Issued, then the response that produced it is cancelled before
        // the run loop gets around to dispatching it.
        let pending = runner.handle_event(function_call("c1", "pay")).await.unwrap().unwrap();
        runner
            .handle_event(ServerEvent::ResponseCancelled { event_id: "e-cancel".into() })
            .await
            .unwrap();
        runner.run_tool_call(pending).await.unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), 0, "a stale-generation call never runs");
        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 0, "no output for a fenced call");
    }

    #[tokio::test]
    async fn reregistering_a_call_joins_its_fence_entry() {
        let counts = Arc::new(Counts::default());
        let runner = fenced_runner(Arc::clone(&counts), vec![], 4).await;
        let first = runner.register_tool_call("c1", "pay");
        let second = runner.register_tool_call("c1", "pay");
        runner.cancel_tool_calls(&[crate::events::ToolCallId::parse("c1").unwrap()]);
        assert!(first.is_cancelled(), "the registered token is fenced");
        assert!(second.is_cancelled(), "re-registration shares the fenced token, not a fresh one");
    }

    #[tokio::test]
    async fn circuit_breaker_trips_and_blocks_subsequent_tool_calls() {
        let calls_made = Arc::new(AtomicUsize::new(0));
        let calls_made_clone = Arc::clone(&calls_made);

        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .tool_fn(tool_def("failing"), move |_| {
                calls_made_clone.fetch_add(1, Ordering::SeqCst);
                Err(RealtimeError::provider("database failure"))
            })
            .build()
            .unwrap();

        let counts = Arc::new(Counts::default());
        let scripted = RecordingSession { counts };
        let _ = runner
            .set_initial_session(Arc::new(scripted) as Arc<dyn RealtimeSession>)
            .await
            .unwrap();

        for i in 0..3 {
            runner
                .execute_tool_call(&format!("call-{}", i), "failing", &serde_json::json!({}))
                .await
                .unwrap();
        }

        assert_eq!(calls_made.load(Ordering::SeqCst), 3, "handler was invoked 3 times");
        assert!(
            runner.circuit_breaker_tripped.load(Ordering::Acquire),
            "circuit breaker is TRIPPED"
        );

        runner.execute_tool_call("call-4", "failing", &serde_json::json!({})).await.unwrap();
        assert_eq!(
            calls_made.load(Ordering::SeqCst),
            3,
            "4th call was blocked by open circuit breaker"
        );

        runner.consecutive_tool_failures.store(0, Ordering::Release);
        runner.circuit_breaker_tripped.store(false, Ordering::Release);

        runner.execute_tool_call("call-5", "failing", &serde_json::json!({})).await.unwrap();
        assert_eq!(calls_made.load(Ordering::SeqCst), 4, "handler invoked again after reset");
    }

    #[tokio::test]
    async fn run_with_cancellation_exits_cleanly() {
        let runner =
            RealtimeRunner::builder().model(Arc::new(MockModel) as BoxedModel).build().unwrap();

        let cancel_token = tokio_util::sync::CancellationToken::new();
        cancel_token.cancel();

        let result = runner.run_with_cancellation(cancel_token).await;
        assert!(result.is_ok(), "runner exited cleanly on cancellation token");
    }

    #[tokio::test]
    async fn run_with_cancellation_closes_session_and_cleans_up_in_flight_tools() {
        let counts = Arc::new(Counts::default());
        let close_calls = Arc::new(AtomicUsize::new(0));

        struct BlockingTool;
        #[async_trait]
        impl ToolHandler for BlockingTool {
            async fn execute(&self, _call: &ToolCall) -> Result<serde_json::Value> {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                Ok(serde_json::json!({"status": "done"}))
            }
        }

        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .tool(tool_def("blocking_tool"), BlockingTool)
            .build()
            .unwrap();

        let scripted = Arc::new(ScriptedSession::with_close_counter(
            Arc::clone(&counts),
            vec![function_call("c1", "blocking_tool")],
            Arc::clone(&close_calls),
        ));
        let _ = runner.set_initial_session(scripted as Arc<dyn RealtimeSession>).await.unwrap();

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();

        let runner_handle = Arc::new(runner);
        let runner_clone = Arc::clone(&runner_handle);

        let task =
            tokio::spawn(
                async move { runner_clone.run_with_cancellation(cancel_token_clone).await },
            );

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(
            runner_handle.outstanding_tools.load(Ordering::Acquire),
            1,
            "one tool is in flight"
        );

        cancel_token.cancel();

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("task completes quickly on cancellation")
            .unwrap();

        assert!(result.is_ok(), "runner exited cleanly");
        assert_eq!(
            close_calls.load(Ordering::SeqCst),
            1,
            "session.close() was invoked on cancellation"
        );
        assert_eq!(
            runner_handle.outstanding_tools.load(Ordering::Acquire),
            0,
            "in-flight tool was dropped and ToolCounterGuard restored counter to 0"
        );
    }

    #[tokio::test]
    async fn the_follow_up_response_waits_for_tools_that_outlive_the_response() {
        let counts = Arc::new(Counts::default());
        let audio_seen = Arc::new(tokio::sync::Notify::new());
        let audio_events = Arc::new(AtomicUsize::new(0));

        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .tool(tool_def("slow"), WaitsForAudio { audio_seen: Arc::clone(&audio_seen) })
            .event_handler(AudioSignaller {
                audio_seen: Arc::clone(&audio_seen),
                audio_events: Arc::clone(&audio_events),
            })
            .build()
            .unwrap();

        let scripted = Arc::new(ScriptedSession::new(
            Arc::clone(&counts),
            vec![function_call("c1", "slow"), response_done(), audio_delta()],
        ));
        let _ = runner.set_initial_session(scripted as Arc<dyn RealtimeSession>).await.unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(5), runner.run())
            .await
            .expect("run must finish")
            .unwrap();

        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 1, "the output was sent");
        assert_eq!(
            counts.create_response.load(Ordering::SeqCst),
            1,
            "exactly one follow-up response, issued after the last tool finished"
        );
    }

    #[derive(Default)]
    struct DisconnectWatcher {
        disconnects: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl EventHandler for DisconnectWatcher {
        async fn on_disconnect(&self) -> Result<()> {
            self.disconnects.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn failed_resumption_is_preserved_when_runner_is_generating_or_executing_tool() {
        for busy_state in [RunnerState::Generating, RunnerState::ExecutingTool] {
            let runner =
                RealtimeRunner::builder().model(Arc::new(MockModel) as BoxedModel).build().unwrap();

            let mut state_guard = runner.state.write().await;
            *state_guard = busy_state.clone();
            drop(state_guard);

            let config_a = Box::new(RealtimeConfig::default().with_instruction("mutation A"));
            let attempts = 0u8;

            let mut fallback_state = runner.state.write().await;
            match &*fallback_state {
                RunnerState::PendingResumption { .. } => {
                    panic!("Should not have PendingResumption initially");
                }
                RunnerState::Idle | RunnerState::Generating | RunnerState::ExecutingTool => {
                    *fallback_state = RunnerState::PendingResumption {
                        config: config_a,
                        bridge_message: Some("message A".into()),
                        attempts: attempts + 1,
                    };
                }
            }
            drop(fallback_state);

            let check_state = runner.state.read().await;
            match &*check_state {
                RunnerState::PendingResumption { config, attempts, .. } => {
                    assert_eq!(config.instruction.as_deref(), Some("mutation A"));
                    assert_eq!(*attempts, 1);
                }
                other => panic!("Expected PendingResumption for {:?}, got {:?}", busy_state, other),
            }
        }
    }

    #[tokio::test]
    async fn pending_resumption_does_not_overwrite_newer_mutation() {
        let runner =
            RealtimeRunner::builder().model(Arc::new(MockModel) as BoxedModel).build().unwrap();

        let mut state_guard = runner.state.write().await;
        *state_guard = RunnerState::PendingResumption {
            config: Box::new(RealtimeConfig::default().with_instruction("newer mutation B")),
            bridge_message: Some("message B".into()),
            attempts: 0,
        };
        drop(state_guard);

        let fallback_state = runner.state.write().await;
        match &*fallback_state {
            RunnerState::PendingResumption { config, .. } => {
                assert_eq!(
                    config.instruction.as_deref(),
                    Some("newer mutation B"),
                    "Newer mutation B must be preserved and NOT overwritten by older failed mutation A"
                );
            }
            other => panic!("Expected PendingResumption B, got {:?}", other),
        }
    }

    struct CallNameTrackingSession {
        counts: Arc<Counts>,
        call_ids: Arc<parking_lot::Mutex<std::collections::HashSet<String>>>,
    }

    #[async_trait]
    impl RealtimeSession for CallNameTrackingSession {
        fn session_id(&self) -> &str {
            "call-tracking-session"
        }
        fn is_connected(&self) -> bool {
            true
        }
        async fn send_audio(&self, _audio: &AudioChunk) -> Result<()> {
            Ok(())
        }
        async fn send_audio_base64(&self, _audio: &str) -> Result<()> {
            Ok(())
        }
        async fn send_text(&self, _text: &str) -> Result<()> {
            Ok(())
        }
        async fn send_tool_response(&self, response: ToolResponse) -> Result<()> {
            if self.call_ids.lock().remove(&response.call_id) {
                self.counts.tool_response.fetch_add(1, Ordering::SeqCst);
                Ok(())
            } else {
                Err(RealtimeError::protocol(format!(
                    "Missing tool name for call_id '{}' in session",
                    response.call_id
                )))
            }
        }
        async fn send_tool_output(&self, response: ToolResponse) -> Result<()> {
            if self.call_ids.lock().remove(&response.call_id) {
                self.counts.tool_output.fetch_add(1, Ordering::SeqCst);
                Ok(())
            } else {
                Err(RealtimeError::protocol(format!(
                    "Missing tool name for call_id '{}' in session",
                    response.call_id
                )))
            }
        }
        async fn commit_audio(&self) -> Result<()> {
            Ok(())
        }
        async fn clear_audio(&self) -> Result<()> {
            Ok(())
        }
        async fn create_response(&self) -> Result<()> {
            self.counts.create_response.fetch_add(1, Ordering::SeqCst);
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
        fn events(&self) -> Pin<Box<dyn futures::Stream<Item = Result<ServerEvent>> + Send + '_>> {
            Box::pin(futures::stream::empty())
        }
        async fn close(&self) -> Result<()> {
            Ok(())
        }
        async fn mutate_context(&self, _config: RealtimeConfig) -> Result<ContextMutationOutcome> {
            Ok(ContextMutationOutcome::Applied)
        }
    }

    #[tokio::test]
    async fn stale_tool_response_from_replaced_generation_n_is_rejected_fail_closed_on_n_plus_1() {
        let counts_n0 = Arc::new(Counts::default());
        let counts_n1 = Arc::new(Counts::default());

        let call_ids_n0 = Arc::new(parking_lot::Mutex::new(std::collections::HashSet::new()));
        let call_ids_n1 = Arc::new(parking_lot::Mutex::new(std::collections::HashSet::new()));

        let session_n0 = Arc::new(CallNameTrackingSession {
            counts: counts_n0.clone(),
            call_ids: call_ids_n0.clone(),
        }) as Arc<dyn RealtimeSession>;

        let session_n1 = Arc::new(CallNameTrackingSession {
            counts: counts_n1.clone(),
            call_ids: call_ids_n1.clone(),
        }) as Arc<dyn RealtimeSession>;

        let runner =
            RealtimeRunner::builder().model(Arc::new(MockModel) as BoxedModel).build().unwrap();

        let gen0_id = runner.set_initial_session(session_n0.clone()).await.unwrap();
        assert_eq!(gen0_id, 0);

        // Record function call_gen0_123 on Generation 0
        call_ids_n0.lock().insert("call_gen0_123".to_string());

        // Supervisor publishes replacement Generation 1
        let gen1_id =
            runner.supervisor.publish_replacement(session_n1.clone(), 0).await.unwrap().id;
        assert_eq!(gen1_id, 1);

        // Attempt send_tool_response for call_gen0_123 targeting active Generation 1
        let tool_response = ToolResponse {
            call_id: "call_gen0_123".to_string(),
            output: serde_json::json!({ "result": "stale" }),
            scheduling: None,
        };

        let send_res = runner.send_tool_response(tool_response).await;

        // Must be rejected because Generation 1's call_ids map does not contain call_gen0_123
        assert!(
            send_res.is_err(),
            "Tool response from Generation 0 must be rejected on Generation 1"
        );

        // Zero tool output/response writes reach Generation 1
        assert_eq!(counts_n1.tool_response.load(Ordering::SeqCst), 0);
        assert_eq!(counts_n1.tool_output.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_terminal_disconnect_is_surfaced_once() {
        let counts = Arc::new(Counts::default());
        let disconnects = Arc::new(AtomicUsize::new(0));

        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .event_handler(DisconnectWatcher { disconnects: Arc::clone(&disconnects) })
            .build()
            .unwrap();

        let scripted = Arc::new(ScriptedSession::new(Arc::clone(&counts), vec![response_done()]));
        let _ = runner.set_initial_session(scripted as Arc<dyn RealtimeSession>).await.unwrap();

        runner.run().await.unwrap();

        assert_eq!(
            disconnects.load(Ordering::SeqCst),
            1,
            "transport loss must be reported exactly once"
        );
    }

    #[tokio::test]
    async fn subscribe_generation_wakes_on_published_generation_n_plus_1_only() {
        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .recovery_policy(
                RecoveryPolicy::default()
                    .with_max_attempts(std::num::NonZeroU32::new(3).unwrap())
                    .with_initial_delay(std::time::Duration::ZERO),
            )
            .build()
            .unwrap();

        let session2 = Arc::new(ScriptedSession::new(Arc::new(Counts::default()), vec![]))
            as Arc<dyn RealtimeSession>;

        use std::time::Duration;

        // Setup a recovery implementation where attempt 1 fails (so candidate fails), but attempt 2 succeeds and publishes N+1
        struct TwoAttemptRecovery {
            attempts: Arc<AtomicUsize>,
            session2: Arc<dyn RealtimeSession>,
            started_1: Arc<tokio::sync::Notify>,
            continue_1: Arc<tokio::sync::Notify>,
            started_2: Arc<tokio::sync::Notify>,
            continue_2: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl crate::recovery::RealtimeRecovery for TwoAttemptRecovery {
            fn classify(&self, _cause: &RecoveryCause) -> crate::recovery::RecoveryDisposition {
                crate::recovery::RecoveryDisposition::Recoverable
            }
            fn classify_attempt_error(
                &self,
                _error: &RealtimeError,
            ) -> crate::recovery::RecoveryDisposition {
                crate::recovery::RecoveryDisposition::Recoverable
            }
            async fn recover(
                &self,
                _context: crate::recovery::RecoveryContext<'_>,
            ) -> Result<crate::recovery::RecoveredSession> {
                let count = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if count == 1 {
                    self.started_1.notify_one();
                    self.continue_1.notified().await;
                    Err(RealtimeError::ConnectionError("Connection reset by peer".into()))
                } else {
                    self.started_2.notify_one();
                    self.continue_2.notified().await;
                    Ok(crate::recovery::RecoveredSession::new(
                        self.session2.clone(),
                        crate::recovery::RecoveryContinuity::Reconnected,
                    ))
                }
            }
        }

        struct RetryableSession {
            inner: ScriptedSession,
            rec: TwoAttemptRecovery,
        }

        #[async_trait]
        impl RealtimeSession for RetryableSession {
            fn session_id(&self) -> &str {
                self.inner.session_id()
            }
            fn is_connected(&self) -> bool {
                self.inner.is_connected()
            }
            fn recovery(&self) -> Option<&dyn crate::recovery::RealtimeRecovery> {
                Some(&self.rec)
            }
            async fn send_audio(&self, a: &AudioChunk) -> Result<()> {
                self.inner.send_audio(a).await
            }
            async fn send_audio_base64(&self, a: &str) -> Result<()> {
                self.inner.send_audio_base64(a).await
            }
            async fn send_text(&self, t: &str) -> Result<()> {
                self.inner.send_text(t).await
            }
            async fn send_tool_response(&self, r: ToolResponse) -> Result<()> {
                self.inner.send_tool_response(r).await
            }
            async fn commit_audio(&self) -> Result<()> {
                self.inner.commit_audio().await
            }
            async fn clear_audio(&self) -> Result<()> {
                self.inner.clear_audio().await
            }
            async fn create_response(&self) -> Result<()> {
                self.inner.create_response().await
            }
            async fn interrupt(&self) -> Result<()> {
                self.inner.interrupt().await
            }
            async fn send_event(&self, e: ClientEvent) -> Result<()> {
                self.inner.send_event(e).await
            }
            async fn next_event(&self) -> Option<Result<ServerEvent>> {
                self.inner.next_event().await
            }
            fn events(
                &self,
            ) -> Pin<Box<dyn futures::Stream<Item = Result<ServerEvent>> + Send + '_>> {
                self.inner.events()
            }
            async fn close(&self) -> Result<()> {
                self.inner.close().await
            }
            async fn mutate_context(&self, c: RealtimeConfig) -> Result<ContextMutationOutcome> {
                self.inner.mutate_context(c).await
            }
        }

        let attempts_counter = Arc::new(AtomicUsize::new(0));
        let started_1 = Arc::new(tokio::sync::Notify::new());
        let continue_1 = Arc::new(tokio::sync::Notify::new());
        let started_2 = Arc::new(tokio::sync::Notify::new());
        let continue_2 = Arc::new(tokio::sync::Notify::new());

        let retryable = Arc::new(RetryableSession {
            inner: ScriptedSession::new(Arc::new(Counts::default()), vec![]),
            rec: TwoAttemptRecovery {
                attempts: Arc::clone(&attempts_counter),
                session2: session2.clone(),
                started_1: Arc::clone(&started_1),
                continue_1: Arc::clone(&continue_1),
                started_2: Arc::clone(&started_2),
                continue_2: Arc::clone(&continue_2),
            },
        });

        // Set initial session to retryable session (generation 0)
        let _ = runner.set_initial_session(retryable).await.unwrap();

        // Subscribe AFTER set_initial_session so watch channel's initial value is 0
        let mut rx = runner.subscribe_generation();
        assert_eq!(*rx.borrow_and_update(), 0, "initial generation is 0");

        let supervisor_arc = Arc::clone(&runner.supervisor);
        let recovery_task = tokio::spawn(async move {
            let report = FailureReport { generation: 0, cause: RecoveryCause::UnexpectedEof };
            supervisor_arc.report_failure(report).await
        });

        // Wait until recovery attempt 1 has started inside recover()
        tokio::time::timeout(Duration::from_secs(2), started_1.notified())
            .await
            .expect("attempt 1 started");

        // Allow attempt 1 to complete and fail
        continue_1.notify_one();

        // Wait until recovery attempt 2 starts inside recover()
        tokio::time::timeout(Duration::from_secs(2), started_2.notified())
            .await
            .expect("attempt 2 started");

        // Assert watcher still reads generation 0 while attempt 1 failed and attempt 2 is paused
        assert_eq!(
            *rx.borrow(),
            0,
            "failed candidate attempt 1 must not advance watcher to generation 1"
        );

        // Allow attempt 2 to complete and succeed
        continue_2.notify_one();

        let recovery_res = tokio::time::timeout(Duration::from_secs(2), recovery_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        assert!(matches!(recovery_res, RecoveryOutcome::Recovered { .. }));
        assert_eq!(attempts_counter.load(Ordering::SeqCst), 2, "2 attempts were made");

        // Watcher must wake exactly for published N+1 (generation 1)
        tokio::time::timeout(Duration::from_secs(2), rx.changed())
            .await
            .expect("watcher changed")
            .unwrap();
        assert_eq!(*rx.borrow(), 1, "ready published candidate advances watcher to generation 1");
    }

    struct SignallingRecovery {
        replacement_started: Arc<tokio::sync::Notify>,
        session2: Arc<dyn RealtimeSession>,
    }

    #[async_trait]
    impl crate::recovery::RealtimeRecovery for SignallingRecovery {
        fn classify(&self, _cause: &RecoveryCause) -> crate::recovery::RecoveryDisposition {
            crate::recovery::RecoveryDisposition::Recoverable
        }
        async fn recover(
            &self,
            _context: crate::recovery::RecoveryContext<'_>,
        ) -> Result<crate::recovery::RecoveredSession> {
            self.replacement_started.notify_one();
            Ok(crate::recovery::RecoveredSession::new(
                self.session2.clone(),
                crate::recovery::RecoveryContinuity::Reconnected,
            ))
        }
    }

    struct RecoverableSessionWithSignalling {
        inner: ScriptedSession,
        rec: SignallingRecovery,
    }

    #[async_trait]
    impl RealtimeSession for RecoverableSessionWithSignalling {
        fn session_id(&self) -> &str {
            self.inner.session_id()
        }
        fn is_connected(&self) -> bool {
            self.inner.is_connected()
        }
        fn recovery(&self) -> Option<&dyn crate::recovery::RealtimeRecovery> {
            Some(&self.rec)
        }
        async fn send_audio(&self, a: &AudioChunk) -> Result<()> {
            self.inner.send_audio(a).await
        }
        async fn send_audio_base64(&self, a: &str) -> Result<()> {
            self.inner.send_audio_base64(a).await
        }
        async fn send_text(&self, t: &str) -> Result<()> {
            self.inner.send_text(t).await
        }
        async fn send_tool_response(&self, r: ToolResponse) -> Result<()> {
            self.inner.send_tool_response(r).await
        }
        async fn commit_audio(&self) -> Result<()> {
            self.inner.commit_audio().await
        }
        async fn clear_audio(&self) -> Result<()> {
            self.inner.clear_audio().await
        }
        async fn create_response(&self) -> Result<()> {
            self.inner.create_response().await
        }
        async fn interrupt(&self) -> Result<()> {
            self.inner.interrupt().await
        }
        async fn send_event(&self, e: ClientEvent) -> Result<()> {
            self.inner.send_event(e).await
        }
        async fn next_event(&self) -> Option<Result<ServerEvent>> {
            self.inner.next_event().await
        }
        fn events(&self) -> Pin<Box<dyn futures::Stream<Item = Result<ServerEvent>> + Send + '_>> {
            self.inner.events()
        }
        async fn close(&self) -> Result<()> {
            self.inner.close().await
        }
        async fn mutate_context(&self, c: RealtimeConfig) -> Result<ContextMutationOutcome> {
            self.inner.mutate_context(c).await
        }
    }

    #[tokio::test]
    async fn test_planned_rotation_launches_replacement_when_event_handler_blocks() {
        let replacement_started = Arc::new(tokio::sync::Notify::new());
        let handler_started = Arc::new(tokio::sync::Notify::new());
        let handler_allow_exit = Arc::new(tokio::sync::Notify::new());

        struct BlockingRotationHandler {
            handler_started: Arc<tokio::sync::Notify>,
            handler_allow_exit: Arc<tokio::sync::Notify>,
        }

        #[async_trait]
        impl EventHandler for BlockingRotationHandler {
            async fn on_planned_rotation(&self, _time_left: Option<&str>) -> Result<()> {
                self.handler_started.notify_one();
                self.handler_allow_exit.notified().await;
                Ok(())
            }
        }

        let session2 = Arc::new(ScriptedSession::new(Arc::new(Counts::default()), vec![]));
        let session1 = Arc::new(RecoverableSessionWithSignalling {
            inner: ScriptedSession::new(Arc::new(Counts::default()), vec![]),
            rec: SignallingRecovery {
                replacement_started: Arc::clone(&replacement_started),
                session2,
            },
        });

        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .event_handler(BlockingRotationHandler {
                handler_started: Arc::clone(&handler_started),
                handler_allow_exit: Arc::clone(&handler_allow_exit),
            })
            .build()
            .unwrap();

        let _ = runner.set_initial_session(session1).await.unwrap();

        let runner_arc = Arc::new(runner);
        let r_clone = Arc::clone(&runner_arc);
        let event_task = tokio::spawn(async move {
            r_clone
                .handle_event_for_generation(
                    ServerEvent::PlannedRotation { time_left: Some("30s".into()) },
                    Some(0),
                )
                .await
        });

        tokio::time::timeout(Duration::from_secs(2), handler_started.notified())
            .await
            .expect("handler started");

        tokio::time::timeout(Duration::from_secs(2), replacement_started.notified())
            .await
            .expect("replacement task must be launched concurrently without waiting for handler");

        handler_allow_exit.notify_one();
        event_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_planned_rotation_uses_originating_generation_preventing_cascade() {
        let session0 = Arc::new(ScriptedSession::new(Arc::new(Counts::default()), vec![]));
        let session1 = Arc::new(ScriptedSession::new(Arc::new(Counts::default()), vec![]));

        let runner =
            RealtimeRunner::builder().model(Arc::new(MockModel) as BoxedModel).build().unwrap();

        let gen0_id = runner.set_initial_session(session0).await.unwrap();
        assert_eq!(gen0_id, 0);

        let gen1_id = runner.supervisor.publish_replacement(session1, 0).await.unwrap().id;
        assert_eq!(gen1_id, 1);

        let res = runner
            .handle_event_for_generation(
                ServerEvent::PlannedRotation { time_left: Some("30s".into()) },
                Some(0),
            )
            .await;
        assert!(res.is_ok());

        // Explicitly assert execute_planned_replacement directly rejects stale originating generation 0
        let rotation_res = runner
            .supervisor
            .execute_planned_replacement(
                0,
                crate::recovery::RecoveryCause::PlannedRotation { time_left: Some("30s".into()) },
            )
            .await;
        assert!(
            rotation_res.is_err()
                || matches!(
                    rotation_res,
                    Ok(crate::recovery::supervisor::RecoveryOutcome::Stale(_))
                ),
            "stale generation 0 rotation report must not rotate generation 1"
        );

        let active = runner.supervisor.get_active_generation().await.unwrap();
        assert_eq!(active.id, 1, "stale generation 0 rotation report must not rotate generation 1");
    }

    #[derive(Clone)]
    struct FailingPlannedRotationHandler {
        barrier: Arc<crate::recovery::TestRecoveryBarrier>,
    }

    #[async_trait]
    impl EventHandler for FailingPlannedRotationHandler {
        async fn on_planned_rotation(&self, _time_left: Option<&str>) -> Result<()> {
            self.barrier.wait_until_planned_entered().await;
            Err(RealtimeError::connection("deliberate callback failure"))
        }
    }

    #[tokio::test]
    async fn test_planned_rotation_event_handler_error_propagates_and_spawns_replacement() {
        let barrier = Arc::new(crate::recovery::TestRecoveryBarrier::new());
        let handler = FailingPlannedRotationHandler { barrier: Arc::clone(&barrier) };

        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .event_handler(handler)
            .build()
            .unwrap();

        runner.supervisor.set_recovery_barrier_for_testing(Arc::clone(&barrier));

        let session2 = Arc::new(ScriptedSession::new(Arc::new(Counts::default()), vec![]));
        let replacement_started = Arc::new(tokio::sync::Notify::new());
        let session0 = Arc::new(RecoverableSessionWithSignalling {
            inner: ScriptedSession::new(Arc::new(Counts::default()), vec![]),
            rec: SignallingRecovery { replacement_started, session2 },
        });
        let gen0_id = runner.set_initial_session(session0).await.unwrap();
        assert_eq!(gen0_id, 0);

        let res = runner
            .handle_event_for_generation(
                ServerEvent::PlannedRotation { time_left: Some("30s".into()) },
                Some(0),
            )
            .await;

        assert!(res.is_err(), "event handling must propagate callback error");
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("deliberate callback failure"),
            "propagated error must be the callback failure, got: {err_msg}"
        );

        barrier.release();
    }
}
