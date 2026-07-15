use crate::{
    AfterToolCallback, AfterToolCallbackFull, BeforeToolCallback, CallbackContext, Content, Event,
    EventActions, InvocationContext, OnToolErrorCallback, Part, Result, RetryBudget, Tool,
    ToolCallbackContext, ToolContext, ToolOutcome,
};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{Instrument, debug, info_span, warn};

/// Result from a tool interceptor.
pub enum ToolInterceptorResult {
    /// Continue with the (possibly modified) arguments.
    Continue(Value),
    /// Short-circuit execution and return this value.
    ShortCircuit(Value),
}

/// A callback that can intercept a tool call before it is executed.
pub type ToolInterceptor = Box<
    dyn Fn(
            Arc<dyn Tool>,
            Value,
            Arc<dyn CallbackContext>,
        ) -> Pin<Box<dyn Future<Output = Result<ToolInterceptorResult>> + Send>>
        + Send
        + Sync,
>;

/// Options for tool execution.
#[derive(Clone)]
pub struct ToolExecutorOptions {
    /// Retry budget for the tool.
    pub retry_budget: Option<RetryBudget>,
    /// Timeout for tool execution.
    pub timeout: Duration,
    /// Interceptors that can modify arguments or short-circuit execution.
    pub interceptors: Arc<Vec<ToolInterceptor>>,
    /// Callbacks to run before tool execution.
    pub before_tool_callbacks: Arc<Vec<BeforeToolCallback>>,
    /// Callbacks to run after tool execution.
    pub after_tool_callbacks: Arc<Vec<AfterToolCallback>>,
    /// Rich after-tool callbacks.
    pub after_tool_callbacks_full: Arc<Vec<AfterToolCallbackFull>>,
    /// Callbacks to run when a tool execution fails.
    pub on_tool_error_callbacks: Arc<Vec<OnToolErrorCallback>>,
    /// Optional progress sender for tool progress events.
    pub progress_tx: Option<tokio::sync::mpsc::UnboundedSender<Event>>,
}

impl Default for ToolExecutorOptions {
    fn default() -> Self {
        Self {
            retry_budget: None,
            timeout: Duration::from_secs(300),
            interceptors: Arc::new(vec![]),
            before_tool_callbacks: Arc::new(vec![]),
            after_tool_callbacks: Arc::new(vec![]),
            after_tool_callbacks_full: Arc::new(vec![]),
            on_tool_error_callbacks: Arc::new(vec![]),
            progress_tx: None,
        }
    }
}

/// A centralized executor for tool calls.
///
/// Handles validation, callbacks, retries, and timeout protection.
pub struct ToolCallExecutor;

impl ToolCallExecutor {
    /// Executes a tool call.
    pub async fn execute(
        ctx: Arc<dyn InvocationContext>,
        tool: Arc<dyn Tool>,
        call_id: &str,
        arguments: Value,
        options: ToolExecutorOptions,
    ) -> (Value, EventActions, Option<ToolOutcome>) {
        let name = tool.name().to_string();
        let invocation_id = ctx.invocation_id().to_string();

        // 1. Basic Validation
        if name.trim().is_empty() {
            return (
                serde_json::json!({ "error": "Tool name is missing or empty" }),
                EventActions::default(),
                None,
            );
        }
        if call_id.trim().is_empty() {
            return (
                serde_json::json!({ "error": "Call ID is missing or empty" }),
                EventActions::default(),
                None,
            );
        }

        // 2. Argument Shape Validation
        if !arguments.is_object() {
            return (
                serde_json::json!({ "error": format!("Arguments for tool '{}' must be an object", name) }),
                EventActions::default(),
                None,
            );
        }

        // 3. Schema Validation (against canonical full input schema)
        if let Some(schema) = tool.parameters_schema() {
            if let Err(e) = validate_against_schema(&schema, &arguments) {
                return (
                    serde_json::json!({ "error": format!("Schema validation failed for tool '{}': {}", name, e) }),
                    EventActions::default(),
                    None,
                );
            }
        }

        // 4. Create Tool Context
        let tool_ctx: Arc<dyn ToolContext> = Arc::new(
            UnifiedToolContext::new(ctx.clone(), call_id.to_string())
                .with_progress(options.progress_tx.clone()),
        );

        // 5. Interceptors (e.g. Enhanced Plugins before_tool_call)
        let mut final_args = arguments;
        for interceptor in options.interceptors.as_ref() {
            match interceptor(
                tool.clone(),
                final_args.clone(),
                tool_ctx.clone() as Arc<dyn CallbackContext>,
            )
            .await
            {
                Ok(ToolInterceptorResult::Continue(modified_args)) => {
                    final_args = modified_args;
                }
                Ok(ToolInterceptorResult::ShortCircuit(result)) => {
                    return (result, tool_ctx.actions(), None);
                }
                Err(e) => {
                    return (
                        serde_json::json!({ "error": e.to_string() }),
                        tool_ctx.actions(),
                        None,
                    );
                }
            }
        }

        // 6. Before-Tool Callbacks
        let tool_cb_ctx = Arc::new(ToolCallbackContext::new(
            ctx.clone() as Arc<dyn CallbackContext>,
            name.clone(),
            final_args.clone(),
        ));

        for callback in options.before_tool_callbacks.as_ref() {
            match callback(tool_cb_ctx.clone() as Arc<dyn CallbackContext>).await {
                Ok(Some(content)) => {
                    // Callback returned content - use it as a shortcut response
                    let result = extract_result_from_content(content, &name);
                    return (result, tool_ctx.actions(), None);
                }
                Ok(None) => continue,
                Err(e) => {
                    return (
                        serde_json::json!({ "error": e.to_string() }),
                        tool_ctx.actions(),
                        None,
                    );
                }
            }
        }

        // 7. Tool Execution with Retries and Timeout
        let budget = options.retry_budget.as_ref();
        let max_attempts = budget.map(|b| b.max_retries + 1).unwrap_or(1);
        let retry_delay = budget.map(|b| b.delay).unwrap_or_default();

        let tool_span = info_span!(
            "execute_tool",
            tool.name = %name,
            tool.call_id = %call_id,
            invocation.id = %invocation_id
        );

        let mut last_error = String::new();
        let mut final_attempt: u32 = 0;
        let mut tool_result: Option<Value> = None;
        let start_time = Instant::now();

        for attempt in 0..max_attempts {
            final_attempt = attempt;
            if attempt > 0 {
                tokio::time::sleep(retry_delay).await;
            }

            let exec_result = async {
                let exec_future = tool.execute(tool_ctx.clone(), final_args.clone());
                let unwind_safe_future = std::panic::AssertUnwindSafe(tokio::time::timeout(
                    options.timeout,
                    exec_future,
                ));

                match futures::FutureExt::catch_unwind(unwind_safe_future).await {
                    Ok(Ok(Ok(value))) => Ok(value),
                    Ok(Ok(Err(e))) => Err(e.to_string()),
                    Ok(Err(_)) => {
                        Err(format!("Tool '{}' timed out after {:?}", name, options.timeout))
                    }
                    Err(_) => Err(format!("Tool '{}' panicked during execution", name)),
                }
            }
            .instrument(tool_span.clone())
            .await;

            match exec_result {
                Ok(value) => {
                    tool_result = Some(value);
                    break;
                }
                Err(e) => {
                    last_error = e;
                    if attempt + 1 < max_attempts {
                        debug!(tool.name = %name, attempt = attempt, error = %last_error, "Tool execution failed, retrying");
                    }
                }
            }
        }

        let duration = start_time.elapsed();
        let success = tool_result.is_some();
        let final_value = tool_result.unwrap_or_else(|| serde_json::json!({ "error": last_error }));

        let outcome = ToolOutcome {
            tool_name: name.clone(),
            tool_args: final_args.clone(),
            success,
            duration,
            error_message: if success { None } else { Some(last_error.clone()) },
            attempt: final_attempt,
        };

        // 8. On-Tool-Error Callbacks
        let mut processed_value = if !success {
            let mut fallback = None;
            for callback in options.on_tool_error_callbacks.as_ref() {
                match callback(
                    ctx.clone() as Arc<dyn CallbackContext>,
                    tool.clone(),
                    final_args.clone(),
                    last_error.clone(),
                )
                .await
                {
                    Ok(Some(result)) => {
                        fallback = Some(result);
                        break;
                    }
                    Ok(None) => continue,
                    Err(e) => {
                        warn!(error = %e, "on_tool_error callback failed");
                        break;
                    }
                }
            }
            fallback.unwrap_or(final_value)
        } else {
            final_value
        };

        // 9. After-Tool Callbacks
        let outcome_ctx = Arc::new(ToolOutcomeCallbackContext {
            inner: ctx.clone() as Arc<dyn CallbackContext>,
            outcome: outcome.clone(),
        });

        let cb_ctx = Arc::new(ToolCallbackContext::new(
            outcome_ctx as Arc<dyn CallbackContext>,
            name.clone(),
            final_args.clone(),
        ));

        for callback in options.after_tool_callbacks.as_ref() {
            match callback(cb_ctx.clone() as Arc<dyn CallbackContext>).await {
                Ok(Some(content)) => {
                    processed_value = extract_result_from_content(content, &name);
                    break;
                }
                Ok(None) => continue,
                Err(e) => {
                    processed_value = serde_json::json!({ "error": e.to_string() });
                    break;
                }
            }
        }

        // 10. After-Tool Full Callbacks
        for callback in options.after_tool_callbacks_full.as_ref() {
            match callback(
                cb_ctx.clone() as Arc<dyn CallbackContext>,
                tool.clone(),
                final_args.clone(),
                processed_value.clone(),
            )
            .await
            {
                Ok(Some(modified)) => {
                    processed_value = modified;
                    break;
                }
                Ok(None) => continue,
                Err(e) => {
                    processed_value = serde_json::json!({ "error": e.to_string() });
                    break;
                }
            }
        }

        (processed_value, tool_ctx.actions(), Some(outcome))
    }
}

/// Validates a value against a JSON Schema.
fn validate_against_schema(schema: &Value, args: &Value) -> std::result::Result<(), String> {
    let validator =
        jsonschema::validator_for(schema).map_err(|e| format!("Invalid schema: {}", e))?;
    if validator.is_valid(args) {
        Ok(())
    } else {
        let errors: Vec<_> = validator.iter_errors(args).map(|e| e.to_string()).collect();
        Err(errors.join("; "))
    }
}

/// Helper to extract a tool result from a `Content` object.
fn extract_result_from_content(content: Content, tool_name: &str) -> Value {
    for part in &content.parts {
        if let Part::FunctionResponse { function_response, .. } = part {
            if function_response.name == tool_name {
                return function_response.response.clone();
            }
        }
    }
    // Fallback: use first part as string if possible
    Value::String(format!("{:?}", content))
}

/// A unified implementation of `ToolContext` used by the centralized executor.
pub struct UnifiedToolContext {
    parent_ctx: Arc<dyn InvocationContext>,
    function_call_id: String,
    actions: std::sync::RwLock<EventActions>,
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<Event>>,
}

impl UnifiedToolContext {
    /// Creates a new `UnifiedToolContext`.
    pub fn new(parent_ctx: Arc<dyn InvocationContext>, function_call_id: String) -> Self {
        Self {
            parent_ctx,
            function_call_id,
            actions: std::sync::RwLock::new(EventActions::default()),
            progress_tx: None,
        }
    }

    /// Attaches a progress sender.
    pub fn with_progress(mut self, tx: Option<tokio::sync::mpsc::UnboundedSender<Event>>) -> Self {
        self.progress_tx = tx;
        self
    }
}

#[async_trait::async_trait]
impl crate::ReadonlyContext for UnifiedToolContext {
    fn invocation_id(&self) -> &str {
        self.parent_ctx.invocation_id()
    }
    fn agent_name(&self) -> &str {
        self.parent_ctx.agent_name()
    }
    fn user_id(&self) -> &str {
        self.parent_ctx.user_id()
    }
    fn app_name(&self) -> &str {
        self.parent_ctx.app_name()
    }
    fn session_id(&self) -> &str {
        self.parent_ctx.session_id()
    }
    fn branch(&self) -> &str {
        self.parent_ctx.branch()
    }
    fn user_content(&self) -> &Content {
        self.parent_ctx.user_content()
    }
}

#[async_trait::async_trait]
impl crate::CallbackContext for UnifiedToolContext {
    fn artifacts(&self) -> Option<Arc<dyn crate::Artifacts>> {
        self.parent_ctx.artifacts()
    }
    fn shared_state(&self) -> Option<Arc<crate::SharedState>> {
        self.parent_ctx.shared_state()
    }
}

#[async_trait::async_trait]
impl ToolContext for UnifiedToolContext {
    fn function_call_id(&self) -> &str {
        &self.function_call_id
    }
    fn actions(&self) -> EventActions {
        self.actions.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn set_actions(&self, actions: EventActions) {
        *self.actions.write().unwrap_or_else(|e| e.into_inner()) = actions;
    }
    async fn search_memory(&self, query: &str) -> Result<Vec<crate::MemoryEntry>> {
        if let Some(memory) = self.parent_ctx.memory() {
            memory.search(query).await
        } else {
            Ok(vec![])
        }
    }
    fn user_scopes(&self) -> Vec<String> {
        self.parent_ctx.user_scopes()
    }
    async fn get_secret(&self, name: &str) -> Result<Option<String>> {
        self.parent_ctx.get_secret(name).await
    }
    async fn emit_progress(&self, stream: &str, chunk: &str) {
        if let Some(tx) = &self.progress_tx {
            let event = Event::tool_progress(
                self.parent_ctx.invocation_id(),
                self.parent_ctx.agent_name(),
                &self.function_call_id,
                stream,
                chunk,
            );
            let _ = tx.send(event);
        }
        debug!(
            target: "adk_core::tool_progress",
            tool_call_id = %self.function_call_id,
            stream = %stream,
            "{chunk}",
        );
    }
}

struct ToolOutcomeCallbackContext {
    inner: Arc<dyn CallbackContext>,
    outcome: ToolOutcome,
}

#[async_trait::async_trait]
impl crate::ReadonlyContext for ToolOutcomeCallbackContext {
    fn invocation_id(&self) -> &str {
        self.inner.invocation_id()
    }
    fn agent_name(&self) -> &str {
        self.inner.agent_name()
    }
    fn user_id(&self) -> &str {
        self.inner.user_id()
    }
    fn app_name(&self) -> &str {
        self.inner.app_name()
    }
    fn session_id(&self) -> &str {
        self.inner.session_id()
    }
    fn branch(&self) -> &str {
        self.inner.branch()
    }
    fn user_content(&self) -> &Content {
        self.inner.user_content()
    }
}

#[async_trait::async_trait]
impl crate::CallbackContext for ToolOutcomeCallbackContext {
    fn artifacts(&self) -> Option<Arc<dyn crate::Artifacts>> {
        self.inner.artifacts()
    }
    fn tool_outcome(&self) -> Option<ToolOutcome> {
        Some(self.outcome.clone())
    }
    fn tool_name(&self) -> Option<&str> {
        self.inner.tool_name()
    }
    fn tool_input(&self) -> Option<&Value> {
        self.inner.tool_input()
    }
    fn shared_state(&self) -> Option<Arc<crate::SharedState>> {
        self.inner.shared_state()
    }
}
