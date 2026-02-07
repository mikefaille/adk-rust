use axum::{
    Json,
    extract::Query,
    http::StatusCode,
};
use adk_ui::{TOOL_ENVELOPE_VERSION, UI_DEFAULT_PROTOCOL, UI_PROTOCOL_CAPABILITIES};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use tracing::{info, warn};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiResource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub mime_type: String,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiResourceContent {
    pub uri: String,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiResourceListResponse {
    pub resources: Vec<UiResource>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiResourceReadResponse {
    pub contents: Vec<UiResourceContent>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterUiResourceRequest {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub mime_type: String,
    pub text: String,
    #[serde(rename = "_meta", default)]
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReadUiResourceQuery {
    pub uri: String,
}

#[derive(Debug, Clone)]
struct UiResourceEntry {
    resource: UiResource,
    content: UiResourceContent,
}

static UI_RESOURCE_REGISTRY: OnceLock<RwLock<HashMap<String, UiResourceEntry>>> = OnceLock::new();

fn resource_registry() -> &'static RwLock<HashMap<String, UiResourceEntry>> {
    UI_RESOURCE_REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

fn validate_ui_resource_uri(uri: &str) -> Result<(), (StatusCode, String)> {
    if !uri.starts_with("ui://") {
        return Err((
            StatusCode::BAD_REQUEST,
            "ui resource uri must start with 'ui://'".to_string(),
        ));
    }
    Ok(())
}

fn validate_ui_resource_mime(mime_type: &str) -> Result<(), (StatusCode, String)> {
    if mime_type != "text/html;profile=mcp-app" {
        return Err((
            StatusCode::BAD_REQUEST,
            "mimeType must be 'text/html;profile=mcp-app'".to_string(),
        ));
    }
    Ok(())
}

fn is_allowed_domain(domain: &str) -> bool {
    domain.starts_with("https://")
        || domain.starts_with("http://localhost")
        || domain.starts_with("http://127.0.0.1")
}

fn validate_domain_list(value: &Value, field_name: &str) -> Result<(), (StatusCode, String)> {
    let list = value.as_array().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!("_meta.ui.csp.{} must be an array of domain strings", field_name),
        )
    })?;
    for entry in list {
        let domain = entry.as_str().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("_meta.ui.csp.{} entries must be strings", field_name),
            )
        })?;
        if !is_allowed_domain(domain) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "_meta.ui.csp.{} contains unsupported domain '{}'",
                    field_name, domain
                ),
            ));
        }
    }
    Ok(())
}

fn validate_ui_meta(meta: &Option<Value>) -> Result<(), (StatusCode, String)> {
    let Some(meta_value) = meta else {
        return Ok(());
    };
    let meta_object = meta_value.as_object().ok_or_else(|| {
        (StatusCode::BAD_REQUEST, "_meta must be a JSON object".to_string())
    })?;
    let Some(ui_value) = meta_object.get("ui") else {
        return Ok(());
    };
    let ui_object = ui_value.as_object().ok_or_else(|| {
        (StatusCode::BAD_REQUEST, "_meta.ui must be a JSON object".to_string())
    })?;

    if let Some(domain_value) = ui_object.get("domain") {
        let domain = domain_value.as_str().ok_or_else(|| {
            (StatusCode::BAD_REQUEST, "_meta.ui.domain must be a string".to_string())
        })?;
        if !is_allowed_domain(domain) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "_meta.ui.domain '{}' is not allowed; use https:// or localhost URLs",
                    domain
                ),
            ));
        }
    }

    if let Some(csp_value) = ui_object.get("csp") {
        let csp_object = csp_value.as_object().ok_or_else(|| {
            (StatusCode::BAD_REQUEST, "_meta.ui.csp must be an object".to_string())
        })?;
        for field in ["connectDomains", "resourceDomains", "frameDomains", "baseUriDomains"] {
            if let Some(field_value) = csp_object.get(field) {
                validate_domain_list(field_value, field)?;
            }
        }
    }

    if let Some(permissions_value) = ui_object.get("permissions") {
        if !permissions_value.is_object() {
            return Err((
                StatusCode::BAD_REQUEST,
                "_meta.ui.permissions must be an object".to_string(),
            ));
        }
    }

    Ok(())
}

fn extract_meta_domain(meta: &Option<Value>) -> Option<String> {
    let meta_object = meta.as_ref()?.as_object()?;
    let ui_object = meta_object.get("ui")?.as_object()?;
    ui_object.get("domain")?.as_str().map(ToString::to_string)
}

/// GET /api/ui/capabilities
pub async fn ui_capabilities() -> Json<UiCapabilities> {
    Json(UiCapabilities {
        default_protocol: UI_DEFAULT_PROTOCOL,
        protocols: UI_PROTOCOL_CAPABILITIES
            .iter()
            .map(|spec| UiProtocolCapability {
                protocol: spec.protocol,
                versions: spec.versions.to_vec(),
                features: spec.features.to_vec(),
            })
            .collect(),
        tool_envelope_version: TOOL_ENVELOPE_VERSION,
    })
}

/// GET /api/ui/resources
pub async fn list_ui_resources() -> Json<UiResourceListResponse> {
    let resources: Vec<UiResource> = resource_registry()
        .read()
        .map(|registry| registry.values().map(|entry| entry.resource.clone()).collect())
        .unwrap_or_default();
    info!(resource_count = resources.len(), "ui resource list requested");
    Json(UiResourceListResponse { resources })
}

/// GET /api/ui/resources/read?uri=ui://...
pub async fn read_ui_resource(
    Query(query): Query<ReadUiResourceQuery>,
) -> Result<Json<UiResourceReadResponse>, (StatusCode, String)> {
    validate_ui_resource_uri(&query.uri)?;
    let guard = resource_registry()
        .read()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "resource registry poisoned".to_string()))?;
    let Some(entry) = guard.get(&query.uri) else {
        warn!(uri = %query.uri, "ui resource read failed: not found");
        return Err((StatusCode::NOT_FOUND, format!("resource not found: {}", query.uri)));
    };
    info!(uri = %query.uri, "ui resource read");
    Ok(Json(UiResourceReadResponse { contents: vec![entry.content.clone()] }))
}

/// POST /api/ui/resources/register
pub async fn register_ui_resource(
    Json(req): Json<RegisterUiResourceRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_ui_resource_uri(&req.uri)?;
    validate_ui_resource_mime(&req.mime_type)?;
    validate_ui_meta(&req.meta)?;

    let uri = req.uri.clone();
    let name = req.name.clone();
    let mime_type = req.mime_type.clone();
    let meta = req.meta.clone();
    let domain = extract_meta_domain(&meta).unwrap_or_else(|| "<none>".to_string());

    let entry = UiResourceEntry {
        resource: UiResource {
            uri: uri.clone(),
            name: name.clone(),
            description: req.description.clone(),
            mime_type: mime_type.clone(),
            meta: meta.clone(),
        },
        content: UiResourceContent {
            uri: uri.clone(),
            mime_type: mime_type.clone(),
            text: Some(req.text),
            blob: None,
            meta,
        },
    };

    resource_registry()
        .write()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "resource registry poisoned".to_string()))?
        .insert(uri.clone(), entry);
    info!(
        uri = %uri,
        name = %name,
        mime_type = %mime_type,
        ui_domain = %domain,
        "ui resource registered"
    );

    Ok(StatusCode::CREATED)
}
