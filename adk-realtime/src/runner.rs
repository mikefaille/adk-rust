//! RealtimeRunner for integrating realtime sessions with agents.
//!
//! This module provides the bridge between realtime audio sessions and
//! the ADK agent framework, handling tool execution and event routing.

use crate::config::{RealtimeConfig, ToolDefinition};
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
            config,
            runner_config: self.runner_config,
            tools: Arc::new(RwLock::new(self.tools)),
            event_handler: self.event_handler.unwrap_or_else(|| Arc::new(NoOpEventHandler)),
            session: Arc::new(RwLock::new(None)),
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
    config: RealtimeConfig,
    runner_config: RunnerConfig,
    tools: Arc<RwLock<HashMap<String, (ToolDefinition, Arc<dyn ToolHandler>)>>>,
    event_handler: Arc<dyn EventHandler>,
    session: Arc<RwLock<Option<BoxedSession>>>,
}

impl RealtimeRunner {
    /// Create a new builder.
    pub fn builder() -> RealtimeRunnerBuilder {
        RealtimeRunnerBuilder::new()
    }

    /// Connect to the realtime provider.
    pub async fn connect(&self) -> Result<()> {
        let session = self.model.connect(self.config.clone()).await?;
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
    pub async fn update_session(&self, config: serde_json::Value) -> Result<()> {
        let guard = self.session.read().await;
        let session = guard.as_ref().ok_or_else(|| RealtimeError::connection("Not connected"))?;
        session.send_event(crate::events::ClientEvent::SessionUpdate { session: config }).await
    }

    /// Get the next raw event from the session.
    pub async fn next_event(&self) -> Option<Result<ServerEvent>> {
        let guard = self.session.read().await;
        if let Some(session) = guard.as_ref() {
            session.next_event().await
        } else {
            Some(Err(RealtimeError::connection("Not connected")))
        }
    }

    /// Update the tool registry dynamically.
    pub async fn update_tools(
        &self,
        tools: HashMap<String, (ToolDefinition, Arc<dyn ToolHandler>)>,
    ) -> Result<()> {
        // Update local handlers
        {
            let mut guard = self.tools.write().await;
            *guard = tools;
        }

        // Send session update to model if connected
        let guard = self.session.read().await;
        if let Some(session) = guard.as_ref() {
            let tools_guard = self.tools.read().await;
            let tool_defs: Vec<ToolDefinition> =
                tools_guard.values().map(|(def, _)| def.clone()).collect();
            session
                .send_event(crate::events::ClientEvent::SessionUpdate {
                    session: serde_json::json!({ "tools": tool_defs }),
                })
                .await?;
        }

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
            }
            ServerEvent::FunctionCallDone { call_id, name, arguments, .. } => {
                if self.runner_config.auto_execute_tools {
                    self.execute_tool_call(&call_id, &name, &arguments).await?;
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

    /// Execute a tool call and optionally send the response.
    async fn execute_tool_call(&self, call_id: &str, name: &str, arguments: &str) -> Result<()> {
        let handler = {
            let guard = self.tools.read().await;
            guard.get(name).map(|(_, h)| h.clone())
        };

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

    /// Get the names of currently registered tools (for testing).
    #[cfg(test)]
    pub async fn tool_names(&self) -> Vec<String> {
        let guard = self.tools.read().await;
        guard.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioChunk;
    use crate::events::ClientEvent;
    use futures::Stream;
    use std::pin::Pin;

    struct MockSession {
        events: Arc<RwLock<Vec<ClientEvent>>>,
    }

    #[async_trait]
    impl crate::session::RealtimeSession for MockSession {
        fn session_id(&self) -> &str {
            "mock-session"
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
        async fn send_event(&self, event: ClientEvent) -> Result<()> {
            self.events.write().await.push(event);
            Ok(())
        }
        async fn next_event(&self) -> Option<Result<ServerEvent>> {
            None
        }
        fn events(&self) -> Pin<Box<dyn Stream<Item = Result<ServerEvent>> + Send + '_>> {
            Box::pin(futures::stream::empty())
        }
        async fn close(&self) -> Result<()> {
            Ok(())
        }
    }

    struct MockModel;

    #[async_trait]
    impl crate::model::RealtimeModel for MockModel {
        fn provider(&self) -> &str {
            "mock"
        }
        fn model_id(&self) -> &str {
            "mock-model"
        }
        fn supported_input_formats(&self) -> Vec<crate::audio::AudioFormat> {
            vec![]
        }
        fn supported_output_formats(&self) -> Vec<crate::audio::AudioFormat> {
            vec![]
        }
        fn available_voices(&self) -> Vec<&str> {
            vec![]
        }
        async fn connect(&self, _config: RealtimeConfig) -> Result<BoxedSession> {
            Ok(Box::new(MockSession { events: Arc::new(RwLock::new(Vec::new())) }))
        }
    }

    #[tokio::test]
    async fn test_runner_tool_swapping() -> Result<()> {
        let runner = RealtimeRunner::builder()
            .model(Arc::new(MockModel) as BoxedModel)
            .tool_fn(ToolDefinition::new("tool1"), |_| Ok(serde_json::json!({"res": "ok"})))
            .build()?;

        runner.connect().await?;

        // Verify initial tool
        {
            let names = runner.tool_names().await;
            assert!(names.contains(&"tool1".to_string()));
        }

        // Swap tools
        let mut new_tools = HashMap::new();
        let handler: Arc<dyn ToolHandler> =
            Arc::new(FnToolHandler::new(|_| Ok(serde_json::json!({"res": "new"}))));
        new_tools.insert("tool2".to_string(), (ToolDefinition::new("tool2"), handler));

        runner.update_tools(new_tools).await?;

        // Verify swapped tool
        {
            let names = runner.tool_names().await;
            assert!(!names.contains(&"tool1".to_string()));
            assert!(names.contains(&"tool2".to_string()));
        }

        Ok(())
    }
}
