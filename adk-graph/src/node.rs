use crate::error::Result;
use crate::interrupt::Interrupt;
use crate::state::State;
use crate::stream::StreamEvent;
use adk_core::{Agent, types::{AdkIdentity, InvocationId, SessionId, UserId}};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Configuration passed to nodes during execution
#[derive(Clone)]
pub struct ExecutionConfig {
    /// Thread identifier for checkpointing
    pub thread_id: SessionId,
    /// Resume from a specific checkpoint
    pub resume_from: Option<String>,
    /// Recursion limit for cycles
    pub recursion_limit: usize,
    /// Additional configuration
    pub metadata: HashMap<String, Value>,
}

impl ExecutionConfig {
    /// Create a new config with the given thread ID
    pub fn new(thread_id: impl Into<SessionId>) -> Self {
        Self {
            thread_id: thread_id.into(),
            resume_from: None,
            recursion_limit: 50,
            metadata: HashMap::new(),
        }
    }

    /// Set the resume from checkpoint
    pub fn with_resume_from(mut self, checkpoint_id: &str) -> Self {
        self.resume_from = Some(checkpoint_id.to_string());
        self
    }

    /// Set the recursion limit
    pub fn with_recursion_limit(mut self, limit: usize) -> Self {
        self.recursion_limit = limit;
        self
    }
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self::new(uuid::Uuid::new_v4().to_string())
    }
}

/// Context passed to nodes during execution
pub struct NodeContext {
    /// Current graph state (read-only view)
    pub state: State,
    /// Configuration for this execution
    pub config: ExecutionConfig,
    /// Metadata map for dynamic attributes
    pub metadata: HashMap<String, String>,
}

impl NodeContext {
    pub fn new(state: State, config: ExecutionConfig) -> Self {
        Self { state, config, metadata: HashMap::new() }
    }
}

/// A node in the graph
#[async_trait]
pub trait Node: Send + Sync {
    /// Unique name of the node
    fn name(&self) -> &str;

    /// Execute the node logic
    async fn execute(&self, ctx: &NodeContext) -> Result<NodeResult>;
}

/// Result of node execution
#[derive(Debug, Clone)]
pub struct NodeResult {
    /// Next nodes to execute
    pub next: Vec<String>,
    /// Data to emit to the stream
    pub events: Vec<StreamEvent>,
}

impl NodeResult {
    /// Create a new result with no events
    pub fn next(name: impl Into<String>) -> Self {
        Self { next: vec![name.into()], events: Vec::new() }
    }

    /// Create a result that ends the current branch
    pub fn end() -> Self {
        Self { next: Vec::new(), events: Vec::new() }
    }

    /// Add an event to the result
    pub fn with_event(mut self, event: StreamEvent) -> Self {
        self.events.push(event);
        self
    }
}

/// Core graph orchestrator
pub struct Graph {
    nodes: HashMap<String, Arc<dyn Node>>,
    entry_point: String,
}

impl Graph {
    /// Create a new graph with an entry point
    pub fn new(entry_point: impl Into<String>) -> Self {
        Self { nodes: HashMap::new(), entry_point: entry_point.into() }
    }

    /// Add a node to the graph
    pub fn add_node(&mut self, node: Arc<dyn Node>) {
        self.nodes.insert(node.name().to_string(), node);
    }

    /// Get the entry point node name
    pub fn entry_point(&self) -> &str {
        &self.entry_point
    }

    /// Get a node by name
    pub fn get_node(&self, name: &str) -> Option<&Arc<dyn Node>> {
        self.nodes.get(name)
    }

    /// Returns true if the graph leads to END from the given nodes
    pub fn leads_to_end(&self, executed: &[String], _state: &State) -> bool {
        executed.is_empty()
    }

    /// Get next nodes to execute (placeholder for complex edge logic)
    pub fn get_next_nodes(&self, _executed: &[String], _state: &State) -> Vec<String> {
        Vec::new()
    }
}

/// Bridge between adk-graph and adk-core
struct GraphInvocationContext {
    identity: AdkIdentity,
    user_content: adk_core::Content,
    agent: Arc<dyn Agent>,
    session: Arc<GraphSession>,
    run_config: adk_core::RunConfig,
    ended: std::sync::atomic::AtomicBool,
    metadata: HashMap<String, String>,
}

impl GraphInvocationContext {
    fn new(
        session_id: SessionId,
        user_content: adk_core::Content,
        agent: Arc<dyn Agent>,
    ) -> Self {
        let mut identity = AdkIdentity::default();
        identity.invocation_id = InvocationId::from(uuid::Uuid::new_v4().to_string());
        identity.session_id = session_id.clone();
        identity.agent_name = agent.name().to_string();
        identity.app_name = "graph_app".to_string();
        identity.user_id = UserId::from("graph_user".to_string());

        let session = Arc::new(GraphSession::new(session_id));
        // Add user content to history
        session.append_content(user_content.clone());
        Self {
            identity,
            user_content,
            agent,
            session,
            run_config: adk_core::RunConfig::default(),
            ended: std::sync::atomic::AtomicBool::new(false),
            metadata: HashMap::new(),
        }
    }
}

// Implement ReadonlyContext (required by CallbackContext)
impl adk_core::ReadonlyContext for GraphInvocationContext {
    fn identity(&self) -> &AdkIdentity {
        &self.identity
    }

    fn user_content(&self) -> &adk_core::Content {
        &self.user_content
    }

    fn metadata(&self) -> &HashMap<String, String> {
        &self.metadata
    }
}

// Implement CallbackContext (required by InvocationContext)
#[async_trait]
impl adk_core::CallbackContext for GraphInvocationContext {
    fn artifacts(&self) -> Option<Arc<dyn adk_core::Artifacts>> {
        None
    }
}

// Implement InvocationContext
#[async_trait]
impl adk_core::InvocationContext for GraphInvocationContext {
    fn agent(&self) -> Arc<dyn Agent> {
        self.agent.clone()
    }

    fn memory(&self) -> Option<Arc<dyn adk_core::Memory>> {
        None
    }

    fn session(&self) -> &dyn adk_core::Session {
        self.session.as_ref()
    }

    fn run_config(&self) -> &adk_core::RunConfig {
        &self.run_config
    }

    fn end_invocation(&self) {
        self.ended.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn ended(&self) -> bool {
        self.ended.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Minimal Session implementation for graph execution
struct GraphSession {
    id: SessionId,
    state: GraphState,
    history: std::sync::RwLock<Vec<adk_core::Content>>,
}

impl GraphSession {
    fn new(id: SessionId) -> Self {
        Self { id, state: GraphState::new(), history: std::sync::RwLock::new(Vec::new()) }
    }

    fn append_content(&self, content: adk_core::Content) {
        if let Ok(mut h) = self.history.write() {
            h.push(content);
        }
    }
}

impl adk_core::Session for GraphSession {
    fn id(&self) -> &str {
        self.id.as_ref()
    }

    fn app_name(&self) -> &str {
        "graph_app"
    }

    fn user_id(&self) -> &str {
        "graph_user"
    }

    fn state(&self) -> &dyn adk_core::State {
        &self.state
    }

    fn conversation_history(&self) -> Vec<adk_core::Content> {
        self.history.read().ok().map(|h| h.clone()).unwrap_or_default()
    }

    fn append_to_history(&self, content: adk_core::Content) {
        self.append_content(content);
    }
}

/// Minimal State implementation for graph execution
struct GraphState {
    data: std::sync::RwLock<HashMap<String, Value>>,
}

impl GraphState {
    fn new() -> Self {
        Self { data: std::sync::RwLock::new(HashMap::new()) }
    }
}

impl adk_core::State for GraphState {
    fn get(&self, key: &str) -> Option<Value> {
        self.data.read().ok()?.get(key).cloned()
    }

    fn set(&mut self, key: String, value: Value) {
        if let Ok(mut data) = self.data.write() {
            data.insert(key, value);
        }
    }

    fn all(&self) -> HashMap<String, Value> {
        self.data.read().ok().map(|d| d.clone()).unwrap_or_default()
    }
}

pub struct FunctionNode {
    name: String,
    pub handler: Arc<dyn Fn(&NodeContext) -> Result<NodeResult> + Send + Sync>,
}

impl FunctionNode {
    pub fn new(name: impl Into<String>, handler: impl Fn(&NodeContext) -> Result<NodeResult> + Send + Sync + 'static) -> Self {
        Self { name: name.into(), handler: Arc::new(handler) }
    }
}

#[async_trait]
impl Node for FunctionNode {
    fn name(&self) -> &str { &self.name }
    async fn execute(&self, ctx: &NodeContext) -> Result<NodeResult> { (self.handler)(ctx) }
}

pub struct PassthroughNode { name: String }
impl PassthroughNode { pub fn new(name: impl Into<String>) -> Self { Self { name: name.into() } } }
#[async_trait]
impl Node for PassthroughNode {
    fn name(&self) -> &str { &self.name }
    async fn execute(&self, _ctx: &NodeContext) -> Result<NodeResult> { Ok(NodeResult::end()) }
}

pub struct AgentNode { name: String, agent: Arc<dyn Agent> }
impl AgentNode { pub fn new(name: impl Into<String>, agent: Arc<dyn Agent>) -> Self { Self { name: name.into(), agent } } }
#[async_trait]
impl Node for AgentNode {
    fn name(&self) -> &str { &self.name }
    async fn execute(&self, _ctx: &NodeContext) -> Result<NodeResult> { Ok(NodeResult::end()) }
}

pub struct NodeOutput { pub executed_nodes: Vec<String> }
