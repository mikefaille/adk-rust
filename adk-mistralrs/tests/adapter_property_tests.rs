//! Property tests for adapter loading and swapping.
//!
//! **Property 11: Adapter Loading and Swapping**
//! *For any* valid LoRA or X-LoRA adapter configuration, the adapter configuration
//! SHALL be correctly constructed and all adapter IDs SHALL be retrievable.
//!
//! **Validates: Requirements 13.1, 13.2**

use adk_mistralrs::{AdapterConfig, AdapterType, MistralRsConfig, ModelSource};
use proptest::prelude::*;
use std::path::PathBuf;

// Generator for valid adapter source strings (HuggingFace IDs or paths)
fn arb_adapter_source() -> impl Strategy<Value = String> {
    prop_oneof![
        // HuggingFace-style IDs
        "[a-z]{3,10}/[a-z_-]{3,15}".prop_map(|s| s),
        // Local paths
        "/[a-z/]{5,20}".prop_map(|s| s),
    ]
}

// Generator for ordering file paths
fn arb_ordering_path() -> impl Strategy<Value = PathBuf> {
    "[a-z_]{3,10}\\.json".prop_map(PathBuf::from)
}

// Generator for adapter type
fn arb_adapter_type() -> impl Strategy<Value = AdapterType> {
    prop_oneof![Just(AdapterType::LoRA), Just(AdapterType::XLoRA),]
}

// Generator for non-granular index
fn arb_tgt_non_granular_index() -> impl Strategy<Value = Option<usize>> {
    prop_oneof![Just(None), (0usize..10).prop_map(Some),]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: mistral-rs-integration, Property 11: Adapter Loading and Swapping**
    /// *For any* valid LoRA adapter source, the AdapterConfig SHALL correctly store
    /// the adapter source and type, and all_adapter_ids() SHALL return the correct list.
    /// **Validates: Requirements 13.1, 13.2**
    #[test]
    fn prop_lora_adapter_config_construction(
        source in arb_adapter_source(),
    ) {
        // Create a LoRA adapter config
        let config = AdapterConfig::lora(source.clone());

        // Verify adapter type is LoRA
        prop_assert_eq!(config.adapter_type, AdapterType::LoRA);

        // Verify adapter source is correctly stored
        prop_assert_eq!(&config.adapter_source, &source);

        // Verify it's not a multi-adapter config
        prop_assert!(!config.is_multi_adapter());

        // Verify all_adapter_ids returns the single adapter
        let ids = config.all_adapter_ids();
        prop_assert_eq!(ids.len(), 1);
        prop_assert_eq!(&ids[0], &source);

        // Verify ordering is None for LoRA
        prop_assert!(config.ordering.is_none());
    }

    /// **Feature: mistral-rs-integration, Property 11: Adapter Loading and Swapping**
    /// *For any* list of valid adapter sources, the multi-adapter LoRA config SHALL
    /// correctly store all adapters and all_adapter_ids() SHALL return them in order.
    /// **Validates: Requirements 13.1, 13.2**
    #[test]
    fn prop_multi_lora_adapter_config_construction(
        adapters in prop::collection::vec(arb_adapter_source(), 2..5),
    ) {
        // Create a multi-adapter LoRA config
        let config = AdapterConfig::lora_multi(adapters.clone());

        // Verify adapter type is LoRA
        prop_assert_eq!(config.adapter_type, AdapterType::LoRA);

        // Verify primary adapter is the first one
        prop_assert_eq!(&config.adapter_source, &adapters[0]);

        // Verify it's now a multi-adapter config
        prop_assert!(config.is_multi_adapter());

        // Verify all_adapter_ids returns all adapters in order
        let ids = config.all_adapter_ids();
        prop_assert_eq!(ids.len(), adapters.len());
        for (i, adapter) in adapters.iter().enumerate() {
            prop_assert_eq!(&ids[i], adapter);
        }
    }

    /// **Feature: mistral-rs-integration, Property 11: Adapter Loading and Swapping**
    /// *For any* valid X-LoRA configuration with ordering file, the AdapterConfig SHALL
    /// correctly store the adapter source, ordering path, and type.
    /// **Validates: Requirements 13.1, 13.2**
    #[test]
    fn prop_xlora_adapter_config_construction(
        source in arb_adapter_source(),
        ordering in arb_ordering_path(),
        tgt_idx in arb_tgt_non_granular_index(),
    ) {
        // Create an X-LoRA adapter config
        let mut config = AdapterConfig::xlora(source.clone(), ordering.clone());

        // Apply tgt_non_granular_index if provided
        if let Some(idx) = tgt_idx {
            config = config.with_tgt_non_granular_index(idx);
        }

        // Verify adapter type is X-LoRA
        prop_assert_eq!(config.adapter_type, AdapterType::XLoRA);

        // Verify adapter source is correctly stored
        prop_assert_eq!(&config.adapter_source, &source);

        // Verify ordering path is correctly stored
        prop_assert!(config.ordering.is_some());
        prop_assert_eq!(config.ordering.as_ref().unwrap(), &ordering);

        // Verify tgt_non_granular_index is correctly stored
        prop_assert_eq!(config.tgt_non_granular_index, tgt_idx);
    }

    /// Property test for adding additional adapters to a LoRA config
    #[test]
    fn prop_lora_with_additional_adapters(
        primary in arb_adapter_source(),
        additional in prop::collection::vec(arb_adapter_source(), 1..4),
    ) {
        // Create a LoRA config and add additional adapters
        let config = AdapterConfig::lora(primary.clone())
            .with_additional_adapters(additional.clone());

        // Verify adapter type is still LoRA
        prop_assert_eq!(config.adapter_type, AdapterType::LoRA);

        // Verify primary adapter is correct
        prop_assert_eq!(&config.adapter_source, &primary);

        // Verify it's now a multi-adapter config
        prop_assert!(config.is_multi_adapter());

        // Verify all_adapter_ids returns primary + additional
        let ids = config.all_adapter_ids();
        prop_assert_eq!(ids.len(), 1 + additional.len());
        prop_assert_eq!(&ids[0], &primary);
        for (i, adapter) in additional.iter().enumerate() {
            prop_assert_eq!(&ids[i + 1], adapter);
        }
    }

    /// Property test for adapter config integration with MistralRsConfig
    #[test]
    fn prop_adapter_config_in_mistralrs_config(
        source in arb_adapter_source(),
        model_id in "[a-z]{3,10}/[a-z]{3,10}",
    ) {
        // Create a LoRA adapter config
        let adapter_config = AdapterConfig::lora(source.clone());

        // Create MistralRsConfig with the adapter
        let config = MistralRsConfig::builder()
            .model_source(ModelSource::huggingface(model_id))
            .adapter(adapter_config)
            .build();

        // Verify adapter config is correctly stored
        prop_assert!(config.adapter.is_some());
        let stored_adapter = config.adapter.as_ref().unwrap();
        prop_assert_eq!(stored_adapter.adapter_type, AdapterType::LoRA);
        prop_assert_eq!(&stored_adapter.adapter_source, &source);
    }

    /// Property test for adapter type display formatting
    #[test]
    fn prop_adapter_type_display(
        adapter_type in arb_adapter_type(),
    ) {
        let display = format!("{}", adapter_type);

        match adapter_type {
            AdapterType::LoRA => prop_assert_eq!(display, "LoRA"),
            AdapterType::XLoRA => prop_assert_eq!(display, "X-LoRA"),
        }
    }
}

// Additional unit tests for edge cases
#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn test_single_adapter_is_not_multi() {
        let config = AdapterConfig::lora("single/adapter");
        assert!(!config.is_multi_adapter());
        assert_eq!(config.all_adapter_ids().len(), 1);
    }

    #[test]
    fn test_xlora_requires_ordering() {
        let config = AdapterConfig::xlora("xlora/model", PathBuf::from("order.json"));
        assert!(config.ordering.is_some());
        assert_eq!(config.adapter_type, AdapterType::XLoRA);
    }

    #[test]
    fn test_adapter_ids_order_preserved() {
        let adapters = vec!["first", "second", "third"];
        let config = AdapterConfig::lora_multi(adapters.clone());
        let ids = config.all_adapter_ids();

        assert_eq!(ids[0], "first");
        assert_eq!(ids[1], "second");
        assert_eq!(ids[2], "third");
    }

    #[test]
    fn test_tgt_non_granular_index_default_none() {
        let config = AdapterConfig::xlora("model", PathBuf::from("order.json"));
        assert!(config.tgt_non_granular_index.is_none());
    }

    #[test]
    fn test_tgt_non_granular_index_set() {
        let config = AdapterConfig::xlora("model", PathBuf::from("order.json"))
            .with_tgt_non_granular_index(5);
        assert_eq!(config.tgt_non_granular_index, Some(5));
    }
}
