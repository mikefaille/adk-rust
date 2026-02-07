use super::surface::UiSurface;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Event name used for surface payload transport via AG-UI custom events.
pub const ADK_UI_SURFACE_EVENT_NAME: &str = "adk.ui.surface";

/// AG-UI event types from the protocol event model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AgUiEventType {
    RunStarted,
    RunFinished,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgUiRunStartedEvent {
    #[serde(rename = "type")]
    pub event_type: AgUiEventType,
    pub thread_id: String,
    pub run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgUiRunFinishedEvent {
    #[serde(rename = "type")]
    pub event_type: AgUiEventType,
    pub thread_id: String,
    pub run_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgUiCustomEvent {
    #[serde(rename = "type")]
    pub event_type: AgUiEventType,
    pub name: String,
    pub value: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_event: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgUiEvent {
    RunStarted(AgUiRunStartedEvent),
    Custom(AgUiCustomEvent),
    RunFinished(AgUiRunFinishedEvent),
}

pub fn surface_to_custom_event(surface: &UiSurface) -> AgUiCustomEvent {
    AgUiCustomEvent {
        event_type: AgUiEventType::Custom,
        name: ADK_UI_SURFACE_EVENT_NAME.to_string(),
        value: json!({
            "format": "adk-ui-surface-v1",
            "surface": surface
        }),
        timestamp: None,
        raw_event: None,
    }
}

pub fn surface_to_event_stream(
    surface: &UiSurface,
    thread_id: impl Into<String>,
    run_id: impl Into<String>,
) -> Vec<AgUiEvent> {
    let thread_id = thread_id.into();
    let run_id = run_id.into();

    vec![
        AgUiEvent::RunStarted(AgUiRunStartedEvent {
            event_type: AgUiEventType::RunStarted,
            thread_id: thread_id.clone(),
            run_id: run_id.clone(),
        }),
        AgUiEvent::Custom(surface_to_custom_event(surface)),
        AgUiEvent::RunFinished(AgUiRunFinishedEvent {
            event_type: AgUiEventType::RunFinished,
            thread_id,
            run_id,
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn surface_custom_event_is_well_formed() {
        let surface = UiSurface::new(
            "main",
            "catalog",
            vec![json!({"id":"root","component":{"Column":{"children":[]}}})],
        );
        let event = surface_to_custom_event(&surface);
        assert_eq!(event.event_type, AgUiEventType::Custom);
        assert_eq!(event.name, ADK_UI_SURFACE_EVENT_NAME);
        assert!(event.value.get("surface").is_some());
    }

    #[test]
    fn event_stream_wraps_custom_event_with_lifecycle() {
        let surface = UiSurface::new(
            "main",
            "catalog",
            vec![json!({"id":"root","component":{"Column":{"children":[]}}})],
        );
        let stream = surface_to_event_stream(&surface, "thread-1", "run-1");
        assert_eq!(stream.len(), 3);

        let first = serde_json::to_value(&stream[0]).unwrap();
        let second = serde_json::to_value(&stream[1]).unwrap();
        let third = serde_json::to_value(&stream[2]).unwrap();

        assert_eq!(first["type"], "RUN_STARTED");
        assert_eq!(second["type"], "CUSTOM");
        assert_eq!(third["type"], "RUN_FINISHED");
    }
}
