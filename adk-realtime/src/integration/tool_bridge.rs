//! # Tool Bridge Adapter
//!
//! Bridges `adk_core::Tool` implementations to the `ToolHandler` trait used by
//! `RealtimeRunner`. Also provides the `ToolContextFactory` trait for creating
//! per-invocation `ToolContext` instances scoped to the current session.

use std::sync::Arc;

use adk_core::{Tool, ToolContext};
use async_trait::async_trait;
use serde_json::Value;

use crate::config::ToolDefinition;
use crate::error::Result;
use crate::events::ToolCall;
use crate::runner::ToolHandler;

use super::context::ToolContextFactory;

/// Wraps an `Arc<dyn adk_core::Tool>` and implements [`ToolHandler`] for use
/// with the `RealtimeRunner`.
///
/// This adapter bridges the ADK tool interface to the realtime runner's
/// tool execution interface, handling context creation and error serialization.
///
/// # Example
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use adk_realtime::integration::tool_bridge::ToolBridgeAdapter;
///
/// let adapter = ToolBridgeAdapter::new(tool, context_factory);
/// // Register with RealtimeRunner as a ToolHandler
/// ```
pub struct ToolBridgeAdapter {
    tool: Arc<dyn Tool>,
    context_factory: Arc<dyn ToolContextFactory>,
}

impl ToolBridgeAdapter {
    /// Creates a new `ToolBridgeAdapter` wrapping the given tool and context factory.
    ///
    /// # Arguments
    ///
    /// * `tool` - The ADK tool to bridge.
    /// * `context_factory` - Factory for creating per-invocation `ToolContext` instances.
    pub fn new(tool: Arc<dyn Tool>, context_factory: Arc<dyn ToolContextFactory>) -> Self {
        Self { tool, context_factory }
    }

    /// Extract a [`ToolDefinition`] from an ADK [`Tool`] for registering with the
    /// realtime provider.
    ///
    /// Maps the tool's `name()`, `description()`, and `parameters_schema()` to the
    /// realtime `ToolDefinition` format.
    ///
    /// # Arguments
    ///
    /// * `tool` - The ADK tool to extract the definition from.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let def = ToolBridgeAdapter::definition(my_tool.as_ref());
    /// assert_eq!(def.name, "my_tool");
    /// ```
    pub fn definition(tool: &dyn Tool) -> ToolDefinition {
        ToolDefinition {
            name: tool.name().to_string(),
            description: Some(tool.description().to_string()),
            parameters: tool.parameters_schema(),
            behavior: None,
        }
    }
}

#[async_trait]
impl ToolHandler for ToolBridgeAdapter {
    async fn execute(&self, call: &ToolCall) -> Result<Value> {
        let ctx: Arc<dyn ToolContext> = self.context_factory.create_context(&call.call_id);
        match self.tool.execute(ctx, call.arguments.clone()).await {
            Ok(value) => Ok(value),
            Err(e) => Ok(serde_json::json!({ "error": e.to_string() })),
        }
    }
}
