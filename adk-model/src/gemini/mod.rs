pub mod client;
pub mod streaming;

pub use crate::retry::RetryConfig;
pub use client::GeminiModel;

// Re-export thinking config types from adk-gemini so users don't need
// a direct dependency on adk-gemini to configure thinking.
pub use adk_gemini::{ThinkingConfig, ThinkingLevel};
