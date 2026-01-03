//! MistralRsEmbeddingModel - embedding model provider for semantic search and RAG.

use std::sync::Arc;

use mistralrs::{
    AutoDeviceMapParams, DeviceMapSetting, EmbeddingModelBuilder, EmbeddingRequest, IsqType,
    Topology,
};
use tracing::{debug, info, instrument, warn};

use crate::config::{Device, MistralRsConfig, ModelSource, QuantizationLevel};
use crate::error::{MistralRsError, Result};

/// Embedding model provider for ADK using mistral.rs.
///
/// This struct wraps a mistral.rs embedding model instance and provides
/// methods for generating embeddings from text inputs.
///
/// # Supported Models
///
/// - EmbeddingGemma (google/embedding-gemma-001)
/// - Qwen3 Embedding models
/// - Other embedding models supported by mistral.rs
///
/// # Example
///
/// ```rust,ignore
/// use adk_mistralrs::{MistralRsEmbeddingModel, MistralRsConfig, ModelSource};
///
/// let model = MistralRsEmbeddingModel::from_hf("google/embedding-gemma-001").await?;
///
/// // Generate a single embedding
/// let embedding = model.embed("What is machine learning?").await?;
///
/// // Generate batch embeddings
/// let embeddings = model.embed_batch(vec![
///     "What is machine learning?",
///     "How does neural network work?",
/// ]).await?;
/// ```
pub struct MistralRsEmbeddingModel {
    /// The underlying mistral.rs model instance
    model: Arc<mistralrs::Model>,
    /// Model name for identification
    name: String,
    /// Configuration used to create this model
    config: MistralRsConfig,
    /// Embedding dimension (cached after first embedding)
    embedding_dim: Option<usize>,
}

impl MistralRsEmbeddingModel {
    /// Create a new embedding model from configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - The configuration specifying model source and options
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let config = MistralRsConfig::builder()
    ///     .model_source(ModelSource::huggingface("google/embedding-gemma-001"))
    ///     .build();
    /// let model = MistralRsEmbeddingModel::new(config).await?;
    /// ```
    #[instrument(skip(config), fields(model_source = ?config.model_source))]
    pub async fn new(config: MistralRsConfig) -> Result<Self> {
        let model_id = match &config.model_source {
            ModelSource::HuggingFace(id) => id.clone(),
            ModelSource::Local(path) => path.display().to_string(),
            ModelSource::Gguf(path) => path.display().to_string(),
            ModelSource::Uqff(path) => path.display().to_string(),
        };

        info!("Loading mistral.rs embedding model: {}", model_id);

        let mut builder = EmbeddingModelBuilder::new(model_id.clone());

        // Apply ISQ quantization if configured
        if let Some(isq) = &config.isq {
            let isq_type = quantization_level_to_isq(isq.level);
            builder = builder.with_isq(isq_type);
            debug!("ISQ quantization enabled: {:?}", isq.level);
        }

        // Apply device selection
        let device_map = device_to_device_map(&config.device.device);
        builder = builder.with_device_mapping(device_map);
        debug!("Device configured: {:?}", config.device.device);

        // Apply topology file if configured (for per-layer quantization)
        if let Some(topology_path) = &config.topology_path {
            if topology_path.exists() {
                match Topology::from_path(topology_path) {
                    Ok(topology) => {
                        builder = builder.with_topology(topology);
                        debug!("Topology loaded from: {:?}", topology_path);
                    }
                    Err(e) => {
                        warn!("Failed to load topology file: {}", e);
                        return Err(MistralRsError::topology_file(
                            topology_path.display().to_string(),
                            e.to_string(),
                        ));
                    }
                }
            } else {
                return Err(MistralRsError::topology_file(
                    topology_path.display().to_string(),
                    "File does not exist",
                ));
            }
        }

        // Apply context length if configured
        if let Some(num_ctx) = config.num_ctx {
            builder = builder.with_max_num_seqs(num_ctx);
            debug!("Max sequences configured: {}", num_ctx);
        }

        // Apply tokenizer path if configured
        if let Some(tokenizer_path) = &config.tokenizer_path {
            builder = builder.with_tokenizer_json(tokenizer_path.to_string_lossy().to_string());
            debug!("Custom tokenizer path: {:?}", tokenizer_path);
        }

        // Enable logging
        builder = builder.with_logging();

        // Build the model
        let model = builder
            .build()
            .await
            .map_err(|e| MistralRsError::model_load(&model_id, e.to_string()))?;

        info!("Embedding model loaded successfully: {}", model_id);

        Ok(Self { model: Arc::new(model), name: model_id, config, embedding_dim: None })
    }

    /// Create from HuggingFace model ID with defaults.
    ///
    /// # Arguments
    ///
    /// * `model_id` - HuggingFace model ID (e.g., "google/embedding-gemma-001")
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let model = MistralRsEmbeddingModel::from_hf("google/embedding-gemma-001").await?;
    /// ```
    pub async fn from_hf(model_id: &str) -> Result<Self> {
        let config =
            MistralRsConfig::builder().model_source(ModelSource::huggingface(model_id)).build();
        Self::new(config).await
    }

    /// Create with ISQ quantization.
    ///
    /// # Arguments
    ///
    /// * `config` - Base configuration
    /// * `level` - Quantization level to apply
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let config = MistralRsConfig::builder()
    ///     .model_source(ModelSource::huggingface("google/embedding-gemma-001"))
    ///     .build();
    /// let model = MistralRsEmbeddingModel::with_isq(config, QuantizationLevel::Q4_0).await?;
    /// ```
    pub async fn with_isq(mut config: MistralRsConfig, level: QuantizationLevel) -> Result<Self> {
        config.isq = Some(crate::config::IsqConfig::new(level));
        Self::new(config).await
    }

    /// Get the model configuration
    pub fn config(&self) -> &MistralRsConfig {
        &self.config
    }

    /// Get the model name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Generate an embedding for a single text input.
    ///
    /// # Arguments
    ///
    /// * `text` - The text to embed
    ///
    /// # Returns
    ///
    /// A vector of floating-point numbers representing the embedding.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let embedding = model.embed("What is machine learning?").await?;
    /// println!("Embedding dimension: {}", embedding.len());
    /// ```
    #[instrument(skip(self, text), fields(model = %self.name))]
    pub async fn embed(&self, text: impl ToString) -> Result<Vec<f32>> {
        let text = text.to_string();
        debug!("Generating embedding for text of length {}", text.len());

        let embedding = self
            .model
            .generate_embedding(text)
            .await
            .map_err(|e| MistralRsError::embedding(e.to_string()))?;

        debug!("Generated embedding with dimension {}", embedding.len());
        Ok(embedding)
    }

    /// Generate embeddings for multiple text inputs in a batch.
    ///
    /// This is more efficient than calling `embed()` multiple times
    /// as it processes all inputs in a single batch.
    ///
    /// # Arguments
    ///
    /// * `texts` - Iterator of texts to embed
    ///
    /// # Returns
    ///
    /// A vector of embeddings, one per input text, in the same order.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let embeddings = model.embed_batch(vec![
    ///     "What is machine learning?",
    ///     "How does neural network work?",
    ///     "Explain deep learning",
    /// ]).await?;
    ///
    /// for (i, emb) in embeddings.iter().enumerate() {
    ///     println!("Embedding {}: dimension {}", i, emb.len());
    /// }
    /// ```
    #[instrument(skip(self, texts), fields(model = %self.name))]
    pub async fn embed_batch<I, S>(&self, texts: I) -> Result<Vec<Vec<f32>>>
    where
        I: IntoIterator<Item = S>,
        S: ToString,
    {
        let texts: Vec<String> = texts.into_iter().map(|t| t.to_string()).collect();
        let count = texts.len();

        if count == 0 {
            return Ok(Vec::new());
        }

        debug!("Generating batch embeddings for {} texts", count);

        let request = EmbeddingRequest::builder().add_prompts(texts);

        let embeddings = self
            .model
            .generate_embeddings(request)
            .await
            .map_err(|e| MistralRsError::embedding(e.to_string()))?;

        debug!(
            "Generated {} embeddings with dimension {}",
            embeddings.len(),
            embeddings.first().map(|e| e.len()).unwrap_or(0)
        );

        Ok(embeddings)
    }

    /// Get the embedding dimension.
    ///
    /// This generates a test embedding to determine the dimension if not already cached.
    ///
    /// # Returns
    ///
    /// The dimension of embeddings produced by this model.
    pub async fn embedding_dimension(&mut self) -> Result<usize> {
        if let Some(dim) = self.embedding_dim {
            return Ok(dim);
        }

        // Generate a test embedding to get the dimension
        let test_embedding = self.embed("test").await?;
        let dim = test_embedding.len();
        self.embedding_dim = Some(dim);
        Ok(dim)
    }
}

/// Convert QuantizationLevel to mistral.rs IsqType
fn quantization_level_to_isq(level: QuantizationLevel) -> IsqType {
    match level {
        QuantizationLevel::Q4_0 => IsqType::Q4_0,
        QuantizationLevel::Q4_1 => IsqType::Q4_1,
        QuantizationLevel::Q5_0 => IsqType::Q5_0,
        QuantizationLevel::Q5_1 => IsqType::Q5_1,
        QuantizationLevel::Q8_0 => IsqType::Q8_0,
        QuantizationLevel::Q8_1 => IsqType::Q8_1,
        QuantizationLevel::Q2K => IsqType::Q2K,
        QuantizationLevel::Q3K => IsqType::Q3K,
        QuantizationLevel::Q4K => IsqType::Q4K,
        QuantizationLevel::Q5K => IsqType::Q5K,
        QuantizationLevel::Q6K => IsqType::Q6K,
    }
}

/// Convert Device to mistral.rs DeviceMapSetting
fn device_to_device_map(device: &Device) -> DeviceMapSetting {
    match device {
        Device::Auto => {
            debug!("Using automatic device mapping");
            DeviceMapSetting::Auto(AutoDeviceMapParams::default_text())
        }
        Device::Cpu => {
            debug!("Forcing CPU device");
            DeviceMapSetting::dummy()
        }
        Device::Cuda(_index) => {
            debug!("Using CUDA device mapping");
            DeviceMapSetting::Auto(AutoDeviceMapParams::default_text())
        }
        Device::Metal => {
            debug!("Using Metal device mapping");
            DeviceMapSetting::Auto(AutoDeviceMapParams::default_text())
        }
    }
}

impl std::fmt::Debug for MistralRsEmbeddingModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MistralRsEmbeddingModel")
            .field("name", &self.name)
            .field("config", &self.config)
            .field("embedding_dim", &self.embedding_dim)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quantization_level_conversion() {
        // Test all quantization levels can be converted
        let levels = [
            QuantizationLevel::Q4_0,
            QuantizationLevel::Q4_1,
            QuantizationLevel::Q5_0,
            QuantizationLevel::Q5_1,
            QuantizationLevel::Q8_0,
            QuantizationLevel::Q8_1,
            QuantizationLevel::Q2K,
            QuantizationLevel::Q3K,
            QuantizationLevel::Q4K,
            QuantizationLevel::Q5K,
            QuantizationLevel::Q6K,
        ];

        for level in levels {
            let _ = quantization_level_to_isq(level);
        }
    }

    #[test]
    fn test_device_conversion() {
        let devices = [Device::Auto, Device::Cpu, Device::Cuda(0), Device::Metal];

        for device in devices {
            let _ = device_to_device_map(&device);
        }
    }
}
