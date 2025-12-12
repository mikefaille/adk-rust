use crate::schema::*;
use adk_core::{Result, Tool, ToolContext};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Parameters for the render_confirm tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RenderConfirmParams {
    /// Confirmation title
    pub title: String,
    /// Message explaining what the user is confirming
    pub message: String,
    /// Action ID triggered when user confirms
    pub confirm_action: String,
    /// Optional cancel action ID (defaults to dismissing)
    #[serde(default)]
    pub cancel_action: Option<String>,
    /// Confirm button label
    #[serde(default = "default_confirm_label")]
    pub confirm_label: String,
    /// Cancel button label
    #[serde(default = "default_cancel_label")]
    pub cancel_label: String,
    /// Whether this is a destructive action (shows danger button)
    #[serde(default)]
    pub destructive: bool,
}

fn default_confirm_label() -> String {
    "Confirm".to_string()
}

fn default_cancel_label() -> String {
    "Cancel".to_string()
}

/// Tool for rendering confirmation dialogs
pub struct RenderConfirmTool;

impl RenderConfirmTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RenderConfirmTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for RenderConfirmTool {
    fn name(&self) -> &str {
        "render_confirm"
    }

    fn description(&self) -> &str {
        "Render a confirmation dialog to get user approval before proceeding. Use this for destructive actions, important decisions, or when you need explicit user consent."
    }

    fn parameters_schema(&self) -> Option<Value> {
        Some(super::generate_gemini_schema::<RenderConfirmParams>())
    }

    async fn execute(&self, _ctx: Arc<dyn ToolContext>, args: Value) -> Result<Value> {
        let params: RenderConfirmParams = serde_json::from_value(args)
            .map_err(|e| adk_core::AdkError::Tool(format!("Invalid parameters: {}", e)))?;

        let confirm_variant =
            if params.destructive { ButtonVariant::Danger } else { ButtonVariant::Primary };

        let footer = vec![
            Component::Button(Button {
                id: None,
                label: params.cancel_label,
                action_id: params.cancel_action.unwrap_or_else(|| "cancel".to_string()),
                variant: ButtonVariant::Ghost,
                disabled: false,
                icon: None,
            }),
            Component::Button(Button {
                id: None,
                label: params.confirm_label,
                action_id: params.confirm_action,
                variant: confirm_variant,
                disabled: false,
                icon: None,
            }),
        ];

        let ui = UiResponse::new(vec![Component::Card(Card {
            id: None,
            title: Some(params.title),
            description: None,
            content: vec![Component::Text(Text {
                id: None,
                content: params.message,
                variant: TextVariant::Body,
            })],
            footer: Some(footer),
        })]);

        serde_json::to_value(ui)
            .map_err(|e| adk_core::AdkError::Tool(format!("Failed to serialize UI: {}", e)))
    }
}
