use serde::{Deserialize, Serialize};

use crate::safety::risk::RiskTier;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handoff_allowlist: Vec<String>,
    pub default_risk: RiskTier,
}

pub fn default_manifests() -> Vec<AppManifest> {
    vec![
        AppManifest {
            id: "ops-center".to_string(),
            name: "Ops Center".to_string(),
            version: "0.1.0".to_string(),
            description: "Service health, incidents, and runbook remediation".to_string(),
            capabilities: vec!["ops".to_string(), "incident".to_string(), "remediation".to_string()],
            permissions: vec!["read.telemetry".to_string(), "write.remediation".to_string()],
            handoff_allowlist: vec!["mail-agent".to_string(), "calendar-agent".to_string()],
            default_risk: RiskTier::Controlled,
        },
        AppManifest {
            id: "mail-agent".to_string(),
            name: "Mail Agent".to_string(),
            version: "0.1.0".to_string(),
            description: "Inbox triage, drafting, and follow-ups".to_string(),
            capabilities: vec!["email".to_string(), "summarize".to_string()],
            permissions: vec!["read.mail".to_string(), "send.mail".to_string()],
            handoff_allowlist: vec!["ops-center".to_string()],
            default_risk: RiskTier::Controlled,
        },
        AppManifest {
            id: "calendar-agent".to_string(),
            name: "Calendar Agent".to_string(),
            version: "0.1.0".to_string(),
            description: "Scheduling, conflict resolution, and prep briefs".to_string(),
            capabilities: vec!["calendar".to_string(), "scheduling".to_string()],
            permissions: vec!["read.calendar".to_string(), "write.calendar".to_string()],
            handoff_allowlist: vec!["ops-center".to_string()],
            default_risk: RiskTier::Safe,
        },
    ]
}
