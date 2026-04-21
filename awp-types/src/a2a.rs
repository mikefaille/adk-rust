use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Agent-to-agent message following the AWP communication format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2aMessage {
    pub id: Uuid,
    pub sender: String,
    pub recipient: String,
    pub message_type: A2aMessageType,
    pub timestamp: DateTime<Utc>,
    pub payload: serde_json::Value,
}

/// Type of an agent-to-agent message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum A2aMessageType {
    Request,
    Response,
    Notification,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a2a_message_serde_round_trip() {
        let msg = A2aMessage {
            id: Uuid::now_v7(),
            sender: "agent-a".to_string(),
            recipient: "agent-b".to_string(),
            message_type: A2aMessageType::Request,
            timestamp: Utc::now(),
            payload: serde_json::json!({"action": "greet"}),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: A2aMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, deserialized);
    }

    #[test]
    fn test_message_type_serde() {
        for mt in [
            A2aMessageType::Request,
            A2aMessageType::Response,
            A2aMessageType::Notification,
            A2aMessageType::Error,
        ] {
            let json = serde_json::to_string(&mt).unwrap();
            let deserialized: A2aMessageType = serde_json::from_str(&json).unwrap();
            assert_eq!(mt, deserialized);
        }
    }

    #[test]
    fn test_message_type_lowercase() {
        assert_eq!(serde_json::to_string(&A2aMessageType::Request).unwrap(), "\"request\"");
        assert_eq!(serde_json::to_string(&A2aMessageType::Response).unwrap(), "\"response\"");
        assert_eq!(
            serde_json::to_string(&A2aMessageType::Notification).unwrap(),
            "\"notification\""
        );
        assert_eq!(serde_json::to_string(&A2aMessageType::Error).unwrap(), "\"error\"");
    }
}
