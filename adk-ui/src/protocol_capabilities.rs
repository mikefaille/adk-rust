use serde::Serialize;

/// Default runtime protocol profile for server integrations.
pub const UI_DEFAULT_PROTOCOL: &str = "adk_ui";

/// Tool envelope version used by protocol-aware legacy tool responses.
pub const TOOL_ENVELOPE_VERSION: &str = "1.0";

/// Supported runtime protocol profile values.
pub const SUPPORTED_UI_PROTOCOLS: &[&str] = &["adk_ui", "a2ui", "ag_ui", "mcp_apps"];

/// Static capability contract for each supported UI protocol.
#[derive(Debug, Clone, Serialize)]
pub struct UiProtocolCapabilitySpec {
    pub protocol: &'static str,
    pub versions: &'static [&'static str],
    pub features: &'static [&'static str],
}

pub const UI_PROTOCOL_CAPABILITIES: &[UiProtocolCapabilitySpec] = &[
    UiProtocolCapabilitySpec {
        protocol: "adk_ui",
        versions: &["1.0"],
        features: &["legacy_components", "theme", "events"],
    },
    UiProtocolCapabilitySpec {
        protocol: "a2ui",
        versions: &["0.9"],
        features: &["jsonl", "createSurface", "updateComponents", "updateDataModel"],
    },
    UiProtocolCapabilitySpec {
        protocol: "ag_ui",
        versions: &["0.1"],
        features: &["run_lifecycle", "custom_events", "event_stream"],
    },
    UiProtocolCapabilitySpec {
        protocol: "mcp_apps",
        versions: &["sep-1865"],
        features: &["ui_resource_uri", "tool_meta", "html_resource"],
    },
];

/// Normalize runtime UI profile aliases to canonical values.
pub fn normalize_runtime_ui_protocol(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "adk_ui" => Some("adk_ui"),
        "a2ui" => Some("a2ui"),
        "ag_ui" | "ag-ui" => Some("ag_ui"),
        "mcp_apps" | "mcp-apps" => Some("mcp_apps"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_runtime_protocol_accepts_aliases() {
        assert_eq!(normalize_runtime_ui_protocol("adk_ui"), Some("adk_ui"));
        assert_eq!(normalize_runtime_ui_protocol("A2UI"), Some("a2ui"));
        assert_eq!(normalize_runtime_ui_protocol("ag-ui"), Some("ag_ui"));
        assert_eq!(normalize_runtime_ui_protocol("mcp-apps"), Some("mcp_apps"));
        assert_eq!(normalize_runtime_ui_protocol("unknown"), None);
    }

    #[test]
    fn capability_specs_cover_supported_protocols() {
        let protocols: Vec<&str> = UI_PROTOCOL_CAPABILITIES.iter().map(|spec| spec.protocol).collect();
        assert_eq!(protocols, SUPPORTED_UI_PROTOCOLS);
    }

    #[test]
    fn capability_specs_include_versions() {
        for spec in UI_PROTOCOL_CAPABILITIES {
            assert!(!spec.versions.is_empty(), "missing versions for {}", spec.protocol);
            assert!(!spec.features.is_empty(), "missing features for {}", spec.protocol);
        }
    }
}
