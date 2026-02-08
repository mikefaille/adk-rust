use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffRequest {
    pub from_app: String,
    pub to_app: String,
    pub context_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffDecision {
    pub allowed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffPolicyDecision {
    pub allowed: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingHandoff {
    pub handoff_id: String,
    pub request: HandoffRequest,
}

pub fn parse_handoff_command(from_app: &str, command: &str) -> Option<HandoffRequest> {
    let trimmed = command.trim();
    if !trimmed.to_lowercase().starts_with("handoff ") {
        return None;
    }
    let rest = trimmed.get(8..)?.trim();
    let (to_app, context_summary) = rest.split_once('|')?;
    let to_app = to_app.trim();
    let context_summary = context_summary.trim();
    if to_app.is_empty() || context_summary.is_empty() {
        return None;
    }
    Some(HandoffRequest {
        from_app: from_app.to_string(),
        to_app: to_app.to_string(),
        context_summary: context_summary.to_string(),
    })
}

pub fn evaluate_handoff(request: &HandoffRequest, user_allowed: bool) -> HandoffDecision {
    if !user_allowed {
        return HandoffDecision {
            allowed: false,
            reason: "user rejected handoff request".to_string(),
        };
    }

    if request.from_app == request.to_app {
        return HandoffDecision {
            allowed: true,
            reason: "intra-app handoff allowed".to_string(),
        };
    }

    HandoffDecision {
        allowed: true,
        reason: "user-governed cross-app handoff".to_string(),
    }
}
