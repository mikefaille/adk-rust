//! RealtimeRunner for integrating realtime sessions with agents.
//!
//! This module provides the bridge between realtime audio sessions and
//! the ADK agent framework, handling tool execution and event routing.

use crate::config::{RealtimeConfig, SessionUpdateConfig, ToolDefinition};
use crate::error::{RealtimeError, Result};
use crate::events::{ServerEvent, ToolCall, ToolResponse};
use crate::model::BoxedModel;
use crate::session::BoxedSession;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

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
    /// Optional transition TTL to prevent routing deadlocks.
    /// Format: `(target_tool_name, max_calls, max_window_size, fallback_tool_name)`
    pub tool_ttl: Option<(String, usize, usize, String)>,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            auto_execute_tools: true,
            auto_respond_tools: true,
            max_concurrent_tools: 4,
            tool_ttl: None,
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
            factory: model,
            config,
            runner_config: self.runner_config,
            tools: self.tools,
            event_handler: self.event_handler.unwrap_or_else(|| Arc::new(NoOpEventHandler)),
            session: Arc::new(RwLock::new(None)),
            executed_tools: Arc::new(RwLock::new(std::collections::VecDeque::new())),
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
#[derive(Debug, Clone)]
pub enum RunnerState {
    Idle,
    Generating,
    ExecutingTool,
    PendingResumption(Box<RealtimeConfig>),
}

pub struct RealtimeRunner {
    factory: BoxedModel,
    config: RealtimeConfig,
    runner_config: RunnerConfig,
    tools: HashMap<String, (ToolDefinition, Arc<dyn ToolHandler>)>,
    event_handler: Arc<dyn EventHandler>,
    session: Arc<RwLock<Option<BoxedSession>>>,
    executed_tools: Arc<RwLock<std::collections::VecDeque<String>>>,
    state: Arc<RwLock<RunnerState>>,
}

impl RealtimeRunner {
    /// Create a new builder.
    pub fn builder() -> RealtimeRunnerBuilder {
        RealtimeRunnerBuilder::new()
    }

    /// Connect to the realtime provider.
    pub async fn connect(&self) -> Result<()> {
        let session = self.factory.connect(self.config.clone()).await?;
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

    /// Update the session configuration.
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
    #[tracing::instrument(skip(self, config), name = "SessionUpdate")]
    pub async fn update_session(
        &self,
        config: SessionUpdateConfig,
        bridge_message: Option<String>,
    ) -> Result<()> {
        let start = std::time::Instant::now();
        let guard = self.session.read().await;
        let session = guard.as_ref().ok_or_else(|| RealtimeError::connection("Not connected"))?;

        match session.mutate_context(config.0).await? {
            crate::session::ContextMutationOutcome::Applied => {
                tracing::info!("Context updated natively mid-flight.");

                // Attention Shock Mitigation: Inject a generic bridge message explicitly provided by the caller
                if let Some(msg) = bridge_message {
                    let bridge_event = crate::events::ClientEvent::Message {
                        role: "user".to_string(),
                        parts: vec![adk_core::types::Part::Text { text: msg }],
                    };
                    if let Err(e) = session.send_event(bridge_event).await {
                        tracing::error!("Failed to inject context bridge message: {}", e);
                    }
                }
            }
            crate::session::ContextMutationOutcome::RequiresResumption(new_config) => {
                tracing::warn!("Provider requires soft reconnect. Checking resumability state...");
                let mut state = self.state.write().await;
                match *state {
                    RunnerState::Idle => {
                        drop(state);
                        drop(guard);
                        self.execute_resumption(*new_config, bridge_message).await?;
                    }
                    _ => {
                        tracing::info!("Runner is busy ({:?}). Queuing resumption.", *state);
                        *state = RunnerState::PendingResumption(new_config);
                        // The bridge_message is currently dropped if queued. For a full implementation,
                        // we'd queue the message too, but we follow the exact spec for now.
                    }
                }
            }
        }

        let duration = start.elapsed();
        if duration > std::time::Duration::from_millis(100) {
            tracing::error!(
                duration_ms = duration.as_millis(),
                "SessionUpdate cycle exceeded 100ms threshold! State-sync telemetry alert."
            );
        }

        Ok(())
    }

    /// Executes the phantom reconnect / soft resumption loop.
    async fn execute_resumption(
        &self,
        new_config: RealtimeConfig,
        bridge_message: Option<String>,
    ) -> Result<()> {
        let _span = tracing::info_span!("cognitive_handoff").entered();
        let mut guard = self.session.write().await;

        if let Some(session) = guard.as_ref() {
            session.close().await?;
        }

        let new_session = self.factory.connect(new_config).await?;

        if let Some(msg) = bridge_message {
            let bridge_event = crate::events::ClientEvent::Message {
                role: "user".to_string(),
                parts: vec![adk_core::types::Part::Text { text: msg }],
            };
            if let Err(e) = new_session.send_event(bridge_event).await {
                tracing::error!("Failed to inject context bridge message after resumption: {}", e);
            }
        }

        *guard = Some(new_session);
        Ok(())
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
        match event {
            ServerEvent::ResponseCreated { .. } | ServerEvent::SpeechStarted { .. } => {
                *self.state.write().await = RunnerState::Generating;

                if let ServerEvent::SpeechStarted { audio_start_ms, .. } = event {
                    self.event_handler.on_speech_started(audio_start_ms).await?;
                }
            }
            ServerEvent::ResponseDone { .. } => {
                let queued_resumption = {
                    let mut state = self.state.write().await;
                    if let RunnerState::PendingResumption(config) = state.clone() {
                        *state = RunnerState::Idle;
                        Some(config)
                    } else {
                        *state = RunnerState::Idle;
                        None
                    }
                };

                if let Some(config) = queued_resumption {
                    tracing::info!("Executing queued session resumption.");
                    self.execute_resumption(*config, None).await?;
                }

                self.event_handler.on_response_done().await?;
            }
            ServerEvent::FunctionCallDone { call_id, name, arguments, .. } => {
                *self.state.write().await = RunnerState::ExecutingTool;

                if self.runner_config.auto_execute_tools {
                    self.execute_tool_call(&call_id, &name, &arguments).await?;
                }

                // Return to Idle (or process pending resumptions)
                let queued_resumption = {
                    let mut state = self.state.write().await;
                    if let RunnerState::PendingResumption(config) = state.clone() {
                        *state = RunnerState::Idle;
                        Some(config)
                    } else {
                        *state = RunnerState::Idle;
                        None
                    }
                };

                if let Some(config) = queued_resumption {
                    tracing::info!("Executing queued session resumption after tool call.");
                    self.execute_resumption(*config, None).await?;
                }
            }
            ServerEvent::AudioDelta { delta, item_id, .. } => {
                self.event_handler.on_audio(&delta, &item_id).await?;
            }
            ServerEvent::TextDelta { delta, item_id, .. } => {
                self.event_handler.on_text(&delta, &item_id).await?;
            }
            ServerEvent::TranscriptDelta { delta, item_id, .. } => {
                self.event_handler.on_transcript(&delta, &item_id).await?;
            }
            ServerEvent::SpeechStopped { audio_end_ms, .. } => {
                self.event_handler.on_speech_stopped(audio_end_ms).await?;
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

    /// Execute a tool call and optionally send the response.
    async fn execute_tool_call(&self, call_id: &str, name: &str, arguments: &str) -> Result<()> {
        let mut resolved_name = name;

        // Enforce generic TTL to prevent routing deadlocks
        if let Some((target_tool, max_calls, window_size, fallback_tool)) =
            &self.runner_config.tool_ttl
        {
            let mut executed_tools = self.executed_tools.write().await;

            if name == target_tool {
                let call_count = executed_tools.iter().filter(|&t| t == target_tool).count();
                if call_count >= *max_calls {
                    tracing::warn!(
                        "Tool {} exceeded TTL ({} calls in {}-turn window). Forcing failover to {}.",
                        name,
                        max_calls,
                        window_size,
                        fallback_tool
                    );
                    resolved_name = fallback_tool;
                }
            }

            executed_tools.push_back(name.to_string());
            if executed_tools.len() > *window_size {
                executed_tools.pop_front();
            }
        }

        let handler = self.tools.get(resolved_name).map(|(_, h)| h.clone());

        let result = if let Some(handler) = handler {
            let args: serde_json::Value = serde_json::from_str(arguments)
                .unwrap_or(serde_json::Value::Object(Default::default()));

            let call = ToolCall {
                call_id: call_id.to_string(),
                name: resolved_name.to_string(),
                arguments: args,
            };

            match handler.execute(&call).await {
                Ok(value) => value,
                Err(e) => serde_json::json!({
                    "error": e.to_string()
                }),
            }
        } else {
            serde_json::json!({
                "error": format!("Unknown tool: {}", resolved_name)
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
