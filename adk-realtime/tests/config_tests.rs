//! Tests for the config module.

use adk_realtime::{RealtimeConfig, VadConfig, VadMode};

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

// ── PartialEq tests for types that gained PartialEq in this PR ──────────────

#[test]
fn test_vad_config_partial_eq_equal() {
    let a = VadConfig {
        mode: VadMode::ServerVad,
        silence_duration_ms: Some(500),
        threshold: Some(0.5),
        prefix_padding_ms: Some(300),
        interrupt_response: Some(true),
        eagerness: None,
    };
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn test_vad_config_partial_eq_not_equal_mode() {
    let a = VadConfig { mode: VadMode::ServerVad, ..Default::default() };
    let b = VadConfig { mode: VadMode::SemanticVad, ..Default::default() };
    assert_ne!(a, b);
}

#[test]
fn test_vad_config_partial_eq_not_equal_threshold() {
    let a = VadConfig { threshold: Some(0.3), ..Default::default() };
    let b = VadConfig { threshold: Some(0.7), ..Default::default() };
    assert_ne!(a, b);
}

#[test]
fn test_vad_config_partial_eq_not_equal_silence_duration() {
    let a = VadConfig { silence_duration_ms: Some(200), ..Default::default() };
    let b = VadConfig { silence_duration_ms: Some(500), ..Default::default() };
    assert_ne!(a, b);
}

#[test]
fn test_tool_definition_partial_eq_equal() {
    use adk_realtime::config::ToolDefinition;
    let a = ToolDefinition::new("my_tool").with_description("Does things");
    let b = ToolDefinition::new("my_tool").with_description("Does things");
    assert_eq!(a, b);
}

#[test]
fn test_tool_definition_partial_eq_not_equal_name() {
    use adk_realtime::config::ToolDefinition;
    let a = ToolDefinition::new("tool_a");
    let b = ToolDefinition::new("tool_b");
    assert_ne!(a, b);
}

#[test]
fn test_tool_definition_partial_eq_not_equal_description() {
    use adk_realtime::config::ToolDefinition;
    let a = ToolDefinition::new("tool").with_description("First");
    let b = ToolDefinition::new("tool").with_description("Second");
    assert_ne!(a, b);
}

#[test]
fn test_tool_definition_partial_eq_with_parameters() {
    use adk_realtime::config::ToolDefinition;
    let schema = serde_json::json!({ "type": "object", "properties": {} });
    let a = ToolDefinition::new("tool").with_parameters(schema.clone());
    let b = ToolDefinition::new("tool").with_parameters(schema);
    assert_eq!(a, b);
}

#[test]
fn test_realtime_config_partial_eq_default() {
    let a = RealtimeConfig::default();
    let b = RealtimeConfig::default();
    assert_eq!(a, b);
}

#[test]
fn test_realtime_config_partial_eq_with_instruction() {
    let a = RealtimeConfig::default().with_instruction("Be helpful");
    let b = RealtimeConfig::default().with_instruction("Be helpful");
    assert_eq!(a, b);
}

#[test]
fn test_realtime_config_partial_eq_not_equal_instruction() {
    let a = RealtimeConfig::default().with_instruction("Be helpful");
    let b = RealtimeConfig::default().with_instruction("Be creative");
    assert_ne!(a, b);
}

#[test]
fn test_realtime_config_partial_eq_not_equal_voice() {
    let a = RealtimeConfig::default().with_voice("alloy");
    let b = RealtimeConfig::default().with_voice("echo");
    assert_ne!(a, b);
}

#[test]
fn test_realtime_config_partial_eq_full() {
    use adk_realtime::config::ToolDefinition;
    let tool = ToolDefinition::new("weather").with_description("Get weather");
    let a = RealtimeConfig::default()
        .with_instruction("You are helpful")
        .with_voice("alloy")
        .with_tool(tool.clone());
    let b = RealtimeConfig::default()
        .with_instruction("You are helpful")
        .with_voice("alloy")
        .with_tool(tool);
    assert_eq!(a, b);
}

#[test]
fn test_transcription_config_partial_eq_equal() {
    use adk_realtime::config::TranscriptionConfig;
    let a = TranscriptionConfig::whisper();
    let b = TranscriptionConfig::whisper();
    assert_eq!(a, b);
}

#[test]
fn test_transcription_config_partial_eq_not_equal() {
    use adk_realtime::config::TranscriptionConfig;
    let a = TranscriptionConfig { model: "whisper-1".to_string() };
    let b = TranscriptionConfig { model: "whisper-2".to_string() };
    assert_ne!(a, b);
}