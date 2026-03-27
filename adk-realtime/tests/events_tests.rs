//! Tests for the events module.

use adk_realtime::{ClientEvent, ServerEvent, ToolCall, ToolResponse};
use adk_realtime::config::ToolDefinition;

#[test]
fn test_tool_call_creation() {
    let call = ToolCall {
        call_id: "call_123".to_string(),
        name: "get_weather".to_string(),
        arguments: serde_json::json!({"location": "NYC"}),
    };

    assert_eq!(call.call_id, "call_123");
    assert_eq!(call.name, "get_weather");
}

#[test]
fn test_tool_response_creation() {
    let response = ToolResponse {
        call_id: "call_123".to_string(),
        output: serde_json::json!({"temperature": 72, "condition": "sunny"}),
    };

    assert_eq!(response.call_id, "call_123");
    assert!(response.output.get("temperature").is_some());
}

#[test]
fn test_tool_response_new() {
    let response = ToolResponse::new("call_456", serde_json::json!({"result": "ok"}));
    assert_eq!(response.call_id, "call_456");
}

#[test]
fn test_tool_response_from_string() {
    let response = ToolResponse::from_string("call_789", "Success!");
    assert_eq!(response.call_id, "call_789");
    assert_eq!(response.output, serde_json::json!("Success!"));
}

#[test]
fn test_client_event_audio_delta_serialization() {
    let event = ClientEvent::AudioDelta { event_id: None, audio: b"hello".to_vec(), format: None };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("input_audio_buffer.append"));
    // Audio should be base64-encoded on the wire
    assert!(json.contains("aGVsbG8=")); // base64("hello")
}

#[test]
fn test_client_event_audio_commit_serialization() {
    let event = ClientEvent::InputAudioBufferCommit;
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("input_audio_buffer.commit"));
}

#[test]
fn test_client_event_create_response_serialization() {
    let event = ClientEvent::ResponseCreate { config: None };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("response.create"));
}

#[test]
fn test_client_event_cancel_response_serialization() {
    let event = ClientEvent::ResponseCancel;
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("response.cancel"));
}

#[test]
fn test_server_event_audio_delta_deserialization() {
    // "base64audio==" decodes to bytes [0x6d, 0xab, 0x6d, 0xb6, 0xa9, 0xb6, 0xab, 0x6e]
    let json = r#"{
        "type": "response.audio.delta",
        "event_id": "evt_123",
        "response_id": "resp_456",
        "item_id": "item_789",
        "output_index": 0,
        "content_index": 0,
        "delta": "aGVsbG8="
    }"#;

    let event: ServerEvent = serde_json::from_str(json).unwrap();
    match event {
        ServerEvent::AudioDelta { event_id, delta, item_id, .. } => {
            assert_eq!(event_id, "evt_123");
            assert_eq!(delta, b"hello"); // decoded from base64
            assert_eq!(item_id, "item_789");
        }
        _ => panic!("Expected AudioDelta event"),
    }
}

#[test]
fn test_server_event_audio_delta_roundtrip() {
    let original = ServerEvent::AudioDelta {
        event_id: "evt_1".to_string(),
        response_id: "resp_1".to_string(),
        item_id: "item_1".to_string(),
        output_index: 0,
        content_index: 0,
        delta: vec![0x00, 0x01, 0x02, 0xFF],
    };

    let json = serde_json::to_string(&original).unwrap();
    let deserialized: ServerEvent = serde_json::from_str(&json).unwrap();

    match deserialized {
        ServerEvent::AudioDelta { delta, .. } => {
            assert_eq!(delta, vec![0x00, 0x01, 0x02, 0xFF]);
        }
        _ => panic!("Expected AudioDelta"),
    }
}

#[test]
fn test_server_event_text_delta_deserialization() {
    let json = r#"{
        "type": "response.text.delta",
        "event_id": "evt_123",
        "response_id": "resp_456",
        "item_id": "item_789",
        "output_index": 0,
        "content_index": 0,
        "delta": "Hello, world!"
    }"#;

    let event: ServerEvent = serde_json::from_str(json).unwrap();
    match event {
        ServerEvent::TextDelta { delta, .. } => {
            assert_eq!(delta, "Hello, world!");
        }
        _ => panic!("Expected TextDelta event"),
    }
}

#[test]
fn test_server_event_function_call_done_deserialization() {
    let json = r#"{
        "type": "response.function_call_arguments.done",
        "event_id": "evt_123",
        "response_id": "resp_456",
        "item_id": "item_789",
        "output_index": 0,
        "call_id": "call_abc",
        "name": "get_weather",
        "arguments": "{\"location\":\"NYC\"}"
    }"#;

    let event: ServerEvent = serde_json::from_str(json).unwrap();
    match event {
        ServerEvent::FunctionCallDone { call_id, name, arguments, .. } => {
            assert_eq!(call_id, "call_abc");
            assert_eq!(name, "get_weather");
            assert!(arguments.contains("NYC"));
        }
        _ => panic!("Expected FunctionCallDone event"),
    }
}

#[test]
fn test_server_event_speech_started_deserialization() {
    let json = r#"{
        "type": "input_audio_buffer.speech_started",
        "event_id": "evt_123",
        "audio_start_ms": 1500
    }"#;

    let event: ServerEvent = serde_json::from_str(json).unwrap();
    match event {
        ServerEvent::SpeechStarted { audio_start_ms, .. } => {
            assert_eq!(audio_start_ms, 1500);
        }
        _ => panic!("Expected SpeechStarted event"),
    }
}

#[test]
fn test_server_event_speech_stopped_deserialization() {
    let json = r#"{
        "type": "input_audio_buffer.speech_stopped",
        "event_id": "evt_456",
        "audio_end_ms": 3200
    }"#;

    let event: ServerEvent = serde_json::from_str(json).unwrap();
    match event {
        ServerEvent::SpeechStopped { audio_end_ms, .. } => {
            assert_eq!(audio_end_ms, 3200);
        }
        _ => panic!("Expected SpeechStopped event"),
    }
}

#[test]
fn test_server_event_error_deserialization() {
    let json = r#"{
        "type": "error",
        "event_id": "evt_123",
        "error": {
            "type": "rate_limit_error",
            "code": "rate_limit",
            "message": "Too many requests"
        }
    }"#;

    let event: ServerEvent = serde_json::from_str(json).unwrap();
    match event {
        ServerEvent::Error { error, .. } => {
            assert_eq!(error.error_type, "rate_limit_error");
            assert_eq!(error.code, Some("rate_limit".to_string()));
            assert_eq!(error.message, "Too many requests");
        }
        _ => panic!("Expected Error event"),
    }
}

#[test]
fn test_server_event_session_created_deserialization() {
    let json = r#"{
        "type": "session.created",
        "event_id": "evt_001",
        "session": {
            "id": "session_abc",
            "model": "gpt-4o-realtime"
        }
    }"#;

    let event: ServerEvent = serde_json::from_str(json).unwrap();
    match event {
        ServerEvent::SessionCreated { event_id, session } => {
            assert_eq!(event_id, "evt_001");
            assert!(session.get("id").is_some());
        }
        _ => panic!("Expected SessionCreated event"),
    }
}

#[test]
fn test_server_event_response_done_deserialization() {
    let json = r#"{
        "type": "response.done",
        "event_id": "evt_999",
        "response": {
            "id": "resp_123",
            "status": "completed"
        }
    }"#;

    let event: ServerEvent = serde_json::from_str(json).unwrap();
    match event {
        ServerEvent::ResponseDone { response, .. } => {
            assert_eq!(response.get("status").unwrap(), "completed");
        }
        _ => panic!("Expected ResponseDone event"),
    }
}

#[test]
fn test_server_event_unknown_type() {
    let json = r#"{
        "type": "some.unknown.event",
        "data": "whatever"
    }"#;

    let event: ServerEvent = serde_json::from_str(json).unwrap();
    assert!(matches!(event, ServerEvent::Unknown));
}

// ── New ClientEvent variant tests (added in PR) ───────────────────────────────

#[test]
fn test_client_event_message_serialization_has_type_tag() {
    let event = ClientEvent::Message {
        role: "user".to_string(),
        parts: vec![adk_core::types::Part::Text { text: "Hello".to_string() }],
    };

    let json = serde_json::to_string(&event).unwrap();
    // The variant has rename = "message", so the type tag should be "message".
    assert!(json.contains("\"type\":\"message\"") || json.contains("\"type\": \"message\""));
}

#[test]
fn test_client_event_message_serialization_includes_role() {
    let event = ClientEvent::Message {
        role: "assistant".to_string(),
        parts: vec![adk_core::types::Part::Text { text: "Hi!".to_string() }],
    };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("assistant"));
}

#[test]
fn test_client_event_message_serialization_includes_parts() {
    let event = ClientEvent::Message {
        role: "user".to_string(),
        parts: vec![adk_core::types::Part::Text { text: "Test content".to_string() }],
    };

    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("Test content"));
}

#[test]
fn test_client_event_message_construction_fields() {
    let parts = vec![
        adk_core::types::Part::Text { text: "Part one".to_string() },
        adk_core::types::Part::Text { text: "Part two".to_string() },
    ];
    let event = ClientEvent::Message {
        role: "user".to_string(),
        parts: parts.clone(),
    };

    match event {
        ClientEvent::Message { role, parts: p } => {
            assert_eq!(role, "user");
            assert_eq!(p.len(), 2);
        }
        _ => panic!("Expected Message variant"),
    }
}

#[test]
fn test_client_event_update_session_not_serialized() {
    // UpdateSession has #[serde(skip_serializing)], so serializing it should
    // NOT produce a valid JSON representation of its fields on the wire.
    // The serializer will output `null` (since it skips the variant entirely).
    let event = ClientEvent::UpdateSession {
        instructions: Some("New instructions".to_string()),
        tools: None,
    };

    // Serializing a skip_serializing variant produces null in serde's tagged enum
    let result = serde_json::to_string(&event);
    // Either it produces null or an empty/opaque representation - crucially it
    // must NOT leak the instructions string to the wire format.
    match result {
        Ok(json) => {
            // The instructions must not appear in the wire format
            assert!(!json.contains("New instructions"),
                "UpdateSession should not serialize its fields to the wire: got {}", json);
        }
        Err(_) => {
            // Serialization failure is also an acceptable outcome for a skip_serializing variant
        }
    }
}

#[test]
fn test_client_event_update_session_construction_with_instructions() {
    let event = ClientEvent::UpdateSession {
        instructions: Some("You are now a travel agent.".to_string()),
        tools: None,
    };

    match event {
        ClientEvent::UpdateSession { instructions, tools } => {
            assert_eq!(instructions.as_deref(), Some("You are now a travel agent."));
            assert!(tools.is_none());
        }
        _ => panic!("Expected UpdateSession variant"),
    }
}

#[test]
fn test_client_event_update_session_construction_with_tools() {
    let tool = ToolDefinition::new("get_weather").with_description("Fetch weather data");
    let event = ClientEvent::UpdateSession {
        instructions: None,
        tools: Some(vec![tool]),
    };

    match event {
        ClientEvent::UpdateSession { instructions, tools } => {
            assert!(instructions.is_none());
            let tools = tools.unwrap();
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].name, "get_weather");
        }
        _ => panic!("Expected UpdateSession variant"),
    }
}

#[test]
fn test_client_event_update_session_construction_both_fields() {
    let tools = vec![
        ToolDefinition::new("tool_a"),
        ToolDefinition::new("tool_b"),
    ];
    let event = ClientEvent::UpdateSession {
        instructions: Some("Handle billing queries.".to_string()),
        tools: Some(tools),
    };

    match event {
        ClientEvent::UpdateSession { instructions, tools } => {
            assert_eq!(instructions.as_deref(), Some("Handle billing queries."));
            assert_eq!(tools.unwrap().len(), 2);
        }
        _ => panic!("Expected UpdateSession variant"),
    }
}

#[test]
fn test_server_event_session_updated_deserialization() {
    // SessionUpdated is used by the runner to handle Gemini resumption tokens.
    let json = r#"{
        "type": "session.updated",
        "event_id": "evt_upd_001",
        "session": {
            "resumeToken": "some-opaque-token-xyz"
        }
    }"#;

    let event: ServerEvent = serde_json::from_str(json).unwrap();
    match event {
        ServerEvent::SessionUpdated { event_id, session } => {
            assert_eq!(event_id, "evt_upd_001");
            assert_eq!(
                session.get("resumeToken").and_then(|t| t.as_str()),
                Some("some-opaque-token-xyz")
            );
        }
        _ => panic!("Expected SessionUpdated event"),
    }
}

#[test]
fn test_server_event_session_updated_no_resume_token() {
    // SessionUpdated without a resumeToken should still deserialize cleanly.
    let json = r#"{
        "type": "session.updated",
        "event_id": "evt_upd_002",
        "session": {
            "voice": "alloy",
            "model": "gpt-4o-realtime"
        }
    }"#;

    let event: ServerEvent = serde_json::from_str(json).unwrap();
    match event {
        ServerEvent::SessionUpdated { session, .. } => {
            assert!(session.get("resumeToken").is_none());
            assert_eq!(session.get("voice").and_then(|v| v.as_str()), Some("alloy"));
        }
        _ => panic!("Expected SessionUpdated event"),
    }
}