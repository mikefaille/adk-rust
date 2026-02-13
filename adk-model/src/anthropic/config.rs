//! Configuration types for Anthropic provider.

use serde::{Deserialize, Serialize};

/// Configuration for Anthropic API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicConfig {
    /// Anthropic API key.
    pub api_key: String,
    /// Model name (e.g., "claude-sonnet-4.5", "claude-3-5-sonnet-20241022").
    pub model: String,
    /// Maximum tokens to generate.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Optional custom base URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

fn default_max_tokens() -> u32 {
    4096
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "claude-sonnet-4.5".to_string(),
            max_tokens: default_max_tokens(),
            base_url: None,
        }
    }
}

impl AnthropicConfig {
    /// Create a new Anthropic config with the given API key and model.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self { api_key: api_key.into(), model: model.into(), ..Default::default() }
    }

    /// Set the maximum tokens to generate.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Set a custom base URL.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }
}
