use crate::schema::{AgentSchema, AgentType, ProjectSchema};
use adk_agent::{Agent, LlmAgentBuilder, LoopAgent, ParallelAgent, SequentialAgent};
use adk_core::Tool;
use adk_model::gemini::GeminiModel;
use adk_tool::{ExitLoopTool, GoogleSearchTool, LoadArtifactsTool};
use anyhow::{anyhow, Result};
use std::sync::Arc;

/// Compile an AgentSchema into a runnable Agent
pub fn compile_agent(name: &str, schema: &AgentSchema, api_key: &str, project: &ProjectSchema) -> Result<Arc<dyn Agent>> {
    match schema.agent_type {
        AgentType::Llm => compile_llm_agent(name, schema, api_key),
        AgentType::Sequential => compile_sequential_agent(name, schema, api_key, project),
        AgentType::Loop => compile_loop_agent(name, schema, api_key, project),
        AgentType::Parallel => compile_parallel_agent(name, schema, api_key, project),
        _ => Err(anyhow!("Agent type {:?} not yet supported", schema.agent_type)),
    }
}

fn compile_llm_agent(name: &str, schema: &AgentSchema, api_key: &str) -> Result<Arc<dyn Agent>> {
    let model_name = schema.model.as_deref().unwrap_or("gemini-2.0-flash");
    let model = Arc::new(GeminiModel::new(api_key, model_name)?);
    let mut builder = LlmAgentBuilder::new(name).model(model);
    
    if !schema.instruction.is_empty() {
        builder = builder.instruction(&schema.instruction);
    }
    
    // Add tools
    for tool_type in &schema.tools {
        if let Some(tool) = compile_tool(tool_type) {
            builder = builder.tool(tool);
        }
    }
    
    Ok(Arc::new(builder.build()?))
}

fn compile_tool(tool_type: &str) -> Option<Arc<dyn Tool>> {
    match tool_type {
        "google_search" => Some(Arc::new(GoogleSearchTool::new())),
        "exit_loop" => Some(Arc::new(ExitLoopTool::new())),
        "load_artifact" => Some(Arc::new(LoadArtifactsTool::new())),
        // These require more setup, skip for now
        "function" | "mcp" | "browser" => None,
        _ => None,
    }
}

fn compile_sequential_agent(name: &str, schema: &AgentSchema, api_key: &str, project: &ProjectSchema) -> Result<Arc<dyn Agent>> {
    let sub_agents = compile_sub_agents(schema, api_key, project)?;
    Ok(Arc::new(SequentialAgent::new(name, sub_agents)))
}

fn compile_loop_agent(name: &str, schema: &AgentSchema, api_key: &str, project: &ProjectSchema) -> Result<Arc<dyn Agent>> {
    let sub_agents = compile_sub_agents(schema, api_key, project)?;
    let max_iter = schema.max_iterations.unwrap_or(3);
    Ok(Arc::new(LoopAgent::new(name, sub_agents).with_max_iterations(max_iter)))
}

fn compile_parallel_agent(name: &str, schema: &AgentSchema, api_key: &str, project: &ProjectSchema) -> Result<Arc<dyn Agent>> {
    let sub_agents = compile_sub_agents(schema, api_key, project)?;
    Ok(Arc::new(ParallelAgent::new(name, sub_agents)))
}

fn compile_sub_agents(schema: &AgentSchema, api_key: &str, project: &ProjectSchema) -> Result<Vec<Arc<dyn Agent>>> {
    let mut sub_agents: Vec<Arc<dyn Agent>> = Vec::new();
    for sub_id in &schema.sub_agents {
        let sub_schema = project.agents.get(sub_id)
            .ok_or_else(|| anyhow!("Sub-agent {} not found", sub_id))?;
        sub_agents.push(compile_agent(sub_id, sub_schema, api_key, project)?);
    }
    if sub_agents.is_empty() {
        return Err(anyhow!("Container agent has no sub-agents"));
    }
    Ok(sub_agents)
}
