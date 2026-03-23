use adk_core::ReadonlyContext;
use tracing::Span;

/// An extension trait that adds synergistic tracing capabilities to any ADK context.
pub trait TraceContextExt: ReadonlyContext {
    /// Creates a top-level invocation span.
    fn invocation_span(&self) -> Span {
        let span = tracing::info_span!(
            "adk.invocation",
            "adk.invocation_id" = %self.invocation_id(),
            "adk.session_id" = %self.session_id(),
            "adk.user_id" = %self.user_id(),
            "adk.app_name" = %self.app_name(),
            "adk.branch" = %self.branch(),
            // Restore Google/Vertex specific keys
            "gcp.vertex.agent.invocation_id" = %self.invocation_id(),
            "gcp.vertex.agent.session_id" = %self.session_id(),
            "gen_ai.conversation.id" = %self.session_id()
        );
        self.record_metadata(&span);
        span
    }

    /// Creates a child span for a specific execution step.
    fn step_span(&self, name: &'static str) -> Span {
        let span = tracing::info_span!(
            "adk.step",
            "adk.step.name" = name,
            "adk.invocation_id" = %self.invocation_id(),
            "adk.session_id" = %self.session_id(),
            "adk.user_id" = %self.user_id(),
            "adk.app_name" = %self.app_name(),
            "adk.branch" = %self.branch(),
            "adk.tool.name" = tracing::field::Empty,
            // Restore Google/Vertex specific keys
            "gcp.vertex.agent.invocation_id" = %self.invocation_id(),
            "gcp.vertex.agent.session_id" = %self.session_id()
        );
        self.record_metadata(&span);
        span
    }

    /// Creates a specialized span for agent execution.
    fn agent_span(&self) -> Span {
        let span = tracing::info_span!(
            "agent.execute",
            "agent.name" = %self.agent_name(),
            "adk.invocation_id" = %self.invocation_id(),
            "adk.session_id" = %self.session_id(),
            "adk.user_id" = %self.user_id(),
            "adk.app_name" = %self.app_name(),
            "adk.branch" = %self.branch(),
            // Restore Google/Vertex specific keys
            "gcp.vertex.agent.invocation_id" = %self.invocation_id(),
            "gcp.vertex.agent.session_id" = %self.session_id(),
            // Agent-specific trace attributes
            "adk.skills.selected_name" = tracing::field::Empty,
            "adk.skills.selected_id" = tracing::field::Empty
        );
        self.record_metadata(&span);
        span
    }

    /// Stamps the current span with all identity attributes and metadata from this context.
    fn record_identity(&self, span: &Span) {
        span.record("adk.invocation_id", self.invocation_id());
        span.record("adk.session_id", self.session_id());
        span.record("adk.user_id", self.user_id());
        span.record("adk.app_name", self.app_name());
        span.record("adk.branch", self.branch());
        // Record Google/Vertex specific keys
        span.record("gcp.vertex.agent.invocation_id", self.invocation_id());
        span.record("gcp.vertex.agent.session_id", self.session_id());
        self.record_metadata(span);
    }

    fn record_metadata(&self, _span: &Span) {
        // No-op for base ReadonlyContext, can be overridden by more specific contexts if needed
    }
}

// Blanket implementation for all types implementing ReadonlyContext.
impl<T: ReadonlyContext> TraceContextExt for T {}
