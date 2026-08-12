#![cfg(feature = "gemini")]

use adk_realtime::error::RealtimeError;
use adk_realtime::events::ServerEvent;
use adk_realtime::gemini::normalize_model_id;
use serde_json::json;

#[tokio::test]
async fn test_challenger_model_id_normalization_matrix() {
    let cases = vec![
        ("gemini-3.1-flash-live-preview", "models/gemini-3.1-flash-live-preview"),
        ("gemini-2.5-flash-native-audio", "models/gemini-2.5-flash-native-audio"),
        ("models/gemini-3.1-flash-live-preview", "models/gemini-3.1-flash-live-preview"),
        (
            "projects/123/locations/us-central1/publishers/google/models/gemini-3.1-flash-live-preview",
            "projects/123/locations/us-central1/publishers/google/models/gemini-3.1-flash-live-preview",
        ),
        (
            "publishers/google/models/gemini-3.1-flash-live-preview",
            "publishers/google/models/gemini-3.1-flash-live-preview",
        ),
        ("", "models/"),
        ("custom-model-id-v1", "models/custom-model-id-v1"),
    ];

    for (input, expected) in cases {
        assert_eq!(
            normalize_model_id(input),
            expected,
            "Failed normalization for input: {}",
            input
        );
    }
}

#[test]
fn test_challenger_tcp_reset_error_classification_exhaustive() {
    // 1. Transient TCP reset error strings
    let reset_strings = vec![
        "read tcp 127.0.0.1:443: connection reset by peer",
        "READ TCP 192.168.1.1:8080: CONNECTION RESET BY PEER",
        "ECONNRESET: socket write failed",
        "econnreset occurred on stream",
        "Broken pipe on socket write",
        "broken pipe error during flush",
        "Connection closed abruptly by remote endpoint",
        "Receive error: connection reset",
    ];

    for msg in reset_strings {
        let err = RealtimeError::ConnectionError(msg.to_string());
        assert!(
            err.is_connection_reset(),
            "Expected ConnectionError({:?}) to be classified as connection reset",
            msg
        );

        let msg_err = RealtimeError::MessageError(msg.to_string());
        assert!(
            msg_err.is_connection_reset(),
            "Expected MessageError({:?}) to be classified as connection reset",
            msg
        );
    }

    // 2. std::io::Error variants
    let io_reset =
        RealtimeError::IoError(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset"));
    assert!(io_reset.is_connection_reset());

    let io_broken =
        RealtimeError::IoError(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe"));
    assert!(io_broken.is_connection_reset());

    let io_aborted = RealtimeError::IoError(std::io::Error::new(
        std::io::ErrorKind::ConnectionAborted,
        "aborted",
    ));
    assert!(io_aborted.is_connection_reset());

    // 3. Non-reset error variants
    let non_reset_errors = vec![
        RealtimeError::ConfigError("Invalid model ID".into()),
        RealtimeError::AuthError("Unauthorized 401".into()),
        RealtimeError::NotConnected,
        RealtimeError::SessionClosed,
        RealtimeError::AudioFormatError("Unsupported sample rate".into()),
        RealtimeError::ToolError("Execution panic".into()),
        RealtimeError::ServerError { code: "400".into(), message: "Bad request".into() },
        RealtimeError::Timeout("Request timed out".into()),
        RealtimeError::OpusCodecError("Frame corruption".into()),
        RealtimeError::WebRTCError("ICE disconnect".into()),
        RealtimeError::LiveKitError("Room disconnected".into()),
    ];

    for err in &non_reset_errors {
        assert!(
            !err.is_connection_reset(),
            "Error {:?} should NOT be classified as connection reset",
            err
        );
    }
}

#[tokio::test]
async fn test_challenger_session_resumption_token_alignment() {
    let server_frame = json!({
        "sessionResumptionUpdate": {
            "resumable": true,
            "newHandle": "challenger_test_token_999"
        }
    })
    .to_string();

    let events =
        adk_realtime::gemini::GeminiRealtimeSession::translate_event_static(&server_frame).unwrap();
    assert_eq!(events.len(), 1);

    if let ServerEvent::SessionUpdated { session, .. } = &events[0] {
        assert_eq!(
            session.get("resumeToken").and_then(|v| v.as_str()),
            Some("challenger_test_token_999")
        );
        assert_eq!(
            session.get("resumeHandle").and_then(|v| v.as_str()),
            Some("challenger_test_token_999")
        );
    } else {
        panic!("Expected SessionUpdated event with resumeToken and resumeHandle");
    }

    // Verify non-resumable frames return empty events
    let non_resumable_frame = json!({
        "sessionResumptionUpdate": {
            "resumable": false,
            "newHandle": "stale_token"
        }
    })
    .to_string();

    let non_resumable_events =
        adk_realtime::gemini::GeminiRealtimeSession::translate_event_static(&non_resumable_frame)
            .unwrap();
    assert!(non_resumable_events.is_empty());
}
