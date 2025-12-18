use crate::server::state::AppState;
use axum::{
    extract::{Path, Query, State},
    response::sse::{Event, Sse},
};
use futures::Stream;
use serde::Deserialize;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

lazy_static::lazy_static! {
    static ref SESSIONS: Arc<Mutex<HashMap<String, SessionProcess>>> = Arc::new(Mutex::new(HashMap::new()));
}

struct SessionProcess {
    stdin: BufWriter<tokio::process::ChildStdin>,
    stdout_rx: tokio::sync::mpsc::Receiver<String>,
    stderr_rx: tokio::sync::mpsc::Receiver<String>,
    _child: Child,
}

#[derive(Deserialize)]
pub struct StreamQuery {
    input: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    binary_path: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
}

async fn get_or_create_session(session_id: &str, binary_path: &str, api_key: &str) -> Result<(), String> {
    let mut sessions = SESSIONS.lock().await;
    if sessions.contains_key(session_id) {
        return Ok(());
    }
    
    let mut child = Command::new(binary_path)
        .arg(session_id)
        .env("GOOGLE_API_KEY", api_key)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start binary: {}", e))?;
    
    let stdin = BufWriter::new(child.stdin.take().unwrap());
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    
    let (stdout_tx, stdout_rx) = tokio::sync::mpsc::channel(100);
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if stdout_tx.send(line).await.is_err() { break; }
        }
    });
    
    let (stderr_tx, stderr_rx) = tokio::sync::mpsc::channel(100);
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if stderr_tx.send(line).await.is_err() { break; }
        }
    });
    
    sessions.insert(session_id.to_string(), SessionProcess { stdin, stdout_rx, stderr_rx, _child: child });
    Ok(())
}

pub async fn stream_handler(
    Path(_id): Path<String>,
    Query(query): Query<StreamQuery>,
    State(_state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let api_key = query.api_key.or_else(|| std::env::var("GOOGLE_API_KEY").ok()).unwrap_or_default();
    let input = query.input;
    let binary_path = query.binary_path;
    let session_id = query.session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let stream = async_stream::stream! {
        let Some(bin_path) = binary_path else {
            yield Ok(Event::default().event("error").data("No binary available. Click 'Build' first."));
            return;
        };

        if let Err(e) = get_or_create_session(&session_id, &bin_path, &api_key).await {
            yield Ok(Event::default().event("error").data(e));
            return;
        }
        
        yield Ok(Event::default().event("session").data(session_id.clone()));
        
        // Send input
        {
            let mut sessions = SESSIONS.lock().await;
            if let Some(session) = sessions.get_mut(&session_id) {
                if session.stdin.write_all(format!("{}\n", input).as_bytes()).await.is_err() 
                    || session.stdin.flush().await.is_err() {
                    yield Ok(Event::default().event("error").data("Failed to send input"));
                    return;
                }
            }
        }
        
        let timeout = tokio::time::Duration::from_secs(60);
        let start = tokio::time::Instant::now();
        
        loop {
            if start.elapsed() > timeout {
                yield Ok(Event::default().event("error").data("Timeout"));
                break;
            }
            
            let (stdout_msg, stderr_msg) = {
                let mut sessions = SESSIONS.lock().await;
                match sessions.get_mut(&session_id) {
                    Some(s) => (s.stdout_rx.try_recv().ok(), s.stderr_rx.try_recv().ok()),
                    None => {
                        yield Ok(Event::default().event("error").data("Session lost"));
                        break;
                    }
                }
            };
            
            let mut got_data = false;
            
            if let Some(line) = stdout_msg {
                got_data = true;
                let line = line.trim_start_matches("> ");
                if let Some(sid) = line.strip_prefix("SESSION:") {
                    yield Ok(Event::default().event("session").data(sid));
                } else if let Some(trace) = line.strip_prefix("TRACE:") {
                    yield Ok(Event::default().event("trace").data(trace));
                } else if let Some(response) = line.strip_prefix("RESPONSE:") {
                    let decoded = serde_json::from_str::<String>(response).unwrap_or_else(|_| response.to_string());
                    yield Ok(Event::default().event("chunk").data(decoded));
                    yield Ok(Event::default().event("end").data(""));
                    break;
                }
            }
            
            if let Some(line) = stderr_msg {
                got_data = true;
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
                    let fields = json.get("fields");
                    let msg = fields.and_then(|f| f.get("message")).and_then(|m| m.as_str()).unwrap_or("");
                    
                    if msg == "tool_call" {
                        let name = fields.and_then(|f| f.get("tool.name")).and_then(|v| v.as_str()).unwrap_or("");
                        let args = fields.and_then(|f| f.get("tool.args")).and_then(|v| v.as_str()).unwrap_or("{}");
                        yield Ok(Event::default().event("tool_call").data(serde_json::json!({"name": name, "args": args}).to_string()));
                    } else if msg == "tool_result" {
                        let name = fields.and_then(|f| f.get("tool.name")).and_then(|v| v.as_str()).unwrap_or("");
                        let result = fields.and_then(|f| f.get("tool.result")).and_then(|v| v.as_str()).unwrap_or("");
                        yield Ok(Event::default().event("tool_result").data(serde_json::json!({"name": name, "result": result}).to_string()));
                    } else if msg == "Starting agent execution" {
                        // Emit node_start for sub-agent
                        let agent = json.get("span").and_then(|s| s.get("agent.name")).and_then(|v| v.as_str()).unwrap_or("");
                        yield Ok(Event::default().event("trace").data(serde_json::json!({"type": "node_start", "node": agent, "step": 0}).to_string()));
                    } else if msg == "Agent execution complete" {
                        // Emit node_end for sub-agent - agent name is in fields
                        let agent = fields.and_then(|f| f.get("agent.name")).and_then(|v| v.as_str()).unwrap_or("");
                        yield Ok(Event::default().event("trace").data(serde_json::json!({"type": "node_end", "node": agent, "step": 0, "duration_ms": 0}).to_string()));
                    } else if msg == "Generating content" {
                        // Model call - extract details
                        let span = json.get("span");
                        let model = span.and_then(|s| s.get("model.name")).and_then(|v| v.as_str()).unwrap_or("");
                        let tools = span.and_then(|s| s.get("request.tools_count")).and_then(|v| v.as_str()).unwrap_or("0");
                        yield Ok(Event::default().event("log").data(serde_json::json!({"message": format!("Calling {} (tools: {})", model, tools)}).to_string()));
                    }
                }
            }
            
            if !got_data {
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }
        }
    };

    Sse::new(stream)
}

pub async fn kill_session(Path(session_id): Path<String>) -> &'static str {
    SESSIONS.lock().await.remove(&session_id);
    "ok"
}
