use axum::Json;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct UiProtocolCapability {
    pub protocol: &'static str,
    pub versions: Vec<&'static str>,
    pub features: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiCapabilities {
    pub default_protocol: &'static str,
    pub protocols: Vec<UiProtocolCapability>,
    pub tool_envelope_version: &'static str,
}

/// GET /api/ui/capabilities
pub async fn ui_capabilities() -> Json<UiCapabilities> {
    Json(UiCapabilities {
        default_protocol: "adk_ui",
        protocols: vec![
            UiProtocolCapability {
                protocol: "adk_ui",
                versions: vec!["1.0"],
                features: vec!["legacy_components", "theme", "events"],
            },
            UiProtocolCapability {
                protocol: "a2ui",
                versions: vec!["0.9"],
                features: vec!["jsonl", "createSurface", "updateComponents", "updateDataModel"],
            },
            UiProtocolCapability {
                protocol: "ag_ui",
                versions: vec!["0.1"],
                features: vec!["run_lifecycle", "custom_events", "event_stream"],
            },
            UiProtocolCapability {
                protocol: "mcp_apps",
                versions: vec!["sep-1865"],
                features: vec!["ui_resource_uri", "tool_meta", "html_resource"],
            },
        ],
        tool_envelope_version: "1.0",
    })
}
