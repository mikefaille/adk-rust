//! Rust code generation from project schemas

use crate::schema::{ProjectSchema, AgentType, ToolConfig, AgentSchema};
use anyhow::Result;

/// Generate a complete Rust project from a studio project
pub fn generate_rust_project(project: &ProjectSchema) -> Result<GeneratedProject> {
    let mut files = Vec::new();
    
    files.push(GeneratedFile {
        path: "src/main.rs".to_string(),
        content: generate_main_rs(project),
    });
    
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

fn has_router(project: &ProjectSchema) -> bool {
    project.agents.values().any(|a| a.agent_type == AgentType::Router)
}

fn generate_main_rs(project: &ProjectSchema) -> String {
    if has_router(project) {
        generate_graph_main(project)
    } else {
        generate_simple_main(project)
    }
}

fn generate_graph_main(project: &ProjectSchema) -> String {
    let mut code = String::new();
    
    code.push_str("#![allow(unused_imports)]\n\n");
    
    // Graph imports
    code.push_str("use adk_agent::LlmAgentBuilder;\n");
    code.push_str("use adk_core::ToolContext;\n");
    code.push_str("use adk_graph::{\n");
    code.push_str("    edge::{Router, END, START},\n");
    code.push_str("    graph::StateGraph,\n");
    code.push_str("    node::{AgentNode, ExecutionConfig, NodeOutput},\n");
    code.push_str("    state::State,\n");
    code.push_str("};\n");
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
    
    code.push_str("#[tokio::main]\n");
    code.push_str("async fn main() -> Result<()> {\n");
    code.push_str("    let api_key = std::env::var(\"GOOGLE_API_KEY\")\n");
    code.push_str("        .or_else(|_| std::env::var(\"GEMINI_API_KEY\"))\n");
    code.push_str("        .expect(\"GOOGLE_API_KEY or GEMINI_API_KEY must be set\");\n\n");
    code.push_str("    let model = Arc::new(GeminiModel::new(&api_key, \"gemini-2.0-flash\")?);\n\n");
    
    // Find all agents excluding sub-agents
    let all_sub_agents: std::collections::HashSet<_> = project.agents.values()
        .flat_map(|a| a.sub_agents.iter().cloned())
        .collect();
    let top_level: Vec<_> = project.agents.keys()
        .filter(|id| !all_sub_agents.contains(*id))
        .collect();
    
    // Generate LLM agents
    for agent_id in &top_level {
        if let Some(agent) = project.agents.get(*agent_id) {
            if agent.agent_type == AgentType::Llm {
                code.push_str(&generate_llm_agent_for_graph(agent_id, agent, project));
            }
        }
    }
    
    // Find router and its routes
    let router = project.agents.iter()
        .find(|(_, a)| a.agent_type == AgentType::Router);
    
    if let Some((router_id, router_agent)) = router {
        // Generate router agent
        code.push_str(&generate_router_agent(router_id, router_agent));
        
        // Build graph
        code.push_str("    // Build the graph\n");
        code.push_str("    let graph = StateGraph::with_channels(&[\"message\", \"classification\", \"response\"])\n");
        
        // Add router node
        code.push_str(&format!("        .add_node({}_node)\n", router_id));
        
        // Add target agent nodes
        for route in &router_agent.routes {
            if route.target != "END" && project.agents.contains_key(&route.target) {
                code.push_str(&format!("        .add_node({}_node)\n", route.target));
            }
        }
        
        // Add edges
        code.push_str(&format!("        .add_edge(START, \"{}\")\n", router_id));
        
        // Add conditional edges from router
        let conditions: Vec<String> = router_agent.routes.iter()
            .map(|r| {
                let target = if r.target == "END" { "END".to_string() } else { format!("\"{}\"", r.target) };
                format!("(\"{}\", {})", r.condition, target)
            })
            .collect();
        
        code.push_str(&format!("        .add_conditional_edges(\n"));
        code.push_str(&format!("            \"{}\",\n", router_id));
        code.push_str("            Router::by_field(\"classification\"),\n");
        code.push_str(&format!("            [{}],\n", conditions.join(", ")));
        code.push_str("        )\n");
        
        // Add edges from targets to END
        for route in &router_agent.routes {
            if route.target != "END" && project.agents.contains_key(&route.target) {
                code.push_str(&format!("        .add_edge(\"{}\", END)\n", route.target));
            }
        }
        
        code.push_str("        .compile()?;\n\n");
        
        // Run loop
        code.push_str("    // Interactive loop\n");
        code.push_str("    println!(\"Graph workflow ready. Type your message:\");\n");
        code.push_str("    let stdin = std::io::stdin();\n");
        code.push_str("    let mut input = String::new();\n");
        code.push_str("    let mut turn = 0;\n");
        code.push_str("    loop {\n");
        code.push_str("        input.clear();\n");
        code.push_str("        print!(\"> \");\n");
        code.push_str("        use std::io::Write;\n");
        code.push_str("        std::io::stdout().flush()?;\n");
        code.push_str("        stdin.read_line(&mut input)?;\n");
        code.push_str("        let msg = input.trim();\n");
        code.push_str("        if msg.is_empty() || msg == \"quit\" { break; }\n\n");
        code.push_str("        let mut state = State::new();\n");
        code.push_str("        state.insert(\"message\".to_string(), json!(msg));\n");
        code.push_str("        let result = graph.invoke(state, ExecutionConfig::new(&format!(\"turn-{}\", turn))).await?;\n");
        code.push_str("        turn += 1;\n\n");
        code.push_str("        if let Some(response) = result.get(\"response\").and_then(|v| v.as_str()) {\n");
        code.push_str("            println!(\"\\n{}\\n\", response);\n");
        code.push_str("        }\n");
        code.push_str("    }\n\n");
    }
    
    code.push_str("    Ok(())\n");
    code.push_str("}\n");
    
    code
}

fn generate_router_agent(id: &str, agent: &AgentSchema) -> String {
    let mut code = String::new();
    let model = agent.model.as_deref().unwrap_or("gemini-2.0-flash");
    
    // Create the classifier agent
    code.push_str(&format!("    // Router: {}\n", id));
    code.push_str(&format!("    let {}_llm = Arc::new(\n", id));
    code.push_str(&format!("        LlmAgentBuilder::new(\"{}\")\n", id));
    code.push_str(&format!("            .model(Arc::new(GeminiModel::new(&api_key, \"{}\")?))\n", model));
    
    // Build instruction with route options
    let route_options: Vec<&str> = agent.routes.iter().map(|r| r.condition.as_str()).collect();
    let instruction = if agent.instruction.is_empty() {
        format!("Classify the input into one of: {}. Respond with ONLY the category name.", route_options.join(", "))
    } else {
        agent.instruction.clone()
    };
    let escaped = instruction.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
    code.push_str(&format!("            .instruction(\"{}\")\n", escaped));
    code.push_str("            .build()?\n");
    code.push_str("    );\n\n");
    
    // Create AgentNode with mappers
    code.push_str(&format!("    let {}_node = AgentNode::new({}_llm)\n", id, id));
    code.push_str("        .with_input_mapper(|state| {\n");
    code.push_str("            let msg = state.get(\"message\").and_then(|v| v.as_str()).unwrap_or(\"\");\n");
    code.push_str("            adk_core::Content::new(\"user\").with_text(msg.to_string())\n");
    code.push_str("        })\n");
    code.push_str("        .with_output_mapper(|events| {\n");
    code.push_str("            let mut updates = std::collections::HashMap::new();\n");
    code.push_str("            for event in events {\n");
    code.push_str("                if let Some(content) = event.content() {\n");
    code.push_str("                    let text: String = content.parts.iter()\n");
    code.push_str("                        .filter_map(|p| p.text())\n");
    code.push_str("                        .collect::<Vec<_>>().join(\"\").to_lowercase();\n");
    
    // Match against routes
    for (i, route) in agent.routes.iter().enumerate() {
        let cond = if i == 0 { "if" } else { "else if" };
        code.push_str(&format!("                    {} text.contains(\"{}\") {{\n", cond, route.condition.to_lowercase()));
        code.push_str(&format!("                        updates.insert(\"classification\".to_string(), json!(\"{}\"));\n", route.condition));
        code.push_str("                    }\n");
    }
    // Default fallback
    if let Some(default_route) = agent.routes.iter().find(|r| r.condition == "default") {
        code.push_str(&format!("                    else {{ updates.insert(\"classification\".to_string(), json!(\"{}\")); }}\n", default_route.condition));
    } else if let Some(first) = agent.routes.first() {
        code.push_str(&format!("                    else {{ updates.insert(\"classification\".to_string(), json!(\"{}\")); }}\n", first.condition));
    }
    
    code.push_str("                }\n");
    code.push_str("            }\n");
    code.push_str("            updates\n");
    code.push_str("        });\n\n");
    
    code
}

fn generate_llm_agent_for_graph(id: &str, agent: &AgentSchema, project: &ProjectSchema) -> String {
    let mut code = String::new();
    let model = agent.model.as_deref().unwrap_or("gemini-2.0-flash");
    
    code.push_str(&format!("    // Agent: {}\n", id));
    code.push_str(&format!("    let {}_llm = Arc::new(\n", id));
    code.push_str(&format!("        LlmAgentBuilder::new(\"{}\")\n", id));
    code.push_str(&format!("            .model(Arc::new(GeminiModel::new(&api_key, \"{}\")?))\n", model));
    
    if !agent.instruction.is_empty() {
        let escaped = agent.instruction.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        code.push_str(&format!("            .instruction(\"{}\")\n", escaped));
    }
    
    // Add tools
    for tool_type in &agent.tools {
        match tool_type.as_str() {
            "google_search" => code.push_str("            .tool(Arc::new(GoogleSearchTool::new()))\n"),
            "exit_loop" => code.push_str("            .tool(Arc::new(ExitLoopTool::new()))\n"),
            "load_artifact" => code.push_str("            .tool(Arc::new(LoadArtifactsTool::new()))\n"),
            "function" => {
                let tool_id = format!("{}_{}", id, tool_type);
                if let Some(ToolConfig::Function(config)) = project.tool_configs.get(&tool_id) {
                    code.push_str(&format!("            .tool(Arc::new(FunctionTool::new(\"{}\", \"{}\", {}_fn)))\n", 
                        config.name, config.description.replace('"', "\\\""), config.name));
                }
            }
            _ => {}
        }
    }
    
    code.push_str("            .build()?\n");
    code.push_str("    );\n\n");
    
    // Create AgentNode
    code.push_str(&format!("    let {}_node = AgentNode::new({}_llm)\n", id, id));
    code.push_str("        .with_input_mapper(|state| {\n");
    code.push_str("            let msg = state.get(\"message\").and_then(|v| v.as_str()).unwrap_or(\"\");\n");
    code.push_str("            adk_core::Content::new(\"user\").with_text(msg.to_string())\n");
    code.push_str("        })\n");
    code.push_str("        .with_output_mapper(|events| {\n");
    code.push_str("            let mut updates = std::collections::HashMap::new();\n");
    code.push_str("            for event in events {\n");
    code.push_str("                if let Some(content) = event.content() {\n");
    code.push_str("                    let text: String = content.parts.iter()\n");
    code.push_str("                        .filter_map(|p| p.text()).collect::<Vec<_>>().join(\"\");\n");
    code.push_str("                    if !text.is_empty() {\n");
    code.push_str("                        updates.insert(\"response\".to_string(), json!(text));\n");
    code.push_str("                    }\n");
    code.push_str("                }\n");
    code.push_str("            }\n");
    code.push_str("            updates\n");
    code.push_str("        });\n\n");
    
    code
}

fn generate_simple_main(project: &ProjectSchema) -> String {
    let mut code = String::new();
    
    code.push_str("#![allow(unused_imports)]\n\n");
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
    
    code.push_str("#[tokio::main]\n");
    code.push_str("async fn main() -> Result<()> {\n");
    code.push_str("    let api_key = std::env::var(\"GOOGLE_API_KEY\")\n");
    code.push_str("        .or_else(|_| std::env::var(\"GEMINI_API_KEY\"))\n");
    code.push_str("        .expect(\"GOOGLE_API_KEY or GEMINI_API_KEY must be set\");\n\n");
    
    let all_sub_agents: std::collections::HashSet<_> = project.agents.values()
        .flat_map(|a| a.sub_agents.iter().cloned())
        .collect();
    let top_level: Vec<_> = project.agents.keys()
        .filter(|id| !all_sub_agents.contains(*id))
        .collect();
    
    for agent_id in &top_level {
        if let Some(agent) = project.agents.get(*agent_id) {
            code.push_str(&generate_agent(agent_id, agent, project));
        }
    }
    
    if let Some(first_agent) = top_level.first() {
        code.push_str("    adk_cli::console::run_console(\n");
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

fn generate_agent(id: &str, agent: &AgentSchema, project: &ProjectSchema) -> String {
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
    let has_graph = has_router(project);
    
    let mut deps = String::new();
    deps.push_str("adk-agent = \"0.1.7\"\n");
    deps.push_str("adk-core = \"0.1.7\"\n");
    deps.push_str("adk-model = \"0.1.7\"\n");
    deps.push_str("adk-tool = \"0.1.7\"\n");
    if has_graph {
        deps.push_str("adk-graph = \"0.1.7\"\n");
    } else {
        deps.push_str("adk-cli = \"0.1.7\"\n");
    }
    deps.push_str("tokio = { version = \"1\", features = [\"full\"] }\n");
    deps.push_str("anyhow = \"1\"\n");
    deps.push_str("serde_json = \"1\"\n");
    
    format!(r#"[package]
name = "{}"
version = "0.1.0"
edition = "2021"

[dependencies]
{}
"#, name, deps)
}
