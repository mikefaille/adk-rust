use std::{
    collections::HashMap, convert::Infallible, net::SocketAddr, path::PathBuf, sync::Arc,
    time::Duration,
};

use anyhow::Context;
use async_stream::stream;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, sse::{Event, KeepAlive, Sse}},
    routing::{get, post},
};
use serde_json::json;
use serde::Deserialize;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

use crate::{
    app_runtime::{
        handoff::{PendingHandoff, evaluate_handoff, parse_handoff_command},
        host::{AgentAppHost, InMemoryAgentHost},
    },
    protocol::{
        AppCatalogResponse, ApprovalRequiredPayload, DonePayload, ErrorPayload, InboundEvent,
        InboundEventAck, InboundEventRequest, MasterPromptRequest, MasterPromptResponse,
        NotificationPayload, PingPayload, SessionCreateResponse, SsePayload,
    },
    safety::{
        approvals::PendingApproval,
        audit::{AuditDecision, AuditEntry},
        risk::RiskTier,
    },
    session::{AppSurfaceLayout, OutboundMessage, SessionManager},
    shell::{compositor, orchestrator, timeline},
};

#[derive(Debug, Clone, Deserialize)]
struct WorkspaceSurfaceSnapshot {
    app_id: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    z_index: i32,
}

fn parse_workspace_layout(layout: &str) -> Option<HashMap<String, AppSurfaceLayout>> {
    let items = serde_json::from_str::<Vec<WorkspaceSurfaceSnapshot>>(layout).ok()?;
    let mut mapped = HashMap::new();
    for item in items {
        if item.app_id.trim().is_empty() {
            continue;
        }
        mapped.insert(
            item.app_id,
            AppSurfaceLayout {
                x: item.x,
                y: item.y,
                w: item.w,
                h: item.h,
                z_index: item.z_index,
            },
        );
    }
    Some(mapped)
}

#[derive(Clone)]
pub struct AppState {
    pub sessions: SessionManager,
    pub host: Arc<dyn AgentAppHost>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

impl AppState {
    pub fn with_state_path(state_path: Option<PathBuf>) -> Self {
        let sessions = state_path
            .map(SessionManager::with_persistence_path)
            .unwrap_or_default();
        Self {
            sessions,
            host: Arc::new(InMemoryAgentHost::default()),
        }
    }

    pub fn from_env() -> Self {
        let state_path = std::env::var("ADK_SPATIAL_OS_STATE_PATH")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Self::with_state_path(state_path)
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::with_state_path(None)
    }
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8199,
        }
    }
}

pub fn app_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/api/os/apps", get(list_apps))
        .route("/api/os/session", post(create_session))
        .route("/api/os/stream/{session_id}", get(stream_session))
        .route("/api/os/prompt/{session_id}", post(master_prompt))
        .route("/api/os/event/{session_id}", post(inbound_event))
        .with_state(state)
        .layer(cors)
}

pub async fn run_server(config: ServerConfig) -> anyhow::Result<()> {
    let state = AppState::from_env();
    let app = app_router(state);
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .with_context(|| "invalid host/port for adk-spatial-os")?;

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("adk-spatial-os listening on http://{}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> impl IntoResponse {
    Html(include_str!("../ui-shell/index.html"))
}

async fn health() -> impl IntoResponse {
    Json(json!({"status":"ok","service":"adk-spatial-os"}))
}

async fn list_apps(State(state): State<AppState>) -> impl IntoResponse {
    let apps = state.host.list_apps().await;
    Json(AppCatalogResponse { apps })
}

async fn create_session(State(state): State<AppState>) -> impl IntoResponse {
    let session_id = state.sessions.create_session().await;
    Json(SessionCreateResponse { session_id })
}

async fn stream_session(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    state.sessions.ensure_session(&session_id).await;
    let mut rx = state
        .sessions
        .subscribe(&session_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;

    let _ = state
        .sessions
        .publish(&session_id, SsePayload::Ping(PingPayload::now()))
        .await;

    if let Some(context) = state.sessions.get_context(&session_id).await {
        if !context.active_apps.is_empty() {
            let _ = state
                .sessions
                .publish(
                    &session_id,
                    SsePayload::ShellState(compositor::shell_state(
                        context.active_apps.clone(),
                        context.focused_app.clone(),
                        context.last_prompt.clone(),
                    )),
                )
                .await;
            let _ = state
                .sessions
                .publish(
                    &session_id,
                    SsePayload::AppSurfaceOps(compositor::build_app_surface_ops(
                        &context.active_apps,
                        &context.workspace_layout,
                    )),
                )
                .await;
        }
        if let Some(pending) = context.pending_approval {
            let _ = state
                .sessions
                .publish(
                    &session_id,
                    SsePayload::ApprovalRequired(ApprovalRequiredPayload {
                        action_id: pending.action_id,
                        app_id: pending.app_id,
                        title: pending.title,
                        rationale: pending.rationale,
                        risk: pending.risk,
                    }),
                )
                .await;
        }
    }

    let stream = stream! {
        loop {
            match rx.recv().await {
                Ok(OutboundMessage { event, data }) => {
                    yield Ok(Event::default().event(event).data(data));
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    let warn = json!({"level":"warn","message":"client lagged","skipped": skipped});
                    yield Ok(Event::default().event("notification").data(warn.to_string()));
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keepalive")))
}

async fn master_prompt(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<MasterPromptRequest>,
) -> Result<Json<MasterPromptResponse>, (StatusCode, Json<MasterPromptResponse>)> {
    let prompt = request.prompt.trim();
    if prompt.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(MasterPromptResponse {
                accepted: false,
                message: "prompt cannot be empty".to_string(),
                selected_apps: vec![],
            }),
        ));
    }

    state.sessions.ensure_session(&session_id).await;
    let existing_layout = state
        .sessions
        .get_context(&session_id)
        .await
        .map(|ctx| ctx.workspace_layout)
        .unwrap_or_default();
    let plan = orchestrator::build_master_plan(state.host.as_ref(), prompt).await;

    let focused_app = plan.selected_apps.first().cloned();
    let _ = state
        .sessions
        .update_context(&session_id, |ctx| {
            ctx.last_prompt = Some(plan.prompt.clone());
            ctx.active_apps = plan.selected_apps.clone();
            ctx.focused_app = focused_app.clone();
            ctx.pending_approval = None;
            ctx.pending_handoff = None;
        })
        .await;

    let _ = state
        .sessions
        .publish(
            &session_id,
            SsePayload::ShellState(compositor::shell_state(
                plan.selected_apps.clone(),
                focused_app.clone(),
                Some(plan.prompt.clone()),
            )),
        )
        .await;

    let _ = state
        .sessions
        .publish(
            &session_id,
            SsePayload::TimelineEntry(timeline::route_entry(
                &plan.prompt,
                &plan.selected_apps,
                &plan.rationale,
            )),
        )
        .await;

    let _ = state
        .sessions
        .publish(
            &session_id,
            SsePayload::AppSurfaceOps(compositor::build_app_surface_ops(
                &plan.selected_apps,
                &existing_layout,
            )),
        )
        .await;

    if matches!(plan.risk, RiskTier::Dangerous) {
        let app_id = focused_app.unwrap_or_else(|| "ops-center".to_string());
        let pending = PendingApproval {
            action_id: format!("approval-{}", uuid::Uuid::new_v4()),
            app_id: app_id.clone(),
            title: "Dangerous action requires approval".to_string(),
            rationale: "Master Prompt implies high-impact operation.".to_string(),
            risk: plan.risk,
        };

        let _ = state
            .sessions
            .update_context(&session_id, |ctx| {
                ctx.pending_approval = Some(pending.clone());
                ctx.audit_log.push(AuditEntry::new(
                    &pending.action_id,
                    &pending.app_id,
                    pending.risk,
                    AuditDecision::Proposed,
                ));
            })
            .await;

        let _ = state
            .sessions
            .publish(
                &session_id,
                SsePayload::ApprovalRequired(ApprovalRequiredPayload {
                    action_id: pending.action_id,
                    app_id: pending.app_id,
                    title: pending.title,
                    rationale: pending.rationale,
                    risk: pending.risk,
                }),
            )
            .await;
    } else {
        let _ = state
            .sessions
            .publish(
                &session_id,
                SsePayload::Done(DonePayload {
                    status: "completed".to_string(),
                }),
            )
            .await;
    }

    Ok(Json(MasterPromptResponse {
        accepted: true,
        message: "master plan created".to_string(),
        selected_apps: plan.selected_apps,
    }))
}

async fn inbound_event(
    Path(session_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<InboundEventRequest>,
) -> Result<Json<InboundEventAck>, (StatusCode, Json<InboundEventAck>)> {
    state.sessions.ensure_session(&session_id).await;
    let _ = state.sessions.record_event(&session_id, request.clone()).await;

    match request.event {
        InboundEvent::MasterPromptSubmit { prompt } => {
            let _ = master_prompt(
                Path(session_id.clone()),
                State(state.clone()),
                Json(MasterPromptRequest { prompt }),
            )
            .await;
        }
        InboundEvent::AppFocus { app_id } => {
            let context = state.sessions.get_context(&session_id).await.unwrap_or_default();
            let active_apps = context.active_apps.clone();
            let _ = state
                .sessions
                .update_context(&session_id, |ctx| {
                    ctx.focused_app = Some(app_id.clone());
                })
                .await;
            let _ = state
                .sessions
                .publish(
                    &session_id,
                    SsePayload::ShellState(compositor::shell_state(
                        active_apps,
                        Some(app_id.clone()),
                        context.last_prompt,
                    )),
                )
                .await;
            let _ = state
                .sessions
                .publish(
                    &session_id,
                    SsePayload::TimelineEntry(compositor::timeline_info(&format!(
                        "Focused app: {app_id}"
                    ))),
                )
                .await;
        }
        InboundEvent::AppCommand { app_id, command } => {
            if let Some(handoff) = parse_handoff_command(&app_id, &command) {
                let handoff_id = format!("handoff-{}", uuid::Uuid::new_v4());
                let policy = state
                    .host
                    .evaluate_handoff_policy(&handoff.from_app, &handoff.to_app)
                    .await;

                let _ = state
                    .sessions
                    .publish(
                        &session_id,
                        SsePayload::TimelineEntry(timeline::handoff_requested_entry(
                            &handoff.from_app,
                            &handoff.to_app,
                            &handoff.context_summary,
                            &handoff_id,
                        )),
                    )
                    .await;
                let _ = state
                    .sessions
                    .publish(
                        &session_id,
                        SsePayload::TimelineEntry(timeline::handoff_policy_entry(
                            &handoff_id,
                            &handoff.from_app,
                            &handoff.to_app,
                            policy.allowed,
                            &policy.reason,
                        )),
                    )
                    .await;

                if !policy.allowed {
                    let _ = state
                        .sessions
                        .update_context(&session_id, |ctx| {
                            ctx.pending_handoff = None;
                            ctx.pending_approval = None;
                            ctx.audit_log.push(AuditEntry::new(
                                &handoff_id,
                                &handoff.from_app,
                                RiskTier::Controlled,
                                AuditDecision::Rejected,
                            ));
                        })
                        .await;
                    let _ = state
                        .sessions
                        .publish(
                            &session_id,
                            SsePayload::Notification(NotificationPayload {
                                level: "warn".to_string(),
                                message: policy.reason,
                            }),
                        )
                        .await;
                    let server_seq = state.sessions.last_server_seq(&session_id).await;
                    return Ok(Json(InboundEventAck {
                        ok: true,
                        server_seq,
                        error: None,
                    }));
                }

                let pending_handoff = PendingHandoff {
                    handoff_id: handoff_id.clone(),
                    request: handoff.clone(),
                };
                let _ = state
                    .sessions
                    .update_context(&session_id, |ctx| {
                        ctx.pending_handoff = Some(pending_handoff.clone());
                        ctx.pending_approval = Some(PendingApproval {
                            action_id: handoff_id.clone(),
                            app_id: handoff.from_app.clone(),
                            title: format!("Allow handoff to {}", handoff.to_app),
                            rationale: format!("{} ({})", handoff.context_summary, policy.reason),
                            risk: RiskTier::Controlled,
                        });
                        ctx.audit_log.push(AuditEntry::new(
                            &handoff_id,
                            &handoff.from_app,
                            RiskTier::Controlled,
                            AuditDecision::Proposed,
                        ));
                    })
                    .await;

                let _ = state
                    .sessions
                    .publish(
                        &session_id,
                        SsePayload::ApprovalRequired(ApprovalRequiredPayload {
                            action_id: handoff_id,
                            app_id: handoff.from_app,
                            title: "Cross-app handoff requires approval".to_string(),
                            rationale: format!(
                                "Transfer context to {}: {} | Policy: {}",
                                handoff.to_app, handoff.context_summary, policy.reason
                            ),
                            risk: RiskTier::Controlled,
                        }),
                    )
                    .await;
                let server_seq = state.sessions.last_server_seq(&session_id).await;
                return Ok(Json(InboundEventAck {
                    ok: true,
                    server_seq,
                    error: None,
                }));
            }

            let dispatched = state.host.execute_command(&app_id, &command).await;
            let _ = state
                .sessions
                .publish(
                    &session_id,
                    SsePayload::TimelineEntry(timeline::app_command_entry(
                        &app_id,
                        &command,
                        dispatched.accepted,
                        &dispatched.summary,
                    )),
                )
                .await;
            let _ = state
                .sessions
                .publish(
                    &session_id,
                    SsePayload::Notification(NotificationPayload {
                        level: if dispatched.accepted { "info" } else { "warn" }.to_string(),
                        message: dispatched.summary,
                    }),
                )
                .await;
        }
        InboundEvent::ApprovalDecision {
            action_id,
            approved,
        } => {
            let context = state.sessions.get_context(&session_id).await.unwrap_or_default();
            if let Some(pending_handoff) = context.pending_handoff.clone() {
                if pending_handoff.handoff_id == action_id {
                    let decision = evaluate_handoff(&pending_handoff.request, approved);
                    let mut next_active = context.active_apps.clone();
                    let mut next_focus = context.focused_app.clone();
                    if decision.allowed {
                        if !next_active.iter().any(|app| app == &pending_handoff.request.to_app) {
                            next_active.push(pending_handoff.request.to_app.clone());
                        }
                        next_focus = Some(pending_handoff.request.to_app.clone());
                    }
                    let _ = state
                        .sessions
                        .update_context(&session_id, |ctx| {
                            ctx.pending_handoff = None;
                            ctx.pending_approval = None;
                            ctx.active_apps = next_active.clone();
                            ctx.focused_app = next_focus.clone();
                            ctx.audit_log.push(AuditEntry::new(
                                &pending_handoff.handoff_id,
                                &pending_handoff.request.from_app,
                                RiskTier::Controlled,
                                if decision.allowed {
                                    AuditDecision::Approved
                                } else {
                                    AuditDecision::Rejected
                                },
                            ));
                        })
                        .await;
                    let _ = state
                        .sessions
                        .publish(
                            &session_id,
                            SsePayload::TimelineEntry(timeline::handoff_decision_entry(
                                &pending_handoff.handoff_id,
                                &pending_handoff.request.from_app,
                                &pending_handoff.request.to_app,
                                decision.allowed,
                                &decision.reason,
                            )),
                        )
                        .await;
                    if decision.allowed {
                        let refreshed_context = state.sessions.get_context(&session_id).await.unwrap_or_default();
                        let _ = state
                            .sessions
                            .publish(
                                &session_id,
                                SsePayload::ShellState(compositor::shell_state(
                                    refreshed_context.active_apps.clone(),
                                    refreshed_context.focused_app.clone(),
                                    refreshed_context.last_prompt.clone(),
                                )),
                            )
                            .await;
                        let _ = state
                            .sessions
                            .publish(
                                &session_id,
                                SsePayload::AppSurfaceOps(compositor::build_app_surface_ops(
                                    &refreshed_context.active_apps,
                                    &refreshed_context.workspace_layout,
                                )),
                            )
                            .await;
                    }
                    let _ = state
                        .sessions
                        .publish(
                            &session_id,
                            SsePayload::Notification(NotificationPayload {
                                level: if decision.allowed { "success" } else { "info" }.to_string(),
                                message: decision.reason,
                            }),
                        )
                        .await;
                    let _ = state
                        .sessions
                        .publish(
                            &session_id,
                            SsePayload::Done(DonePayload {
                                status: "handoff_resolved".to_string(),
                            }),
                        )
                        .await;
                    let server_seq = state.sessions.last_server_seq(&session_id).await;
                    return Ok(Json(InboundEventAck {
                        ok: true,
                        server_seq,
                        error: None,
                    }));
                }
            }
            let Some(pending) = context.pending_approval else {
                let _ = state
                    .sessions
                    .publish(
                        &session_id,
                        SsePayload::Notification(NotificationPayload {
                            level: "warn".to_string(),
                            message: "No pending approval found".to_string(),
                        }),
                    )
                    .await;
                let server_seq = state.sessions.last_server_seq(&session_id).await;
                return Ok(Json(InboundEventAck {
                    ok: true,
                    server_seq,
                    error: None,
                }));
            };

            if pending.action_id != action_id {
                let _ = state
                    .sessions
                    .publish(
                        &session_id,
                        SsePayload::Error(ErrorPayload {
                            code: "approval_mismatch".to_string(),
                            message: "Action ID does not match pending approval".to_string(),
                        }),
                    )
                    .await;
            } else {
                let decision = if approved {
                    AuditDecision::Approved
                } else {
                    AuditDecision::Rejected
                };
                let _ = state
                    .sessions
                    .update_context(&session_id, |ctx| {
                        ctx.audit_log
                            .push(AuditEntry::new(&action_id, &pending.app_id, pending.risk, decision));
                        ctx.pending_approval = None;
                        ctx.pending_handoff = None;
                    })
                    .await;
                let _ = state
                    .sessions
                    .publish(
                        &session_id,
                        SsePayload::TimelineEntry(timeline::approval_entry(&action_id, approved)),
                    )
                    .await;
                let _ = state
                    .sessions
                    .publish(
                        &session_id,
                        SsePayload::Notification(NotificationPayload {
                            level: if approved { "success" } else { "info" }.to_string(),
                            message: if approved {
                                "Approval accepted. Execution can proceed.".to_string()
                            } else {
                                "Approval rejected. Action blocked.".to_string()
                            },
                        }),
                    )
                    .await;
                let _ = state
                    .sessions
                    .publish(
                        &session_id,
                        SsePayload::Done(DonePayload {
                            status: "approval_resolved".to_string(),
                        }),
                    )
                    .await;
            }
        }
        InboundEvent::WorkspaceLayoutChange { layout } => {
            let parsed_layout = parse_workspace_layout(&layout);
            if let Some(next_layout) = parsed_layout {
                let _ = state
                    .sessions
                    .update_context(&session_id, |ctx| {
                        ctx.workspace_layout = next_layout;
                    })
                    .await;
            }
            let _ = state
                .sessions
                .publish(
                    &session_id,
                    SsePayload::TimelineEntry(timeline::workspace_layout_entry(&layout)),
                )
                .await;
        }
    }

    let server_seq = state.sessions.last_server_seq(&session_id).await;
    Ok(Json(InboundEventAck {
        ok: true,
        server_seq,
        error: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::AppState;
    use crate::session::AppSurfaceLayout;
    use uuid::Uuid;

    #[tokio::test]
    async fn app_state_with_state_path_restores_session_context() {
        let state_file = std::env::temp_dir().join(format!(
            "adk-spatial-os-state-app-state-{}.json",
            Uuid::new_v4()
        ));

        let initial = AppState::with_state_path(Some(state_file.clone()));
        let session_id = initial.sessions.create_session().await;
        let _ = initial
            .sessions
            .update_context(&session_id, |ctx| {
                ctx.focused_app = Some("ops-center".to_string());
                ctx.active_apps = vec!["ops-center".to_string(), "mail-agent".to_string()];
                ctx.last_prompt = Some("triage production incident".to_string());
                ctx.workspace_layout.insert(
                    "ops-center".to_string(),
                    AppSurfaceLayout {
                        x: 188,
                        y: 132,
                        w: 560,
                        h: 340,
                        z_index: 18,
                    },
                );
            })
            .await;
        drop(initial);

        let restored_state = AppState::with_state_path(Some(state_file.clone()));
        restored_state.sessions.ensure_session(&session_id).await;
        let restored = restored_state
            .sessions
            .get_context(&session_id)
            .await
            .expect("restored context");
        assert_eq!(restored.focused_app.as_deref(), Some("ops-center"));
        assert_eq!(restored.active_apps.len(), 2);
        assert_eq!(restored.last_prompt.as_deref(), Some("triage production incident"));
        let layout = restored
            .workspace_layout
            .get("ops-center")
            .expect("workspace layout restored");
        assert_eq!(layout.x, 188);
        assert_eq!(layout.y, 132);
        assert_eq!(layout.z_index, 18);

        let _ = tokio::fs::remove_file(state_file).await;
    }
}
