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
fn test_gemini_31_tts_request_serialization() {
    use adk_gemini::interactions::{
        GenerationConfig, PrebuiltVoiceConfig, ResponseModality, SpeechConfig, VoiceConfig,
    };

    let speech_config = SpeechConfig {
        voice_config: Some(VoiceConfig {
            prebuilt_voice_config: PrebuiltVoiceConfig { voice_name: "Kore".to_string() },
        }),
        multi_speaker_voice_config: None,
    };

    let req = CreateInteractionRequest {
        model: Some("gemini-3.1-flash-tts-preview".to_string()),
        input: Input::Text("Hello world".to_string()),
        response_modalities: vec![ResponseModality::Audio],
        generation_config: Some(GenerationConfig {
            speech_config: Some(speech_config),
            ..Default::default()
        }),
        ..Default::default()
    };

    let value = serde_json::to_value(&req).unwrap();

    assert_eq!(value["model"], json!("gemini-3.1-flash-tts-preview"));
    assert_eq!(value["input"], json!("Hello world"));
    assert_eq!(value["response_modalities"][0], json!("audio"));

    let gen_cfg = &value["generation_config"];
    let sp_cfg = &gen_cfg["speech_config"];
    let voice_cfg = &sp_cfg["voice_config"];
    let prebuilt = &voice_cfg["prebuilt_voice_config"];

    assert_eq!(prebuilt["voice_name"], json!("Kore"));
}

#[test]
fn test_sse_fixture_parsing_audio_deltas() {
    let raw1 = json!({
        "event_type": "step.delta",
        "index": 1,
        "delta": {
            "type": "audio",
            "data": "UklGRgAAAABXQVZFZg==",
            "mime_type": "audio/pcm",
            "sample_rate": 24000,
            "channels": 1
        }
    });

    let raw2 = json!({
        "event_type": "step.delta",
        "index": 1,
        "delta": {
            "type": "audio",
            "data": "Zm10IAAAAAA="
        }
    });

    let event1: InteractionSseEvent = serde_json::from_value(raw1).unwrap();
    let event2: InteractionSseEvent = serde_json::from_value(raw2).unwrap();

    match event1 {
        InteractionSseEvent::StepDelta { index, delta, .. } => {
            assert_eq!(index, 1);
            match delta {
                StepDelta::Audio { data, mime_type, sample_rate, channels, uri } => {
                    assert_eq!(data.as_deref(), Some("UklGRgAAAABXQVZFZg=="));
                    assert_eq!(mime_type.as_deref(), Some("audio/pcm"));
                    assert_eq!(sample_rate, Some(24000));
                    assert_eq!(channels, Some(1));
                    assert!(uri.is_none());
                }
                other => panic!("expected Audio delta, got {other:?}"),
            }
        }
        other => panic!("expected StepDelta event, got {other:?}"),
    }

    match event2 {
        InteractionSseEvent::StepDelta { index, delta, .. } => {
            assert_eq!(index, 1);
            match delta {
                StepDelta::Audio { data, mime_type, sample_rate, channels, uri } => {
                    assert_eq!(data.as_deref(), Some("Zm10IAAAAAA="));
                    assert!(mime_type.is_none());
                    assert!(sample_rate.is_none());
                    assert!(channels.is_none());
                    assert!(uri.is_none());
                }
                other => panic!("expected Audio delta, got {other:?}"),
            }
        }
        other => panic!("expected StepDelta event, got {other:?}"),
    }
}
