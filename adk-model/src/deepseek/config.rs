//! Configuration types for DeepSeek provider.

use serde::{Deserialize, Serialize};

/// Default DeepSeek API base URL.
pub const DEEPSEEK_API_BASE: &str = "https://api.deepseek.com";

/// DeepSeek beta API base URL (for FIM completion).
pub const DEEPSEEK_BETA_API_BASE: &str = "https://api.deepseek.com/beta";

/// Configuration for DeepSeek API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepSeekConfig {
    /// DeepSeek API key.
    pub api_key: String,
    /// Model name (e.g., "deepseek-chat", "deepseek-reasoner").
    pub model: String,
    /// Optional custom base URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Enable thinking mode for reasoning models.
    /// When enabled, the model outputs chain-of-thought reasoning before the final answer.
    #[serde(default)]
    pub thinking_enabled: bool,
    /// Maximum tokens for output (default: 4096, max for reasoner: 64K).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

impl Default for DeepSeekConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "deepseek-chat".to_string(),
            base_url: None,
            thinking_enabled: false,
            max_tokens: None,
        }
    }
}

impl DeepSeekConfig {
    /// Create a new DeepSeek config with the given API key and model.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self { api_key: api_key.into(), model: model.into(), ..Default::default() }
    }

    /// Create a config for deepseek-chat model.
    pub fn chat(api_key: impl Into<String>) -> Self {
        Self::new(api_key, "deepseek-chat")
    }

    /// Create a config for deepseek-reasoner model with thinking enabled.
    pub fn reasoner(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: "deepseek-reasoner".to_string(),
            thinking_enabled: true,
            max_tokens: Some(8192),
            ..Default::default()
        }
    }

    /// Enable thinking mode (chain-of-thought reasoning).
    pub fn with_thinking(mut self, enabled: bool) -> Self {
        self.thinking_enabled = enabled;
        self
    }

    /// Set max tokens for output.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Set custom base URL.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Get the effective base URL.
    pub fn effective_base_url(&self) -> &str {
        self.base_url.as_deref().unwrap_or(DEEPSEEK_API_BASE)
    }
}
