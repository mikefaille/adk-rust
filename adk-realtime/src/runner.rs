//! RealtimeRunner for integrating realtime sessions with agents.
//!
//! This module provides the bridge between realtime audio sessions and
//! the ADK agent framework, handling tool execution and event routing.

use crate::config::{RealtimeConfig, SessionUpdateConfig, ToolDefinition};
use crate::error::{RealtimeError, Result};
use crate::events::{CallerActivitySource, ServerEvent, ToolCall, ToolResponse};
use crate::model::BoxedModel;
use crate::session::ContextMutationOutcome;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::RwLock;

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
    /// **Provider Context:** This state is only utilized by providers that do *not* support
    /// native mid-flight mutability (e.g., Gemini Live), requiring a physical transport teardown
    /// and rebuild (Phantom Reconnect). Providers like OpenAI natively apply `session.update`
    /// frames instantly and will never enter this queued state.
    ///
    /// **Queue Policy:** The runner keeps only one pending resumption. If a new session update
    /// arrives while a resumption is already pending, the previous pending resumption is replaced.
    /// This is intentional: pending session updates represent desired end state, not an ordered
    /// command queue. The policy is last write wins.
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
///
/// The single place this question is answered. Adding a provider event that
/// indicates caller activity means adding it here, and every consumer of the
/// runner picks it up at once.
pub fn caller_activity_source(event: &ServerEvent) -> Option<CallerActivitySource> {
    match event {
        ServerEvent::InputTranscriptDelta { .. } => Some(CallerActivitySource::TranscriptDelta),
        ServerEvent::InputTranscriptCompleted { .. } => {
            Some(CallerActivitySource::TranscriptCompleted)
        }
        ServerEvent::SpeechStopped { .. } => Some(CallerActivitySource::SpeechStopped),
        // `SpeechStarted` is deliberately absent: the caller starting to speak
        // is a barge-in signal, not evidence that they said anything an
        // application should act on.
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
    ///
    /// Consumers that only need a turn boundary should use `item_id` and avoid
    /// retaining `transcript`; the callback includes both so applications that
    /// explicitly own transcript persistence do not need a parallel event loop.
    ///
    /// Not every provider emits this. Gemini Live does not, so a handler that
    /// treats it as its only caller-activity signal observes nothing at all on
    /// that backend — use [`EventHandler::on_caller_activity`] instead.
    async fn on_input_transcript_completed(&self, _transcript: &str, _item_id: &str) -> Result<()> {
        Ok(())
    }

    /// Called whenever a server event indicates the caller was active.
    ///
    /// This exists because "the caller did something" was previously only
    /// derivable by picking the right subset of events yourself, and two
    /// consumers of the same runner picked different subsets: one observed
    /// `InputTranscriptDelta` directly off the event stream and worked on
    /// Gemini, the other used `on_input_transcript_completed` and — since
    /// Gemini never emits it — silently observed nothing. Both were reading the
    /// same session. Deriving activity in the runner, once, is what stops that
    /// class of divergence: a provider event added to the mapping reaches every
    /// consumer at the same time.
    ///
    /// Fires *in addition to* the specific callback for the same event, so a
    /// handler implementing both sees both. Ordering between the two is not
    /// part of the contract.
    ///
    /// See [`CallerActivitySource`] for what each source does and does not
    /// establish; they are not interchangeable strengths of evidence.
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

    /// Called when the provider withdraws function calls it already issued,
    /// normally because the caller interrupted the turn that produced them.
    ///
    /// The ids match `call_id` values from earlier tool calls. A handler that
    /// has not yet performed the effect should drop it; one that already has
    /// must decide whether to compensate, because the provider considers the
    /// call to have never been authorized.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// async fn on_tool_calls_cancelled(&self, call_ids: &[ToolCallId]) -> Result<()> {
    ///     for id in call_ids {
    ///         if !self.pending.lock().remove(id.as_str()) {
    ///             tracing::warn!(call_id = %id, "cancelled after the effect landed");
    ///         }
    ///     }
    ///     Ok(())
    /// }
    /// ```
    async fn on_tool_calls_cancelled(&self, _call_ids: &[crate::events::ToolCallId]) -> Result<()> {
        Ok(())
    }

    /// Called when a response is cancelled or interrupted before completion
    /// (e.g. caller barge-in). Distinct from [`EventHandler::on_error`]:
    /// cancellation is a normal lifecycle boundary, not a failure, but like
    /// the done/error boundaries it invalidates any partially received item
    /// state a handler may be carrying.
    async fn on_response_cancelled(&self) -> Result<()> {
        Ok(())
    }

    /// Called on any error.
    /// Called when the provider transport ends and [`RealtimeRunner::run`] is returning.
    ///
    /// The runner does **not** reconnect automatically. Reconnection is deliberate: it
    /// requires deciding what context to replay and, on Gemini, whether a resumption token
    /// is still valid. Without this hook `run` returned `Ok(())` on transport loss, which a
    /// caller could not tell apart from a graceful [`RealtimeRunner::close`].
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// async fn on_disconnect(&self) -> adk_realtime::Result<()> {
    ///     tracing::warn!("realtime transport ended; reconnecting");
    ///     self.reconnect.notify_one();
    ///     Ok(())
    /// }
    /// ```
    async fn on_disconnect(&self) -> Result<()> {
        Ok(())
    }

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
    tools: HashMap<String, (ToolDefinition, Arc<dyn ToolHandler>)>,
    event_handler: Option<Arc<dyn EventHandler>>,
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
            tools: HashMap::new(),
            event_handler: None,
        }
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
    pub fn tool(mut self, definition: ToolDefinition, handler: impl ToolHandler + 'static) -> Self {
        let name = definition.name.clone();
        self.tools.insert(name, (definition, Arc::new(handler)));
        self
    }

    /// Register a tool with a sync function handler.
    pub fn tool_fn<F>(self, definition: ToolDefinition, handler: F) -> Self
    where
        F: Fn(&ToolCall) -> Result<serde_json::Value> + Send + Sync + 'static,
    {
        self.tool(definition, FnToolHandler::new(handler))
    }

    /// Register a tool with an `Arc<dyn ToolHandler>` directly.
    ///
    /// This is useful when you already have a shared handler and want to avoid
    /// an extra `Arc` wrapping that the `tool()` method would perform.
    pub fn tool_arc(mut self, definition: ToolDefinition, handler: Arc<dyn ToolHandler>) -> Self {
        let name = definition.name.clone();
        self.tools.insert(name, (definition, handler));
        self
    }

    /// Set the event handler.
    pub fn event_handler(mut self, handler: impl EventHandler + 'static) -> Self {
        self.event_handler = Some(Arc::new(handler));
        self
    }

    /// Set the event handler from an `Arc<dyn EventHandler>` directly.
    ///
    /// This is useful when you already have a shared handler and want to avoid
    /// an extra `Arc` wrapping that the `event_handler()` method would perform.
    pub fn event_handler_arc(mut self, handler: Arc<dyn EventHandler>) -> Self {
        self.event_handler = Some(handler);
        self
    }

    /// Build the runner (does not connect yet).
    pub fn build(self) -> Result<RealtimeRunner> {
        let model = self.model.ok_or_else(|| RealtimeError::config("Model is required"))?;

        // Add tool definitions to config
        let mut config = self.config;
        if !self.tools.is_empty() {
            let tool_defs: Vec<ToolDefinition> =
                self.tools.values().map(|(def, _)| def.clone()).collect();
            config.tools = Some(tool_defs);
        }

        let max_concurrent_tools = self.runner_config.max_concurrent_tools.max(1);

        Ok(RealtimeRunner {
            model,
            config: Arc::new(RwLock::new(config)),
            runner_config: self.runner_config,
            tools: self.tools,
            event_handler: self.event_handler.unwrap_or_else(|| Arc::new(NoOpEventHandler)),
            session: Arc::new(RwLock::new(None)),
            state: Arc::new(RwLock::new(RunnerState::Idle)),
            pending_tool_response: AtomicBool::new(false),
            tool_permits: Arc::new(tokio::sync::Semaphore::new(max_concurrent_tools)),
            outstanding_tools: Arc::new(AtomicUsize::new(0)),
            response_closed_awaiting_tools: Arc::new(AtomicBool::new(false)),
            consecutive_tool_failures: Arc::new(AtomicUsize::new(0)),
            circuit_breaker_tripped: Arc::new(AtomicBool::new(false)),
        })
    }
}

/// A runner that manages a realtime session with tool execution.
///
/// RealtimeRunner provides a high-level interface for:
/// - Connecting to realtime providers
/// - Automatically executing tool calls
/// - Routing events to handlers
/// - Managing the session lifecycle
///
/// # Example
///
/// ```rust,ignore
/// use adk_realtime::{RealtimeRunner, RealtimeConfig, ToolDefinition};
/// use adk_realtime::openai::OpenAIRealtimeModel;
///
/// #[tokio::main]
/// async fn main() -> Result<()> {
///     let model = OpenAIRealtimeModel::new(api_key, "gpt-realtime");
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
    config: Arc<RwLock<RealtimeConfig>>,
    runner_config: RunnerConfig,
    tools: HashMap<String, (ToolDefinition, Arc<dyn ToolHandler>)>,
    event_handler: Arc<dyn EventHandler>,
    session: Arc<RwLock<Option<Arc<dyn crate::session::RealtimeSession>>>>,
    state: Arc<RwLock<RunnerState>>,
    /// Set when tool output(s) have been sent for the in-flight response and a
    /// single follow-up `create_response` is owed once that response finishes.
    pending_tool_response: AtomicBool,
    /// Bounds how many tool handlers run at once, from
    /// [`RunnerConfig::max_concurrent_tools`].
    tool_permits: Arc<tokio::sync::Semaphore>,
    /// Tool calls dispatched for the current response and not yet finished.
    outstanding_tools: Arc<AtomicUsize>,
    /// Set when the dispatching response closed while tool calls were still running, so
    /// the follow-up `create_response` is owed by whichever tool finishes last.
    response_closed_awaiting_tools: Arc<AtomicBool>,
    /// Tracks consecutive tool execution failures to enforce circuit breaking.
    consecutive_tool_failures: Arc<AtomicUsize>,
    /// Set when the circuit breaker trips, putting tool execution into Open state.
    circuit_breaker_tripped: Arc<AtomicBool>,
}

impl RealtimeRunner {
    /// Helper to safely acquire a cloned Arc of the current session, dropping the lock.
    async fn session_handle(&self) -> Result<Arc<dyn crate::session::RealtimeSession>> {
        let guard = self.session.read().await;
        guard.as_ref().cloned().ok_or_else(|| RealtimeError::connection("Not connected"))
    }

    /// Create a new builder.
    pub fn builder() -> RealtimeRunnerBuilder {
        RealtimeRunnerBuilder::new()
    }

    /// Connect to the realtime provider.
    pub async fn connect(&self) -> Result<()> {
        self.consecutive_tool_failures.store(0, Ordering::Release);
        self.circuit_breaker_tripped.store(false, Ordering::Release);
        let config = self.config.read().await.clone();
        let session = self.model.connect(config).await?;
        let mut guard = self.session.write().await;
        *guard = Some(session.into());
        Ok(())
    }

    /// Check if currently connected.
    pub async fn is_connected(&self) -> bool {
        let guard = self.session.read().await;
        guard.as_ref().map(|s| s.is_connected()).unwrap_or(false)
    }

    /// Get the session ID if connected.
    pub async fn session_id(&self) -> Option<String> {
        let guard = self.session.read().await;
        guard.as_ref().map(|s| s.session_id().to_string())
    }

    /// The connected provider's native audio output format (see
    /// [`RealtimeSession::native_audio_output_format`](crate::session::RealtimeSession::native_audio_output_format)).
    /// Returns an error if not connected — callers that need this are about to
    /// label live `AudioDelta` bytes and must not fall back to a guessed format.
    pub async fn native_audio_output_format(&self) -> Result<crate::audio::AudioFormat> {
        let guard = self.session.read().await;
        guard
            .as_ref()
            .map(|s| s.native_audio_output_format())
            .ok_or_else(|| RealtimeError::connection("Not connected"))
    }

    /// Send a client event directly to the session.
    ///
    /// This method intercepts internal control-plane events (like `UpdateSession`) to route
    /// them through the provider-agnostic orchestration layer instead of forwarding raw JSON
    /// to the underlying WebSocket transport. This guarantees that `adk-realtime` never leaks
    /// invalid event payloads to providers (e.g., OpenAI or Gemini) and universally bridges
    /// the Cognitive Handoff mechanics transparently.
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
            other => {
                let session = self.session_handle().await?;
                session.send_event(other).await
            }
        }
    }

    /// Internal helper to merge a `SessionUpdateConfig` delta into the canonical `RealtimeConfig` state.
    ///
    /// **Why this exists**: The `RealtimeRunner` must maintain an absolute, single source of truth
    /// for its configuration (`self.config`). Orchestrators fire `SessionUpdateConfig`s as sparse
    /// partial deltas (intents to hot-swap instructions or tools mid-flight). By accumulating
    /// these sparse updates into the single `base` config, any subsequent "Phantom Reconnect"
    /// (e.g., due to a Gemini domain shift or an unexpected network drop) natively inherits all
    /// prior hot-swaps alongside the immutable transport parameters (like sample rates) defined at startup.
    ///
    /// Note: This is intentionally narrow and specifically scoped to merge only
    /// hot-swappable cognitive fields (instruction, tools, voice, temperature, extra).
    /// Transport-level attributes like sample rates and audio formats are not dynamically swappable.
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
    }

    /// Update the session configuration.
    ///
    /// Delegates to [`Self::update_session_with_bridge`] with no bridge message.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use adk_realtime::config::{SessionUpdateConfig, RealtimeConfig};
    ///
    /// async fn example(runner: &adk_realtime::RealtimeRunner) {
    ///     let update = SessionUpdateConfig(
    ///         RealtimeConfig::default().with_instruction("You are now a pirate.")
    ///     );
    ///     runner.update_session(update).await.unwrap();
    /// }
    /// ```
    pub async fn update_session(&self, config: SessionUpdateConfig) -> Result<()> {
        self.update_session_with_bridge(config, None).await
    }

    /// Update the session configuration, optionally injecting a bridge message if
    /// a transport resumption (Phantom Reconnect) occurs.
    ///
    /// The RealtimeRunner will attempt to mutate the session natively if the underlying
    /// API supports it (e.g., OpenAI). If it does not (e.g., Gemini), the Runner will
    /// queue a transport resumption, executing it only when the session
    /// is in a resumable state (Idle) to prevent data corruption.
    ///
    /// The runner keeps only one pending resumption. If a new session update arrives while
    /// a resumption is already pending, the previous pending resumption is replaced. This is
    /// intentional: pending session updates represent desired end state, not an ordered command queue.
    /// The policy is last write wins.
    pub async fn update_session_with_bridge(
        &self,
        config: SessionUpdateConfig,
        bridge_message: Option<String>,
    ) -> Result<()> {
        // 1. Merge the incoming delta into the runner's canonical, persisted configuration.
        // This ensures that any future reconnects (e.g., due to network drops) naturally
        // inherit this latest state.
        let mut full_config = self.config.write().await;
        Self::merge_config(&mut full_config, &config);

        let cloned_config = full_config.clone();
        drop(full_config); // Free the write lock early to avoid deadlocks.

        // 2. Safely obtain a cloned handle of the active session.
        let session = self.session_handle().await?;

        // 3. Delegate the mutation attempt to the provider-specific adapter.
        match session.mutate_context(cloned_config).await? {
            // PATH A: Native Mutability (e.g., OpenAI)
            // The provider natively updated the context over the active WebSocket.
            ContextMutationOutcome::Applied => {
                tracing::info!("Context mutated natively mid-flight.");

                // Since the transport wasn't dropped, we can inject the bridge message
                // immediately as a standard user message to update the model's short-term memory.
                if let Some(msg) = bridge_message {
                    let event = crate::events::ClientEvent::Message {
                        role: "user".to_string(),
                        parts: vec![adk_core::types::Part::Text { text: msg }],
                    };
                    session.send_event(event).await?;
                }
                Ok(())
            }

            // PATH B: Rigid Initialization (e.g., Gemini)
            // The provider requires us to tear down the WebSocket and establish a new one (Phantom Reconnect).
            ContextMutationOutcome::RequiresResumption(new_config) => {
                drop(session); // CRITICAL: Drop the cloned handle before attempting state mutation.

                // 4. Check the Runner's internal state machine to ensure it is safe to tear down the socket.
                let mut state_guard = self.state.write().await;

                if *state_guard == RunnerState::Idle {
                    // Safe to reconnect: The model is neither generating audio nor executing a tool.
                    drop(state_guard); // Free state lock before the heavy async network operation.
                    tracing::info!("Runner is idle. Executing resumption immediately.");

                    if let Err(e) =
                        self.execute_resumption((*new_config).clone(), bridge_message.clone()).await
                    {
                        tracing::error!("Immediate resumption failed: {}. Queueing for retry.", e);
                        // If the reconnect fails (e.g., transient network issue), we must not lose the mutation intent.
                        // We push it back into the queue for the background loop to retry.
                        let mut fallback_state = self.state.write().await;
                        *fallback_state = RunnerState::PendingResumption {
                            config: Box::new(*new_config),
                            bridge_message,
                            attempts: 1,
                        };
                        return Err(e);
                    }
                } else {
                    // Unsafe to reconnect: Tearing down the socket now would corrupt the in-flight context.
                    // We must queue the mutation. The event loop will execute it once it returns to Idle.
                    if let RunnerState::PendingResumption { .. } = *state_guard {
                        tracing::warn!(
                            "Runner already had a pending resumption. Overwriting with last-write-wins policy."
                        );
                    } else {
                        tracing::info!("Runner is busy ({:?}). Queueing resumption.", *state_guard);
                    }

                    // Queue the intent using a last-write-wins policy.
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

    /// Internal helper to execute a transport resumption (teardown and rebuild).
    async fn execute_resumption(
        &self,
        new_config: crate::config::RealtimeConfig,
        bridge_message: Option<String>,
    ) -> Result<()> {
        tracing::warn!("Executing transport resumption with new configuration.");

        // 1. Extract the old session safely under the write lock.
        let old_session = {
            let mut write_guard = self.session.write().await;
            write_guard.take()
        };

        // 2. Explicitly tear down the old WebSocket connection to release upstream resources.
        // Do this WITHOUT holding the lock across `.await`.
        if let Some(session) = old_session
            && let Err(e) = session.close().await
        {
            tracing::warn!("Failed to cleanly close old session during resumption: {}", e);
        }

        // 3. Establish a brand new connection using the provider-agnostic factory interface.
        // If the provider supports resumption natively (like Gemini), the `new_config`
        // payload already contains the cached `resumeToken`.
        let new_session = self.model.connect(new_config).await?;

        // 4. Overwrite the active session pointer with the newly connected transport.
        {
            let mut write_guard = self.session.write().await;
            *write_guard = Some(new_session.into());
        }

        // 5. If the orchestrator provided a bridge message (e.g. to explain the domain shift),
        // safely inject it into the new connection's context window.
        if let Some(msg) = bridge_message {
            self.inject_bridge_message(msg).await?;
        }

        tracing::info!("Resumption complete. New transport established.");
        Ok(())
    }

    /// Internal helper to safely inject a bridge message directly into the active session.
    ///
    /// This intentionally bypasses the `send_client_event` router to avoid `E0733`
    /// (un-Boxed async recursion) where `send_client_event` -> `update_session` ->
    /// `execute_resumption` -> `send_client_event` creates an infinite compiler loop.
    async fn inject_bridge_message(&self, msg: String) -> Result<()> {
        tracing::info!("Injecting bridge message post-resumption.");
        let event = crate::events::ClientEvent::Message {
            role: "user".to_string(),
            parts: vec![adk_core::types::Part::Text { text: msg }],
        };
        let session = self.session_handle().await?;
        session.send_event(event).await
    }

    /// Send a typed raw-audio chunk to the session.
    ///
    /// This preserves the audio format at the provider boundary and lets the
    /// provider choose its native encoding path. Prefer this method when the
    /// caller already owns raw audio bytes.
    pub async fn send_audio_chunk(&self, audio: &crate::audio::AudioChunk) -> Result<()> {
        let session = self.session_handle().await?;
        session.send_audio(audio).await
    }

    /// Send base64-encoded audio to the session.
    ///
    /// This compatibility entry point is useful when the caller already has a
    /// base64 payload. Raw-audio callers should use
    /// [`send_audio_chunk`](Self::send_audio_chunk) to avoid forcing an encoding
    /// decision at the provider-neutral runner boundary. This method forwards
    /// to [`send_audio_base64`](Self::send_audio_base64).
    pub async fn send_audio(&self, audio_base64: &str) -> Result<()> {
        self.send_audio_base64(audio_base64).await
    }

    /// Send base64-encoded audio to the session.
    pub async fn send_audio_base64(&self, audio_base64: &str) -> Result<()> {
        let session = self.session_handle().await?;
        session.send_audio_base64(audio_base64).await
    }

    /// Send text to the session.
    pub async fn send_text(&self, text: &str) -> Result<()> {
        let session = self.session_handle().await?;
        session.send_text(text).await
    }

    /// Send a base64-encoded video/image frame (e.g. `image/jpeg`) for
    /// multimodal input, where the provider supports it (Gemini Live; OpenAI as
    /// an image-in-context item).
    pub async fn send_video_frame(&self, mime_type: &str, data_base64: &str) -> Result<()> {
        let session = self.session_handle().await?;
        session.send_video_frame(mime_type, data_base64).await
    }

    /// Commit the audio buffer (for manual VAD mode).
    pub async fn commit_audio(&self) -> Result<()> {
        let session = self.session_handle().await?;
        session.commit_audio().await
    }

    /// Trigger a response from the model.
    pub async fn create_response(&self) -> Result<()> {
        let session = self.session_handle().await?;
        session.create_response().await
    }

    /// Interrupt the current response.
    pub async fn interrupt(&self) -> Result<()> {
        let session = self.session_handle().await?;
        session.interrupt().await
    }

    /// Get the next raw event from the session.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use adk_realtime::events::ServerEvent;
    /// use tracing::{info, error};
    ///
    /// async fn process_events(runner: &adk_realtime::RealtimeRunner) {
    ///     while let Some(event) = runner.next_event().await {
    ///         match event {
    ///             Ok(ServerEvent::SpeechStarted { .. }) => info!("User is speaking"),
    ///             Ok(_) => info!("Received other event"),
    ///             Err(e) => error!("Error: {e}"),
    ///         }
    ///     }
    /// }
    /// ```
    /// Why the provider ended the stream, once [`Self::next_event`] has returned
    /// `None`.
    ///
    /// Callers that poll `next_event` never see the runner's `on_disconnect`
    /// dispatch, so without this a provider that deliberately closed an idle
    /// session is indistinguishable from a dropped socket — and both get
    /// recorded as the same generic stream failure.
    pub async fn disconnect_reason(&self) -> Option<crate::session::DisconnectReason> {
        self.session_handle().await.ok().and_then(|session| session.disconnect_reason())
    }

    pub async fn next_event(&self) -> Option<Result<ServerEvent>> {
        let session = match self.session_handle().await {
            Ok(session) => session,
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                return None;
            }
        };

        // Some sessions might yield inside next_event, but just in case, yield here too
        tokio::task::yield_now().await;
        session.next_event().await
    }

    /// Send a tool response to the session.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use adk_realtime::events::ToolResponse;
    /// use serde_json::json;
    ///
    /// async fn example(runner: &adk_realtime::RealtimeRunner) {
    ///     let response = ToolResponse {
    ///         call_id: "call_123".to_string(),
    ///         output: json!({"temperature": 72}),
    ///     };
    ///     runner.send_tool_response(response).await.unwrap();
    /// }
    /// ```
    pub async fn send_tool_response(&self, response: ToolResponse) -> Result<()> {
        let session = self.session_handle().await?;
        session.send_tool_response(response).await
    }

    /// Execute a tool call against the registered handlers, sending the result
    /// back to the model when `auto_respond_tools` is enabled.
    ///
    /// This is the same dispatch the [`run`](Self::run) loop performs for a
    /// `response.function_call_arguments.done` event, exposed so that callers
    /// driving the session manually via [`next_event`](Self::next_event) — such
    /// as `IntegratedRealtimeRunner` (available with the `integration` feature) —
    /// can execute tools without re-implementing the lookup/respond logic.
    pub async fn dispatch_tool_call(
        &self,
        call_id: &str,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<()> {
        self.execute_tool_call(call_id, name, arguments).await
    }

    /// Sends a tool result the caller produced, honouring `auto_respond_tools`.
    ///
    /// Used by the integration layer, which runs ADK tools through its own policy pipeline and
    /// then needs the result delivered exactly as `execute_tool_call` would deliver it: the
    /// output is sent now, and the single follow-up `create_response` is deferred until the
    /// dispatching response closes, so several parallel calls produce one response.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let result = my_policy_pipeline.run(&call).await?;
    /// runner.send_tool_result(&call.call_id, result).await?;
    /// ```
    pub async fn send_tool_result(&self, call_id: &str, output: serde_json::Value) -> Result<()> {
        if !self.runner_config.auto_respond_tools {
            return Ok(());
        }
        if let Ok(session) = self.session_handle().await {
            session.send_tool_output(ToolResponse { call_id: call_id.to_string(), output }).await?;
            self.pending_tool_response.store(true, Ordering::Release);
        }
        Ok(())
    }

    /// Run the event loop, processing events until disconnected or until `cancel_token` is cancelled.
    pub async fn run_with_cancellation(
        &self,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        self.run_loop(Some(cancel_token)).await
    }

    /// Run the event loop, processing events until disconnected.
    pub async fn run(&self) -> Result<()> {
        self.run_loop(None).await
    }

    /// Internal unified event loop supporting optional cancellation and clean session closure.
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
                self.close_active_session().await;
                self.event_handler.on_disconnect().await?;
                break;
            }

            let session = self.session_handle().await?;
            let old_session_id = session.session_id().to_string();

            let event = if running_tools.is_empty() {
                if let Some(token) = &cancel_token {
                    tokio::select! {
                        _ = token.cancelled() => {
                            tracing::info!("RealtimeRunner run loop cancelled via cancellation token.");
                            self.close_active_session().await;
                            self.event_handler.on_disconnect().await?;
                            return Ok(());
                        }
                        ev = session.next_event() => ev,
                    }
                } else {
                    session.next_event().await
                }
            } else if let Some(token) = &cancel_token {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        tracing::info!("RealtimeRunner run loop cancelled via cancellation token.");
                        self.close_active_session().await;
                        self.event_handler.on_disconnect().await?;
                        return Ok(());
                    }
                    Some(finished) = running_tools.next() => {
                        let () = finished?;
                        continue;
                    }
                    event = session.next_event() => event,
                }
            } else {
                tokio::select! {
                    biased;
                    Some(finished) = running_tools.next() => {
                        let () = finished?;
                        continue;
                    }
                    event = session.next_event() => event,
                }
            };

            match event {
                Some(Ok(event)) => {
                    if let Some(call) = self.handle_event(event).await? {
                        running_tools.push(self.run_tool_call(call));
                    }
                }
                Some(Err(e)) => {
                    self.event_handler.on_error(&e).await?;
                    return Err(e);
                }
                None => {
                    let current_session_id = self.session_id().await;
                    if let Some(id) = current_session_id
                        && id != old_session_id
                    {
                        continue;
                    }
                    while let Some(finished) = running_tools.next().await {
                        finished?;
                    }
                    self.event_handler.on_disconnect().await?;
                    break;
                }
            }
        }
        Ok(())
    }

    /// Internal helper to safely close and release active session on cancellation or shutdown.
    async fn close_active_session(&self) {
        let old_session = {
            let mut guard = self.session.write().await;
            guard.take()
        };
        if let Some(session) = old_session {
            let _ = session.close().await;
        }
    }

    /// Run one dispatched tool call under the configured concurrency bound.
    ///
    /// The permit is acquired inside the future so queueing a call never blocks the event
    /// loop; the bound applies to execution, not to admission.
    async fn run_tool_call(&self, call: PendingToolCall) -> Result<()> {
        struct ToolCounterGuard<'a>(&'a RealtimeRunner);
        impl<'a> Drop for ToolCounterGuard<'a> {
            fn drop(&mut self) {
                self.0.outstanding_tools.fetch_sub(1, Ordering::AcqRel);
            }
        }

        self.outstanding_tools.fetch_add(1, Ordering::AcqRel);
        let guard = ToolCounterGuard(self);

        let permit = Arc::clone(&self.tool_permits).acquire_owned().await;
        let result = self.execute_tool_call(&call.call_id, &call.name, &call.arguments).await;
        drop(permit);

        // Explicitly drop the guard here so outstanding_tools is decremented synchronously
        drop(guard);

        // Now check if this was the last tool to finish and a response is owed
        if self.outstanding_tools.load(Ordering::Acquire) == 0
            && self.response_closed_awaiting_tools.swap(false, Ordering::AcqRel)
        {
            self.respond_after_tools().await?;
        }

        result
    }

    /// Process a single event.
    async fn handle_event(&self, event: ServerEvent) -> Result<Option<PendingToolCall>> {
        // Track state transitions before forwarding the event
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

        // Derived ahead of the match below, and from the event rather than from
        // inside an arm, so that caller-activity coverage cannot drift with
        // whichever arms happen to exist. The match below ends in `_ => {}`, so
        // a missing arm is silent — which is exactly how `InputTranscriptDelta`
        // came to reach one consumer of this runner and not another.
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
                self.event_handler.on_tool_calls_cancelled(&call_ids).await?;
            }
            ServerEvent::ResponseCancelled { .. } => {
                // Both atomic flags must be reset on cancellation (caller barge-in):
                // 1) `pending_tool_response`: Prevents completed tool outputs from firing create_response in a future turn.
                // 2) `response_closed_awaiting_tools`: Prevents late-finishing tools from triggering a follow-up response mid-utterance.
                self.pending_tool_response.store(false, Ordering::Release);
                self.response_closed_awaiting_tools.store(false, Ordering::Release);
                // An abandoned generation must not leave the state machine
                // mid-turn: `Generating` blocks queued resumptions and teardown,
                // and only `ResponseDone` clears it — which a cancelled turn
                // never reaches.
                //
                // Latent rather than live today: this event is produced only by
                // the Gemini translator, and Gemini never emits
                // `ResponseCreated`, so that path's state never leaves `Idle`.
                // It becomes reachable the moment another provider emits a
                // cancellation, or Gemini starts reporting response starts.
                {
                    let mut state = self.state.write().await;
                    if let RunnerState::Generating = *state {
                        *state = RunnerState::Idle;
                    }
                }
                // Until now nothing constructed this event, so
                // `on_response_cancelled` was an orphan: declared on the trait,
                // implemented by consumers, never called. Barge-in therefore
                // never reached an audio sink on the Gemini backend.
                self.event_handler.on_response_cancelled().await?;
            }
            ServerEvent::ResponseDone { .. } => {
                self.event_handler.on_response_done().await?;
                // If this response dispatched tool call(s), send the one owed
                // follow-up response now that it's closed.
                self.respond_after_tools().await?;
                self.check_resumption_queue().await?;
            }
            ServerEvent::FunctionCallDone { call_id, name, arguments, .. }
                if self.runner_config.auto_execute_tools =>
            {
                // Returned rather than awaited: the run loop dispatches it so event
                // intake — audio deltas included — continues while the tool runs.
                return Ok(Some(PendingToolCall { call_id, name, arguments }));
            }
            ServerEvent::SessionUpdated { session, .. } => {
                // Check if the generic session update contains a resumption token
                if let Some(token) = session.get("resumeToken").and_then(|t| t.as_str()) {
                    tracing::info!(
                        "Received Gemini sessionResumption token, saving for future reconnects."
                    );
                    let mut config = self.config.write().await;
                    let mut extra = config.extra.clone().unwrap_or_else(|| serde_json::json!({}));
                    extra["resumeToken"] = serde_json::Value::String(token.to_string());
                    config.extra = Some(extra);
                }
            }
            ServerEvent::Error { error, .. } => {
                let err = RealtimeError::server(error.code.unwrap_or_default(), error.message);
                self.event_handler.on_error(&err).await?;
            }
            _ => {
                // Ignore other events
            }
        }
        Ok(None)
    }

    /// Safely transitions the runner back to Idle and executes any queued resumptions.
    async fn check_resumption_queue(&self) -> Result<()> {
        // 1. Acquire the state lock to inspect the queue.
        let mut state = self.state.write().await;

        // 2. Extract the pending configuration and attempt count if one exists.
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

            // 3. Mark the state as Idle so the background loop is unblocked.
            *state = RunnerState::Idle;

            // 4. Release the state lock *before* performing the heavy async socket connection.
            drop(state);

            // 5. Attempt the actual transport teardown/rebuild.
            if let Err(e) = self.execute_resumption((*config).clone(), bridge_message.clone()).await
            {
                tracing::error!("Resumption failed: {}.", e);

                // 6. If the reconnect fails (e.g., transient network error), re-acquire the lock
                // to safely handle the retry logic without crashing the event loop.
                let mut fallback_state = self.state.write().await;

                // 7. Enforce a maximum retry budget to prevent infinite "hot-looping"
                if attempts + 1 >= 3 {
                    tracing::error!(
                        "Maximum resumption attempts reached (3). Dropping queued mutation to prevent infinite loop."
                    );
                    *fallback_state = RunnerState::Idle;
                } else {
                    tracing::info!("Restoring pending queue state for retry.");
                    *fallback_state = RunnerState::PendingResumption {
                        config,
                        bridge_message,
                        attempts: attempts + 1,
                    };
                }

                // 8. Do not return Err(e) here, as that would permanently kill the `run()` loop.
                // Instead, report the error to the downstream handler and allow the event loop to continue spinning.
                let _ = self.event_handler.on_error(&e).await;
            }
        } else {
            // No resumptions were queued; simply mark as Idle.
            *state = RunnerState::Idle;
        }
        Ok(())
    }

    /// Execute a tool call and optionally send the response.
    async fn execute_tool_call(
        &self,
        call_id: &str,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<()> {
        // If circuit breaker has tripped (Open state), block execution and return deterministic rejection
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
            if self.runner_config.auto_respond_tools
                && let Ok(session) = self.session_handle().await
            {
                session
                    .send_tool_output(ToolResponse { call_id: call_id.to_string(), output: result })
                    .await?;
                self.pending_tool_response.store(true, Ordering::Release);
            }
            return Ok(());
        }

        let handler = self.tools.get(name).map(|(_, h)| h.clone());

        let result = if let Some(handler) = handler {
            let call = ToolCall {
                call_id: call_id.to_string(),
                name: name.to_string(),
                arguments: arguments.clone(),
            };

            match handler.execute(&call).await {
                Ok(value) => {
                    self.consecutive_tool_failures.store(0, Ordering::Release);
                    value
                }
                Err(e) => {
                    let failures =
                        self.consecutive_tool_failures.fetch_add(1, Ordering::AcqRel) + 1;
                    if failures >= self.runner_config.max_consecutive_tool_failures {
                        // Atomic transition: notify error callback exactly ONCE when entering Open state
                        if !self.circuit_breaker_tripped.swap(true, Ordering::AcqRel) {
                            tracing::error!(
                                call_id,
                                name,
                                failures,
                                max = self.runner_config.max_consecutive_tool_failures,
                                "Tool execution circuit breaker TRIPPED: entering Open state"
                            );
                            let _ = self.event_handler.on_error(&RealtimeError::provider(format!(
                                "Tool execution circuit breaker tripped after {} consecutive failures: {}",
                                failures, e
                            ))).await;
                        }
                        serde_json::json!({
                            "status": "rejected",
                            "reason": "tool_unavailable",
                            "error": format!("circuit_breaker_tripped: {}", e)
                        })
                    } else {
                        serde_json::json!({
                            "error": e.to_string()
                        })
                    }
                }
            }
        } else {
            serde_json::json!({
                "error": format!("Unknown tool: {}", name)
            })
        };

        if self.runner_config.auto_respond_tools {
            let response = ToolResponse { call_id: call_id.to_string(), output: result };

            if let Ok(session) = self.session_handle().await {
                // Send the output now, but defer the response trigger: several
                // parallel tool calls in one response must produce a *single*
                // `create_response`, issued once the dispatch response finishes
                // (see `respond_after_tools`). Firing one per output collides
                // with the still-active response on OpenAI.
                session.send_tool_output(response).await?;
                self.pending_tool_response.store(true, Ordering::Release);
            }
        }

        Ok(())
    }

    /// The system instruction the next connection will use.
    ///
    /// Exposed so callers and tests can confirm what context a session was actually created
    /// with, rather than inferring it from log lines.
    pub async fn instruction(&self) -> Option<String> {
        self.config.read().await.instruction.clone()
    }

    /// Prepends a context block to the system instruction before connecting.
    ///
    /// The integration layer uses this to carry prior conversation history and recalled memory
    /// into the provider session. Call it before [`RealtimeRunner::connect`]: providers read
    /// the instruction at session creation, so a later change needs `update_session`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// runner.prepend_instruction_context("Previously discussed: the refund policy.").await;
    /// runner.connect().await?;
    /// ```
    pub async fn prepend_instruction_context(&self, block: &str) {
        if block.is_empty() {
            return;
        }

        let mut config = self.config.write().await;
        config.instruction = Some(match config.instruction.take() {
            Some(existing) if !existing.is_empty() => format!("{block}\n\n{existing}"),
            _ => block.to_string(),
        });
    }

    /// Trigger the single follow-up response owed after a tool-dispatching turn.
    ///
    /// Call this when a response finishes (`ResponseDone`). If tool output(s)
    /// were sent back during that response (`auto_respond_tools`), the model now
    /// needs one `create_response` to speak its answer — issued here, after the
    /// dispatch response is closed and every parallel tool output is in, rather
    /// than once per tool call. Gemini's `create_response` is a no-op, so this is
    /// safely uniform across providers. No-op when nothing is pending.
    pub async fn respond_after_tools(&self) -> Result<()> {
        // Tools run concurrently with event intake, so this can be reached before their
        // output is in. Defer to the last tool to finish rather than firing a response the
        // model cannot yet answer — or, worse, dropping it because
        // `pending_tool_response` is not set yet.
        if self.outstanding_tools.load(Ordering::Acquire) > 0 {
            self.response_closed_awaiting_tools.store(true, Ordering::Release);
            return Ok(());
        }

        if self.pending_tool_response.swap(false, Ordering::AcqRel)
            && let Ok(session) = self.session_handle().await
        {
            session.create_response().await?;
        }
        Ok(())
    }

    /// Close the session.
    pub async fn close(&self) -> Result<()> {
        if let Ok(session) = self.session_handle().await {
            session.close().await?;
        }
        Ok(())
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

    /// A model just good enough to satisfy the builder (never connects in tests).
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

    /// Records which provider-session entry points the runner calls.
    #[derive(Default)]
    struct Counts {
        raw_audio: AtomicUsize,
        base64_audio: AtomicUsize,
        last_audio: parking_lot::Mutex<Option<AudioChunk>>,
        tool_output: AtomicUsize,
        tool_response: AtomicUsize,
        create_response: AtomicUsize,
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
        ToolDefinition { name: name.into(), description: None, parameters: None }
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

        // The unit test owns the runner and installs the same session handle
        // that `connect` would publish after provider setup.
        *runner.session.write().await = Some(session);
        runner
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

    /// Two parallel tool calls in one response must produce exactly one
    /// `create_response` — issued after the dispatch response finishes — not one
    /// per tool (which collides with the still-active response on OpenAI).
    #[tokio::test]
    async fn parallel_tool_calls_trigger_a_single_response() {
        let counts = Arc::new(Counts::default());
        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .tool(tool_def("get_weather"), ok_tool())
            .tool(tool_def("get_time"), ok_tool())
            .build()
            .unwrap();
        *runner.session.write().await =
            Some(Arc::new(RecordingSession { counts: counts.clone() }) as Arc<dyn RealtimeSession>);

        // One model response dispatching two tool calls, then ending. Driven through
        // `run`, because dispatch is the run loop's job now — `handle_event` returns the
        // call rather than awaiting it, so event intake is not blocked by a tool.
        let scripted = Arc::new(ScriptedSession::new(
            counts.clone(),
            vec![
                function_call("c1", "get_weather"),
                function_call("c2", "get_time"),
                response_done(),
            ],
        ));
        *runner.session.write().await = Some(scripted as Arc<dyn RealtimeSession>);
        runner.run().await.unwrap();

        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 2, "both outputs sent");
        assert_eq!(counts.create_response.load(Ordering::SeqCst), 1, "exactly one response");
        assert_eq!(
            counts.tool_response.load(Ordering::SeqCst),
            0,
            "auto path must use send_tool_output, not the output+create combo"
        );

        // The follow-up (spoken-answer) response finishing creates nothing more.
        runner.handle_event(response_done()).await.unwrap();
        assert_eq!(counts.create_response.load(Ordering::SeqCst), 1, "no extra response");
    }

    /// A response with no tool calls must not trigger an auto follow-up response.
    #[tokio::test]
    async fn plain_response_triggers_no_auto_response() {
        let counts = Arc::new(Counts::default());
        let runner =
            RealtimeRunner::builder().model(Arc::new(MockModel) as BoxedModel).build().unwrap();
        *runner.session.write().await =
            Some(Arc::new(RecordingSession { counts: counts.clone() }) as Arc<dyn RealtimeSession>);

        runner.handle_event(response_done()).await.unwrap();
        assert_eq!(counts.create_response.load(Ordering::SeqCst), 0);
        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 0);
    }

    // ── Bounded concurrent tool dispatch ──────────────────────────────────
    //
    // `RunnerConfig::max_concurrent_tools` defaulted to four and was read by nothing: no
    // semaphore, no scheduler. `FunctionCallDone` was awaited inline inside `handle_event`,
    // which the run loop awaited before reading the next event, so tool calls ran strictly
    // one at a time *and* stalled audio and every other event for the duration.

    /// A session that replays a scripted event sequence, then reports disconnect.
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
                // Pending indefinitely simulates an open WebSocket connection without populating
                // Tokio timer wheels or waking up after artificial sleep timeouts.
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

    /// A tool that reports how many copies of itself run at once.
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
                // Every participant must be running for this to return, so it can only
                // complete if the dispatcher truly overlaps them.
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
        // Only satisfiable if all three run at once, which serial dispatch cannot do.
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
        *runner.session.write().await = Some(scripted as Arc<dyn RealtimeSession>);

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
        *runner.session.write().await = Some(scripted as Arc<dyn RealtimeSession>);

        runner.run().await.unwrap();

        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "the bound was exceeded: peak {}",
            peak.load(Ordering::SeqCst)
        );
        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 4, "all four still complete");
    }

    /// A tool that blocks until an audio event has been handled.
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

    /// Signals the tool once audio is delivered.
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

    /// The event that reached one consumer of this runner and not another.
    ///
    /// Gemini Live emits `InputTranscriptDelta` and never
    /// `InputTranscriptCompleted`, so a handler relying on the completed
    /// callback observed no caller activity for an entire call while a consumer
    /// reading the raw event stream observed it correctly. Both were reading the
    /// same session.
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
                item_id: String::new(), // Gemini leaves this empty.
                content_index: 0,
                delta: "sensitive caller content".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(sources.lock().as_slice(), &[CallerActivitySource::TranscriptDelta]);
    }

    /// Every source in the mapping must actually arrive, and be labelled for
    /// what it is — the ranking is what lets a consumer refuse to act on the
    /// weakest evidence.
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
            // Not caller activity: starting to speak is barge-in.
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

        // The tool can only finish once the audio delta *after* it has been handled, so
        // this sequence completes only if event intake continues during tool execution.
        let scripted = Arc::new(ScriptedSession::new(
            Arc::clone(&counts),
            vec![function_call("c1", "slow"), audio_delta(), response_done()],
        ));
        *runner.session.write().await = Some(scripted as Arc<dyn RealtimeSession>);

        tokio::time::timeout(std::time::Duration::from_secs(5), runner.run())
            .await
            .expect("a tool awaiting a later event deadlocks when dispatch blocks intake")
            .unwrap();

        assert_eq!(audio_events.load(Ordering::SeqCst), 1, "the audio delta was handled");
        assert_eq!(counts.tool_output.load(Ordering::SeqCst), 1, "the tool still reported output");
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

        // 3 consecutive failures trip the breaker into Open state
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

        // 4th call must be BLOCKED from calling the handler
        runner.execute_tool_call("call-4", "failing", &serde_json::json!({})).await.unwrap();
        assert_eq!(
            calls_made.load(Ordering::SeqCst),
            3,
            "4th call was blocked by open circuit breaker"
        );

        // Reconnect resets breaker
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
        *runner.session.write().await = Some(scripted as Arc<dyn RealtimeSession>);

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();

        let runner_handle = Arc::new(runner);
        let runner_clone = Arc::clone(&runner_handle);

        let task =
            tokio::spawn(
                async move { runner_clone.run_with_cancellation(cancel_token_clone).await },
            );

        // Give the runner time to process the event and launch the in-flight tool call
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(
            runner_handle.outstanding_tools.load(Ordering::Acquire),
            1,
            "one tool is in flight"
        );

        // Cancel mid-flight
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

        // `ResponseDone` arrives while the tool is still running — impossible before
        // dispatch was concurrent, and the case that would silently drop the follow-up
        // response, since `pending_tool_response` is only set once output is sent.
        let scripted = Arc::new(ScriptedSession::new(
            Arc::clone(&counts),
            vec![function_call("c1", "slow"), response_done(), audio_delta()],
        ));
        *runner.session.write().await = Some(scripted as Arc<dyn RealtimeSession>);

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

    /// Records terminal disconnects.
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
    async fn a_terminal_disconnect_is_surfaced_once() {
        let counts = Arc::new(Counts::default());
        let disconnects = Arc::new(AtomicUsize::new(0));

        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .event_handler(DisconnectWatcher { disconnects: Arc::clone(&disconnects) })
            .build()
            .unwrap();

        let scripted = Arc::new(ScriptedSession::new(Arc::clone(&counts), vec![response_done()]));
        *runner.session.write().await = Some(scripted as Arc<dyn RealtimeSession>);

        // `run` returns `Ok(())` on transport loss, which on its own is indistinguishable
        // from a graceful `close`.
        runner.run().await.unwrap();

        assert_eq!(
            disconnects.load(Ordering::SeqCst),
            1,
            "transport loss must be reported exactly once"
        );
    }
}
