use serde_json::json;

use crate::protocol::TimelineEntryPayload;

pub fn route_entry(prompt: &str, selected_apps: &[String], rationale: &str) -> TimelineEntryPayload {
    TimelineEntryPayload {
        level: "info".to_string(),
        message: "Master Prompt routed to app set".to_string(),
        fields: json!({
            "prompt": prompt,
            "selected_apps": selected_apps,
            "rationale": rationale,
        })
        .as_object()
        .cloned()
        .unwrap_or_default(),
    }
}

pub fn approval_entry(action_id: &str, approved: bool) -> TimelineEntryPayload {
    TimelineEntryPayload {
        level: if approved { "success".to_string() } else { "warn".to_string() },
        message: if approved {
            "User approved pending action".to_string()
        } else {
            "User rejected pending action".to_string()
        },
        fields: json!({
            "action_id": action_id,
            "approved": approved,
        })
        .as_object()
        .cloned()
        .unwrap_or_default(),
    }
}

pub fn workspace_layout_entry(layout: &str) -> TimelineEntryPayload {
    let parsed_layout = serde_json::from_str::<serde_json::Value>(layout)
        .unwrap_or_else(|_| serde_json::Value::String(layout.to_string()));
    let surface_count = parsed_layout
        .as_array()
        .map(|items| items.len())
        .unwrap_or(0);

    TimelineEntryPayload {
        level: "info".to_string(),
        message: format!("Workspace layout updated ({surface_count} surfaces)"),
        fields: json!({
            "surface_count": surface_count,
            "layout": parsed_layout,
        })
        .as_object()
        .cloned()
        .unwrap_or_default(),
    }
}

pub fn app_command_entry(app_id: &str, command: &str, accepted: bool, summary: &str) -> TimelineEntryPayload {
    TimelineEntryPayload {
        level: if accepted { "info".to_string() } else { "warn".to_string() },
        message: format!("Command dispatched to {app_id}"),
        fields: json!({
            "app_id": app_id,
            "command": command,
            "accepted": accepted,
            "summary": summary,
        })
        .as_object()
        .cloned()
        .unwrap_or_default(),
    }
}

pub fn handoff_requested_entry(from_app: &str, to_app: &str, context_summary: &str, handoff_id: &str) -> TimelineEntryPayload {
    TimelineEntryPayload {
        level: "warn".to_string(),
        message: format!("Handoff requested: {from_app} -> {to_app}"),
        fields: json!({
            "handoff_id": handoff_id,
            "from_app": from_app,
            "to_app": to_app,
            "context_summary": context_summary,
        })
        .as_object()
        .cloned()
        .unwrap_or_default(),
    }
}

pub fn handoff_decision_entry(handoff_id: &str, from_app: &str, to_app: &str, allowed: bool, reason: &str) -> TimelineEntryPayload {
    TimelineEntryPayload {
        level: if allowed { "success".to_string() } else { "warn".to_string() },
        message: if allowed {
            format!("Handoff approved: {from_app} -> {to_app}")
        } else {
            format!("Handoff denied: {from_app} -> {to_app}")
        },
        fields: json!({
            "handoff_id": handoff_id,
            "from_app": from_app,
            "to_app": to_app,
            "allowed": allowed,
            "reason": reason,
        })
        .as_object()
        .cloned()
        .unwrap_or_default(),
    }
}

pub fn handoff_policy_entry(handoff_id: &str, from_app: &str, to_app: &str, allowed: bool, reason: &str) -> TimelineEntryPayload {
    TimelineEntryPayload {
        level: if allowed { "info".to_string() } else { "warn".to_string() },
        message: if allowed {
            format!("Handoff policy allows {from_app} -> {to_app}")
        } else {
            format!("Handoff policy blocked {from_app} -> {to_app}")
        },
        fields: json!({
            "handoff_id": handoff_id,
            "from_app": from_app,
            "to_app": to_app,
            "policy_allowed": allowed,
            "policy_reason": reason,
        })
        .as_object()
        .cloned()
        .unwrap_or_default(),
    }
}
