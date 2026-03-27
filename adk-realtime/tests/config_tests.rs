//! Tests for the config module.

use adk_realtime::{RealtimeConfig, VadConfig, VadMode};
use adk_realtime::config::{ToolDefinition, TranscriptionConfig};

#[test]
fn test_realtime_config_default() {
    let config = RealtimeConfig::default();
    assert!(config.instruction.is_none());
    assert!(config.voice.is_none());
    assert!(config.tools.is_none());
}

#[test]
fn test_realtime_config_builder() {
    let config = RealtimeConfig::default().with_instruction("You are helpful.").with_voice("alloy");

    assert_eq!(config.instruction, Some("You are helpful.".to_string()));
    assert_eq!(config.voice, Some("alloy".to_string()));
}

#[test]
fn test_vad_config_server_vad() {
    let vad = VadConfig {
        mode: VadMode::ServerVad,
        threshold: Some(0.5),
        prefix_padding_ms: Some(300),
        silence_duration_ms: Some(500),
        interrupt_response: Some(true),
        eagerness: None,
    };

    assert!(matches!(vad.mode, VadMode::ServerVad));
    assert_eq!(vad.threshold, Some(0.5));
}

#[test]
fn test_vad_config_semantic_vad() {
    let vad = VadConfig {
        mode: VadMode::SemanticVad,
        threshold: None,
        prefix_padding_ms: None,
        silence_duration_ms: None,
        interrupt_response: None,
        eagerness: Some("high".to_string()),
    };

    assert!(matches!(vad.mode, VadMode::SemanticVad));
    assert_eq!(vad.eagerness, Some("high".to_string()));
}

#[test]
fn test_config_modalities() {
    let config = RealtimeConfig {
        modalities: Some(vec!["text".to_string(), "audio".to_string()]),
        ..Default::default()
    };

    let modalities = config.modalities.unwrap();
    assert!(modalities.contains(&"text".to_string()));
    assert!(modalities.contains(&"audio".to_string()));
}

#[test]
fn test_config_temperature() {
    let config = RealtimeConfig { temperature: Some(0.7), ..Default::default() };

    assert_eq!(config.temperature, Some(0.7));
}

// ── PartialEq tests for newly derived impls ──────────────────────────────────

#[test]
fn test_vad_config_partial_eq_equal() {
    let a = VadConfig {
        mode: VadMode::ServerVad,
        silence_duration_ms: Some(500),
        threshold: None,
        prefix_padding_ms: None,
        interrupt_response: Some(true),
        eagerness: None,
    };
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn test_vad_config_partial_eq_different_mode() {
    let a = VadConfig::server_vad();
    let b = VadConfig::semantic_vad();
    assert_ne!(a, b);
}

#[test]
fn test_vad_config_partial_eq_different_silence_duration() {
    let a = VadConfig::server_vad().with_silence_duration(300);
    let b = VadConfig::server_vad().with_silence_duration(800);
    assert_ne!(a, b);
}

#[test]
fn test_vad_config_partial_eq_different_interrupt() {
    let a = VadConfig::server_vad().with_interrupt(true);
    let b = VadConfig::server_vad().with_interrupt(false);
    assert_ne!(a, b);
}

#[test]
fn test_tool_definition_partial_eq_equal() {
    let a = ToolDefinition::new("my_tool").with_description("Does something");
    let b = ToolDefinition::new("my_tool").with_description("Does something");
    assert_eq!(a, b);
}

#[test]
fn test_tool_definition_partial_eq_different_name() {
    let a = ToolDefinition::new("tool_a");
    let b = ToolDefinition::new("tool_b");
    assert_ne!(a, b);
}

#[test]
fn test_tool_definition_partial_eq_different_description() {
    let a = ToolDefinition::new("tool").with_description("First");
    let b = ToolDefinition::new("tool").with_description("Second");
    assert_ne!(a, b);
}

#[test]
fn test_tool_definition_partial_eq_description_vs_none() {
    let a = ToolDefinition::new("tool").with_description("Some desc");
    let b = ToolDefinition::new("tool");
    assert_ne!(a, b);
}

#[test]
fn test_tool_definition_partial_eq_with_parameters() {
    let a = ToolDefinition::new("tool")
        .with_parameters(serde_json::json!({"type": "object", "properties": {}}));
    let b = ToolDefinition::new("tool")
        .with_parameters(serde_json::json!({"type": "object", "properties": {}}));
    assert_eq!(a, b);
}

#[test]
fn test_tool_definition_partial_eq_different_parameters() {
    let a = ToolDefinition::new("tool")
        .with_parameters(serde_json::json!({"type": "string"}));
    let b = ToolDefinition::new("tool")
        .with_parameters(serde_json::json!({"type": "integer"}));
    assert_ne!(a, b);
}

#[test]
fn test_realtime_config_partial_eq_defaults_equal() {
    let a = RealtimeConfig::default();
    let b = RealtimeConfig::default();
    assert_eq!(a, b);
}

#[test]
fn test_realtime_config_partial_eq_same_instruction() {
    let a = RealtimeConfig::default().with_instruction("Be helpful.");
    let b = RealtimeConfig::default().with_instruction("Be helpful.");
    assert_eq!(a, b);
}

#[test]
fn test_realtime_config_partial_eq_different_instruction() {
    let a = RealtimeConfig::default().with_instruction("Be helpful.");
    let b = RealtimeConfig::default().with_instruction("Be terse.");
    assert_ne!(a, b);
}

#[test]
fn test_realtime_config_partial_eq_different_voice() {
    let a = RealtimeConfig::default().with_voice("alloy");
    let b = RealtimeConfig::default().with_voice("nova");
    assert_ne!(a, b);
}

#[test]
fn test_realtime_config_partial_eq_different_tools() {
    let tool_a = ToolDefinition::new("tool_a");
    let tool_b = ToolDefinition::new("tool_b");
    let a = RealtimeConfig::default().with_tool(tool_a);
    let b = RealtimeConfig::default().with_tool(tool_b);
    assert_ne!(a, b);
}

#[test]
fn test_realtime_config_partial_eq_with_extra() {
    let a = RealtimeConfig { extra: Some(serde_json::json!({"resumeToken": "abc"})), ..Default::default() };
    let b = RealtimeConfig { extra: Some(serde_json::json!({"resumeToken": "abc"})), ..Default::default() };
    assert_eq!(a, b);
}

#[test]
fn test_realtime_config_partial_eq_different_extra() {
    let a = RealtimeConfig { extra: Some(serde_json::json!({"resumeToken": "abc"})), ..Default::default() };
    let b = RealtimeConfig { extra: Some(serde_json::json!({"resumeToken": "xyz"})), ..Default::default() };
    assert_ne!(a, b);
}

#[test]
fn test_transcription_config_partial_eq_equal() {
    let a = TranscriptionConfig::whisper();
    let b = TranscriptionConfig::whisper();
    assert_eq!(a, b);
}

#[test]
fn test_transcription_config_partial_eq_different_model() {
    let a = TranscriptionConfig { model: "whisper-1".to_string() };
    let b = TranscriptionConfig { model: "whisper-2".to_string() };
    assert_ne!(a, b);
}