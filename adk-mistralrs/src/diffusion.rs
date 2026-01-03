//! Diffusion model support for image generation.
//!
//! This module provides support for diffusion models like FLUX.1,
//! enabling text-to-image generation with configurable parameters.
//!
//! ## Features
//!
//! - Load diffusion models from HuggingFace Hub
//! - Generate images from text prompts
//! - Configurable generation parameters (steps, guidance scale, size)
//! - Output images in various formats
//!
//! ## Example
//!
//! ```rust,ignore
//! use adk_mistralrs::{MistralRsDiffusionModel, DiffusionConfig, DiffusionParams};
//!
//! let model = MistralRsDiffusionModel::from_hf(
//!     "black-forest-labs/FLUX.1-schnell"
//! ).await?;
//!
//! // Generate an image
//! let image = model.generate_image(
//!     "A beautiful sunset over mountains",
//!     DiffusionParams::default(),
//! ).await?;
//! ```

use std::sync::Arc;

use mistralrs::{
    DiffusionGenerationParams, DiffusionLoaderType, DiffusionModelBuilder,
    ImageGenerationResponseFormat,
};
use tracing::{debug, info, instrument};

use crate::config::{Device, ModelSource};
use crate::error::{MistralRsError, Result};

/// Configuration for diffusion generation parameters.
///
/// # Example
///
/// ```rust
/// use adk_mistralrs::DiffusionParams;
///
/// let params = DiffusionParams::default()
///     .with_size(1024, 1024);
/// ```
#[derive(Debug, Clone, Default)]
pub struct DiffusionParams {
    /// Image width in pixels
    pub width: Option<u32>,
    /// Image height in pixels
    pub height: Option<u32>,
}

impl DiffusionParams {
    /// Create new diffusion parameters with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the image size.
    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }

    /// Set the image width.
    pub fn with_width(mut self, width: u32) -> Self {
        self.width = Some(width);
        self
    }

    /// Set the image height.
    pub fn with_height(mut self, height: u32) -> Self {
        self.height = Some(height);
        self
    }

    /// Convert to mistral.rs DiffusionGenerationParams.
    pub(crate) fn to_mistralrs(&self) -> DiffusionGenerationParams {
        let mut params = DiffusionGenerationParams::default();

        if let Some(width) = self.width {
            params.width = width as usize;
        }
        if let Some(height) = self.height {
            params.height = height as usize;
        }

        params
    }
}

/// Configuration for diffusion model loading.
///
/// # Example
///
/// ```rust
/// use adk_mistralrs::{DiffusionConfig, ModelSource, DiffusionModelType};
///
/// let config = DiffusionConfig::builder()
///     .model_source(ModelSource::huggingface("black-forest-labs/FLUX.1-schnell"))
///     .model_type(DiffusionModelType::FluxOffloaded)
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct DiffusionConfig {
    /// Model source: HuggingFace ID or local path
    pub model_source: ModelSource,
    /// Diffusion model type
    pub model_type: DiffusionModelType,
    /// Device configuration
    pub device: Device,
    /// Maximum number of sequences
    pub max_num_seqs: Option<usize>,
}

impl Default for DiffusionConfig {
    fn default() -> Self {
        Self {
            model_source: ModelSource::HuggingFace(String::new()),
            model_type: DiffusionModelType::FluxOffloaded,
            device: Device::Auto,
            max_num_seqs: None,
        }
    }
}

impl DiffusionConfig {
    /// Create a new config builder.
    pub fn builder() -> DiffusionConfigBuilder {
        DiffusionConfigBuilder::default()
    }
}

/// Builder for DiffusionConfig.
#[derive(Debug, Clone, Default)]
pub struct DiffusionConfigBuilder {
    config: DiffusionConfig,
}

impl DiffusionConfigBuilder {
    /// Set the model source.
    pub fn model_source(mut self, source: ModelSource) -> Self {
        self.config.model_source = source;
        self
    }

    /// Set the diffusion model type.
    pub fn model_type(mut self, model_type: DiffusionModelType) -> Self {
        self.config.model_type = model_type;
        self
    }

    /// Set the device.
    pub fn device(mut self, device: Device) -> Self {
        self.config.device = device;
        self
    }

    /// Set the maximum number of sequences.
    pub fn max_num_seqs(mut self, max: usize) -> Self {
        self.config.max_num_seqs = Some(max);
        self
    }

    /// Build the configuration.
    pub fn build(self) -> DiffusionConfig {
        self.config
    }
}

/// Type of diffusion model to load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffusionModelType {
    /// FLUX model with layer offloading for reduced memory
    #[default]
    FluxOffloaded,
    /// FLUX model without offloading (requires more VRAM)
    Flux,
}

impl DiffusionModelType {
    /// Convert to mistral.rs DiffusionLoaderType.
    pub(crate) fn to_mistralrs(self) -> DiffusionLoaderType {
        match self {
            DiffusionModelType::FluxOffloaded => DiffusionLoaderType::FluxOffloaded,
            DiffusionModelType::Flux => DiffusionLoaderType::Flux,
        }
    }
}

/// Output from image generation.
#[derive(Debug, Clone)]
pub struct ImageOutput {
    /// Path to the generated image file (if saved to disk)
    pub file_path: Option<String>,
    /// Base64-encoded image data (if requested)
    pub base64_data: Option<String>,
    /// Image width in pixels
    pub width: u32,
    /// Image height in pixels
    pub height: u32,
}

impl ImageOutput {
    /// Create a new image output with file path.
    pub fn from_path(path: String, width: u32, height: u32) -> Self {
        Self { file_path: Some(path), base64_data: None, width, height }
    }

    /// Create a new image output with base64 data.
    pub fn from_base64(data: String, width: u32, height: u32) -> Self {
        Self { file_path: None, base64_data: Some(data), width, height }
    }
}

/// A mistral.rs diffusion model for image generation.
///
/// This struct wraps a mistral.rs diffusion model and provides methods for
/// generating images from text prompts.
///
/// # Supported Models
///
/// - FLUX.1-schnell (black-forest-labs/FLUX.1-schnell)
/// - FLUX.1-dev (black-forest-labs/FLUX.1-dev)
///
/// # Example
///
/// ```rust,ignore
/// use adk_mistralrs::{MistralRsDiffusionModel, DiffusionParams};
///
/// let model = MistralRsDiffusionModel::from_hf(
///     "black-forest-labs/FLUX.1-schnell"
/// ).await?;
///
/// let image = model.generate_image(
///     "A cat sitting on a windowsill",
///     DiffusionParams::default(),
/// ).await?;
/// ```
pub struct MistralRsDiffusionModel {
    /// The underlying mistral.rs model instance
    model: Arc<mistralrs::Model>,
    /// Model name for identification
    name: String,
    /// Configuration used to create this model
    config: DiffusionConfig,
}

impl MistralRsDiffusionModel {
    /// Create a new diffusion model from configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration specifying model source and options
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let config = DiffusionConfig::builder()
    ///     .model_source(ModelSource::huggingface("black-forest-labs/FLUX.1-schnell"))
    ///     .model_type(DiffusionModelType::FluxOffloaded)
    ///     .build();
    /// let model = MistralRsDiffusionModel::new(config).await?;
    /// ```
    #[instrument(skip(config), fields(model_source = ?config.model_source))]
    pub async fn new(config: DiffusionConfig) -> Result<Self> {
        let model_id = match &config.model_source {
            ModelSource::HuggingFace(id) => id.clone(),
            ModelSource::Local(path) => path.display().to_string(),
            ModelSource::Gguf(path) => path.display().to_string(),
            ModelSource::Uqff(path) => path.display().to_string(),
        };

        info!("Loading mistral.rs diffusion model: {}", model_id);

        let loader_type = config.model_type.to_mistralrs();
        let mut builder = DiffusionModelBuilder::new(&model_id, loader_type);

        // Apply max sequences if configured
        if let Some(max_seqs) = config.max_num_seqs {
            builder = builder.with_max_num_seqs(max_seqs);
            debug!("Max sequences configured: {}", max_seqs);
        }

        // Apply device selection
        if matches!(config.device, Device::Cpu) {
            builder = builder.with_force_cpu();
            debug!("Forcing CPU device");
        }

        // Enable logging
        builder = builder.with_logging();

        // Build the model
        let model = builder
            .build()
            .await
            .map_err(|e| MistralRsError::model_load(&model_id, e.to_string()))?;

        info!("Diffusion model loaded successfully: {}", model_id);

        Ok(Self { model: Arc::new(model), name: model_id, config })
    }

    /// Create from HuggingFace model ID with defaults.
    ///
    /// # Arguments
    ///
    /// * `model_id` - HuggingFace model ID (e.g., "black-forest-labs/FLUX.1-schnell")
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let model = MistralRsDiffusionModel::from_hf(
    ///     "black-forest-labs/FLUX.1-schnell"
    /// ).await?;
    /// ```
    pub async fn from_hf(model_id: &str) -> Result<Self> {
        let config =
            DiffusionConfig::builder().model_source(ModelSource::huggingface(model_id)).build();
        Self::new(config).await
    }

    /// Create from HuggingFace model ID with specific model type.
    ///
    /// # Arguments
    ///
    /// * `model_id` - HuggingFace model ID
    /// * `model_type` - Type of diffusion model
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let model = MistralRsDiffusionModel::from_hf_with_type(
    ///     "black-forest-labs/FLUX.1-schnell",
    ///     DiffusionModelType::Flux,
    /// ).await?;
    /// ```
    pub async fn from_hf_with_type(model_id: &str, model_type: DiffusionModelType) -> Result<Self> {
        let config = DiffusionConfig::builder()
            .model_source(ModelSource::huggingface(model_id))
            .model_type(model_type)
            .build();
        Self::new(config).await
    }

    /// Get the model name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the model configuration.
    pub fn config(&self) -> &DiffusionConfig {
        &self.config
    }

    /// Get a reference to the underlying mistral.rs model.
    pub fn inner(&self) -> &mistralrs::Model {
        &self.model
    }

    /// Generate an image from a text prompt.
    ///
    /// # Arguments
    ///
    /// * `prompt` - Text description of the image to generate
    /// * `params` - Generation parameters (steps, guidance, size)
    ///
    /// # Returns
    ///
    /// Image output containing the path to the generated image.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let params = DiffusionParams::default()
    ///     .with_steps(20)
    ///     .with_size(1024, 1024);
    ///
    /// let image = model.generate_image(
    ///     "A beautiful landscape with mountains",
    ///     params,
    /// ).await?;
    ///
    /// println!("Image saved at: {:?}", image.file_path);
    /// ```
    pub async fn generate_image(
        &self,
        prompt: &str,
        params: DiffusionParams,
    ) -> Result<ImageOutput> {
        debug!("Generating image for prompt: {}", prompt);

        let mistralrs_params = params.to_mistralrs();
        let width = params.width.unwrap_or(1024);
        let height = params.height.unwrap_or(1024);

        let response = self
            .model
            .generate_image(
                prompt.to_string(),
                ImageGenerationResponseFormat::Url,
                mistralrs_params,
            )
            .await
            .map_err(|e| MistralRsError::diffusion(format!("Image generation failed: {}", e)))?;

        // Extract the URL (file path) from the response
        let path = response
            .data
            .first()
            .and_then(|d| d.url.clone())
            .ok_or_else(|| MistralRsError::diffusion("No image URL in response"))?;

        Ok(ImageOutput::from_path(path, width, height))
    }

    /// Generate an image and return as base64-encoded data.
    ///
    /// # Arguments
    ///
    /// * `prompt` - Text description of the image to generate
    /// * `params` - Generation parameters
    ///
    /// # Returns
    ///
    /// Image output containing base64-encoded image data.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let image = model.generate_image_base64(
    ///     "A sunset over the ocean",
    ///     DiffusionParams::default(),
    /// ).await?;
    ///
    /// println!("Base64 data length: {}", image.base64_data.unwrap().len());
    /// ```
    pub async fn generate_image_base64(
        &self,
        prompt: &str,
        params: DiffusionParams,
    ) -> Result<ImageOutput> {
        debug!("Generating base64 image for prompt: {}", prompt);

        let mistralrs_params = params.to_mistralrs();
        let width = params.width.unwrap_or(1024);
        let height = params.height.unwrap_or(1024);

        let response = self
            .model
            .generate_image(
                prompt.to_string(),
                ImageGenerationResponseFormat::B64Json,
                mistralrs_params,
            )
            .await
            .map_err(|e| MistralRsError::diffusion(format!("Image generation failed: {}", e)))?;

        // Extract the base64 data from the response
        let data = response
            .data
            .first()
            .and_then(|d| d.b64_json.clone())
            .ok_or_else(|| MistralRsError::diffusion("No base64 data in response"))?;

        Ok(ImageOutput::from_base64(data, width, height))
    }
}

impl std::fmt::Debug for MistralRsDiffusionModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MistralRsDiffusionModel")
            .field("name", &self.name)
            .field("config", &self.config)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diffusion_params_builder() {
        let params = DiffusionParams::new().with_size(512, 512);

        assert_eq!(params.width, Some(512));
        assert_eq!(params.height, Some(512));
    }

    #[test]
    fn test_diffusion_params_default() {
        let params = DiffusionParams::default();

        assert!(params.width.is_none());
        assert!(params.height.is_none());
    }

    #[test]
    fn test_diffusion_params_to_mistralrs() {
        let params = DiffusionParams::new().with_size(768, 768);

        let mistralrs_params = params.to_mistralrs();

        assert_eq!(mistralrs_params.width, 768);
        assert_eq!(mistralrs_params.height, 768);
    }

    #[test]
    fn test_diffusion_params_individual_dimensions() {
        let params = DiffusionParams::new().with_width(1024).with_height(768);

        assert_eq!(params.width, Some(1024));
        assert_eq!(params.height, Some(768));
    }

    #[test]
    fn test_diffusion_config_builder() {
        let config = DiffusionConfig::builder()
            .model_source(ModelSource::huggingface("test/model"))
            .model_type(DiffusionModelType::Flux)
            .device(Device::Cuda(0))
            .max_num_seqs(16)
            .build();

        assert!(matches!(config.model_source, ModelSource::HuggingFace(_)));
        assert_eq!(config.model_type, DiffusionModelType::Flux);
        assert_eq!(config.device, Device::Cuda(0));
        assert_eq!(config.max_num_seqs, Some(16));
    }

    #[test]
    fn test_diffusion_model_type_conversion() {
        assert!(matches!(
            DiffusionModelType::FluxOffloaded.to_mistralrs(),
            DiffusionLoaderType::FluxOffloaded
        ));
        assert!(matches!(DiffusionModelType::Flux.to_mistralrs(), DiffusionLoaderType::Flux));
    }

    #[test]
    fn test_image_output_from_path() {
        let output = ImageOutput::from_path("/tmp/image.png".to_string(), 1024, 1024);

        assert_eq!(output.file_path, Some("/tmp/image.png".to_string()));
        assert!(output.base64_data.is_none());
        assert_eq!(output.width, 1024);
        assert_eq!(output.height, 1024);
    }

    #[test]
    fn test_image_output_from_base64() {
        let output = ImageOutput::from_base64("abc123".to_string(), 512, 512);

        assert!(output.file_path.is_none());
        assert_eq!(output.base64_data, Some("abc123".to_string()));
        assert_eq!(output.width, 512);
        assert_eq!(output.height, 512);
    }

    #[test]
    fn test_all_diffusion_model_types() {
        let types = [DiffusionModelType::FluxOffloaded, DiffusionModelType::Flux];
        assert_eq!(types.len(), 2);

        // Verify all can be converted
        for t in types {
            let _ = t.to_mistralrs();
        }
    }
}
