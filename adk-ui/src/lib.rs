//! # adk-ui
//!
//! UI components and tools for ADK-Rust agents.
//!
//! ## Overview
//!
//! The `adk-ui` crate provides a comprehensive set of tools for agents to render rich UI components
//! in compatible clients (like ADK Studio or custom frontends). It allows agents to go beyond simple text
//! responses and present structured data, forms, charts, and interactive elements.
//!
//! ## Key Features
//!
//! - **UI Toolset**: A collection of tools (`UiToolset`) that agents can use to render UI components.
//! - **Component Library**: Support for various UI components including:
//!   - Forms and inputs
//!   - Data tables and charts
//!   - Cards and layouts
//!   - Alerts, confirmations, and modals
//!   - Progress indicators and toasts
//! - **Kit Generation**: Tools for generating consistent UI kits and design systems (`kit` module).
//! - **Templating**: Utilities for rendering UI templates with data.
//!
//! ## Quick Start
//!
//! To add UI capabilities to your agent, use the `UiToolset`:
//!
//! ```rust,no_run
//! use adk_ui::UiToolset;
//! // Assuming you have an agent builder available
//! // let mut builder = AgentBuilder::new("my-agent");
//!
//! // Add all UI tools to the agent
//! // builder.with_toolset(UiToolset::new());
//! ```
//!
//! You can also selectively enable specific UI capabilities:
//!
//! ```rust,no_run
//! use adk_ui::UiToolset;
//!
//! // Create a toolset with only form rendering capabilities
//! let tools = UiToolset::forms_only();
//! ```

pub mod a2ui;
pub mod catalog_registry;
pub mod interop;
pub mod kit;
pub mod model;
pub mod prompts;
pub mod protocol_capabilities;
pub mod schema;
pub mod templates;
pub mod tools;
pub mod toolset;
pub mod validation;

pub use a2ui::*;
pub use catalog_registry::{CatalogArtifact, CatalogError, CatalogRegistry, CatalogSource};
pub use interop::*;
pub use kit::{KitArtifacts, KitGenerator, KitSpec};
pub use model::{ToolEnvelope, ToolEnvelopeProtocol};
pub use prompts::{UI_AGENT_PROMPT, UI_AGENT_PROMPT_SHORT};
pub use protocol_capabilities::{
    ADK_UI_LEGACY_DEPRECATION, SUPPORTED_UI_PROTOCOLS, TOOL_ENVELOPE_VERSION, UI_DEFAULT_PROTOCOL,
    UI_PROTOCOL_CAPABILITIES, UiProtocolCapabilitySpec, UiProtocolDeprecationSpec,
    normalize_runtime_ui_protocol,
};
pub use schema::*;
pub use templates::{StatItem, TemplateData, UiTemplate, UserData, render_template};
pub use tools::*;
pub use toolset::UiToolset;
pub use validation::{Validate, ValidationError, validate_ui_response};
