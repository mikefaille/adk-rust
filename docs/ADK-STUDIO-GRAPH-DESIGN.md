# ADK Studio Graph-Based Workflow Design

## Overview

This document describes the integration of `adk-graph` into ADK Studio for deterministic, visual workflow orchestration.

## Design Principles

1. **Keep native agent types** - Sequential, Loop, Parallel agents retain their container semantics
2. **Router uses conditional edges** - Leverages adk-graph's `conditional_edge` for branching
3. **Edges are explicit** - Auto-generated from workflow, manually editable
4. **Generated code uses GraphAgent** - For workflows with routing; simple agents use direct execution

## Agent Types

| Type | Implementation | Use Case |
|------|---------------|----------|
| LLM Agent | `LlmAgent` | Single agent with tools |
| Sequential | `SequentialAgent` | Run sub-agents in order |
| Loop | `LoopAgent` | Repeat sub-agents N times |
| Parallel | `ParallelAgent` | Run sub-agents concurrently |
| Router | `GraphAgent` + `conditional_edge` | Route based on LLM classification |

## Router Agent Design

### Schema
```rust
struct AgentSchema {
    agent_type: AgentType::Router,
    model: String,           // LLM for classification
    instruction: String,     // "Classify as: billing, support, general"
    routes: Vec<Route>,      // condition -> target mappings
}

struct Route {
    condition: String,       // "billing", "support", "default"
    target: String,          // agent_id or "END"
}
```

### Generated Code Pattern
```rust
let graph = GraphAgent::builder("router")
    .node(AgentNode::new(classifier_agent))
    .node(AgentNode::new(billing_agent))
    .node(AgentNode::new(support_agent))
    .edge(START, "classifier")
    .conditional_edge(
        "classifier",
        |state| {
            let response = state.get("messages").last();
            // Match against route conditions
            if response.contains("billing") { "billing" }
            else if response.contains("support") { "support" }
            else { END }
        },
        [("billing", "billing"), ("support", "support"), (END, END)],
    )
    .edge("billing", END)
    .edge("support", END)
    .build()?;
```

## Edge Management

### Auto-Generation Rules
1. START → first agent (or router)
2. Agent → END (unless router with explicit routes)
3. Router → targets defined in routes

### Manual Override
- User can drag edges in canvas
- Edge properties panel for conditions
- Delete edges with backspace/delete

## Runtime Execution

### Simple Workflows (no router)
Use existing runtime compiler with native agents.

### Workflows with Router
Compile to `GraphAgent` and execute:

```rust
pub async fn run_project(project: &ProjectSchema) -> Result<String> {
    if has_router(project) {
        run_as_graph(project).await
    } else {
        run_native(project).await  // existing implementation
    }
}
```

## Code Generation

### Without Router
```rust
let agent = LlmAgentBuilder::new("assistant")
    .instruction("...")
    .model(model)
    .build()?;

adk_cli::console::run_console(Arc::new(agent), ...).await?;
```

### With Router
```rust
let graph = GraphAgent::builder("workflow")
    .node(...)
    .edge(...)
    .conditional_edge(...)
    .build()?;

adk_cli::console::run_console(Arc::new(graph), ...).await?;
```

## Implementation Plan

### Phase 1: Router Compilation ✓
- [x] Add Router agent type to schema
- [x] Add routes field to AgentSchema
- [x] Router node rendering in canvas
- [x] Router properties panel

### Phase 2: Graph Code Generation
- [ ] Update codegen to detect router agents
- [ ] Generate GraphAgent code for router workflows
- [ ] Generate conditional_edge based on routes

### Phase 3: Runtime Graph Execution
- [ ] Add graph compilation to runtime
- [ ] Execute router workflows with GraphAgent
- [ ] Stream graph execution events

### Phase 4: Edge UI Enhancements
- [ ] Show condition labels on edges
- [ ] Edge click to edit properties
- [ ] Visual distinction for conditional edges

## Example Workflow

```
┌─────────┐
│  START  │
└────┬────┘
     │
     ▼
┌─────────────┐
│  🔀 Router  │ "Classify: billing, support, general"
└──┬───┬───┬──┘
   │   │   │
   │   │   └──────────────┐
   │   │                  │
   ▼   ▼                  ▼
┌─────┐ ┌─────────┐  ┌─────────┐
│ END │ │ Billing │  │ Support │
└─────┘ └────┬────┘  └────┬────┘
             │            │
             ▼            ▼
          ┌─────┐     ┌─────┐
          │ END │     │ END │
          └─────┘     └─────┘
```

## References

- [adk-graph README](/adk-graph/README.md)
- [Router example](/examples/graph_conditional/main.rs)
- [LangGraph concepts](https://langchain-ai.github.io/langgraph/)
