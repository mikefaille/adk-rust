use adk_core::ReadonlyContext;
use tracing::Span;

/// An extension trait that adds tracing capabilities to any ADK context.
/// 
/// By providing this as an extension trait in `adk-telemetry`, we keep `adk-core`
/// lightweight and free of tracing dependencies, while ensuring all frameworks
/// (Realtime, Skill, Model) can gain high-fidelity observability with zero effort.
pub trait TraceContextExt: ReadonlyContext {
    /// Create a tracing span pre-populated with ADK standard attributes.
    /// This uses the metadata provided by the `ReadonlyContext` trait.
    fn span(&self) -> Span {
        tracing::info_span!(
            "adk_invocation",
            adk.agent.invocation_id = %self.invocation_id(),
            adk.agent.session_id = %self.session_id(),
            adk.agent.name = %self.agent_name(),
            adk.user.id = %self.user_id(),
            adk.app.name = %self.app_name(),
            adk.branch = %self.branch(),
            // Compatibility fields
            gcp.vertex.agent.invocation_id = %self.invocation_id(),
            gcp.vertex.agent.session_id = %self.session_id(),
            gen_ai.conversation.id = %self.session_id(),
        )
    }

    /// Create a child span for specific operations, inheriting all parent context identifiers.
    fn child_span(&self, name: &'static str) -> Span {
        tracing::info_span!(
            "adk_step",
            step.name = name,
            adk.agent.invocation_id = %self.invocation_id(),
            adk.agent.session_id = %self.session_id(),
            adk.agent.name = %self.agent_name(),
            adk.user.id = %self.user_id(),
            adk.app.name = %self.app_name(),
            adk.branch = %self.branch(),
            adk.skills.selected_name = tracing::field::Empty,
            adk.skills.selected_id = tracing::field::Empty,
            adk.tool.name = tracing::field::Empty,
            // Compatibility fields
            gcp.vertex.agent.invocation_id = %self.invocation_id(),
            gcp.vertex.agent.session_id = %self.session_id(),
            gcp.vertex.agent.tool_name = tracing::field::Empty,
            gen_ai.conversation.id = %self.session_id(),
        )
    }

    /// Create a specialized span for agent execution with high-fidelity attributes.
    fn agent_span(&self) -> Span {
        tracing::info_span!(
            "agent.execute",
            adk.agent.invocation_id = %self.invocation_id(),
            adk.agent.session_id = %self.session_id(),
            adk.agent.name = %self.agent_name(),
            adk.user.id = %self.user_id(),
            adk.app.name = %self.app_name(),
            adk.branch = %self.branch(),
            adk.skills.selected_name = tracing::field::Empty,
            adk.skills.selected_id = tracing::field::Empty,
            // Compatibility fields
            gcp.vertex.agent.invocation_id = %self.invocation_id(),
            gcp.vertex.agent.session_id = %self.session_id(),
            gcp.vertex.agent.event_id = %self.invocation_id(),
            gen_ai.conversation.id = %self.session_id(),
            agent.name = %self.agent_name(),
        )
    }
}

// Blanket implementation for all types implementing ReadonlyContext
impl<T: ReadonlyContext> TraceContextExt for T {}
