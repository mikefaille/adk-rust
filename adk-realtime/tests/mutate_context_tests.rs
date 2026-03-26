use adk_realtime::config::{RealtimeConfig, SessionUpdateConfig};
use serde_json::json;

#[test]
fn test_session_update_config_deserialize() {
    let delta = SessionUpdateConfig(
        RealtimeConfig::default().with_instruction("You are now a travel agent."),
    );

    let val = serde_json::to_value(&delta).unwrap();
    assert_eq!(val["instruction"], "You are now a travel agent.");
    assert!(val.get("model").is_none());
}
