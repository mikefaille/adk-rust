//! Property tests for config variant completeness.
//!
//! **Property 4: Config Variant Completeness**
//! *For any* combination of `ModelArchitecture`, `DataType`, `Device`, and `QuantizationLevel`
//! enum variants, the `MistralRsConfig` SHALL accept and represent them without error.
//!
//! **Validates: Requirements 2.4, 2.5, 3.1, 4.1, 4.3**

use adk_mistralrs::{
    DataType, Device, DeviceConfig, IsqConfig, MistralRsConfig, ModelArchitecture, ModelSource,
    QuantizationLevel,
};
use proptest::prelude::*;
use std::collections::HashMap;

// Generators for enum variants
fn arb_model_architecture() -> impl Strategy<Value = ModelArchitecture> {
    prop_oneof![
        Just(ModelArchitecture::Plain),
        Just(ModelArchitecture::Vision),
        Just(ModelArchitecture::Diffusion),
        Just(ModelArchitecture::Speech),
        Just(ModelArchitecture::Embedding),
        Just(ModelArchitecture::XLora),
        Just(ModelArchitecture::Lora),
    ]
}

fn arb_data_type() -> impl Strategy<Value = DataType> {
    prop_oneof![
        Just(DataType::F32),
        Just(DataType::F16),
        Just(DataType::BF16),
        Just(DataType::Auto),
    ]
}

fn arb_device() -> impl Strategy<Value = Device> {
    prop_oneof![
        Just(Device::Auto),
        Just(Device::Cpu),
        (0usize..8).prop_map(Device::Cuda),
        Just(Device::Metal),
    ]
}

fn arb_quantization_level() -> impl Strategy<Value = QuantizationLevel> {
    prop_oneof![
        Just(QuantizationLevel::Q4_0),
        Just(QuantizationLevel::Q4_1),
        Just(QuantizationLevel::Q5_0),
        Just(QuantizationLevel::Q5_1),
        Just(QuantizationLevel::Q8_0),
        Just(QuantizationLevel::Q8_1),
        Just(QuantizationLevel::Q2K),
        Just(QuantizationLevel::Q3K),
        Just(QuantizationLevel::Q4K),
        Just(QuantizationLevel::Q5K),
        Just(QuantizationLevel::Q6K),
    ]
}

fn arb_model_source() -> impl Strategy<Value = ModelSource> {
    prop_oneof![
        "[a-z]{3,10}/[a-z]{3,10}".prop_map(ModelSource::huggingface),
        "/[a-z/]{5,20}".prop_map(ModelSource::local),
        "/[a-z/]{5,20}\\.gguf".prop_map(ModelSource::gguf),
        "/[a-z/]{5,20}\\.uqff".prop_map(ModelSource::uqff),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: mistral-rs-integration, Property 4: Config Variant Completeness**
    /// *For any* combination of ModelArchitecture, DataType, Device, and QuantizationLevel,
    /// the MistralRsConfig SHALL accept and represent them without error.
    #[test]
    fn prop_config_accepts_all_variant_combinations(
        arch in arb_model_architecture(),
        dtype in arb_data_type(),
        device in arb_device(),
        quant in arb_quantization_level(),
        source in arb_model_source(),
    ) {
        // Build config with all variant combinations
        let config = MistralRsConfig::builder()
            .model_source(source.clone())
            .architecture(arch)
            .dtype(dtype)
            .device(DeviceConfig::new(device))
            .isq(quant)
            .build();

        // Verify all values are correctly stored
        prop_assert_eq!(config.architecture, arch);
        prop_assert_eq!(config.dtype, dtype);
        prop_assert_eq!(config.device.device, device);
        prop_assert!(config.isq.is_some());
        prop_assert_eq!(config.isq.as_ref().unwrap().level, quant);
    }

    /// Property test for ISQ config with layer overrides
    #[test]
    fn prop_isq_config_with_overrides(
        base_level in arb_quantization_level(),
        override_level in arb_quantization_level(),
        layer_name in "[a-z_]{3,10}",
    ) {
        let mut overrides = HashMap::new();
        overrides.insert(layer_name.clone(), override_level);

        let isq = IsqConfig::with_overrides(base_level, overrides);

        prop_assert_eq!(isq.level, base_level);
        prop_assert!(isq.layer_overrides.is_some());
        prop_assert_eq!(
            isq.layer_overrides.as_ref().unwrap().get(&layer_name),
            Some(&override_level)
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property test for device config with mapping
    #[test]
    fn prop_device_config_with_mapping(
        primary in arb_device(),
        secondary in arb_device(),
        layer_name in "[a-z_]{3,10}",
    ) {
        let mut device_map = HashMap::new();
        device_map.insert(layer_name.clone(), secondary);

        let config = DeviceConfig::with_map(primary, device_map);

        prop_assert_eq!(config.device, primary);
        prop_assert!(config.device_map.is_some());
        prop_assert_eq!(
            config.device_map.as_ref().unwrap().get(&layer_name),
            Some(&secondary)
        );
    }

    /// Property test for generation config parameters
    #[test]
    fn prop_generation_config_parameters(
        temp in 0.0f32..2.0f32,
        top_p in 0.0f32..1.0f32,
        top_k in 1i32..100i32,
        max_tokens in 1i32..4096i32,
        num_ctx in 512usize..8192usize,
    ) {
        let config = MistralRsConfig::builder()
            .model_source(ModelSource::huggingface("test/model"))
            .temperature(temp)
            .top_p(top_p)
            .top_k(top_k)
            .max_tokens(max_tokens)
            .num_ctx(num_ctx)
            .build();

        prop_assert_eq!(config.temperature, Some(temp));
        prop_assert_eq!(config.top_p, Some(top_p));
        prop_assert_eq!(config.top_k, Some(top_k));
        prop_assert_eq!(config.max_tokens, Some(max_tokens));
        prop_assert_eq!(config.num_ctx, Some(num_ctx));
    }

    /// **Feature: mistral-rs-integration, Property: ISQ Config Completeness**
    /// *For any* quantization level, the IsqConfig SHALL correctly store and retrieve it.
    /// This validates that all ISQ quantization levels are representable.
    /// **Validates: Requirements 3.1**
    #[test]
    fn prop_isq_config_completeness(
        level in arb_quantization_level(),
    ) {
        // Create ISQ config with the level
        let isq = IsqConfig::new(level);

        // Verify the level is correctly stored
        prop_assert_eq!(isq.level, level);
        prop_assert!(isq.layer_overrides.is_none());

        // Verify it can be used in MistralRsConfig
        let config = MistralRsConfig::builder()
            .model_source(ModelSource::huggingface("test/model"))
            .isq(level)
            .build();

        prop_assert!(config.isq.is_some());
        prop_assert_eq!(config.isq.as_ref().unwrap().level, level);
    }

    /// **Feature: mistral-rs-integration, Property 10: Context Length Configuration**
    /// *For any* `num_ctx` value set in config, the model SHALL accept prompts up to that length.
    /// This validates that context length configuration is correctly stored and retrievable.
    /// **Validates: Requirements 5.3**
    #[test]
    fn prop_context_length_configuration(
        num_ctx in 128usize..32768usize,
    ) {
        // Create config with context length
        let config = MistralRsConfig::builder()
            .model_source(ModelSource::huggingface("test/model"))
            .num_ctx(num_ctx)
            .build();

        // Verify the context length is correctly stored
        prop_assert!(config.num_ctx.is_some());
        prop_assert_eq!(config.num_ctx.unwrap(), num_ctx);

        // Verify it can be combined with other settings
        let config_with_paged = MistralRsConfig::builder()
            .model_source(ModelSource::huggingface("test/model"))
            .num_ctx(num_ctx)
            .paged_attention(true)
            .build();

        prop_assert_eq!(config_with_paged.num_ctx, Some(num_ctx));
        prop_assert!(config_with_paged.paged_attention);
    }
}
