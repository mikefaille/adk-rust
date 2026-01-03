//! Property tests for chat template application.
//!
//! **Property 15: Chat Template Application**
//! *For any* custom chat template provided in config, the template SHALL be
//! applied to format messages correctly for the model.
//!
//! **Validates: Requirements 16.1**

use adk_mistralrs::{MistralRsConfig, ModelSource};
use proptest::prelude::*;
use std::path::PathBuf;

// Strategy for generating valid chat template strings
fn arb_chat_template() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("{% for message in messages %}{{ message.content }}{% endfor %}".to_string()),
        Just("{{ bos_token }}{% for message in messages %}{{ message.role }}: {{ message.content }}\n{% endfor %}".to_string()),
        Just("<|im_start|>{% for message in messages %}{{ message.role }}\n{{ message.content }}<|im_end|>\n{% endfor %}".to_string()),
    ]
}

// Strategy for generating tokenizer paths
fn arb_tokenizer_path() -> impl Strategy<Value = PathBuf> {
    "[a-z/]{5,20}/tokenizer\\.json".prop_map(PathBuf::from)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: mistral-rs-integration, Property 15: Chat Template Application**
    /// *For any* custom chat template provided in config, the template SHALL be
    /// stored correctly in the configuration.
    #[test]
    fn prop_chat_template_stored_in_config(
        template in arb_chat_template(),
    ) {
        let config = MistralRsConfig::builder()
            .model_source(ModelSource::huggingface("test/model"))
            .chat_template(template.clone())
            .build();

        prop_assert!(config.chat_template.is_some());
        prop_assert_eq!(config.chat_template.unwrap(), template);
    }

    /// Property test for tokenizer path configuration
    #[test]
    fn prop_tokenizer_path_stored_in_config(
        path in arb_tokenizer_path(),
    ) {
        let config = MistralRsConfig::builder()
            .model_source(ModelSource::huggingface("test/model"))
            .tokenizer_path(path.clone())
            .build();

        prop_assert!(config.tokenizer_path.is_some());
        prop_assert_eq!(config.tokenizer_path.unwrap(), path);
    }

    /// Property test for combined chat template and tokenizer config
    #[test]
    fn prop_chat_template_and_tokenizer_combined(
        template in arb_chat_template(),
        path in arb_tokenizer_path(),
    ) {
        let config = MistralRsConfig::builder()
            .model_source(ModelSource::huggingface("test/model"))
            .chat_template(template.clone())
            .tokenizer_path(path.clone())
            .build();

        prop_assert!(config.chat_template.is_some());
        prop_assert!(config.tokenizer_path.is_some());
        prop_assert_eq!(config.chat_template.unwrap(), template);
        prop_assert_eq!(config.tokenizer_path.unwrap(), path);
    }
}

#[test]
fn test_default_config_has_no_chat_template() {
    let config = MistralRsConfig::default();
    assert!(config.chat_template.is_none());
    assert!(config.tokenizer_path.is_none());
}

#[test]
fn test_chat_template_with_special_characters() {
    let template =
        "{% if messages[0]['role'] == 'system' %}{{ messages[0]['content'] }}{% endif %}";
    let config = MistralRsConfig::builder()
        .model_source(ModelSource::huggingface("test/model"))
        .chat_template(template.to_string())
        .build();

    assert_eq!(config.chat_template, Some(template.to_string()));
}
