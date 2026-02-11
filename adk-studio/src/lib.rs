//! # ADK Studio
//!
//! Visual development environment and server for ADK-Rust agents.
//!
//! ADK Studio provides a web-based interface for designing, configuring, and testing
//! AI agents. It uses a build-only architecture where agents are designed visually
//! but compiled into native Rust binaries for execution.
//!
//! ## Architecture
//!
//! - **Visual Design**: Users create agent workflows, define tools, and configure models in the UI.
//! - **Code Generation**: The studio generates Rust code (`codegen` module) from the visual design.
//! - **Compilation**: The generated code is compiled into a standalone executable.
//! - **Execution**: The compiled agent runs as a separate process, communicating back to the Studio via events.
//!
//! ## Modules
//!
//! - **`codegen`**: Generates Rust source code from agent definitions.
//! - **`server`**: The backend API server that powers the Studio UI and manages builds.
//! - **`schema`**: Data structures representing the visual agent design (JSON schema).
//! - **`storage`**: File system abstraction for project persistence.
//! - **`embedded`**: Embedded assets for the Studio frontend.

pub mod codegen;
pub mod embedded;
pub mod schema;
pub mod server;
pub mod storage;

pub use schema::{AgentSchema, ProjectSchema, ToolSchema, WorkflowSchema};
pub use server::{AppState, api_routes, cleanup_stale_sessions, start_scheduler, stop_scheduler};
pub use storage::FileStorage;
