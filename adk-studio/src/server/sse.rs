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

pub fn session_service() -> &'static Arc<InMemorySessionService> {
    static INSTANCE: OnceLock<Arc<InMemorySessionService>> = OnceLock::new();
    INSTANCE.get_or_init(|| Arc::new(InMemorySessionService::new()))
}

#[derive(Deserialize)]
pub struct StreamQuery {
    input: String,
    #[serde(default)]
    api_key: Option<String>,
}

pub async fn stream_handler(
    Path(id): Path<String>,
    Query(query): Query<StreamQuery>,
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let api_key = query.api_key
        .or_else(|| std::env::var("GOOGLE_API_KEY").ok())
        .unwrap_or_default();

    let stream = async_stream::stream! {
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

        let content = Content::new("user").with_text(&query.input);
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
