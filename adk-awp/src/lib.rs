//! # adk-awp
//!
//! Agentic Web Protocol (AWP) implementation for ADK-Rust.
//!
//! This crate provides the full AWP protocol implementation including:
//!
//! - **Configuration**: TOML-based business context loading with hot-reload
//! - **Discovery**: Auto-generated discovery documents from business context
//! - **Manifest**: JSON-LD capability manifest builder
//! - **Detection**: Requester type detection (human vs. agent)
//! - **Trust**: Trust level assignment from request headers
//! - **Middleware**: AWP version negotiation
//! - **Error responses**: AWP error to HTTP response conversion
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use adk_awp::{BusinessContextLoader, generate_discovery_document, build_manifest};
//!
//! let loader = BusinessContextLoader::from_file("business.toml".as_ref())?;
//! let ctx = loader.load();
//! let discovery = generate_discovery_document(&ctx);
//! let manifest = build_manifest(&ctx);
//! ```

pub mod config;
pub mod detect;
pub mod discovery;
pub mod error_response;
pub mod loader;
pub mod manifest;
pub mod middleware;
pub mod trust;

pub use config::{AwpConfigError, business_context_to_toml};
pub use detect::detect_requester_type;
pub use discovery::generate_discovery_document;
pub use loader::BusinessContextLoader;
pub use manifest::build_manifest;
pub use trust::{DefaultTrustAssigner, TrustLevelAssigner};
