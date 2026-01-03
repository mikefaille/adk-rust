//! Property tests for diffusion configuration completeness.
//!
//! **Property 8: Diffusion Config Completeness**
//! *For any* diffusion generation request with size parameters, the values SHALL be
//! correctly propagated and stored in the configuration.
//!
//! **Validates: Requirements 8.3**

use adk_mistralrs::{Device, DiffusionConfig, DiffusionModelType, DiffusionParams, ModelSource};
use proptest::prelude::*;

// Generators for diffusion parameters
fn arb_width() -> impl Strategy<Value = Option<u32>> {
    prop_oneof![Just(None), (256u32..2048).prop_map(Some),]
}

fn arb_height() -> impl Strategy<Value = Option<u32>> {
    prop_oneof![Just(None), (256u32..2048).prop_map(Some),]
}

fn arb_device() -> impl Strategy<Value = Device> {
    prop_oneof![
        Just(Device::Auto),
        Just(Device::Cpu),
        (0usize..8).prop_map(Device::Cuda),
        Just(Device::Metal),
    ]
}

fn arb_diffusion_model_type() -> impl Strategy<Value = DiffusionModelType> {
    prop_oneof![Just(DiffusionModelType::FluxOffloaded), Just(DiffusionModelType::Flux),]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: mistral-rs-integration, Property 8: Diffusion Config Completeness**
    /// *For any* diffusion generation request with size parameters, the values SHALL be
    /// correctly propagated and stored in the configuration.
    /// **Validates: Requirements 8.3**
    #[test]
    fn prop_diffusion_config_completeness(
        width in arb_width(),
        height in arb_height(),
    ) {
        // Build diffusion params with size
        let mut params = DiffusionParams::new();

        if let Some(w) = width {
            params = params.with_width(w);
        }
        if let Some(h) = height {
            params = params.with_height(h);
        }

        // Verify values are correctly stored
        prop_assert_eq!(params.width, width);
        prop_assert_eq!(params.height, height);
    }

    /// Property test for diffusion params with_size method
    #[test]
    fn prop_diffusion_params_with_size(
        width in 256u32..2048,
        height in 256u32..2048,
    ) {
        let params = DiffusionParams::new()
            .with_size(width, height);

        prop_assert_eq!(params.width, Some(width));
        prop_assert_eq!(params.height, Some(height));
    }

    /// Property test for diffusion config builder
    #[test]
    fn prop_diffusion_config_builder(
        model_type in arb_diffusion_model_type(),
        device in arb_device(),
        max_seqs in 1usize..64,
    ) {
        let config = DiffusionConfig::builder()
            .model_source(ModelSource::huggingface("test/model"))
            .model_type(model_type)
            .device(device)
            .max_num_seqs(max_seqs)
            .build();

        prop_assert_eq!(config.model_type, model_type);
        prop_assert_eq!(config.device, device);
        prop_assert_eq!(config.max_num_seqs, Some(max_seqs));
    }

    /// Property test for all diffusion model types are representable
    #[test]
    fn prop_diffusion_model_type_completeness(
        model_type in arb_diffusion_model_type(),
    ) {
        // Verify all model types can be stored in config
        let config = DiffusionConfig::builder()
            .model_source(ModelSource::huggingface("test/model"))
            .model_type(model_type)
            .build();

        prop_assert_eq!(config.model_type, model_type);
    }

    /// Property test for individual dimension setters
    #[test]
    fn prop_diffusion_params_individual_dimensions(
        width in 256u32..2048,
        height in 256u32..2048,
    ) {
        let params = DiffusionParams::new()
            .with_width(width)
            .with_height(height);

        prop_assert_eq!(params.width, Some(width));
        prop_assert_eq!(params.height, Some(height));
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn test_diffusion_params_default_values() {
        let params = DiffusionParams::default();

        // Default values should be None
        assert!(params.width.is_none());
        assert!(params.height.is_none());
    }

    #[test]
    fn test_image_output_types() {
        use adk_mistralrs::ImageOutput;

        let path_output = ImageOutput::from_path("/tmp/test.png".to_string(), 1024, 1024);
        assert!(path_output.file_path.is_some());
        assert!(path_output.base64_data.is_none());

        let base64_output = ImageOutput::from_base64("abc123".to_string(), 512, 512);
        assert!(base64_output.file_path.is_none());
        assert!(base64_output.base64_data.is_some());
    }

    #[test]
    fn test_all_diffusion_model_types() {
        let types = [DiffusionModelType::FluxOffloaded, DiffusionModelType::Flux];
        assert_eq!(types.len(), 2);
    }
}
