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
fn test_realtime_config_language_pin() {
    let config = RealtimeConfig::default().try_with_language("en-US").unwrap();

    assert_eq!(config.language.as_ref().map(|code| code.as_str()), Some("en-US"));
    assert!(config.extra.is_none(), "the pin must not be stored in extra");
    assert!(RealtimeConfig::default().language.is_none());
}

#[test]
fn language_code_parse_accepts_well_formed_tags() {
    use adk_realtime::config::LanguageCode;
    for tag in ["en", "fr-CA", "en-US", "zh-Hans-CN", "de-CH-1901"] {
        assert_eq!(LanguageCode::parse(tag).unwrap().as_str(), tag);
    }
}

#[test]
fn language_code_parse_canonicalizes_casing() {
    use adk_realtime::config::LanguageCode;
    assert_eq!(LanguageCode::parse("en-us").unwrap().as_str(), "en-US");
    assert_eq!(LanguageCode::parse("ZH-hant").unwrap().as_str(), "zh-Hant");
    assert_eq!(LanguageCode::parse("es-419").unwrap().as_str(), "es-419");
    assert_eq!(LanguageCode::parse("en-us").unwrap(), LanguageCode::parse("en-US").unwrap());
}

#[test]
fn language_code_parse_rejects_malformed_tags() {
    use adk_realtime::config::LanguageCode;
    for tag in
        ["", " ", "en US", "en_US", "fr_CA", "héllo", "-en", "en-", "en--US", "e1", "engl", "en-x"]
    {
        assert!(LanguageCode::parse(tag).is_err(), "must reject {tag:?}");
    }
}

#[test]
fn language_code_serde_is_transparent_and_fail_closed() {
    use adk_realtime::config::LanguageCode;
    let code = LanguageCode::parse("fr-CA").unwrap();
    assert_eq!(serde_json::to_string(&code).unwrap(), "\"fr-CA\"");
    assert_eq!(serde_json::from_str::<LanguageCode>("\"fr-CA\"").unwrap(), code);
    assert!(serde_json::from_str::<LanguageCode>("\"fr_CA\"").is_err());
    assert!(serde_json::from_str::<LanguageCode>("\"\"").is_err());
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
        start_of_speech_sensitivity: None,
        end_of_speech_sensitivity: None,
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
        start_of_speech_sensitivity: None,
        end_of_speech_sensitivity: None,
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
