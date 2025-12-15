//! Rust code generation from project schemas - Always uses adk-graph

use crate::schema::{ProjectSchema, AgentType, ToolConfig, AgentSchema};
use anyhow::Result;

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

fn generate_main_rs(project: &ProjectSchema) -> String {
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
    code.push_str("    StreamEvent,\n");
    code.push_str("};\n");
    code.push_str("use adk_model::gemini::GeminiModel;\n");
    code.push_str("use adk_tool::{FunctionTool, GoogleSearchTool, ExitLoopTool, LoadArtifactsTool};\n");
    code.push_str("use anyhow::Result;\n");
    code.push_str("use serde_json::{json, Value};\n");
    code.push_str("use std::sync::Arc;\n");
    code.push_str("use tracing_subscriber::{fmt, EnvFilter};\n\n");
    
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
    // Initialize tracing with JSON output
    code.push_str("    // Initialize tracing\n");
    code.push_str("    fmt().with_env_filter(EnvFilter::from_default_env().add_directive(\"adk=info\".parse()?)).json().with_writer(std::io::stderr).init();\n\n");
    code.push_str("    let api_key = std::env::var(\"GOOGLE_API_KEY\")\n");
    code.push_str("        .or_else(|_| std::env::var(\"GEMINI_API_KEY\"))\n");
    code.push_str("        .expect(\"GOOGLE_API_KEY or GEMINI_API_KEY must be set\");\n\n");
    
    // Find top-level agents (not sub-agents of containers)
    let all_sub_agents: std::collections::HashSet<_> = project.agents.values()
        .flat_map(|a| a.sub_agents.iter().cloned())
        .collect();
    let top_level: Vec<_> = project.agents.keys()
        .filter(|id| !all_sub_agents.contains(*id))
        .collect();
    
    // Find first agent (connected from START)
    let first_agent: Option<&str> = project.workflow.edges.iter()
        .find(|e| e.from == "START")
        .map(|e| e.to.as_str());
    
    // Generate all agent nodes
    for agent_id in &top_level {
        if let Some(agent) = project.agents.get(*agent_id) {
            let is_first = first_agent == Some(agent_id.as_str());
            match agent.agent_type {
                AgentType::Router => {
                    code.push_str(&generate_router_node(agent_id, agent));
                }
                AgentType::Llm => {
                    code.push_str(&generate_llm_node(agent_id, agent, project, is_first));
                }
                _ => {
                    // Sequential/Loop/Parallel - generate as single node wrapping container
                    code.push_str(&generate_container_node(agent_id, agent, project));
                }
            }
        }
    }
    
    // Build graph
    code.push_str("    // Build the graph\n");
    code.push_str("    let graph = StateGraph::with_channels(&[\"message\", \"classification\", \"response\"])\n");
    
    // Add all nodes
    for agent_id in &top_level {
        code.push_str(&format!("        .add_node({}_node)\n", agent_id));
    }
    
    // Add edges from workflow
    for edge in &project.workflow.edges {
        let from = if edge.from == "START" { "START".to_string() } else { format!("\"{}\"", edge.from) };
        let to = if edge.to == "END" { "END".to_string() } else { format!("\"{}\"", edge.to) };
        
        // Check if source is a router - use conditional edges
        if let Some(agent) = project.agents.get(&edge.from) {
            if agent.agent_type == AgentType::Router && !agent.routes.is_empty() {
                // Generate conditional edges for router
                let conditions: Vec<String> = agent.routes.iter()
                    .map(|r| {
                        let target = if r.target == "END" { "END".to_string() } else { format!("\"{}\"", r.target) };
                        format!("(\"{}\", {})", r.condition, target)
                    })
                    .collect();
                
                code.push_str(&format!("        .add_conditional_edges(\n"));
                code.push_str(&format!("            \"{}\",\n", edge.from));
                code.push_str("            Router::by_field(\"classification\"),\n");
                code.push_str(&format!("            [{}],\n", conditions.join(", ")));
                code.push_str("        )\n");
                continue;
            }
        }
        
        code.push_str(&format!("        .add_edge({}, {})\n", from, to));
    }
    
    code.push_str("        .compile()?;\n\n");
    
    // Interactive loop with streaming
    code.push_str("    // Interactive loop\n");
    code.push_str("    println!(\"Graph workflow ready. Type your message (or 'quit' to exit):\");\n");
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
    code.push_str("        \n");
    code.push_str("        use adk_graph::StreamMode;\n");
    code.push_str("        use tokio_stream::StreamExt;\n");
    code.push_str("        let stream = graph.stream(state, ExecutionConfig::new(&format!(\"turn-{}\", turn)), StreamMode::Debug);\n");
    code.push_str("        tokio::pin!(stream);\n");
    code.push_str("        let mut final_response = String::new();\n");
    code.push_str("        \n");
    code.push_str("        while let Some(event) = stream.next().await {\n");
    code.push_str("            match event {\n");
    code.push_str("                Ok(e) => {\n");
    code.push_str("                    // Output trace event as JSON\n");
    code.push_str("                    if let Ok(json) = serde_json::to_string(&e) {\n");
    code.push_str("                        println!(\"TRACE:{}\", json);\n");
    code.push_str("                    }\n");
    code.push_str("                    // Capture final response from Done event\n");
    code.push_str("                    if let adk_graph::StreamEvent::Done { state, .. } = &e {\n");
    code.push_str("                        if let Some(resp) = state.get(\"response\").and_then(|v| v.as_str()) {\n");
    code.push_str("                            final_response = resp.to_string();\n");
    code.push_str("                        }\n");
    code.push_str("                    }\n");
    code.push_str("                }\n");
    code.push_str("                Err(e) => eprintln!(\"Error: {}\", e),\n");
    code.push_str("            }\n");
    code.push_str("        }\n");
    code.push_str("        turn += 1;\n\n");
    code.push_str("        if !final_response.is_empty() {\n");
    code.push_str("            println!(\"RESPONSE:{}\", final_response);\n");
    code.push_str("        }\n");
    code.push_str("    }\n\n");
    
    code.push_str("    Ok(())\n");
    code.push_str("}\n");
    
    code
}

fn generate_router_node(id: &str, agent: &AgentSchema) -> String {
    let mut code = String::new();
    let model = agent.model.as_deref().unwrap_or("gemini-2.0-flash");
    
    code.push_str(&format!("    // Router: {}\n", id));
    code.push_str(&format!("    let {}_llm = Arc::new(\n", id));
    code.push_str(&format!("        LlmAgentBuilder::new(\"{}\")\n", id));
    code.push_str(&format!("            .model(Arc::new(GeminiModel::new(&api_key, \"{}\")?))\n", model));
    
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
    
    for (i, route) in agent.routes.iter().enumerate() {
        let cond = if i == 0 { "if" } else { "else if" };
        code.push_str(&format!("                    {} text.contains(\"{}\") {{\n", cond, route.condition.to_lowercase()));
        code.push_str(&format!("                        updates.insert(\"classification\".to_string(), json!(\"{}\"));\n", route.condition));
        code.push_str("                    }\n");
    }
    if let Some(first) = agent.routes.first() {
        code.push_str(&format!("                    else {{ updates.insert(\"classification\".to_string(), json!(\"{}\")); }}\n", first.condition));
    }
    
    code.push_str("                }\n");
    code.push_str("            }\n");
    code.push_str("            updates\n");
    code.push_str("        });\n\n");
    
    code
}

fn generate_llm_node(id: &str, agent: &AgentSchema, project: &ProjectSchema, is_first: bool) -> String {
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
    
    code.push_str(&format!("    let {}_node = AgentNode::new({}_llm)\n", id, id));
    code.push_str("        .with_input_mapper(|state| {\n");
    
    // First agent reads from "message", subsequent agents read from "response"
    if is_first {
        code.push_str("            let msg = state.get(\"message\").and_then(|v| v.as_str()).unwrap_or(\"\");\n");
    } else {
        code.push_str("            // Read previous agent's response, fall back to original message\n");
        code.push_str("            let msg = state.get(\"response\").and_then(|v| v.as_str())\n");
        code.push_str("                .or_else(|| state.get(\"message\").and_then(|v| v.as_str())).unwrap_or(\"\");\n");
    }
    
    code.push_str("            adk_core::Content::new(\"user\").with_text(msg.to_string())\n");
    code.push_str("        })\n");
    code.push_str("        .with_output_mapper(|events| {\n");
    code.push_str("            let mut updates = std::collections::HashMap::new();\n");
    code.push_str("            let mut full_text = String::new();\n");
    code.push_str("            for event in events {\n");
    code.push_str("                if let Some(content) = event.content() {\n");
    code.push_str("                    for part in &content.parts {\n");
    code.push_str("                        if let Some(text) = part.text() {\n");
    code.push_str("                            full_text.push_str(text);\n");
    code.push_str("                        }\n");
    code.push_str("                    }\n");
    code.push_str("                }\n");
    code.push_str("            }\n");
    code.push_str("            if !full_text.is_empty() {\n");
    code.push_str("                updates.insert(\"response\".to_string(), json!(full_text));\n");
    code.push_str("            }\n");
    code.push_str("            updates\n");
    code.push_str("        });\n\n");
    
    code
}

fn generate_container_node(id: &str, agent: &AgentSchema, project: &ProjectSchema) -> String {
    let mut code = String::new();
    
    // Generate sub-agents first
    for sub_id in &agent.sub_agents {
        if let Some(sub) = project.agents.get(sub_id) {
            let model = sub.model.as_deref().unwrap_or("gemini-2.0-flash");
            code.push_str(&format!("    let {}_model = Arc::new(GeminiModel::new(&api_key, \"{}\")?);\n", sub_id, model));
            code.push_str(&format!("    let {}_agent = LlmAgentBuilder::new(\"{}\")\n", sub_id, sub_id));
            if !sub.instruction.is_empty() {
                let escaped = sub.instruction.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
                code.push_str(&format!("        .instruction(\"{}\")\n", escaped));
            }
            code.push_str(&format!("        .model({}_model)\n", sub_id));
            code.push_str("        .build()?;\n\n");
        }
    }
    
    // Create container
    let subs: Vec<_> = agent.sub_agents.iter().map(|s| format!("Arc::new({}_agent)", s)).collect();
    let container_type = match agent.agent_type {
        AgentType::Sequential => "adk_agent::SequentialAgent",
        AgentType::Loop => "adk_agent::LoopAgent",
        AgentType::Parallel => "adk_agent::ParallelAgent",
        _ => "adk_agent::SequentialAgent",
    };
    
    code.push_str(&format!("    // Container: {} ({:?})\n", id, agent.agent_type));
    if agent.agent_type == AgentType::Loop {
        let max_iter = agent.max_iterations.unwrap_or(3);
        code.push_str(&format!("    let {}_container = {}::new(\"{}\", vec![{}]).with_max_iterations({});\n\n", 
            id, container_type, id, subs.join(", "), max_iter));
    } else {
        code.push_str(&format!("    let {}_container = {}::new(\"{}\", vec![{}]);\n\n", 
            id, container_type, id, subs.join(", ")));
    }
    
    // Wrap in AgentNode
    code.push_str(&format!("    let {}_node = AgentNode::new(Arc::new({}_container))\n", id, id));
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

fn generate_function_tool(config: &crate::schema::FunctionToolConfig) -> String {
    let mut code = String::new();
    let fn_name = &config.name;
    
    code.push_str(&format!("async fn {}_fn(_ctx: Arc<dyn ToolContext>, args: Value) -> Result<Value, adk_core::AdkError> {{\n", fn_name));
    
    let mut param_names = Vec::new();
    for param in &config.parameters {
        param_names.push(param.name.clone());
        let extract = match param.param_type {
            crate::schema::ParamType::String => format!("    let {} = args[\"{}\"].as_str().unwrap_or(\"\");\n", param.name, param.name),
            crate::schema::ParamType::Number => format!("    let {} = args[\"{}\"].as_f64().unwrap_or(0.0);\n", param.name, param.name),
            crate::schema::ParamType::Boolean => format!("    let {} = args[\"{}\"].as_bool().unwrap_or(false);\n", param.name, param.name),
        };
        code.push_str(&extract);
    }
    
    // Generate dummy response that includes all parameters
    code.push_str("\n    Ok(json!({\n");
    code.push_str(&format!("        \"function\": \"{}\",\n", fn_name));
    for name in &param_names {
        code.push_str(&format!("        \"{}\": {},\n", name, name));
    }
    code.push_str("        \"status\": \"success\"\n");
    code.push_str("    }))\n");
    code.push_str("}\n\n");
    
    code
}

fn generate_cargo_toml(project: &ProjectSchema) -> String {
    let mut name = project.name.to_lowercase().replace(' ', "_").replace(|c: char| !c.is_alphanumeric() && c != '_', "");
    // Cargo package names can't start with a digit
    if name.is_empty() || name.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        name = format!("project_{}", name);
    }
    
    format!(r#"[package]
name = "{}"
version = "0.1.0"
edition = "2021"

[dependencies]
adk-agent = "0.1.7"
adk-core = "0.1.7"
adk-model = "0.1.7"
adk-tool = "0.1.7"
adk-graph = "0.1.7"
tokio = {{ version = "1", features = ["full", "macros"] }}
tokio-stream = "0.1"
anyhow = "1"
serde_json = "1"
tracing-subscriber = {{ version = "0.3", features = ["json", "env-filter"] }}
"#, name)
}
