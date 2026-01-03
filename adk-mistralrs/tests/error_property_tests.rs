//! Property tests for error message quality.
//!
//! **Property 5: Error Message Quality**
//! *For any* error condition, the error message SHALL contain:
//! - A clear description of what went wrong
//! - Contextual information (model ID, path, etc.)
//! - Actionable suggestions for resolution
//!
//! **Validates: Requirements 2.6, 12.1, 12.2**

use adk_mistralrs::MistralRsError;
use proptest::prelude::*;

// ============================================================================
// Generators for error inputs
// ============================================================================

/// Generate valid model IDs (HuggingFace format)
fn arb_model_id() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z]{3,10}/[a-z_-]{3,15}".prop_map(|s| s),
        "microsoft/[A-Za-z0-9_-]{5,20}".prop_map(|s| s),
        "meta-llama/[A-Za-z0-9_-]{5,20}".prop_map(|s| s),
    ]
}

/// Generate file paths
fn arb_path() -> impl Strategy<Value = String> {
    prop_oneof![
        "/[a-z/]{5,30}".prop_map(|s| s),
        "/home/[a-z]{3,8}/models/[a-z_-]{5,15}".prop_map(|s| s),
        "./[a-z_-]{3,15}".prop_map(|s| s),
    ]
}

/// Generate error reasons
fn arb_reason() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("connection timeout".to_string()),
        Just("file not found".to_string()),
        Just("out of memory".to_string()),
        Just("network error".to_string()),
        Just("permission denied".to_string()),
        Just("invalid format".to_string()),
        Just("model not found 404".to_string()),
        "[a-z ]{10,50}".prop_map(|s| s),
    ]
}

/// Generate device names
fn arb_device_name() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("CUDA:0".to_string()),
        Just("CUDA:1".to_string()),
        Just("Metal".to_string()),
        Just("CPU".to_string()),
        (0usize..8).prop_map(|i| format!("CUDA:{}", i)),
    ]
}

/// Generate adapter names
fn arb_adapter_name() -> impl Strategy<Value = String> {
    prop_oneof!["[a-z]{3,10}/[a-z_-]{3,15}".prop_map(|s| s), "[a-z_-]{5,20}".prop_map(|s| s),]
}

/// Generate tool names
fn arb_tool_name() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z_]{3,15}".prop_map(|s| s),
        "get_[a-z_]{3,10}".prop_map(|s| s),
        "search_[a-z_]{3,10}".prop_map(|s| s),
    ]
}

/// Generate server names
fn arb_server_name() -> impl Strategy<Value = String> {
    prop_oneof!["[a-z_-]{3,15}".prop_map(|s| s), "mcp-server-[a-z]{3,10}".prop_map(|s| s),]
}

// ============================================================================
// Property Tests
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: mistral-rs-integration, Property 5.1: Model Load Error Quality**
    /// *For any* model load error, the message SHALL contain the model ID and a suggestion.
    /// **Validates: Requirements 12.1, 12.2**
    #[test]
    fn prop_model_load_error_contains_context(
        model_id in arb_model_id(),
        reason in arb_reason(),
    ) {
        let error = MistralRsError::model_load(&model_id, &reason);
        let message = error.to_string();

        // Error message must contain the model ID
        prop_assert!(
            message.contains(&model_id),
            "Error message '{}' should contain model ID '{}'",
            message, model_id
        );

        // Error message must contain the reason
        prop_assert!(
            message.contains(&reason),
            "Error message '{}' should contain reason '{}'",
            message, reason
        );

        // Error message must contain a suggestion (indicated by common suggestion words)
        let has_suggestion = message.contains("Try")
            || message.contains("Verify")
            || message.contains("Check")
            || message.contains("Ensure")
            || message.contains("huggingface");
        prop_assert!(
            has_suggestion,
            "Error message '{}' should contain an actionable suggestion",
            message
        );

        // Verify error category
        prop_assert_eq!(error.category(), "model_load");
    }

    /// **Feature: mistral-rs-integration, Property 5.2: Model Not Found Error Quality**
    /// *For any* model not found error, the message SHALL contain the path and a suggestion.
    /// **Validates: Requirements 12.1, 12.2**
    #[test]
    fn prop_model_not_found_error_contains_path(
        path in arb_path(),
    ) {
        let error = MistralRsError::model_not_found(&path);
        let message = error.to_string();

        // Error message must contain the path
        prop_assert!(
            message.contains(&path),
            "Error message '{}' should contain path '{}'",
            message, path
        );

        // Error message must contain a suggestion
        let has_suggestion = message.contains("Verify")
            || message.contains("Check")
            || message.contains("HuggingFace")
            || message.contains("ensure");
        prop_assert!(
            has_suggestion,
            "Error message '{}' should contain an actionable suggestion",
            message
        );

        // Verify error classification
        prop_assert!(error.is_resource_error());
        prop_assert_eq!(error.category(), "model_not_found");
    }

    /// **Feature: mistral-rs-integration, Property 5.3: Out of Memory Error Quality**
    /// *For any* OOM error, the message SHALL contain the operation, details, and memory-related suggestions.
    /// **Validates: Requirements 12.1, 12.2**
    #[test]
    fn prop_out_of_memory_error_contains_suggestions(
        operation in prop_oneof![
            Just("loading model".to_string()),
            Just("inference".to_string()),
            Just("tokenization".to_string()),
        ],
        details in arb_reason(),
    ) {
        let error = MistralRsError::out_of_memory(&operation, &details);
        let message = error.to_string();

        // Error message must contain the operation
        prop_assert!(
            message.contains(&operation),
            "Error message '{}' should contain operation '{}'",
            message, operation
        );

        // Error message must contain the details
        prop_assert!(
            message.contains(&details),
            "Error message '{}' should contain details '{}'",
            message, details
        );

        // Error message must contain memory-related suggestions
        let has_memory_suggestion = message.contains("ISQ")
            || message.contains("quantization")
            || message.contains("context length")
            || message.contains("PagedAttention")
            || message.contains("smaller model");
        prop_assert!(
            has_memory_suggestion,
            "Error message '{}' should contain memory-related suggestions",
            message
        );

        // Verify error classification
        prop_assert!(error.is_resource_error());
        prop_assert_eq!(error.category(), "out_of_memory");
    }

    /// **Feature: mistral-rs-integration, Property 5.4: Device Not Available Error Quality**
    /// *For any* device error, the message SHALL contain the device name and platform-specific suggestions.
    /// **Validates: Requirements 12.1, 12.2**
    #[test]
    fn prop_device_not_available_error_contains_device(
        device in arb_device_name(),
    ) {
        let error = MistralRsError::device_not_available(&device);
        let message = error.to_string();

        // Error message must contain the device name
        prop_assert!(
            message.contains(&device),
            "Error message '{}' should contain device '{}'",
            message, device
        );

        // Error message must contain platform-specific suggestions
        let has_platform_suggestion = if device.contains("CUDA") {
            message.contains("NVIDIA") || message.contains("cuda") || message.contains("GPU")
        } else if device.contains("Metal") {
            message.contains("macOS") || message.contains("Apple") || message.contains("metal")
        } else {
            message.contains("Auto") || message.contains("device")
        };
        prop_assert!(
            has_platform_suggestion,
            "Error message '{}' should contain platform-specific suggestions for device '{}'",
            message, device
        );

        // Verify error classification
        prop_assert!(error.is_resource_error());
        prop_assert_eq!(error.category(), "device_not_available");
    }

    /// **Feature: mistral-rs-integration, Property 5.5: Inference Error Quality**
    /// *For any* inference error, the message SHALL contain the model ID and reason.
    /// **Validates: Requirements 12.1**
    #[test]
    fn prop_inference_error_contains_context(
        model_id in arb_model_id(),
        reason in arb_reason(),
    ) {
        let error = MistralRsError::inference(&model_id, &reason);
        let message = error.to_string();

        // Error message must contain the model ID
        prop_assert!(
            message.contains(&model_id),
            "Error message '{}' should contain model ID '{}'",
            message, model_id
        );

        // Error message must contain the reason
        prop_assert!(
            message.contains(&reason),
            "Error message '{}' should contain reason '{}'",
            message, reason
        );

        // Verify error classification
        prop_assert!(error.is_recoverable());
        prop_assert_eq!(error.category(), "inference");
    }

    /// **Feature: mistral-rs-integration, Property 5.6: Adapter Not Found Error Quality**
    /// *For any* adapter not found error, the message SHALL list available adapters.
    /// **Validates: Requirements 12.1, 12.2**
    #[test]
    fn prop_adapter_not_found_lists_available(
        missing_adapter in arb_adapter_name(),
        available_adapters in prop::collection::vec(arb_adapter_name(), 1..5),
    ) {
        let error = MistralRsError::adapter_not_found(&missing_adapter, available_adapters.clone());
        let message = error.to_string();

        // Error message must contain the missing adapter name
        prop_assert!(
            message.contains(&missing_adapter),
            "Error message '{}' should contain missing adapter '{}'",
            message, missing_adapter
        );

        // Error message must list available adapters
        for adapter in &available_adapters {
            prop_assert!(
                message.contains(adapter),
                "Error message '{}' should list available adapter '{}'",
                message, adapter
            );
        }

        // Verify error category
        prop_assert_eq!(error.category(), "adapter_not_found");
    }

    /// **Feature: mistral-rs-integration, Property 5.7: Tool Conversion Error Quality**
    /// *For any* tool conversion error, the message SHALL contain the tool name and reason.
    /// **Validates: Requirements 12.1**
    #[test]
    fn prop_tool_conversion_error_contains_tool_name(
        tool_name in arb_tool_name(),
        reason in arb_reason(),
    ) {
        let error = MistralRsError::tool_conversion(&tool_name, &reason);
        let message = error.to_string();

        // Error message must contain the tool name
        prop_assert!(
            message.contains(&tool_name),
            "Error message '{}' should contain tool name '{}'",
            message, tool_name
        );

        // Error message must contain the reason
        prop_assert!(
            message.contains(&reason),
            "Error message '{}' should contain reason '{}'",
            message, reason
        );

        // Verify error category
        prop_assert_eq!(error.category(), "tool_conversion");
    }

    /// **Feature: mistral-rs-integration, Property 5.8: MCP Client Error Quality**
    /// *For any* MCP client error, the message SHALL contain the server name and reason.
    /// **Validates: Requirements 12.1**
    #[test]
    fn prop_mcp_client_error_contains_server(
        server in arb_server_name(),
        reason in arb_reason(),
    ) {
        let error = MistralRsError::mcp_client(&server, &reason);
        let message = error.to_string();

        // Error message must contain the server name
        prop_assert!(
            message.contains(&server),
            "Error message '{}' should contain server '{}'",
            message, server
        );

        // Error message must contain the reason
        prop_assert!(
            message.contains(&reason),
            "Error message '{}' should contain reason '{}'",
            message, reason
        );

        // Verify error classification
        prop_assert!(error.is_recoverable());
        prop_assert_eq!(error.category(), "mcp_client");
    }

    /// **Feature: mistral-rs-integration, Property 5.9: Image Processing Error Quality**
    /// *For any* image processing error, the message SHALL mention supported formats.
    /// **Validates: Requirements 12.1, 12.2**
    #[test]
    fn prop_image_processing_error_mentions_formats(
        reason in arb_reason(),
    ) {
        let error = MistralRsError::image_processing(&reason);
        let message = error.to_string();

        // Error message must contain the reason
        prop_assert!(
            message.contains(&reason),
            "Error message '{}' should contain reason '{}'",
            message, reason
        );

        // Error message must mention supported formats
        let mentions_formats = message.contains("JPEG")
            || message.contains("PNG")
            || message.contains("WebP")
            || message.contains("GIF");
        prop_assert!(
            mentions_formats,
            "Error message '{}' should mention supported image formats",
            message
        );

        // Verify error category
        prop_assert_eq!(error.category(), "image_processing");
    }

    /// **Feature: mistral-rs-integration, Property 5.10: Audio Processing Error Quality**
    /// *For any* audio processing error, the message SHALL mention supported formats.
    /// **Validates: Requirements 12.1, 12.2**
    #[test]
    fn prop_audio_processing_error_mentions_formats(
        reason in arb_reason(),
    ) {
        let error = MistralRsError::audio_processing(&reason);
        let message = error.to_string();

        // Error message must contain the reason
        prop_assert!(
            message.contains(&reason),
            "Error message '{}' should contain reason '{}'",
            message, reason
        );

        // Error message must mention supported formats
        let mentions_formats = message.contains("WAV")
            || message.contains("MP3")
            || message.contains("FLAC")
            || message.contains("OGG");
        prop_assert!(
            mentions_formats,
            "Error message '{}' should mention supported audio formats",
            message
        );

        // Verify error category
        prop_assert_eq!(error.category(), "audio_processing");
    }
}

// ============================================================================
// Error Classification Property Tests
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: mistral-rs-integration, Property 5.11: Error Classification Consistency**
    /// *For any* error, the classification methods SHALL be consistent with the error type.
    /// **Validates: Requirements 12.3**
    #[test]
    fn prop_error_classification_consistency(
        model_id in arb_model_id(),
        reason in arb_reason(),
    ) {
        // Test recoverable errors
        let inference_err = MistralRsError::inference(&model_id, &reason);
        prop_assert!(inference_err.is_recoverable());
        prop_assert!(!inference_err.is_config_error());
        prop_assert!(!inference_err.is_resource_error());

        // Test config errors
        let config_err = MistralRsError::invalid_config("field", &reason, "suggestion");
        prop_assert!(config_err.is_config_error());
        prop_assert!(!config_err.is_recoverable());
        prop_assert!(!config_err.is_resource_error());

        // Test resource errors
        let resource_err = MistralRsError::out_of_memory("operation", &reason);
        prop_assert!(resource_err.is_resource_error());
        prop_assert!(!resource_err.is_recoverable());
        prop_assert!(!resource_err.is_config_error());
    }

    /// **Feature: mistral-rs-integration, Property 5.12: Error Category Uniqueness**
    /// *For any* error, the category SHALL be a non-empty string.
    /// **Validates: Requirements 12.3**
    #[test]
    fn prop_error_category_non_empty(
        model_id in arb_model_id(),
        path in arb_path(),
        reason in arb_reason(),
    ) {
        let errors = vec![
            MistralRsError::model_load(&model_id, &reason),
            MistralRsError::model_not_found(&path),
            MistralRsError::inference(&model_id, &reason),
            MistralRsError::image_processing(&reason),
            MistralRsError::audio_processing(&reason),
            MistralRsError::embedding(&reason),
            MistralRsError::speech(&reason),
            MistralRsError::diffusion(&reason),
        ];

        for error in errors {
            let category = error.category();
            prop_assert!(
                !category.is_empty(),
                "Error category should not be empty for {:?}",
                error
            );
            prop_assert!(
                category.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "Error category '{}' should be lowercase with underscores",
                category
            );
        }
    }
}

// ============================================================================
// Unit Tests for Edge Cases
// ============================================================================

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn test_model_load_error_with_memory_reason() {
        let error = MistralRsError::model_load("test/model", "out of memory during loading");
        let message = error.to_string();

        // Should suggest ISQ or quantization for memory issues
        assert!(
            message.contains("ISQ") || message.contains("quantization"),
            "Memory-related error should suggest quantization: {}",
            message
        );
    }

    #[test]
    fn test_model_load_error_with_network_reason() {
        let error = MistralRsError::model_load("test/model", "network connection failed");
        let message = error.to_string();

        // Should suggest offline use or checking connection
        assert!(
            message.contains("internet")
                || message.contains("offline")
                || message.contains("connection"),
            "Network-related error should suggest checking connection: {}",
            message
        );
    }

    #[test]
    fn test_model_load_error_with_not_found_reason() {
        let error = MistralRsError::model_load("test/model", "model not found 404");
        let message = error.to_string();

        // Should suggest verifying model ID
        assert!(
            message.contains("Verify") || message.contains("huggingface"),
            "Not found error should suggest verifying model ID: {}",
            message
        );
    }

    #[test]
    fn test_device_cuda_suggestion() {
        let error = MistralRsError::device_not_available("CUDA:0");
        let message = error.to_string();

        assert!(message.contains("NVIDIA") || message.contains("CUDA"));
        assert!(message.contains("cuda") || message.contains("feature"));
    }

    #[test]
    fn test_device_metal_suggestion() {
        let error = MistralRsError::device_not_available("Metal");
        let message = error.to_string();

        assert!(message.contains("macOS") || message.contains("Apple"));
        assert!(message.contains("metal") || message.contains("feature"));
    }

    #[test]
    fn test_chat_template_error_has_suggestion() {
        let error = MistralRsError::chat_template("invalid syntax");
        let message = error.to_string();

        assert!(message.contains("invalid syntax"));
        assert!(
            message.contains("Jinja2")
                || message.contains("template")
                || message.contains("Verify"),
            "Chat template error should mention template format: {}",
            message
        );
    }

    #[test]
    fn test_adapter_load_error_has_suggestion() {
        let error = MistralRsError::adapter_load("my-adapter", "incompatible base model");
        let message = error.to_string();

        assert!(message.contains("my-adapter"));
        assert!(message.contains("incompatible base model"));
        assert!(
            message.contains("Verify") || message.contains("compatible"),
            "Adapter load error should suggest checking compatibility: {}",
            message
        );
    }

    #[test]
    fn test_multi_model_routing_lists_available() {
        let error = MistralRsError::multi_model_routing(
            "missing-model",
            vec!["model-a".to_string(), "model-b".to_string()],
        );
        let message = error.to_string();

        assert!(message.contains("missing-model"));
        assert!(message.contains("model-a"));
        assert!(message.contains("model-b"));
    }

    #[test]
    fn test_invalid_config_has_all_fields() {
        let error = MistralRsError::invalid_config(
            "temperature",
            "must be between 0 and 2",
            "Use a value like 0.7 for balanced output",
        );
        let message = error.to_string();

        assert!(message.contains("temperature"));
        assert!(message.contains("must be between 0 and 2"));
        assert!(message.contains("Use a value like 0.7"));
    }
}
