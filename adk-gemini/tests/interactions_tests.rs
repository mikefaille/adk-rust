//! Wire-format and behavior tests for the Interactions API types.
//!
//! These tests validate (de)serialization against the documented Gemini
//! Interactions API schema (steps schema, `Api-Revision: 2026-05-20`). They run
//! with `--features interactions` and require no network access.

#![cfg(feature = "interactions")]

use adk_gemini::interactions::{
    Content, CreateInteractionRequest, Input, Interaction, InteractionSseEvent, InteractionStatus,
    ResponseFormat, Step, StepDelta, Tool,
};
use serde_json::json;

#[test]
fn parses_simple_model_output_response() {
    let body = json!({
        "id": "v1_abc",
        "model": "gemini-3.5-flash",
        "object": "interaction",
        "status": "completed",
        "steps": [
            { "type": "model_output", "content": [{ "type": "text", "text": "Hello there." }] }
        ],
        "usage": {
            "total_input_tokens": 7,
            "total_output_tokens": 20,
            "total_thought_tokens": 22,
            "total_tokens": 49
        }
    });

    let interaction: Interaction = serde_json::from_value(body).unwrap();
    assert_eq!(interaction.id, "v1_abc");
    assert_eq!(interaction.status, InteractionStatus::Completed);
    assert!(interaction.status.is_terminal());
    assert_eq!(interaction.output_text().as_deref(), Some("Hello there."));
    assert_eq!(interaction.usage.unwrap().total_tokens, 49);
}

#[test]
fn output_text_returns_last_model_output() {
    // Interleaved thought + model_output: output_text should skip the thought
    // and return the final model_output text.
    let body = json!({
        "id": "v1_x",
        "status": "completed",
        "steps": [
            { "type": "thought", "summary": [{ "type": "text", "text": "thinking..." }] },
            { "type": "model_output", "content": [{ "type": "text", "text": "The answer is 42." }] }
        ]
    });
    let interaction: Interaction = serde_json::from_value(body).unwrap();
    assert_eq!(interaction.output_text().as_deref(), Some("The answer is 42."));
}

#[test]
fn parses_function_call_requires_action() {
    let body = json!({
        "id": "v1_fc",
        "status": "requires_action",
        "steps": [
            {
                "type": "function_call",
                "id": "gth23981",
                "name": "get_weather",
                "arguments": { "location": "Boston, MA" }
            }
        ]
    });

    let interaction: Interaction = serde_json::from_value(body).unwrap();
    assert!(interaction.status.requires_action());
    let pending = interaction.pending_function_calls();
    assert_eq!(pending.len(), 1);
    let (id, name, args) = &pending[0];
    assert_eq!(id, "gth23981");
    assert_eq!(name, "get_weather");
    assert_eq!(args["location"], json!("Boston, MA"));
    // A requires_action interaction has no final text output.
    assert_eq!(interaction.output_text(), None);
}

#[test]
fn server_side_tool_steps_round_trip_as_other() {
    // Server-side tool steps we don't model explicitly must survive as Step::Other.
    let body = json!({
        "id": "v1_search",
        "status": "completed",
        "steps": [
            { "type": "google_search_call", "id": "search_1", "arguments": { "queries": ["x"] } },
            { "type": "model_output", "content": [{ "type": "text", "text": "done" }] }
        ]
    });
    let interaction: Interaction = serde_json::from_value(body).unwrap();
    assert_eq!(interaction.steps.len(), 2);
    assert!(matches!(interaction.steps[0], Step::Other(_)));
    assert_eq!(interaction.output_text().as_deref(), Some("done"));
}

#[test]
fn request_serializes_with_bare_text_input() {
    let req = CreateInteractionRequest {
        model: Some("gemini-3.5-flash".to_string()),
        input: Input::Text("Hello".to_string()),
        ..Default::default()
    };
    let value = serde_json::to_value(&req).unwrap();
    assert_eq!(value["model"], json!("gemini-3.5-flash"));
    assert_eq!(value["input"], json!("Hello"));
    // Optional/empty fields are omitted.
    assert!(value.get("tools").is_none());
    assert!(value.get("agent").is_none());
    assert!(value.get("stream").is_none());
}

#[test]
fn request_serializes_function_tool() {
    let req = CreateInteractionRequest {
        model: Some("gemini-3-flash-preview".to_string()),
        input: Input::Text("weather in Boston?".to_string()),
        tools: vec![Tool::function(
            "get_weather",
            "Get the current weather",
            json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        )],
        ..Default::default()
    };
    let value = serde_json::to_value(&req).unwrap();
    let tool = &value["tools"][0];
    assert_eq!(tool["type"], json!("function"));
    assert_eq!(tool["name"], json!("get_weather"));
    assert_eq!(tool["parameters"]["type"], json!("object"));
}

#[test]
fn built_in_tools_serialize_by_discriminator() {
    let value = serde_json::to_value(Tool::CodeExecution).unwrap();
    assert_eq!(value, json!({ "type": "code_execution" }));

    let value = serde_json::to_value(Tool::UrlContext).unwrap();
    assert_eq!(value, json!({ "type": "url_context" }));
}

#[test]
fn multi_turn_steps_input_serializes() {
    let req = CreateInteractionRequest {
        model: Some("gemini-3.5-flash".to_string()),
        input: Input::Steps(vec![
            Step::UserInput { content: vec![Content::text("Hello!")] },
            Step::ModelOutput { content: vec![Content::text("Hi there!")] },
            Step::UserInput { content: vec![Content::text("Capital of France?")] },
        ]),
        ..Default::default()
    };
    let value = serde_json::to_value(&req).unwrap();
    let steps = value["input"].as_array().unwrap();
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0]["type"], json!("user_input"));
    assert_eq!(steps[0]["content"][0]["text"], json!("Hello!"));
    assert_eq!(steps[1]["type"], json!("model_output"));
}

#[test]
fn response_format_json_schema_serializes() {
    let fmt = ResponseFormat::json_schema(json!({
        "type": "object",
        "properties": { "summary": { "type": "string" } }
    }));
    let value = serde_json::to_value(&fmt).unwrap();
    assert_eq!(value["type"], json!("text"));
    assert_eq!(value["mime_type"], json!("application/json"));
    assert_eq!(value["schema"]["type"], json!("object"));
}

#[test]
fn function_result_step_serializes_with_call_id() {
    let step = Step::FunctionResult {
        call_id: "gth23981".to_string(),
        name: Some("get_weather".to_string()),
        result: json!({ "temperature": "72F" }),
        is_error: None,
        signature: None,
    };
    let value = serde_json::to_value(&step).unwrap();
    assert_eq!(value["type"], json!("function_result"));
    assert_eq!(value["call_id"], json!("gth23981"));
    assert_eq!(value["result"]["temperature"], json!("72F"));
    // is_error omitted when None.
    assert!(value.get("is_error").is_none());
}

// ── SSE event parsing ───────────────────────────────────────────────────

#[test]
fn parses_interaction_created_event() {
    let raw = json!({
        "event_type": "interaction.created",
        "interaction": {
            "id": "v1_abc",
            "model": "gemini-3-flash-preview",
            "status": "in_progress"
        },
        "event_id": "evt_123"
    });
    let event: InteractionSseEvent = serde_json::from_value(raw).unwrap();
    match event {
        InteractionSseEvent::InteractionCreated { interaction, event_id } => {
            assert_eq!(interaction.id, "v1_abc");
            assert_eq!(interaction.status, InteractionStatus::InProgress);
            assert_eq!(event_id.as_deref(), Some("evt_123"));
        }
        other => panic!("expected interaction.created, got {other:?}"),
    }
}

#[test]
fn parses_step_delta_text_event() {
    let raw = json!({
        "event_type": "step.delta",
        "index": 0,
        "delta": { "type": "text", "text": "Hello" }
    });
    let event: InteractionSseEvent = serde_json::from_value(raw).unwrap();
    assert_eq!(event.text_delta(), Some("Hello"));
    match event {
        InteractionSseEvent::StepDelta { index, delta, .. } => {
            assert_eq!(index, 0);
            assert_eq!(delta, StepDelta::Text { text: "Hello".to_string() });
        }
        other => panic!("expected step.delta, got {other:?}"),
    }
}

#[test]
fn parses_step_lifecycle_events() {
    let start: InteractionSseEvent = serde_json::from_value(json!({
        "event_type": "step.start",
        "index": 0,
        "step": { "type": "model_output" }
    }))
    .unwrap();
    assert!(matches!(start, InteractionSseEvent::StepStart { index: 0, .. }));

    let stop: InteractionSseEvent =
        serde_json::from_value(json!({ "event_type": "step.stop", "index": 0 })).unwrap();
    assert!(matches!(stop, InteractionSseEvent::StepStop { index: 0, .. }));
}

#[test]
fn parses_error_event() {
    let raw = json!({
        "event_type": "error",
        "error": { "message": "Result not found.", "code": "not_found" }
    });
    let event: InteractionSseEvent = serde_json::from_value(raw).unwrap();
    match event {
        InteractionSseEvent::Error { error, .. } => {
            assert_eq!(error.message, "Result not found.");
            assert_eq!(error.code.as_deref(), Some("not_found"));
        }
        other => panic!("expected error event, got {other:?}"),
    }
}

#[test]
fn unknown_sse_event_falls_back_to_other() {
    let raw = json!({ "event_type": "interaction.future_thing", "data": 1 });
    let event: InteractionSseEvent = serde_json::from_value(raw).unwrap();
    assert!(matches!(event, InteractionSseEvent::Other(_)));
}

#[test]
fn tts_request_with_audio_format_and_speech_config_serializes() {
    use adk_gemini::generation::{SpeakerVoiceConfig, SpeechConfig};
    let req = CreateInteractionRequest {
        model: Some("gemini-3.1-flash-tts-preview".to_string()),
        input: Input::Text("Alice: Hello Bob!\nBob: Hi Alice!".to_string()),
        response_format: Some(ResponseFormat::Audio {
            mime_type: Some("audio/pcm".to_string()),
            sample_rate: Some(24000),
        }),
        stream: Some(true),
        generation_config: Some(adk_gemini::interactions::GenerationConfig {
            ..Default::default()
        }),
        ..Default::default()
    };

    let value = serde_json::to_value(&req).unwrap();
    assert_eq!(value["model"], json!("gemini-3.1-flash-tts-preview"));
    assert_eq!(value["stream"], json!(true));
    assert_eq!(value["response_format"]["type"], json!("audio"));
    assert_eq!(value["response_format"]["mime_type"], json!("audio/pcm"));
    assert_eq!(value["response_format"]["sample_rate"], json!(24000));

    // Also test speech config serialization in generation config if passed via standard SpeechConfig
    let speech_cfg = SpeechConfig::multi_speaker(vec![
        SpeakerVoiceConfig::new("Alice", "Kore"),
        SpeakerVoiceConfig::new("Bob", "Puck"),
    ]);
    let speech_json = serde_json::to_value(&speech_cfg).unwrap();
    assert!(speech_json.get("multiSpeakerVoiceConfig").is_some());
}

#[test]
fn parses_step_delta_audio_event() {
    let raw = json!({
        "event_type": "step.delta",
        "index": 0,
        "delta": {
            "type": "audio",
            "data": "AQIDBA==",
            "mime_type": "audio/pcm;rate=24000",
            "sample_rate": 24000,
            "channels": 1
        }
    });

    let event: InteractionSseEvent = serde_json::from_value(raw).unwrap();
    match event {
        InteractionSseEvent::StepDelta { index, delta, .. } => {
            assert_eq!(index, 0);
            match delta {
                StepDelta::Audio { data, mime_type, sample_rate, channels } => {
                    assert_eq!(data.as_deref(), Some("AQIDBA=="));
                    assert_eq!(mime_type.as_deref(), Some("audio/pcm;rate=24000"));
                    assert_eq!(sample_rate, Some(24000));
                    assert_eq!(channels, Some(1));
                }
                other => panic!("expected StepDelta::Audio, got {other:?}"),
            }
        }
        other => panic!("expected step.delta, got {other:?}"),
    }
}
