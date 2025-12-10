//! DeepSeek provider implementation for ADK.
//!
//! This module provides support for DeepSeek models including:
//! - `deepseek-chat` - General-purpose chat model
//! - `deepseek-reasoner` - Reasoning model with thinking mode (chain-of-thought)
//!
//! # Features
//!
//! - **Thinking Mode**: Enable chain-of-thought reasoning with `reasoning_content`
//! - **Tool Calling**: Full function/tool calling support
//! - **Streaming**: Real-time streaming responses
//! - **Prefix Caching**: Automatic KV cache optimization
//!
//! # Example
//!
//! ```rust,ignore
//! use adk_model::deepseek::{DeepSeekClient, DeepSeekConfig};
//!
//! // Basic chat model
//! let chat = DeepSeekClient::new(DeepSeekConfig::chat(
//!     std::env::var("DEEPSEEK_API_KEY").unwrap()
//! ))?;
//!
//! // Reasoner model with thinking enabled
//! let reasoner = DeepSeekClient::new(DeepSeekConfig::reasoner(
//!     std::env::var("DEEPSEEK_API_KEY").unwrap()
//! ))?;
//!
//! // Custom configuration
//! let custom = DeepSeekClient::new(
//!     DeepSeekConfig::new("api-key", "deepseek-chat")
//!         .with_thinking(true)
//!         .with_max_tokens(8192)
//! )?;
//! ```
//!
//! # Supported Models
//!
//! | Model | Description |
//! |-------|-------------|
//! | `deepseek-chat` | Fast, capable chat model |
//! | `deepseek-reasoner` | Reasoning model with thinking mode |
//!
//! # Thinking Mode
//!
//! When using `deepseek-reasoner` or enabling thinking mode, the model outputs
//! its chain-of-thought reasoning before providing the final answer. The reasoning
//! is returned in `reasoning_content` and automatically formatted with `<thinking>` tags.

mod client;
mod config;
mod convert;

pub use client::DeepSeekClient;
pub use config::{DeepSeekConfig, DEEPSEEK_API_BASE, DEEPSEEK_BETA_API_BASE};
