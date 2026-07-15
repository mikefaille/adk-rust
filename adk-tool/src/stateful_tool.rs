use adk_core::{Result, Tool, ToolContext};
use adk_schema::{SchemaDocument, static_schema};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

type AsyncStatefulHandler<S> = Box<
    dyn Fn(
            Arc<S>,
            Arc<dyn ToolContext>,
            Value,
        ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>>
        + Send
        + Sync,
>;

pub struct StatefulTool<S: Send + Sync + 'static> {
    name: String,
    description: String,
    state: Arc<S>,
    handler: AsyncStatefulHandler<S>,
    long_running: bool,
    read_only: bool,
    concurrency_safe: bool,
    parameters_schema: Option<SchemaDocument>,
    response_schema: Option<SchemaDocument>,
    scopes: Vec<&'static str>,
}

impl<S: Send + Sync + 'static> StatefulTool<S> {
    pub fn new<F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        state: Arc<S>,
        handler: F,
    ) -> Self
    where
        F: Fn(Arc<S>, Arc<dyn ToolContext>, Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        Self {
            name: name.into(),
            description: description.into(),
            state,
            handler: Box::new(move |s, ctx, args| Box::pin(handler(s, ctx, args))),
            long_running: false,
            read_only: false,
            concurrency_safe: false,
            parameters_schema: None,
            response_schema: None,
            scopes: Vec::new(),
        }
    }

    pub fn with_long_running(mut self, long_running: bool) -> Self {
        self.long_running = long_running;
        self
    }

    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn with_concurrency_safe(mut self, concurrency_safe: bool) -> Self {
        self.concurrency_safe = concurrency_safe;
        self
    }

    pub fn with_parameters_schema<T>(mut self) -> Self
    where
        T: JsonSchema + Serialize,
    {
        self.parameters_schema = Some(
            static_schema::for_deserialize::<T>().expect("failed to generate parameters schema"),
        );
        self
    }

    pub fn with_response_schema<T>(mut self) -> Self
    where
        T: JsonSchema + Serialize,
    {
        self.response_schema =
            Some(static_schema::for_serialize::<T>().expect("failed to generate response schema"));
        self
    }

    pub fn with_scopes(mut self, scopes: &[&'static str]) -> Self {
        self.scopes = scopes.to_vec();
        self
    }

    pub fn parameters_schema(&self) -> Option<&SchemaDocument> {
        self.parameters_schema.as_ref()
    }

    pub fn response_schema(&self) -> Option<&SchemaDocument> {
        self.response_schema.as_ref()
    }
}

const LONG_RUNNING_NOTE: &str = "NOTE: This is a long-running operation. Do not call this tool again if it has already returned some intermediate or pending status.";

#[async_trait]
impl<S: Send + Sync + 'static> Tool for StatefulTool<S> {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn enhanced_description(&self) -> String {
        if self.long_running {
            if self.description.is_empty() {
                LONG_RUNNING_NOTE.to_string()
            } else {
                format!("{}\n\n{}", self.description, LONG_RUNNING_NOTE)
            }
        } else {
            self.description.clone()
        }
    }

    fn is_long_running(&self) -> bool {
        self.long_running
    }

    fn is_read_only(&self) -> bool {
        self.read_only
    }

    fn is_concurrency_safe(&self) -> bool {
        self.concurrency_safe
    }

    fn parameters_schema(&self) -> Option<SchemaDocument> {
        self.parameters_schema.clone()
    }

    fn response_schema(&self) -> Option<SchemaDocument> {
        self.response_schema.clone()
    }

    fn required_scopes(&self) -> &[&str] {
        &self.scopes
    }

    #[adk_telemetry::instrument(
        skip(self, ctx, args),
        fields(
            tool.name = %self.name,
            tool.description = %self.description,
            tool.long_running = %self.long_running,
            function_call.id = %ctx.function_call_id()
        )
    )]
    async fn execute(&self, ctx: Arc<dyn ToolContext>, args: Value) -> Result<Value> {
        adk_telemetry::debug!("Executing stateful tool");
        let state = Arc::clone(&self.state);
        (self.handler)(state, ctx, args).await
    }
}
