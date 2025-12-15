//! Rust code generation from project schemas

use crate::schema::{ProjectSchema, AgentType, ToolConfig};
use anyhow::Result;

/// Generate a complete Rust project from a studio project
pub fn generate_rust_project(project: &ProjectSchema) -> Result<GeneratedProject> {
    let mut files = Vec::new();
    
    // Generate main.rs
    files.push(GeneratedFile {
        path: "src/main.rs".to_string(),
        content: generate_main_rs(project),
    });
    
    // Generate Cargo.toml
    files.push(GeneratedFile {
        path: "Cargo.toml".to_string(),
        content: generate_cargo_toml(project),
    });
    
    Ok(GeneratedProject { files })
}

#[derive(Debug, serde::Serialize)]
pub struct GeneratedProject {
    pub files: Vec<GeneratedFile>,
}

#[derive(Debug, serde::Serialize)]
pub struct GeneratedFile {
    pub path: String,
    pub content: String,
}

fn generate_main_rs(project: &ProjectSchema) -> String {
    let mut code = String::new();
    
    // Allow unused imports for flexibility
    code.push_str("#![allow(unused_imports)]\n\n");
    
    // Imports
    code.push_str("use adk_agent::{LlmAgentBuilder, SequentialAgent, LoopAgent, ParallelAgent};\n");
    code.push_str("use adk_core::ToolContext;\n");
    code.push_str("use adk_model::gemini::GeminiModel;\n");
    code.push_str("use adk_tool::{FunctionTool, GoogleSearchTool, ExitLoopTool, LoadArtifactsTool};\n");
    code.push_str("use anyhow::Result;\n");
    code.push_str("use serde_json::{json, Value};\n");
    code.push_str("use std::sync::Arc;\n\n");
    
    // Generate function tools
    for (agent_id, agent) in &project.agents {
        for tool_type in &agent.tools {
            if tool_type == "function" {
                let tool_id = format!("{}_{}", agent_id, tool_type);
                if let Some(ToolConfig::Function(config)) = project.tool_configs.get(&tool_id) {
                    code.push_str(&generate_function_tool(config));
                }
            }
        }
    }
    
    // Main function
    code.push_str("#[tokio::main]\n");
    code.push_str("async fn main() -> Result<()> {\n");
    code.push_str("    let api_key = std::env::var(\"GOOGLE_API_KEY\")\n");
    code.push_str("        .or_else(|_| std::env::var(\"GEMINI_API_KEY\"))\n");
    code.push_str("        .expect(\"GOOGLE_API_KEY or GEMINI_API_KEY must be set\");\n\n");
    
    // Find top-level agents (not sub-agents)
    let all_sub_agents: std::collections::HashSet<_> = project.agents.values()
        .flat_map(|a| a.sub_agents.iter().cloned())
        .collect();
    
    let top_level: Vec<_> = project.agents.keys()
        .filter(|id| !all_sub_agents.contains(*id))
        .collect();
    
    // Generate agents
    for agent_id in &top_level {
        if let Some(agent) = project.agents.get(*agent_id) {
            code.push_str(&generate_agent(agent_id, agent, project));
        }
    }
    
    // Run with console
    if let Some(first_agent) = top_level.first() {
        code.push_str(&format!("    adk_cli::console::run_console(\n"));
        code.push_str(&format!("        Arc::new({}_agent),\n", first_agent));
        code.push_str(&format!("        \"{}\".to_string(),\n", project.name));
        code.push_str("        \"user\".to_string(),\n");
        code.push_str("    ).await?;\n\n");
    }
    
    code.push_str("    Ok(())\n");
    code.push_str("}\n");
    
    code
}

fn generate_function_tool(config: &crate::schema::FunctionToolConfig) -> String {
    let mut code = String::new();
    let fn_name = &config.name;
    
    code.push_str(&format!("async fn {}_fn(_ctx: Arc<dyn ToolContext>, args: Value) -> Result<Value, adk_core::AdkError> {{\n", fn_name));
    
    // Extract parameters
    for param in &config.parameters {
        let extract = match param.param_type {
            crate::schema::ParamType::String => format!("    let {} = args[\"{}\"].as_str().unwrap_or(\"\");\n", param.name, param.name),
            crate::schema::ParamType::Number => format!("    let {} = args[\"{}\"].as_f64().unwrap_or(0.0);\n", param.name, param.name),
            crate::schema::ParamType::Boolean => format!("    let {} = args[\"{}\"].as_bool().unwrap_or(false);\n", param.name, param.name),
        };
        code.push_str(&extract);
    }
    
    code.push_str("\n    // TODO: Implement your function logic here\n");
    code.push_str("    Ok(json!({\n");
    code.push_str(&format!("        \"function\": \"{}\",\n", fn_name));
    code.push_str("        \"status\": \"success\"\n");
    code.push_str("    }))\n");
    code.push_str("}\n\n");
    
    code
}

fn generate_agent(id: &str, agent: &crate::schema::AgentSchema, project: &ProjectSchema) -> String {
    let mut code = String::new();
    let var_name = format!("{}_agent", id);
    
    match agent.agent_type {
        AgentType::Llm => {
            let model = agent.model.as_deref().unwrap_or("gemini-2.0-flash");
            code.push_str(&format!("    let {}_model = Arc::new(GeminiModel::new(&api_key, \"{}\")?);\n", id, model));
            code.push_str(&format!("    let {} = LlmAgentBuilder::new(\"{}\")\n", var_name, id));
            if !agent.instruction.is_empty() {
                let escaped = agent.instruction.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
                code.push_str(&format!("        .instruction(\"{}\")\n", escaped));
            }
            code.push_str(&format!("        .model({}_model)\n", id));
            
            // Add tools
            for tool_type in &agent.tools {
                match tool_type.as_str() {
                    "google_search" => code.push_str("        .tool(Arc::new(GoogleSearchTool::new()))\n"),
                    "exit_loop" => code.push_str("        .tool(Arc::new(ExitLoopTool::new()))\n"),
                    "load_artifact" => code.push_str("        .tool(Arc::new(LoadArtifactsTool::new()))\n"),
                    "function" => {
                        let tool_id = format!("{}_{}", id, tool_type);
                        if let Some(ToolConfig::Function(config)) = project.tool_configs.get(&tool_id) {
                            code.push_str(&format!("        .tool(Arc::new(FunctionTool::new(\"{}\", \"{}\", {}_fn)))\n", 
                                config.name, config.description.replace('"', "\\\""), config.name));
                        }
                    }
                    _ => {}
                }
            }
            code.push_str("        .build()?;\n\n");
        }
        AgentType::Sequential => {
            // Generate sub-agents first
            for sub_id in &agent.sub_agents {
                if let Some(sub) = project.agents.get(sub_id) {
                    code.push_str(&generate_agent(sub_id, sub, project));
                }
            }
            let subs: Vec<_> = agent.sub_agents.iter().map(|s| format!("Arc::new({}_agent)", s)).collect();
            code.push_str(&format!("    let {} = SequentialAgent::new(\"{}\", vec![{}]);\n\n", var_name, id, subs.join(", ")));
        }
        AgentType::Loop => {
            for sub_id in &agent.sub_agents {
                if let Some(sub) = project.agents.get(sub_id) {
                    code.push_str(&generate_agent(sub_id, sub, project));
                }
            }
            let subs: Vec<_> = agent.sub_agents.iter().map(|s| format!("Arc::new({}_agent)", s)).collect();
            let max_iter = agent.max_iterations.unwrap_or(3);
            code.push_str(&format!("    let {} = LoopAgent::new(\"{}\", vec![{}]).with_max_iterations({});\n\n", var_name, id, subs.join(", "), max_iter));
        }
        AgentType::Parallel => {
            for sub_id in &agent.sub_agents {
                if let Some(sub) = project.agents.get(sub_id) {
                    code.push_str(&generate_agent(sub_id, sub, project));
                }
            }
            let subs: Vec<_> = agent.sub_agents.iter().map(|s| format!("Arc::new({}_agent)", s)).collect();
            code.push_str(&format!("    let {} = ParallelAgent::new(\"{}\", vec![{}]);\n\n", var_name, id, subs.join(", ")));
        }
        _ => {}
    }
    
    code
}

fn generate_cargo_toml(project: &ProjectSchema) -> String {
    let name = project.name.to_lowercase().replace(' ', "_");
    format!(r#"[package]
name = "{}"
version = "0.1.0"
edition = "2021"

[dependencies]
adk-agent = "0.1"
adk-core = "0.1"
adk-model = "0.1"
adk-tool = "0.1"
adk-cli = "0.1"
tokio = {{ version = "1", features = ["full"] }}
anyhow = "1"
serde_json = "1"
"#, name)
}
