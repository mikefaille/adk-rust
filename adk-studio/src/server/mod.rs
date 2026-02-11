//! # Studio Server
//!
//! The backend server implementation for ADK Studio.
//!
//! This module provides the HTTP API, WebSocket endpoints, and background services
//! that power the Studio environment.
//!
//! ## Key Components
//!
//! - **`AppState`**: Shared state managed by the server (projects, builds, sessions).
//! - **`WorkflowExecutor`**: Orchestrates the execution of agent workflows.
//! - **`GraphRunner`**: Manages the runtime state of visual graphs.
//! - **`Scheduler`**: Background task scheduler for periodic jobs.
//! - **`SSE`**: Server-Sent Events for real-time updates to the UI.

pub mod events;
pub mod graph_runner;
mod handlers;
mod routes;
pub mod runner;
pub mod scheduler;
pub mod sse;
pub mod state;

pub use events::{ExecutionStateTracker, StateSnapshot, TraceEventV2};
pub use graph_runner::{
    GraphInterruptHandler, INTERRUPTED_SESSIONS, InterruptData, InterruptedSessionState,
    InterruptedSessionStore,
};
pub use routes::api_routes;
pub use runner::{ActionError, ActionNodeEvent, ActionResult, WorkflowExecutor};
pub use scheduler::{ScheduledJobInfo, get_project_schedules, start_scheduler, stop_scheduler};
pub use sse::cleanup_stale_sessions;
pub use state::AppState;
