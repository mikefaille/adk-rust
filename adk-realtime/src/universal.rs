//! Universal Data Contracts and Execution Interface for ADK Realtime.
//!
//! This module defines the provider-agnostic boundary between AI Execution Engines
//! and internal logic (e.g. GatewayBridge, SurrealDB, n8n).

use crate::config::ToolDefinition;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The Universal Request (The AI asking to do something)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocationRequest {
    /// Unique ID to match async responses
    pub call_id: String,
    /// The name of the tool requested
    pub tool_name: String,
    /// The extracted JSON parameters from the AI
    pub arguments: Value,
}

/// The Universal Response (Rust giving the answer back)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocationResult {
    /// Unique ID matching the original request
    pub call_id: String,
    /// Did the execution succeed?
    pub success: bool,
    /// The actual data or error message
    pub result: Value,
}

/// The Universal Tool Interface.
///
/// Any tool in the system (local Rust code, database query, outbound webhook)
/// must implement this trait to interoperate across any AI provider (Gemini, OpenAI, etc.).
#[async_trait]
pub trait UniversalTool: Send + Sync {
    /// Returns the universal definition to be registered with the AI provider.
    fn definition(&self) -> ToolDefinition;

    /// Executes the actual business logic based on the generic request.
    async fn execute(&self, request: ToolInvocationRequest) -> ToolInvocationResult;
}
