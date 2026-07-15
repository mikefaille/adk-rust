/// Bypass wrapper for built-in tools (`bypass_multi_tools_limit`).
//
/// The Gemini Interactions API rejects mixing custom function tools with
/// built-in (server-side) tools such as `google_search` in a single request.
/// ADK-Python solves this with `bypass_multi_tools_limit=True`, which converts a
/// built-in tool into a *function-calling* tool so every tool in the request is
/// uniform.
//
/// [`BypassBuiltinTool`] is the Rust equivalent. It reports
/// `is_builtin() == false`, exposes a normal function-calling parameter schema,
/// and routes execution through an internal single-turn agent (reusing
/// [`AgentTool`](crate::AgentTool)). The supplied agent is expected to be an
/// `LlmAgent` configured with the corresponding built-in tool and a Gemini
/// model, so the grounded behaviour (e.g. Google Search) is performed
/// server-side and its result is returned as a function response — exactly
/// ADK-Python's `GoogleSearchAgentTool` pattern.
//
/// Because `adk-tool` cannot depend on `adk-agent` (that would be circular), the
/// internal agent is supplied by the caller rather than constructed here.
use crate::AgentTool;
use adk_core::{Agent, Result, Tool, ToolContext};
use adk_schema::SchemaDocument;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

/// Generalizes the `bypass_multi_tools_limit` conversion so any built-in tool
/// can adopt it ergonomically.
///
/// The Gemini Interactions API forbids mixing built-in (server-side) tools with
/// custom function tools in a single request. Implementing this trait lets a
/// built-in tool be converted into a function-calling [`BypassBuiltinTool`] —
/// reporting `is_builtin() == false` and declaring a normal function schema — so
/// every tool in the request is uniform.
///
/// Implementors only describe *what* the bypass tool looks like (its name,
/// description, parameter schema, and which argument field carries the query);
/// the shared [`with_bypass_multi_tools_limit`](BypassMultiToolsLimit::with_bypass_multi_tools_limit)
/// default method performs the actual conversion identically for every tool.
/// This guarantees the conversion is uniform across `GoogleSearchTool`,
/// `UrlContextTool`, `GeminiFileSearchTool`, and any future built-in.
///
/// Because `adk-tool` cannot depend on `adk-agent` (that would be circular), the
/// internal agent that performs the built-in behaviour is supplied by the
/// caller. It is expected to be an `LlmAgent` configured with the matching
/// built-in tool and a Gemini model.
///
/// # Example
///
/// ```rust,ignore
/// use adk_tool::{BypassMultiToolsLimit, GoogleSearchTool, UrlContextTool};
/// use std::sync::Arc;
///
/// // `search_agent` / `url_agent` are LlmAgents with the matching built-in tool.
/// let search = GoogleSearchTool::new().with_bypass_multi_tools_limit(Arc::new(search_agent));
/// let url = UrlContextTool::new().with_bypass_multi_tools_limit(Arc::new(url_agent));
/// assert!(!search.is_builtin());
/// assert!(!url.is_builtin());
/// ```
pub trait BypassMultiToolsLimit: Sized {
    /// The function-tool name surfaced to the model after bypass conversion.
    ///
    /// Defaults to the tool's own `Tool::name()` and rarely needs overriding.
    fn bypass_name(&self) -> String;

    /// The function-tool description surfaced to the model.
    fn bypass_description(&self) -> String;

    /// The JSON Schema for the bypass function's parameters.
    fn bypass_parameters_schema(&self) -> Value;

    /// The argument field that carries the natural-language query forwarded to
    /// the internal agent (e.g. `"query"` for search, `"url"` for URL context).
    fn bypass_query_field(&self) -> String;

    /// Convert this built-in tool into a function-calling [`BypassBuiltinTool`]
    /// so it can coexist with custom function tools under the Interactions API.
    ///
    /// `agent` is the internal single-turn agent that performs the built-in
    /// behaviour and whose answer is returned as the function response.
    fn with_bypass_multi_tools_limit(self, agent: Arc<dyn Agent>) -> Arc<dyn Tool> {
        Arc::new(BypassBuiltinTool::new(
            self.bypass_name(),
            self.bypass_description(),
            self.bypass_parameters_schema(),
            self.bypass_query_field(),
            agent,
        ))
    }
}

/// A built-in tool converted into a function-calling tool so it can coexist with
/// custom function tools under the Gemini Interactions API.
///
/// The wrapper is provider-neutral and generic: any built-in tool can adopt it
/// by supplying a name, description, parameter schema, the field that carries
/// the natural-language query, and an internal agent that performs the built-in
/// behaviour.
///
/// # Example
///
/// ```rust,ignore
/// use adk_tool::GoogleSearchTool;
/// use std::sync::Arc;
///
/// // `search_agent` is an LlmAgent configured with GoogleSearchTool + a Gemini model.
/// let tool = GoogleSearchTool::new().with_bypass_multi_tools_limit(Arc::new(search_agent));
/// assert!(!tool.is_builtin());
/// ```
pub struct BypassBuiltinTool {
    name: String,
    description: String,
    parameters_schema: Value,
    query_field: String,
    inner: AgentTool,
}

impl BypassBuiltinTool {
    /// Create a bypass wrapper around `agent`.
    ///
    /// * `name` / `description` — surfaced to the model as a function tool.
    /// * `parameters_schema` — the JSON Schema for the function's parameters.
    /// * `query_field` — the property in the incoming arguments that carries the
    ///   natural-language query forwarded to the internal agent.
    /// * `agent` — the internal single-turn agent that performs the built-in
    ///   behaviour (e.g. an `LlmAgent` with the built-in Google Search tool).
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters_schema: Value,
        query_field: impl Into<String>,
        agent: Arc<dyn Agent>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters_schema,
            query_field: query_field.into(),
            inner: AgentTool::new(agent).skip_summarization(true),
        }
    }

    /// Extract the query string from the incoming tool arguments.
    fn extract_query(&self, args: &Value) -> String {
        if let Some(query) = args.get(&self.query_field).and_then(Value::as_str) {
            return query.to_string();
        }
        match args {
            Value::String(s) => s.clone(),
            _ => serde_json::to_string(args).unwrap_or_default(),
        }
    }
}

#[async_trait]
impl Tool for BypassBuiltinTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    /// Bypass-converted tools are ordinary function-calling tools, never
    /// built-in. This is what allows them to coexist with custom function tools.
    fn is_builtin(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Option<SchemaDocument> {
        Some(SchemaDocument::for_input(self.parameters_schema.clone()))
    }

    async fn execute(&self, ctx: Arc<dyn ToolContext>, args: Value) -> Result<Value> {
        // Map the bypass tool's query argument onto AgentTool's `request` field
        // and run the internal single-turn agent.
        let query = self.extract_query(&args);
        self.inner.execute(ctx, json!({ "request": query })).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::{GeminiFileSearchTool, GoogleSearchTool, UrlContextTool};
    use adk_core::{
        Artifacts, CallbackContext, Content, Event, EventActions, InvocationContext, MemoryEntry,
        ReadonlyContext,
    };
    use std::sync::Mutex;

    // Minimal single-turn agent that echoes a grounded answer.
    struct MockSearchAgent;

    #[async_trait]
    impl Agent for MockSearchAgent {
        fn name(&self) -> &str {
            "google_search_agent"
        }

        fn description(&self) -> &str {
            "Performs grounded Google search."
        }

        fn sub_agents(&self) -> &[Arc<dyn Agent>] {
            &[]
        }

        async fn run(&self, _ctx: Arc<dyn InvocationContext>) -> Result<adk_core::EventStream> {
            use async_stream::stream;
            let s = stream! {
                let mut event = Event::new("mock-inv");
                event.author = "google_search_agent".to_string();
                event.llm_response.content =
                    Some(Content::new("model").with_text("grounded answer"));
                yield Ok(event);
            };
            Ok(Box::pin(s))
        }
    }

    struct MockToolContext {
        actions: Mutex<EventActions>,
        content: Content,
    }

    impl MockToolContext {
        fn new() -> Self {
            Self { actions: Mutex::new(EventActions::default()), content: Content::new("user") }
        }
    }

    #[async_trait]
    impl ReadonlyContext for MockToolContext {
        fn invocation_id(&self) -> &str {
            "inv-1"
        }
        fn agent_name(&self) -> &str {
            "test-agent"
        }
        fn user_id(&self) -> &str {
            "user-1"
        }
        fn app_name(&self) -> &str {
            "test-app"
        }
        fn session_id(&self) -> &str {
            "session-1"
        }
        fn branch(&self) -> &str {
            ""
        }
        fn user_content(&self) -> &Content {
            &self.content
        }
    }

    #[async_trait]
    impl CallbackContext for MockToolContext {
        fn artifacts(&self) -> Option<Arc<dyn Artifacts>> {
            None
        }
    }

    #[async_trait]
    impl ToolContext for MockToolContext {
        fn function_call_id(&self) -> &str {
            "call-1"
        }
        fn actions(&self) -> EventActions {
            self.actions.lock().unwrap().clone()
        }
        fn set_actions(&self, actions: EventActions) {
            *self.actions.lock().unwrap() = actions;
        }
        async fn search_memory(&self, _query: &str) -> Result<Vec<MemoryEntry>> {
            Ok(vec![])
        }
    }

    #[test]
    fn bypass_reports_not_builtin() {
        let tool = GoogleSearchTool::new().with_bypass_multi_tools_limit(Arc::new(MockSearchAgent));
        assert!(!tool.is_builtin(), "bypass tool must report is_builtin() == false");
    }

    #[test]
    fn bypass_declares_function_query_param() {
        let tool = GoogleSearchTool::new().with_bypass_multi_tools_limit(Arc::new(MockSearchAgent));

        assert_eq!(tool.name(), "google_search");

        let schema = tool.parameters_schema().expect("bypass tool must declare a function schema");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["query"]["type"], "string");
        assert_eq!(schema["required"][0], "query");

        // The declaration must be a plain function tool (no built-in metadata).
        let decl = tool.declaration();
        assert!(decl.get("x-adk-gemini-tool").is_none());
        assert!(decl.get("parameters").is_some());
    }

    #[tokio::test]
    async fn bypass_executes_via_internal_agent() {
        let tool = GoogleSearchTool::new().with_bypass_multi_tools_limit(Arc::new(MockSearchAgent));
        let ctx = Arc::new(MockToolContext::new()) as Arc<dyn ToolContext>;

        let result = tool
            .execute(ctx, json!({ "query": "what is adk-rust" }))
            .await
            .expect("bypass execution should succeed");

        assert_eq!(result["response"], "grounded answer");
    }

    #[test]
    fn url_context_bypass_reports_not_builtin_and_declares_url_param() {
        let tool = UrlContextTool::new().with_bypass_multi_tools_limit(Arc::new(MockSearchAgent));

        assert!(!tool.is_builtin(), "bypassed url_context must report is_builtin() == false");
        assert_eq!(tool.name(), "url_context");

        let schema = tool.parameters_schema().expect("bypass tool must declare a function schema");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["url"]["type"], "string");
        assert_eq!(schema["required"][0], "url");

        // A plain function tool — no built-in metadata in the declaration.
        let decl = tool.declaration();
        assert!(decl.get("x-adk-gemini-tool").is_none());
        assert!(decl.get("parameters").is_some());
    }

    #[test]
    fn file_search_bypass_reports_not_builtin_and_declares_query_param() {
        let tool = GeminiFileSearchTool::new(["my-store"])
            .with_bypass_multi_tools_limit(Arc::new(MockSearchAgent));

        assert!(!tool.is_builtin(), "bypassed file_search must report is_builtin() == false");
        assert_eq!(tool.name(), "gemini_file_search");

        let schema = tool.parameters_schema().expect("bypass tool must declare a function schema");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["query"]["type"], "string");
        assert_eq!(schema["required"][0], "query");

        let decl = tool.declaration();
        assert!(decl.get("x-adk-gemini-tool").is_none());
        assert!(decl.get("parameters").is_some());
    }

    /// The generalized trait path must convert every adopting built-in tool into
    /// a uniform, non-built-in function tool (Property 7: bypass conversion
    /// uniformity) — demonstrated here across three different built-ins.
    #[test]
    fn trait_path_is_uniform_across_tools() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            GoogleSearchTool::new().with_bypass_multi_tools_limit(Arc::new(MockSearchAgent)),
            UrlContextTool::new().with_bypass_multi_tools_limit(Arc::new(MockSearchAgent)),
            GeminiFileSearchTool::new(["store"])
                .with_bypass_multi_tools_limit(Arc::new(MockSearchAgent)),
        ];

        for tool in &tools {
            assert!(!tool.is_builtin(), "{} must not be built-in after bypass", tool.name());
            assert!(
                tool.parameters_schema().is_some(),
                "{} must declare a function schema after bypass",
                tool.name()
            );
            assert!(
                tool.declaration().get("x-adk-gemini-tool").is_none(),
                "{} must not retain built-in metadata after bypass",
                tool.name()
            );
        }
    }

    #[tokio::test]
    async fn url_context_bypass_executes_via_internal_agent() {
        let tool = UrlContextTool::new().with_bypass_multi_tools_limit(Arc::new(MockSearchAgent));
        let ctx = Arc::new(MockToolContext::new()) as Arc<dyn ToolContext>;

        let result = tool
            .execute(ctx, json!({ "url": "https://example.com" }))
            .await
            .expect("bypass execution should succeed");

        assert_eq!(result["response"], "grounded answer");
    }
}
