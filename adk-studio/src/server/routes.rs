use crate::server::{handlers, sse, state::AppState};
use axum::{
    Router,
    routing::{delete, get, post, put},
};

pub fn api_routes() -> Router<AppState> {
    Router::new()
        .route("/projects", get(handlers::list_projects))
        .route("/projects", post(handlers::create_project))
        .route("/projects/:id", get(handlers::get_project))
        .route("/projects/:id", put(handlers::update_project))
        .route("/projects/:id", delete(handlers::delete_project))
        .route("/projects/:id/run", post(handlers::run_project))
        .route("/projects/:id/stream", get(sse::stream_handler))
        .route("/projects/:id/session", delete(handlers::clear_session))
        .route("/projects/:id/compile", get(handlers::compile_project))
        .route("/projects/:id/build", post(handlers::build_project))
        .route("/projects/:id/build-stream", get(handlers::build_project_stream))
        .route("/sessions/:session_id", delete(sse::kill_session))
        // Task 10: HITL Resume Endpoint
        // POST /api/sessions/{session_id}/resume - Resume an interrupted workflow
        // Requirements: 3.2, 5.2
        .route("/sessions/:session_id/resume", post(handlers::resume_session))
}
