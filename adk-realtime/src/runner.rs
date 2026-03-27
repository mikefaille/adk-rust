//! RealtimeRunner for integrating realtime sessions with agents.
//!
//! This module provides the bridge between realtime audio sessions and
//! the ADK agent framework, handling tool execution and event routing.

use crate::config::{RealtimeConfig, SessionUpdateConfig, ToolDefinition};
use crate::error::{RealtimeError, Result};
use crate::events::{ServerEvent, ToolCall, ToolResponse};
use crate::model::BoxedModel;
use crate::session::{BoxedSession, ContextMutationOutcome};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Internal state machine tracking the resumability status of the RealtimeRunner.
#[derive(Debug, Clone, PartialEq)]
pub enum RunnerState {
    /// Runner is ready to accept transport resumption immediately.
    Idle,
    /// Model is currently generating a response; tearing down the connection would corrupt context.
    Generating,
    /// A tool is currently executing; teardown would cause tool loss.
    ExecutingTool,
    /// A context mutation was queued while the runner was busy, and must be executed once Idle.
    ///
    /// The runner keeps only one pending resumption. If a new session update arrives while
    /// a resumption is already pending, the previous pending resumption is replaced. This is
    /// intentional: pending session updates represent desired end state, not an ordered command queue.
    /// The policy is last write wins.
    PendingResumption {
        /// The new configuration to apply on reconnection.
        config: crate::config::RealtimeConfig,
        /// An optional message to inject immediately after resumption.
        bridge_message: Option<String>,
        /// Number of failed reconnection attempts for this mutation.
        attempts: u8,
    },
}

impl Default for RunnerState {
    fn default() -> Self {
        Self::Idle
    }
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

/// Configuration for the RealtimeRunner.
#[derive(Clone)]
pub struct RunnerConfig {
    /// Whether to automatically execute tool calls.
    pub auto_execute_tools: bool,
    /// Whether to automatically send tool responses.
    pub auto_respond_tools: bool,
    /// Maximum concurrent tool executions.
    pub max_concurrent_tools: usize,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self { auto_execute_tools: true, auto_respond_tools: true, max_concurrent_tools: 4 }
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

    /// Set the event handler.
    pub fn event_handler(mut self, handler: impl EventHandler + 'static) -> Self {
        self.event_handler = Some(Arc::new(handler));
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

        Ok(RealtimeRunner {
            model,
            config: Arc::new(RwLock::new(config)),
            runner_config: self.runner_config,
            tools: self.tools,
            event_handler: self.event_handler.unwrap_or_else(|| Arc::new(NoOpEventHandler)),
            session: Arc::new(RwLock::new(None)),
            state: Arc::new(RwLock::new(RunnerState::Idle)),
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
///     let model = OpenAIRealtimeModel::new(api_key, "gpt-4o-realtime-preview-2024-12-17");
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
    session: Arc<RwLock<Option<BoxedSession>>>,
    state: Arc<RwLock<RunnerState>>,
}

impl RealtimeRunner {
    /// Create a new builder.
    pub fn builder() -> RealtimeRunnerBuilder {
        RealtimeRunnerBuilder::new()
    }

    /// Connect to the realtime provider.
    pub async fn connect(&self) -> Result<()> {
        let config = self.config.read().await.clone();
        let session = self.model.connect(config).await?;
        let mut guard = self.session.write().await;
        *guard = Some(session);
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

    /// Send a client event directly to the session.
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
                let guard = self.session.read().await;
                let session = guard.as_ref().ok_or_else(|| RealtimeError::connection("Not connected"))?;
                session.send_event(other).await
            }
        }
    }

    /// Internal helper to merge a `SessionUpdateConfig` into a base `RealtimeConfig`.
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
    /// Delegates to [`update_session_with_bridge`] with no bridge message.
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
        let mut full_config = self.config.write().await;
        Self::merge_config(&mut full_config, &config);

        let cloned_config = full_config.clone();
        drop(full_config);

        let guard = self.session.read().await;
        let session = guard.as_ref().ok_or_else(|| RealtimeError::connection("Not connected"))?;

        match session.mutate_context(cloned_config).await? {
            ContextMutationOutcome::Applied => {
                tracing::info!("Context mutated natively mid-flight.");
                // If applied natively, we can just inject the bridge message directly as a standard message.
                if let Some(msg) = bridge_message {
                    let event = crate::events::ClientEvent::Message {
                        role: "user".to_string(),
                        parts: vec![adk_core::types::Part::Text { text: msg }],
                    };
                    session.send_event(event).await?;
                }
                Ok(())
            }
            ContextMutationOutcome::RequiresResumption(new_config) => {
                drop(guard); // release the read lock before potential async ops
                let mut state_guard = self.state.write().await;
                if *state_guard == RunnerState::Idle {
                    drop(state_guard);
                    tracing::info!("Runner is idle. Executing resumption immediately.");
                    if let Err(e) = self.execute_resumption(new_config.clone(), bridge_message.clone()).await {
                        tracing::error!("Immediate resumption failed: {}. Queueing for retry.", e);
                        let mut fallback_state = self.state.write().await;
                        *fallback_state = RunnerState::PendingResumption {
                            config: new_config,
                            bridge_message,
                            attempts: 1,
                        };
                        return Err(e);
                    }
                } else {
                    if let RunnerState::PendingResumption { .. } = *state_guard {
                        tracing::warn!("Runner already had a pending resumption. Overwriting with last-write-wins policy.");
                    } else {
                        tracing::info!("Runner is busy ({:?}). Queueing resumption.", *state_guard);
                    }

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

        let mut write_guard = self.session.write().await;
        if let Some(old_session) = write_guard.as_ref() {
            if let Err(e) = old_session.close().await {
                tracing::warn!("Failed to cleanly close old session during resumption: {}", e);
            }
        }

        // Reconnect via the generic model interface.
        let new_session = self.model.connect(new_config).await?;

        // Swap pointer before injecting events so that it is the active runner session.
        *write_guard = Some(new_session);
        drop(write_guard); // Free the lock explicitly

        // Inject bridge message into the new session if provided
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
        let guard = self.session.read().await;
        let session = guard.as_ref().ok_or_else(|| RealtimeError::connection("Not connected"))?;
        session.send_event(event).await
    }

    /// Send audio to the session.
    pub async fn send_audio(&self, audio_base64: &str) -> Result<()> {
        let guard = self.session.read().await;
        let session = guard.as_ref().ok_or_else(|| RealtimeError::connection("Not connected"))?;
        session.send_audio_base64(audio_base64).await
    }

    /// Send text to the session.
    pub async fn send_text(&self, text: &str) -> Result<()> {
        let guard = self.session.read().await;
        let session = guard.as_ref().ok_or_else(|| RealtimeError::connection("Not connected"))?;
        session.send_text(text).await
    }

    /// Commit the audio buffer (for manual VAD mode).
    pub async fn commit_audio(&self) -> Result<()> {
        let guard = self.session.read().await;
        let session = guard.as_ref().ok_or_else(|| RealtimeError::connection("Not connected"))?;
        session.commit_audio().await
    }

    /// Trigger a response from the model.
    pub async fn create_response(&self) -> Result<()> {
        let guard = self.session.read().await;
        let session = guard.as_ref().ok_or_else(|| RealtimeError::connection("Not connected"))?;
        session.create_response().await
    }

    /// Interrupt the current response.
    pub async fn interrupt(&self) -> Result<()> {
        let guard = self.session.read().await;
        let session = guard.as_ref().ok_or_else(|| RealtimeError::connection("Not connected"))?;
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
    pub async fn next_event(&self) -> Option<Result<ServerEvent>> {
        let guard = self.session.read().await;
        if let Some(session) = guard.as_ref() {
            // Some sessions might yield inside next_event, but just in case, yield here too
            tokio::task::yield_now().await;
            session.next_event().await
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            None
        }
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
        let guard = self.session.read().await;
        let session = guard.as_ref().ok_or_else(|| RealtimeError::connection("Not connected"))?;
        session.send_tool_response(response).await
    }

    /// Run the event loop, processing events until disconnected.
    pub async fn run(&self) -> Result<()> {
        loop {
            let event = {
                let guard = self.session.read().await;
                let session =
                    guard.as_ref().ok_or_else(|| RealtimeError::connection("Not connected"))?;
                session.next_event().await
            };

            match event {
                Some(Ok(event)) => {
                    self.handle_event(event).await?;
                }
                Some(Err(e)) => {
                    self.event_handler.on_error(&e).await?;
                    return Err(e);
                }
                None => {
                    // Session closed
                    break;
                }
            }
        }
        Ok(())
    }

    /// Process a single event.
    async fn handle_event(&self, event: ServerEvent) -> Result<()> {
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
            ServerEvent::SpeechStarted { audio_start_ms, .. } => {
                self.event_handler.on_speech_started(audio_start_ms).await?;
            }
            ServerEvent::SpeechStopped { audio_end_ms, .. } => {
                self.event_handler.on_speech_stopped(audio_end_ms).await?;
            }
            ServerEvent::ResponseDone { .. } => {
                self.event_handler.on_response_done().await?;
                self.check_resumption_queue().await?;
            }
            ServerEvent::FunctionCallDone { call_id, name, arguments, .. } => {
                if self.runner_config.auto_execute_tools {
                    self.execute_tool_call(&call_id, &name, &arguments).await?;
                }
            }
            ServerEvent::SessionUpdated { session, .. } => {
                // Check if the generic session update contains a resumption token
                if let Some(token) = session.get("resumeToken").and_then(|t| t.as_str()) {
                    tracing::info!("Received Gemini sessionResumption token, saving for future reconnects.");
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
        Ok(())
    }

    /// Safely transitions the runner back to Idle and executes any queued resumptions.
    async fn check_resumption_queue(&self) -> Result<()> {
        let mut state = self.state.write().await;

        let pending = if let RunnerState::PendingResumption { config, bridge_message, attempts } = &*state {
            Some((config.clone(), bridge_message.clone(), *attempts))
        } else {
            None
        };

        if let Some((config, bridge_message, attempts)) = pending {
            tracing::info!("Executing queued resumption after turn completion. (Attempt {})", attempts + 1);
            *state = RunnerState::Idle;
            drop(state); // Free lock before async execution

            // If the reconnection fails, we must restore the intent safely without hot-looping.
            if let Err(e) = self.execute_resumption(config.clone(), bridge_message.clone()).await {
                tracing::error!("Resumption failed: {}.", e);

                let mut fallback_state = self.state.write().await;
                if attempts + 1 >= 3 {
                    tracing::error!("Maximum resumption attempts reached (3). Dropping queued mutation to prevent infinite loop.");
                    *fallback_state = RunnerState::Idle;
                } else {
                    tracing::info!("Restoring pending queue state for retry.");
                    *fallback_state = RunnerState::PendingResumption { config, bridge_message, attempts: attempts + 1 };
                }

                // Do not return Err(e) here, as that would kill the `run()` loop.
                // Instead, report it to the event handler and allow the next turn to retry.
                let _ = self.event_handler.on_error(&e).await;
            }
        } else {
            *state = RunnerState::Idle;
        }
        Ok(())
    }

    /// Execute a tool call and optionally send the response.
    async fn execute_tool_call(&self, call_id: &str, name: &str, arguments: &str) -> Result<()> {
        let handler = self.tools.get(name).map(|(_, h)| h.clone());

        let result = if let Some(handler) = handler {
            let args: serde_json::Value = serde_json::from_str(arguments)
                .unwrap_or(serde_json::Value::Object(Default::default()));

            let call =
                ToolCall { call_id: call_id.to_string(), name: name.to_string(), arguments: args };

            match handler.execute(&call).await {
                Ok(value) => value,
                Err(e) => serde_json::json!({
                    "error": e.to_string()
                }),
            }
        } else {
            serde_json::json!({
                "error": format!("Unknown tool: {}", name)
            })
        };

        if self.runner_config.auto_respond_tools {
            let response = ToolResponse { call_id: call_id.to_string(), output: result };

            let guard = self.session.read().await;
            if let Some(session) = guard.as_ref() {
                session.send_tool_response(response).await?;
            }
        }

        Ok(())
    }

    /// Close the session.
    pub async fn close(&self) -> Result<()> {
        let guard = self.session.read().await;
        if let Some(session) = guard.as_ref() {
            session.close().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RealtimeConfig, SessionUpdateConfig, ToolDefinition};

    // ── RunnerState tests ──────────────────────────────────────────────────

    #[test]
    fn test_runner_state_default_is_idle() {
        let state = RunnerState::default();
        assert_eq!(state, RunnerState::Idle);
    }

    #[test]
    fn test_runner_state_partial_eq_idle() {
        assert_eq!(RunnerState::Idle, RunnerState::Idle);
        assert_ne!(RunnerState::Idle, RunnerState::Generating);
    }

    #[test]
    fn test_runner_state_partial_eq_generating() {
        assert_eq!(RunnerState::Generating, RunnerState::Generating);
        assert_ne!(RunnerState::Generating, RunnerState::ExecutingTool);
    }

    #[test]
    fn test_runner_state_partial_eq_executing_tool() {
        assert_eq!(RunnerState::ExecutingTool, RunnerState::ExecutingTool);
        assert_ne!(RunnerState::ExecutingTool, RunnerState::Idle);
    }

    #[test]
    fn test_runner_state_partial_eq_pending_resumption() {
        let config = RealtimeConfig::default().with_instruction("New instructions");
        let a = RunnerState::PendingResumption {
            config: config.clone(),
            bridge_message: None,
            attempts: 0,
        };
        let b = RunnerState::PendingResumption {
            config: config.clone(),
            bridge_message: None,
            attempts: 0,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_runner_state_partial_eq_pending_resumption_different_attempts() {
        let config = RealtimeConfig::default();
        let a = RunnerState::PendingResumption {
            config: config.clone(),
            bridge_message: None,
            attempts: 0,
        };
        let b = RunnerState::PendingResumption {
            config: config.clone(),
            bridge_message: None,
            attempts: 1,
        };
        assert_ne!(a, b);
    }

    #[test]
    fn test_runner_state_partial_eq_pending_resumption_different_bridge() {
        let config = RealtimeConfig::default();
        let a = RunnerState::PendingResumption {
            config: config.clone(),
            bridge_message: Some("Hello".to_string()),
            attempts: 0,
        };
        let b = RunnerState::PendingResumption {
            config: config.clone(),
            bridge_message: None,
            attempts: 0,
        };
        assert_ne!(a, b);
    }

    #[test]
    fn test_runner_state_clone() {
        let config = RealtimeConfig::default().with_instruction("Test");
        let original = RunnerState::PendingResumption {
            config,
            bridge_message: Some("bridge".to_string()),
            attempts: 2,
        };
        let cloned = original.clone();
        assert_eq!(original, cloned);
    }

    #[test]
    fn test_runner_state_debug() {
        let state = RunnerState::Idle;
        let debug_str = format!("{:?}", state);
        assert!(debug_str.contains("Idle"));

        let state = RunnerState::Generating;
        let debug_str = format!("{:?}", state);
        assert!(debug_str.contains("Generating"));

        let state = RunnerState::ExecutingTool;
        let debug_str = format!("{:?}", state);
        assert!(debug_str.contains("ExecutingTool"));
    }

    #[test]
    fn test_runner_state_pending_resumption_contains_config() {
        let config = RealtimeConfig::default().with_instruction("Context A");
        let state = RunnerState::PendingResumption {
            config: config.clone(),
            bridge_message: Some("transition".to_string()),
            attempts: 0,
        };

        match state {
            RunnerState::PendingResumption { config: c, bridge_message, attempts } => {
                assert_eq!(c.instruction.as_deref(), Some("Context A"));
                assert_eq!(bridge_message.as_deref(), Some("transition"));
                assert_eq!(attempts, 0);
            }
            _ => panic!("Expected PendingResumption"),
        }
    }

    // ── merge_config tests ─────────────────────────────────────────────────

    #[test]
    fn test_merge_config_instruction_applied() {
        let mut base = RealtimeConfig::default().with_instruction("Old instruction");
        let update = SessionUpdateConfig(RealtimeConfig {
            instruction: Some("New instruction".to_string()),
            ..Default::default()
        });
        RealtimeRunner::merge_config(&mut base, &update);
        assert_eq!(base.instruction.as_deref(), Some("New instruction"));
    }

    #[test]
    fn test_merge_config_instruction_none_leaves_base() {
        let mut base = RealtimeConfig::default().with_instruction("Keep me");
        let update = SessionUpdateConfig(RealtimeConfig::default()); // instruction = None
        RealtimeRunner::merge_config(&mut base, &update);
        assert_eq!(base.instruction.as_deref(), Some("Keep me"));
    }

    #[test]
    fn test_merge_config_tools_applied() {
        let mut base = RealtimeConfig::default();
        let tool = ToolDefinition::new("new_tool").with_description("A new capability");
        let update = SessionUpdateConfig(RealtimeConfig {
            tools: Some(vec![tool.clone()]),
            ..Default::default()
        });
        RealtimeRunner::merge_config(&mut base, &update);
        let tools = base.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "new_tool");
    }

    #[test]
    fn test_merge_config_tools_none_leaves_base() {
        let tool = ToolDefinition::new("existing_tool");
        let mut base = RealtimeConfig::default().with_tool(tool.clone());
        let update = SessionUpdateConfig(RealtimeConfig::default()); // tools = None
        RealtimeRunner::merge_config(&mut base, &update);
        // The base tool should remain
        let tools = base.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "existing_tool");
    }

    #[test]
    fn test_merge_config_tools_replaced_not_appended() {
        let old_tool = ToolDefinition::new("old_tool");
        let new_tool = ToolDefinition::new("new_tool");
        let mut base = RealtimeConfig::default().with_tool(old_tool);
        let update = SessionUpdateConfig(RealtimeConfig {
            tools: Some(vec![new_tool.clone()]),
            ..Default::default()
        });
        RealtimeRunner::merge_config(&mut base, &update);
        let tools = base.tools.unwrap();
        // Should replace, not append
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "new_tool");
    }

    #[test]
    fn test_merge_config_voice_applied() {
        let mut base = RealtimeConfig::default().with_voice("alloy");
        let update = SessionUpdateConfig(RealtimeConfig {
            voice: Some("nova".to_string()),
            ..Default::default()
        });
        RealtimeRunner::merge_config(&mut base, &update);
        assert_eq!(base.voice.as_deref(), Some("nova"));
    }

    #[test]
    fn test_merge_config_voice_none_leaves_base() {
        let mut base = RealtimeConfig::default().with_voice("alloy");
        let update = SessionUpdateConfig(RealtimeConfig::default()); // voice = None
        RealtimeRunner::merge_config(&mut base, &update);
        assert_eq!(base.voice.as_deref(), Some("alloy"));
    }

    #[test]
    fn test_merge_config_temperature_applied() {
        let mut base = RealtimeConfig::default().with_temperature(0.5);
        let update = SessionUpdateConfig(RealtimeConfig {
            temperature: Some(0.9),
            ..Default::default()
        });
        RealtimeRunner::merge_config(&mut base, &update);
        assert_eq!(base.temperature, Some(0.9));
    }

    #[test]
    fn test_merge_config_temperature_none_leaves_base() {
        let mut base = RealtimeConfig::default().with_temperature(0.7);
        let update = SessionUpdateConfig(RealtimeConfig::default()); // temperature = None
        RealtimeRunner::merge_config(&mut base, &update);
        assert_eq!(base.temperature, Some(0.7));
    }

    #[test]
    fn test_merge_config_extra_applied() {
        let mut base = RealtimeConfig::default();
        let extra = serde_json::json!({"resumeToken": "abc123"});
        let update = SessionUpdateConfig(RealtimeConfig {
            extra: Some(extra),
            ..Default::default()
        });
        RealtimeRunner::merge_config(&mut base, &update);
        let stored_extra = base.extra.unwrap();
        assert_eq!(stored_extra.get("resumeToken").and_then(|t| t.as_str()), Some("abc123"));
    }

    #[test]
    fn test_merge_config_extra_none_leaves_base() {
        let original_extra = serde_json::json!({"key": "value"});
        let mut base = RealtimeConfig { extra: Some(original_extra.clone()), ..Default::default() };
        let update = SessionUpdateConfig(RealtimeConfig::default()); // extra = None
        RealtimeRunner::merge_config(&mut base, &update);
        assert_eq!(base.extra, Some(original_extra));
    }

    #[test]
    fn test_merge_config_model_is_not_merged() {
        // The model field is intentionally NOT merged (transport-level attribute).
        let mut base = RealtimeConfig::default().with_model("gpt-4o-realtime-preview");
        let update = SessionUpdateConfig(RealtimeConfig {
            model: Some("different-model".to_string()),
            ..Default::default()
        });
        // merge_config only merges hot-swappable cognitive fields, not model
        RealtimeRunner::merge_config(&mut base, &update);
        // Model should remain unchanged because merge_config does not touch it
        assert_eq!(base.model.as_deref(), Some("gpt-4o-realtime-preview"));
    }

    #[test]
    fn test_merge_config_multiple_fields_at_once() {
        let tool = ToolDefinition::new("weather_tool");
        let mut base = RealtimeConfig::default()
            .with_instruction("Old instruction")
            .with_voice("alloy");
        let update = SessionUpdateConfig(RealtimeConfig {
            instruction: Some("New instruction".to_string()),
            tools: Some(vec![tool]),
            voice: Some("nova".to_string()),
            temperature: Some(0.8),
            ..Default::default()
        });
        RealtimeRunner::merge_config(&mut base, &update);
        assert_eq!(base.instruction.as_deref(), Some("New instruction"));
        assert_eq!(base.voice.as_deref(), Some("nova"));
        assert_eq!(base.temperature, Some(0.8));
        assert!(base.tools.is_some());
    }
}