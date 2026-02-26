use crate::{
    Agent, Result,
    types::{AdkIdentity, Content, InvocationId, SessionId, UserId},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

/// Foundation for all ADK contexts.
pub trait ReadonlyContext: Send + Sync {
    /// Returns the consolidated identity capsule for this context.
    fn identity(&self) -> &AdkIdentity;

    /// Convenience: returns the invocation ID.
    fn invocation_id(&self) -> &InvocationId { &self.identity().invocation_id }
    
    /// Convenience: returns the session ID.
    fn session_id(&self) -> &SessionId { &self.identity().session_id }
    
    /// Convenience: returns the user ID.
    fn user_id(&self) -> &UserId { &self.identity().user_id }
    
    /// Convenience: returns the app name.
    fn app_name(&self) -> &str { &self.identity().app_name }
    
    /// Convenience: returns the branch name.
    fn branch(&self) -> &str { &self.identity().branch }
    
    /// Convenience: returns the agent name.
    fn agent_name(&self) -> &str { &self.identity().agent_name }

    /// Returns the initial user content that triggered this context.
    fn user_content(&self) -> &Content;

    /// Returns the metadata map for platform-specific identifiers.
    fn metadata(&self) -> &HashMap<String, String>;
}

// Manual delegation for Arc to ensure we don't hit recursive trait issues
impl<T: ?Sized + ReadonlyContext> ReadonlyContext for Arc<T> {
    fn identity(&self) -> &AdkIdentity {
        (**self).identity()
    }
    fn user_content(&self) -> &Content {
        (**self).user_content()
    }
    fn metadata(&self) -> &HashMap<String, String> {
        (**self).metadata()
    }
}

/// A concrete, domain-focused implementation of `ReadonlyContext`.
#[derive(Debug, Clone, Default)]
pub struct AdkContext {
    identity: AdkIdentity,
    user_content: Content,
    metadata: HashMap<String, String>,
}

impl AdkContext {
    pub fn builder() -> AdkContextBuilder {
        AdkContextBuilder::default()
    }

    pub fn set_branch(&mut self, branch: impl Into<String>) {
        self.identity.branch = branch.into();
    }
}

/// Fluent builder for `AdkContext`.
#[derive(Debug, Clone, Default)]
pub struct AdkContextBuilder {
    identity: AdkIdentity,
    user_content: Option<Content>,
    metadata: HashMap<String, String>,
}

impl AdkContextBuilder {
    pub fn invocation_id(mut self, id: impl Into<InvocationId>) -> Self {
        self.identity.invocation_id = id.into();
        self
    }

    pub fn agent_name(mut self, name: impl Into<String>) -> Self {
        self.identity.agent_name = name.into();
        self
    }

    pub fn user_id(mut self, id: impl Into<UserId>) -> Self {
        self.identity.user_id = id.into();
        self
    }

    pub fn app_name(mut self, name: impl Into<String>) -> Self {
        self.identity.app_name = name.into();
        self
    }

    pub fn session_id(mut self, id: impl Into<SessionId>) -> Self {
        self.identity.session_id = id.into();
        self
    }

    pub fn branch(mut self, branch: impl Into<String>) -> Self {
        self.identity.branch = branch.into();
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
            identity: self.identity,
            user_content: self.user_content.unwrap_or_default(),
            metadata: self.metadata,
        }
    }
}

impl ReadonlyContext for AdkContext {
    fn identity(&self) -> &AdkIdentity {
        &self.identity
    }

    fn user_content(&self) -> &Content {
        &self.user_content
    }

    fn metadata(&self) -> &HashMap<String, String> {
        &self.metadata
    }
}

// State management traits
pub const MAX_STATE_KEY_LEN: usize = 256;

pub fn validate_state_key(key: &str) -> std::result::Result<(), &'static str> {
    if key.is_empty() { return Err("state key must not be empty"); }
    if key.len() > MAX_STATE_KEY_LEN { return Err("state key exceeds maximum length"); }
    if key.contains('/') || key.contains('\\') || key.contains("..") { return Err("state key must not contain path separators"); }
    if key.contains('\0') { return Err("state key must not contain null bytes"); }
    Ok(())
}

pub trait State: Send + Sync {
    fn get(&self, key: &str) -> Option<Value>;
    fn set(&mut self, key: String, value: Value);
    fn all(&self) -> HashMap<String, Value>;
}

pub trait ReadonlyState: Send + Sync {
    fn get(&self, key: &str) -> Option<Value>;
    fn all(&self) -> HashMap<String, Value>;
}

pub trait Session: Send + Sync {
    fn id(&self) -> &str;
    fn app_name(&self) -> &str;
    fn user_id(&self) -> &str;
    fn state(&self) -> &dyn State;
    fn conversation_history(&self) -> Vec<Content>;
    fn append_to_history(&self, _content: Content) {}
}

#[async_trait]
pub trait CallbackContext: ReadonlyContext {
    fn artifacts(&self) -> Option<Arc<dyn Artifacts>>;
}

#[async_trait]
pub trait InvocationContext: CallbackContext {
    fn agent(&self) -> Arc<dyn Agent>;
    fn memory(&self) -> Option<Arc<dyn Memory>>;
    fn session(&self) -> &dyn Session;
    fn run_config(&self) -> &RunConfig;
    fn end_invocation(&self);
    fn ended(&self) -> bool;
}

#[async_trait]
pub trait Artifacts: Send + Sync {
    async fn save(&self, name: &str, data: &crate::Part) -> Result<i64>;
    async fn load(&self, name: &str) -> Result<crate::Part>;
    async fn list(&self) -> Result<Vec<String>>;
}

#[async_trait]
pub trait Memory: Send + Sync {
    async fn search(&self, query: &str) -> Result<Vec<MemoryEntry>>;
}

#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub content: Content,
    pub author: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamingMode {
    None,
    #[default]
    SSE,
    Bidi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IncludeContents {
    None,
    #[default]
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolConfirmationDecision {
    Approve,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolConfirmationPolicy {
    #[default]
    Never,
    Always,
    PerTool(BTreeSet<String>),
}

impl ToolConfirmationPolicy {
    pub fn requires_confirmation(&self, tool_name: &str) -> bool {
        match self {
            Self::Never => false,
            Self::Always => true,
            Self::PerTool(tools) => tools.contains(tool_name),
        }
    }

    pub fn with_tool(mut self, tool_name: impl Into<String>) -> Self {
        let tool_name = tool_name.into();
        match &mut self {
            Self::Never => {
                let mut tools = BTreeSet::new();
                tools.insert(tool_name);
                Self::PerTool(tools)
            }
            Self::Always => Self::Always,
            Self::PerTool(tools) => {
                tools.insert(tool_name);
                self
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolConfirmationRequest {
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call_id: Option<String>,
    pub args: Value,
}

#[derive(Debug, Clone)]
pub struct RunConfig {
    pub streaming_mode: StreamingMode,
    pub tool_confirmation_decisions: HashMap<String, ToolConfirmationDecision>,
    pub cached_content: Option<String>,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            streaming_mode: StreamingMode::SSE,
            tool_confirmation_decisions: HashMap::new(),
            cached_content: None,
        }
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

        let id = ctx.identity();
        assert_eq!(id.invocation_id.as_ref(), "inv-123");
        assert_eq!(id.agent_name, "test-agent");
        assert_eq!(id.user_id.as_ref(), "user-456");
        assert_eq!(id.session_id.as_ref(), "sess-789");
        assert_eq!(ctx.metadata().get("custom.key").unwrap(), "custom-value");
    }
}
