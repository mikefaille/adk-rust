use adk_core::ReadonlyContext;
use tracing::Span;

/// An extension trait that adds synergistic tracing capabilities to any ADK context.
pub trait TraceContextExt: ReadonlyContext {
    /// Creates a top-level invocation span.
    fn invocation_span(&self) -> Span {
        let id = self.identity();
        let span = tracing::info_span!(
            "adk.invocation",
            "adk.invocation_id" = %id.invocation_id,
            "adk.session_id" = %id.session_id,
            "adk.user_id" = %id.user_id,
            "adk.app_name" = %id.app_name,
            "adk.branch" = %id.branch,
            // Vertex AI Observability: Native integration for Google Cloud monitoring
            "gcp.vertex.invocation_id" = %id.invocation_id,
            "gcp.vertex.session_id" = %id.session_id,
            "gen_ai.conversation.id" = %id.session_id
        );
        self.record_metadata(&span);
        span
    }

    /// Creates a child span for a specific execution step.
    fn step_span(&self, name: &'static str) -> Span {
        let id = self.identity();
        let span = tracing::info_span!(
            "adk.step",
            "adk.step.name" = name,
            "adk.invocation_id" = %id.invocation_id,
            "adk.session_id" = %id.session_id,
            "adk.user_id" = %id.user_id,
            "adk.app_name" = %id.app_name,
            "adk.branch" = %id.branch,
            "adk.tool.name" = tracing::field::Empty,
            // Vertex AI Observability: Native integration for Google Cloud monitoring
            "gcp.vertex.invocation_id" = %id.invocation_id,
            "gcp.vertex.session_id" = %id.session_id
        );
        self.record_metadata(&span);
        span
    }

    /// Creates a specialized span for agent execution.
    fn agent_span(&self) -> Span {
        let id = self.identity();
        let span = tracing::info_span!(
            "agent.execute",
            "agent.name" = %id.agent_name,
            "adk.invocation_id" = %id.invocation_id,
            "adk.session_id" = %id.session_id,
            "adk.user_id" = %id.user_id,
            "adk.app_name" = %id.app_name,
            "adk.branch" = %id.branch,
            // Vertex AI Observability: Native integration for Google Cloud monitoring
            "gcp.vertex.invocation_id" = %id.invocation_id,
            "gcp.vertex.session_id" = %id.session_id,
            // Agent-specific trace attributes
            "adk.skills.selected_name" = tracing::field::Empty,
            "adk.skills.selected_id" = tracing::field::Empty
        );
        self.record_metadata(&span);
        span
    }

    /// Stamps the current span with all identity attributes and metadata from this context.
    fn record_identity(&self, span: &Span) {
        let id = self.identity();
        span.record("adk.invocation_id", id.invocation_id.to_string());
        span.record("adk.session_id", id.session_id.to_string());
        span.record("adk.user_id", id.user_id.to_string());
        span.record("adk.app_name", &id.app_name);
        span.record("adk.branch", &id.branch);
        // Vertex AI Observability: Native integration for Google Cloud monitoring
        span.record("gcp.vertex.invocation_id", id.invocation_id.to_string());
        span.record("gcp.vertex.session_id", id.session_id.to_string());
        self.record_metadata(span);
    }

    /// Records all key-value pairs from the context's metadata map onto the span.
    fn record_metadata(&self, span: &Span) {
        for (key, value) in self.metadata() {
            span.record(key.as_str(), value.as_str());
        }
    }
}

// Blanket implementation for all types implementing ReadonlyContext.
impl<T: ReadonlyContext> TraceContextExt for T {}
