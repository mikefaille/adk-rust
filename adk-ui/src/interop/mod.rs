pub mod ag_ui;
pub mod mcp_apps;
pub mod surface;

pub use ag_ui::{
    ADK_UI_SURFACE_EVENT_NAME, AgUiCustomEvent, AgUiEvent, AgUiEventType, surface_to_event_stream,
};
pub use mcp_apps::{
    MCP_APPS_HTML_MIME_TYPE, McpAppsRenderOptions, McpAppsSurfacePayload, McpToolVisibility,
    surface_to_mcp_apps_payload,
};
pub use surface::{UiProtocol, UiSurface};
