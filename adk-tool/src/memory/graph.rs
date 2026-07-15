//! Knowledge-graph memory tools — agent-callable **write/curation** for the
//! bi-temporal graph in [`adk_memory::GraphMemoryService`].
//!
//! Recall is already covered by [`LoadMemoryTool`](super::LoadMemoryTool), which
//! works over any [`MemoryService`](adk_memory::MemoryService) — including
//! `GraphMemoryService`. What the base `MemoryService` trait can't express is
//! *structured writes*: creating entities, attaching observations, and relating
//! entities. These tools expose the graph's concrete inherent ops so an agent
//! can curate its own long-term memory:
//!
//! - [`RememberTool`] (`remember`) — save durable facts about an entity.
//! - [`RelateTool`] (`relate`) — record a typed relationship between entities.
//! - [`GraphMemoryToolset`] — bundles both, so an agent opts in with one line.
//!
//! All three are scoped to the calling invocation's `(app_name, user_id)` taken
//! from the [`ToolContext`], exactly like [`LoadMemoryTool`](super::LoadMemoryTool).
//!
//! # Feature gate
//!
//! Requires `graph-memory-tools` (which enables `memory-tools` and
//! `adk-memory/graph-memory`):
//!
//! ```toml
//! adk-tool = { version = "1.1", features = ["graph-memory-tools"] }
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use adk_tool::memory::GraphMemoryToolset;
//! use adk_memory::GraphMemoryService;
//! use std::sync::Arc;
//!
//! let kg = Arc::new(GraphMemoryService::new("sqlite:kg.db").await?);
//! kg.migrate().await?;
//! // Add the whole KG curation toolset to an agent:
//! let toolset = GraphMemoryToolset::new(kg);
//! ```

use std::collections::HashSet;
use std::sync::Arc;

use adk_core::{AdkError, ReadonlyContext, Result, Tool, ToolContext, Toolset};
use adk_memory::{CreateEntityInput, CreateRelationInput, GraphMemoryService};
use adk_schema::SchemaDocument;
use async_trait::async_trait;
use serde_json::{Value, json};

/// Default entity category used when the caller doesn't supply one.
const DEFAULT_ENTITY_TYPE: &str = "general";

// ─── remember ────────────────────────────────────────────────────────────────

/// A tool that saves durable facts about an entity into the knowledge graph.
///
/// Appends to the entity if it already exists (preserving its type); otherwise
/// creates it. Intended for stable facts — names, preferences, goals, life
/// events — not transient conversation.
pub struct RememberTool {
    kg: Arc<GraphMemoryService>,
}

impl RememberTool {
    /// Create a `remember` tool backed by the given graph.
    pub fn new(kg: Arc<GraphMemoryService>) -> Self {
        Self { kg }
    }
}

#[async_trait]
impl Tool for RememberTool {
    fn name(&self) -> &str {
        "remember"
    }

    fn description(&self) -> &str {
        "Save durable facts about an entity (e.g. the user) to long-term memory. \
         Use for stable, reusable facts — name spelling, preferences, goals, \
         relationships, life events — not small talk. Do not re-save facts you \
         already know. `entity` is what the facts are about (e.g. the user's \
         name); `facts` is one or more short statements."
    }

    fn parameters_schema(&self) -> Option<SchemaDocument> {
        Some(json!({
            "type": "object",
            "properties": {
                "entity": {
                    "type": "string",
                    "description": "The thing the facts are about (e.g. a person's name, a place, a topic)."
                },
                "entity_type": {
                    "type": "string",
                    "description": "Optional category for a new entity, e.g. \"person\", \"place\", \"preference\". Ignored if the entity already exists."
                },
                "facts": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "description": "One or more short factual statements to remember."
                }
            },
            "required": ["entity", "facts"]
        }))
    }

    async fn execute(&self, ctx: Arc<dyn ToolContext>, args: Value) -> Result<Value> {
        let entity = args
            .get("entity")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AdkError::tool("`entity` is required and must be a non-empty string"))?
            .to_string();

        let facts: Vec<String> = args
            .get("facts")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        if facts.is_empty() {
            return Err(AdkError::tool("`facts` must contain at least one non-empty string"));
        }

        let entity_type = args.get("entity_type").and_then(|v| v.as_str()).map(str::trim);
        let app = ctx.app_name();
        let user = ctx.user_id();

        if let Some(ty) = entity_type.filter(|s| !s.is_empty()) {
            // Explicit type: upsert the entity (sets/updates type) and append facts.
            self.kg
                .create_entities(
                    app,
                    user,
                    vec![CreateEntityInput {
                        name: entity.clone(),
                        entity_type: ty.to_string(),
                        observations: facts.clone(),
                    }],
                )
                .await?;
        } else {
            // No type given: prefer appending so an existing entity's type is
            // preserved; fall back to creating it (as "general") if unknown.
            if self.kg.add_observations(app, user, &entity, facts.clone()).await.is_err() {
                self.kg
                    .create_entities(
                        app,
                        user,
                        vec![CreateEntityInput {
                            name: entity.clone(),
                            entity_type: DEFAULT_ENTITY_TYPE.to_string(),
                            observations: facts.clone(),
                        }],
                    )
                    .await?;
            }
        }

        Ok(json!({ "stored": true, "entity": entity, "facts_added": facts.len() }))
    }
}

// ─── relate ──────────────────────────────────────────────────────────────────

/// A tool that records a typed relationship between two entities, creating any
/// that don't yet exist (without disturbing the type of those that do).
pub struct RelateTool {
    kg: Arc<GraphMemoryService>,
}

impl RelateTool {
    /// Create a `relate` tool backed by the given graph.
    pub fn new(kg: Arc<GraphMemoryService>) -> Self {
        Self { kg }
    }
}

#[async_trait]
impl Tool for RelateTool {
    fn name(&self) -> &str {
        "relate"
    }

    fn description(&self) -> &str {
        "Record a typed relationship between two entities in long-term memory, \
         e.g. source \"Shai\" relation \"located_in\" target \"Bay Area\". \
         Creates either entity if it does not exist yet. Use snake_case for the \
         relation type."
    }

    fn parameters_schema(&self) -> Option<SchemaDocument> {
        Some(json!({
            "type": "object",
            "properties": {
                "source": { "type": "string", "description": "The source entity name." },
                "relation": { "type": "string", "description": "The relation type, e.g. \"located_in\", \"works_at\", \"prefers\"." },
                "target": { "type": "string", "description": "The target entity name." }
            },
            "required": ["source", "relation", "target"]
        }))
    }

    async fn execute(&self, ctx: Arc<dyn ToolContext>, args: Value) -> Result<Value> {
        let field = |key: &str| -> Result<String> {
            args.get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| AdkError::tool(format!("`{key}` is required and must be non-empty")))
        };
        let source = field("source")?;
        let relation = field("relation")?;
        let target = field("target")?;

        let app = ctx.app_name();
        let user = ctx.user_id();

        // Create only the endpoints that don't already exist, so existing
        // entities keep their type/observations.
        let existing = self.kg.open_nodes(app, user, vec![source.clone(), target.clone()]).await?;
        let known: HashSet<&str> = existing.iter().map(|r| r.entity.name.as_str()).collect();
        let to_create: Vec<CreateEntityInput> = [&source, &target]
            .into_iter()
            .filter(|n| !known.contains(n.as_str()))
            .map(|n| CreateEntityInput {
                name: n.clone(),
                entity_type: DEFAULT_ENTITY_TYPE.to_string(),
                observations: vec![],
            })
            .collect();
        if !to_create.is_empty() {
            self.kg.create_entities(app, user, to_create).await?;
        }

        self.kg
            .create_relations(
                app,
                user,
                vec![CreateRelationInput {
                    source: source.clone(),
                    relation_type: relation.clone(),
                    target: target.clone(),
                }],
            )
            .await?;

        Ok(json!({ "related": true, "source": source, "relation": relation, "target": target }))
    }
}

// ─── toolset ─────────────────────────────────────────────────────────────────

/// Bundles the knowledge-graph curation tools ([`RememberTool`], [`RelateTool`])
/// so an agent can opt into KG memory writes in one line.
///
/// Pair with [`LoadMemoryTool`](super::LoadMemoryTool) for recall.
pub struct GraphMemoryToolset {
    kg: Arc<GraphMemoryService>,
}

impl GraphMemoryToolset {
    /// Create the toolset backed by the given graph.
    pub fn new(kg: Arc<GraphMemoryService>) -> Self {
        Self { kg }
    }
}

#[async_trait]
impl Toolset for GraphMemoryToolset {
    fn name(&self) -> &str {
        "graph_memory"
    }

    async fn tools(&self, _ctx: Arc<dyn ReadonlyContext>) -> Result<Vec<Arc<dyn Tool>>> {
        Ok(vec![
            Arc::new(RememberTool::new(self.kg.clone())),
            Arc::new(RelateTool::new(self.kg.clone())),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_core::{Content, EventActions, ReadonlyContext};
    use adk_schema::SchemaDocument;

    struct MockToolContext;

    #[async_trait]
    impl ReadonlyContext for MockToolContext {
        fn invocation_id(&self) -> &str {
            "inv-1"
        }
        fn agent_name(&self) -> &str {
            "test-agent"
        }
        fn user_id(&self) -> &str {
            "shai"
        }
        fn app_name(&self) -> &str {
            "test-app"
        }
        fn session_id(&self) -> &str {
            "session-1"
        }
        fn branch(&self) -> &str {
            ""
        }
        fn user_content(&self) -> &Content {
            // Leak a tiny Content once; fine for tests.
            static CONTENT: std::sync::OnceLock<Content> = std::sync::OnceLock::new();
            CONTENT.get_or_init(|| Content::new("user").with_text("hi"))
        }
    }

    #[async_trait]
    impl adk_core::CallbackContext for MockToolContext {
        fn artifacts(&self) -> Option<Arc<dyn adk_core::Artifacts>> {
            None
        }
    }

    #[async_trait]
    impl ToolContext for MockToolContext {
        fn function_call_id(&self) -> &str {
            "call-1"
        }
        fn actions(&self) -> EventActions {
            EventActions::default()
        }
        fn set_actions(&self, _actions: EventActions) {}
        async fn search_memory(&self, _query: &str) -> Result<Vec<adk_core::MemoryEntry>> {
            Ok(vec![])
        }
    }

    async fn kg() -> Arc<GraphMemoryService> {
        let kg = GraphMemoryService::new("sqlite::memory:").await.unwrap();
        kg.migrate().await.unwrap();
        Arc::new(kg)
    }

    fn ctx() -> Arc<dyn ToolContext> {
        Arc::new(MockToolContext) as Arc<dyn ToolContext>
    }

    #[tokio::test]
    async fn metadata_and_schema() {
        let tool = RememberTool::new(kg().await);
        assert_eq!(tool.name(), "remember");
        let schema = tool.parameters_schema().unwrap();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["entity"].is_object());
        assert!(schema["required"].as_array().unwrap().contains(&json!("entity")));
        assert!(schema["required"].as_array().unwrap().contains(&json!("facts")));
    }

    #[tokio::test]
    async fn remember_creates_entity_with_type_and_facts() {
        let kg = kg().await;
        let tool = RememberTool::new(kg.clone());
        let out = tool
            .execute(
                ctx(),
                json!({
                    "entity": "Shai",
                    "entity_type": "person",
                    "facts": ["Name is spelled S-H-A-I", "Relocated to the Bay Area"]
                }),
            )
            .await
            .unwrap();
        assert_eq!(out["stored"], true);
        assert_eq!(out["facts_added"], 2);

        let hits = kg.search_nodes("test-app", "shai", "Shai", 5).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entity.entity_type, "person");
        assert_eq!(hits[0].entity.observations.len(), 2);
    }

    #[tokio::test]
    async fn remember_without_type_preserves_existing_type() {
        let kg = kg().await;
        let tool = RememberTool::new(kg.clone());
        // Seed with an explicit type.
        tool.execute(ctx(), json!({"entity": "Shai", "entity_type": "person", "facts": ["a"]}))
            .await
            .unwrap();
        // Append without a type — must not clobber "person" back to "general".
        tool.execute(ctx(), json!({"entity": "Shai", "facts": ["b"]})).await.unwrap();

        let hits = kg.open_nodes("test-app", "shai", vec!["Shai".into()]).await.unwrap();
        assert_eq!(hits[0].entity.entity_type, "person");
        assert_eq!(hits[0].entity.observations.len(), 2);
    }

    #[tokio::test]
    async fn remember_rejects_empty_facts() {
        let kg = kg().await;
        let tool = RememberTool::new(kg);
        assert!(tool.execute(ctx(), json!({"entity": "Shai", "facts": []})).await.is_err());
        assert!(tool.execute(ctx(), json!({"entity": "", "facts": ["x"]})).await.is_err());
    }

    #[tokio::test]
    async fn relate_creates_missing_endpoints_and_preserves_existing_type() {
        let kg = kg().await;
        let remember = RememberTool::new(kg.clone());
        // Shai exists as a person.
        remember
            .execute(ctx(), json!({"entity": "Shai", "entity_type": "person", "facts": ["x"]}))
            .await
            .unwrap();

        let relate = RelateTool::new(kg.clone());
        let out = relate
            .execute(
                ctx(),
                json!({"source": "Shai", "relation": "located_in", "target": "Bay Area"}),
            )
            .await
            .unwrap();
        assert_eq!(out["related"], true);

        let (entities, relations) = kg.read_graph("test-app", "shai").await.unwrap();
        // Shai kept "person"; Bay Area was created as "general".
        let shai = entities.iter().find(|e| e.name == "Shai").unwrap();
        assert_eq!(shai.entity_type, "person");
        assert!(entities.iter().any(|e| e.name == "Bay Area" && e.entity_type == "general"));
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].relation_type, "located_in");
    }

    #[tokio::test]
    async fn toolset_exposes_remember_and_relate() {
        let kg = kg().await;
        let toolset = GraphMemoryToolset::new(kg);
        let ctx = Arc::new(MockToolContext) as Arc<dyn ReadonlyContext>;
        let tools = toolset.tools(ctx).await.unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"remember"));
        assert!(names.contains(&"relate"));
    }
}
