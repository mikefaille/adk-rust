//! Universal Data Contracts and Execution Interface for ADK Realtime.
//!
//! This module acts as an Anti-Corruption Layer (ACL), strictly defining the provider-agnostic
//! boundary between AI Execution Engines and internal application logic (e.g. GatewayBridge,
//! SurrealDB, n8n webhooks).
//!
//! By enforcing these primitives, the execution core never couples with volatile external LLM APIs
//! (like specific Gemini or OpenAI protobufs), achieving zero vendor lock-in. Internal business domains
//! can define rich context metadata (such as access rules or UI elements) and flatten them
//! exclusively into `ToolDefinition` right before crossing this interface.
//!
//! ## Architectural Benefits
//! - **Concurrency Safe (`Send + Sync`)**: Tools are guaranteed thread-safe. They can be safely
//!   wrapped in `Arc<dyn UniversalTool>` and distributed across complex ingress and egress tokio actors.
//! - **Agnostic Serialization (`serde_json::Value`)**: Accommodates LLM structural hallucinations.
//!   Implementations can gracefully parse dynamic JSON inputs without edge-crashing rigid deserializers.
//! - **Asynchronous Lifecycles**: The `ToolInvocationRequest` uniquely pairs with a matching `call_id`.
//!   A gateway can issue long-running queries concurrently while audio continues to pump, pushing
//!   the final `ToolInvocationResult` upon execution resolution.

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

    /// Executes the business logic asynchronously.
    ///
    /// Returns `Ok(Value)` for a successful business operation.
    /// Returns `Err(String)` for an execution failure.
    ///
    /// Note: The Adapter layer is expected to catch the standard Rust `Result` and map it into
    /// a flat `ToolInvocationResult` containing a `success` boolean and standard JSON payload,
    /// allowing the LLM to gracefully apologize to the user or retry.
    async fn execute(&self, request: ToolInvocationRequest) -> std::result::Result<Value, String>;
}
