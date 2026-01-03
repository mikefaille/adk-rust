//! Property tests for MatFormer configuration.
//!
//! **Property: MatFormer Configuration**
//! *For any* valid MatFormer configuration, the system SHALL correctly store
//! and apply the target size and optional config path.
//!
//! **Validates: Requirements 18.1, 18.2, 18.3**

use adk_mistralrs::{MatFormerConfig, MistralRsConfig, ModelArchitecture, ModelSource};
use proptest::prelude::*;
use std::path::PathBuf;

// ============================================================================
// Generators
// ============================================================================

/// Generate valid MatFormer target sizes
fn arb_target_size() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("2b".to_string()),
        Just("4b".to_string()),
        Just("E2B".to_string()),
        Just("E4B".to_string()),
        "[a-zA-Z0-9]{1,10}".prop_map(|s| s),
    ]
}

/// Generate optional config paths
fn arb_config_path() -> impl Strategy<Value = Option<PathBuf>> {
    prop_oneof![Just(None), "[a-z/]{5,30}\\.csv".prop_map(|s| Some(PathBuf::from(s))),]
}

// ============================================================================
// Property Tests
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: mistral-rs-integration, Property: MatFormer Target Size Storage**
    /// *For any* valid target size, the MatFormerConfig SHALL store it correctly.
    /// **Validates: Requirements 18.1, 18.3**
    #[test]
    fn prop_matformer_target_size_stored(target_size in arb_target_size()) {
        let config = MatFormerConfig::new(target_size.clone());
        prop_assert_eq!(&config.target_size, &target_size);
        prop_assert!(config.config_path.is_none());
    }

    /// **Feature: mistral-rs-integration, Property: MatFormer Config Path Storage**
    /// *For any* valid config path, the MatFormerConfig SHALL store it correctly.
    /// **Validates: Requirements 18.1**
    #[test]
    fn prop_matformer_config_path_stored(
        target_size in arb_target_size(),
        config_path in arb_config_path()
    ) {
        let config = if let Some(path) = config_path.clone() {
            MatFormerConfig::with_config_path(target_size.clone(), path)
        } else {
            MatFormerConfig::new(target_size.clone())
        };

        prop_assert_eq!(&config.target_size, &target_size);
        prop_assert_eq!(&config.config_path, &config_path);
    }

    /// **Feature: mistral-rs-integration, Property: MatFormer in MistralRsConfig**
    /// *For any* MatFormer configuration, it SHALL be correctly stored in MistralRsConfig.
    /// **Validates: Requirements 18.1, 18.2**
    #[test]
    fn prop_matformer_in_mistralrs_config(target_size in arb_target_size()) {
        let matformer = MatFormerConfig::new(target_size.clone());
        let config = MistralRsConfig::builder()
            .model_source(ModelSource::huggingface("google/gemma-3n-E4B-it"))
            .architecture(ModelArchitecture::Vision)
            .matformer(matformer)
            .build();

        prop_assert!(config.matformer.is_some());
        let stored = config.matformer.as_ref().unwrap();
        prop_assert_eq!(&stored.target_size, &target_size);
    }

    /// **Feature: mistral-rs-integration, Property: MatFormer Builder Pattern**
    /// *For any* configuration built with the builder pattern, values SHALL be preserved.
    /// **Validates: Requirements 18.1, 18.3**
    #[test]
    fn prop_matformer_builder_pattern(
        target_size in arb_target_size(),
        path_suffix in "[a-z]{3,10}"
    ) {
        let path = PathBuf::from(format!("/config/{}.csv", path_suffix));
        let config = MatFormerConfig::new(target_size.clone())
            .config_path(path.clone());

        prop_assert_eq!(&config.target_size, &target_size);
        prop_assert_eq!(config.config_path, Some(path));
    }

    /// **Feature: mistral-rs-integration, Property: MatFormer Default Config Path**
    /// *For any* MatFormerConfig created with new(), config_path SHALL be None.
    /// **Validates: Requirements 18.1**
    #[test]
    fn prop_matformer_default_config_path(target_size in arb_target_size()) {
        let config = MatFormerConfig::new(target_size);
        prop_assert!(config.config_path.is_none());
    }
}

// ============================================================================
// Unit Tests
// ============================================================================

#[test]
fn test_matformer_known_sizes() {
    // Test known Gemma 3n sizes
    let sizes = ["2b", "4b", "E2B", "E4B"];
    for size in sizes {
        let config = MatFormerConfig::new(size);
        assert_eq!(config.target_size, size);
    }
}

#[test]
fn test_matformer_with_config_path() {
    let config = MatFormerConfig::with_config_path("E4B", "/path/to/matformer_config.csv");
    assert_eq!(config.target_size, "E4B");
    assert_eq!(config.config_path, Some(PathBuf::from("/path/to/matformer_config.csv")));
}

#[test]
fn test_matformer_config_in_vision_model_config() {
    let config = MistralRsConfig::builder()
        .model_source(ModelSource::huggingface("google/gemma-3n-E4B-it"))
        .architecture(ModelArchitecture::Vision)
        .matformer(MatFormerConfig::new("E4B"))
        .build();

    assert!(config.matformer.is_some());
    assert_eq!(config.matformer.as_ref().unwrap().target_size, "E4B");
    assert_eq!(config.architecture, ModelArchitecture::Vision);
}

#[test]
fn test_matformer_config_optional() {
    let config = MistralRsConfig::builder()
        .model_source(ModelSource::huggingface("microsoft/Phi-3.5-vision-instruct"))
        .architecture(ModelArchitecture::Vision)
        .build();

    assert!(config.matformer.is_none());
}
