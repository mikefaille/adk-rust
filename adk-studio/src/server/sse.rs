use crate::compiler::compile_project;
use crate::server::state::AppState;
use adk_core::Content;
use adk_runner::{Runner, RunnerConfig};
use adk_session::{CreateRequest, GetRequest, InMemorySessionService, SessionService};
use axum::{
    extract::{Path, Query, State},
    response::sse::{Event, Sse},
};
use futures::{Stream, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

pub fn session_service() -> &'static Arc<InMemorySessionService> {
    static INSTANCE: OnceLock<Arc<InMemorySessionService>> = OnceLock::new();
    INSTANCE.get_or_init(|| Arc::new(InMemorySessionService::new()))
}

#[derive(Deserialize)]
pub struct StreamQuery {
    input: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    binary_path: Option<String>,
}

pub async fn stream_handler(
    Path(id): Path<String>,
    Query(query): Query<StreamQuery>,
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let api_key = query.api_key
        .or_else(|| std::env::var("GOOGLE_API_KEY").ok())
        .unwrap_or_default();
    let input = query.input;
    let binary_path = query.binary_path;

    let stream = async_stream::stream! {
        // If binary_path provided, run the compiled binary
        if let Some(bin_path) = binary_path {
            let mut child = match Command::new(&bin_path)
                .env("GOOGLE_API_KEY", &api_key)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        yield Ok(Event::default().event("error").data(format!("Failed to start binary: {}", e)));
                        return;
                    }
                };
            
            let mut stdin = child.stdin.take().unwrap();
            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();
            
            if let Err(e) = stdin.write_all(format!("{}\nquit\n", input).as_bytes()).await {
                yield Ok(Event::default().event("error").data(e.to_string()));
                return;
            }
            drop(stdin);
            
            // Read stdout and stderr concurrently
            let mut stdout_reader = BufReader::new(stdout).lines();
            let mut stderr_reader = BufReader::new(stderr).lines();
            
            loop {
                tokio::select! {
                    line = stdout_reader.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                let line = line.trim_start_matches("> ");
                                if let Some(trace_json) = line.strip_prefix("TRACE:") {
                                    yield Ok(Event::default().event("trace").data(trace_json));
                                } else if let Some(response) = line.strip_prefix("RESPONSE:") {
                                    yield Ok(Event::default().event("chunk").data(response));
                                }
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }
                    line = stderr_reader.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                // Parse JSON tracing output from stderr
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
                                    let fields = json.get("fields");
                                    let msg = fields.and_then(|f| f.get("message")).and_then(|m| m.as_str()).unwrap_or("");
                                    
                                    // Check for tool_call event
                                    if msg == "tool_call" {
                                        let name = fields.and_then(|f| f.get("tool.name")).and_then(|v| v.as_str()).unwrap_or("");
                                        let args = fields.and_then(|f| f.get("tool.args")).and_then(|v| v.as_str()).unwrap_or("{}");
                                        let tool_data = serde_json::json!({"name": name, "args": args}).to_string();
                                        yield Ok(Event::default().event("tool_call").data(tool_data));
                                    }
                                    // Check for tool_result event
                                    else if msg == "tool_result" {
                                        let name = fields.and_then(|f| f.get("tool.name")).and_then(|v| v.as_str()).unwrap_or("");
                                        let result = fields.and_then(|f| f.get("tool.result")).and_then(|v| v.as_str()).unwrap_or("");
                                        let result_data = serde_json::json!({"name": name, "result": result}).to_string();
                                        yield Ok(Event::default().event("tool_result").data(result_data));
                                    }
                                    // Other log messages
                                    else if let Some(target) = json.get("target").and_then(|v| v.as_str()) {
                                        if target.starts_with("adk") {
                                            let span = json.get("span").and_then(|s| s.get("agent.name")).and_then(|n| n.as_str());
                                            if let Some(agent) = span {
                                                yield Ok(Event::default().event("log").data(format!("{{\"agent\":\"{}\",\"message\":\"{}\"}}", agent, msg)));
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(None) => {}
                            Err(_) => {}
                        }
                    }
                }
            }
            
            let _ = child.wait().await;
            yield Ok(Event::default().event("end").data(""));
            return;
        }

        // Runtime compiler path
        let project_id: uuid::Uuid = match id.parse() {
            Ok(id) => id,
            Err(e) => {
                yield Ok(Event::default().event("error").data(e.to_string()));
                return;
            }
        };

        let storage = state.storage.read().await;
        let project = match storage.get(project_id).await {
            Ok(p) => p,
            Err(e) => {
                yield Ok(Event::default().event("error").data(e.to_string()));
                return;
            }
        };

        let agent = match compile_project(&project, &api_key) {
            Ok(a) => a,
            Err(e) => {
                yield Ok(Event::default().event("error").data(e.to_string()));
                return;
            }
        };
        let agent_count = project.agents.len();
        drop(storage);

        yield Ok(Event::default().event("start").data(format!("{} agent(s)", agent_count)));

        let svc = session_service().clone();
        let session_id = project_id.to_string();

        let session = match svc.get(GetRequest {
            app_name: "studio".into(),
            user_id: "user".into(),
            session_id: session_id.clone(),
            num_recent_events: None,
            after: None,
        }).await {
            Ok(s) => s,
            Err(_) => match svc.create(CreateRequest {
                app_name: "studio".into(),
                user_id: "user".into(),
                session_id: Some(session_id),
                state: HashMap::new(),
            }).await {
                Ok(s) => s,
                Err(e) => {
                    yield Ok(Event::default().event("error").data(e.to_string()));
                    return;
                }
            }
        };

        let runner = match Runner::new(RunnerConfig {
            app_name: "studio".into(),
            agent,
            session_service: svc,
            artifact_service: None,
            memory_service: None,
        }) {
            Ok(r) => r,
            Err(e) => {
                yield Ok(Event::default().event("error").data(e.to_string()));
                return;
            }
        };

        let content = Content::new("user").with_text(&input);
        let mut run_stream = match runner.run("user".into(), session.id().to_string(), content).await {
            Ok(s) => s,
            Err(e) => {
                yield Ok(Event::default().event("error").data(e.to_string()));
                return;
            }
        };

        let mut last_text = String::new();
        let mut current_agent = String::new();
        while let Some(result) = run_stream.next().await {
            if let Ok(event) = result {
                // Check if agent changed
                if event.author != current_agent {
                    current_agent = event.author.clone();
                    yield Ok(Event::default().event("agent").data(&current_agent));
                }
                if let Some(c) = event.content() {
                    for part in &c.parts {
                        match part {
                            adk_core::Part::Text { text } => {
                                if text != &last_text {
                                    yield Ok(Event::default().event("chunk").data(text));
                                    last_text = text.clone();
                                }
                            }
                            adk_core::Part::FunctionCall { name, args, .. } => {
                                let tool_data = serde_json::json!({"name": name, "args": args}).to_string();
                                yield Ok(Event::default().event("tool_call").data(tool_data));
                            }
                            adk_core::Part::FunctionResponse { name, response, .. } => {
                                let result_data = serde_json::json!({"name": name, "result": response}).to_string();
                                yield Ok(Event::default().event("tool_result").data(result_data));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        yield Ok(Event::default().event("end").data(""));
    };

    Sse::new(stream)
}
