use adk_core::prelude::*;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Severity level for guardrail failures
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

/// Result of guardrail validation
#[derive(Debug, Clone)]
pub enum GuardrailResult {
    /// Content passed validation
    Pass,
    /// Content failed validation
    Fail { reason: String, severity: Severity },
    /// Content was transformed (e.g., PII redacted)
    Transform { new_content: Content, reason: String },
}

impl GuardrailResult {
    pub fn pass() -> Self {
        Self::Pass
    }

    pub fn fail(reason: impl Into<String>, severity: Severity) -> Self {
        Self::Fail { reason: reason.into(), severity }
    }

    pub fn transform(new_content: Content, reason: impl Into<String>) -> Self {
        Self::Transform { new_content, reason: reason.into() }
    }

    pub fn is_pass(&self) -> bool {
        matches!(self, Self::Pass)
    }

    pub fn is_fail(&self) -> bool {
        matches!(self, Self::Fail { .. })
    }
}

/// Core guardrail trait for input/output validation
#[async_trait]
pub trait Guardrail: Send + Sync {
    /// Unique name for this guardrail
    fn name(&self) -> &str;

    /// Validate content and return result
    async fn validate(&self, content: &Content) -> GuardrailResult;

    /// Whether to run in parallel with other guardrails (default: true)
    fn run_parallel(&self) -> bool {
        true
    }

    /// Whether to fail fast on this guardrail's failure (default: true for High/Critical)
    fn fail_fast(&self) -> bool {
        true
    }
}
