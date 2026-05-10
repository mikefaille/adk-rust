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
fn test_response_modality_helpers_are_output_focused() {
    assert_eq!(
        RealtimeConfig::default().with_audio_response().modalities,
        Some(vec!["audio".to_string()])
    );
    assert_eq!(
        RealtimeConfig::default().with_text_response().modalities,
        Some(vec!["text".to_string()])
    );
}

#[test]
fn test_config_input_audio_sample_rate() {
    let config = RealtimeConfig::default().with_input_audio_sample_rate(24_000);
    assert_eq!(config.input_audio_sample_rate, Some(24_000));
}

#[test]
fn test_config_temperature() {
    let config = RealtimeConfig { temperature: Some(0.7), ..Default::default() };

    assert_eq!(config.temperature, Some(0.7));
}
