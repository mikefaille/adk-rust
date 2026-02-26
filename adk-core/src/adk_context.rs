use crate::context::ReadonlyContext;
use crate::types::Content;
use std::collections::HashMap;

/// A concrete, domain-focused implementation of `ReadonlyContext`.
///
/// This struct holds the foundational identifiers for an ADK execution (Invocation, Session, etc.)
/// without being tied to any specific observability framework.
///
/// It is the standard, lightweight context implementation for use cases where the full `Runner`
/// environment is not required (e.g., lightweight tools, simple agents, or tests).
///
/// # Extensibility
///
/// This struct is designed to be reusable and extendable. For example, high-fidelity observability
/// can be added by importing the `TraceContextExt` trait from `adk-telemetry`, which implements
/// tracing logic on top of any `ReadonlyContext`.
///
/// Tracing capabilities are provided as extension traits in `adk-telemetry`.
#[derive(Debug, Clone, Default)]
pub struct AdkContext {
    pub invocation_id: String,
    pub agent_name: String,
    pub user_id: String,
    pub app_name: String,
    pub session_id: String,
    pub branch: String,
    pub user_content: Content,
    /// Extensible metadata for any framework-specific attributes.
    pub metadata: HashMap<String, String>,
}

impl AdkContext {
    /// Create a new builder for `AdkContext`.
    pub fn builder() -> AdkContextBuilder {
        AdkContextBuilder::default()
    }
}

/// Fluent builder for `AdkContext` following Rust API guidelines.
#[derive(Debug, Clone, Default)]
pub struct AdkContextBuilder {
    invocation_id: Option<String>,
    agent_name: Option<String>,
    user_id: Option<String>,
    app_name: Option<String>,
    session_id: Option<String>,
    branch: Option<String>,
    user_content: Option<Content>,
    metadata: HashMap<String, String>,
}

impl AdkContextBuilder {
    pub fn invocation_id(mut self, id: impl Into<String>) -> Self {
        self.invocation_id = Some(id.into());
        self
    }

    pub fn agent_name(mut self, name: impl Into<String>) -> Self {
        self.agent_name = Some(name.into());
        self
    }

    pub fn user_id(mut self, id: impl Into<String>) -> Self {
        self.user_id = Some(id.into());
        self
    }

    pub fn app_name(mut self, name: impl Into<String>) -> Self {
        self.app_name = Some(name.into());
        self
    }

    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    pub fn branch(mut self, branch: impl Into<String>) -> Self {
        self.branch = Some(branch.into());
        self
    }

    pub fn user_content(mut self, content: impl Into<Content>) -> Self {
        self.user_content = Some(content.into());
        self
    }

    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn build(self) -> AdkContext {
        AdkContext {
            invocation_id: self.invocation_id.unwrap_or_default(),
            agent_name: self.agent_name.unwrap_or_default(),
            user_id: self.user_id.unwrap_or_else(|| "anonymous".to_string()),
            app_name: self.app_name.unwrap_or_else(|| "adk-app".to_string()),
            session_id: self.session_id.unwrap_or_default(),
            branch: self.branch.unwrap_or_else(|| "main".to_string()),
            user_content: self.user_content.unwrap_or_default(),
            metadata: self.metadata,
        }
    }
}

impl ReadonlyContext for AdkContext {
    fn invocation_id(&self) -> &str {
        &self.invocation_id
    }

    fn agent_name(&self) -> &str {
        &self.agent_name
    }

    fn user_id(&self) -> &str {
        &self.user_id
    }

    fn app_name(&self) -> &str {
        &self.app_name
    }

    fn session_id(&self) -> &str {
        &self.session_id
    }

    fn branch(&self) -> &str {
        &self.branch
    }

    fn user_content(&self) -> &Content {
        &self.user_content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adk_context_builder() {
        let ctx = AdkContext::builder()
            .invocation_id("inv-123")
            .agent_name("test-agent")
            .user_id("user-456")
            .session_id("sess-789")
            .metadata("custom.key", "custom-value")
            .build();

        assert_eq!(ctx.invocation_id, "inv-123");
        assert_eq!(ctx.agent_name, "test-agent");
        assert_eq!(ctx.user_id, "user-456");
        assert_eq!(ctx.session_id, "sess-789");
        assert_eq!(ctx.app_name, "adk-app"); // Default
        assert_eq!(ctx.metadata.get("custom.key").unwrap(), "custom-value");
    }
}
