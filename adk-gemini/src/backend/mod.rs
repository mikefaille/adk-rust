use async_trait::async_trait;
use futures::stream::BoxStream;
use mime::Mime;

use crate::batch::model::{
    BatchGenerateContentRequest, BatchGenerateContentResponse, BatchOperation,
    ListBatchesResponse,
};
use crate::cache::model::{
    CacheExpirationRequest, CachedContent, CreateCachedContentRequest,
    ListCachedContentsResponse,
};
use crate::error::Error;
use crate::embedding::{
    BatchContentEmbeddingResponse, BatchEmbedContentsRequest, ContentEmbeddingResponse,
    EmbedContentRequest,
};
use crate::files::model::{File, ListFilesResponse};
use crate::generation::{GenerateContentRequest, GenerationResponse};

#[cfg(feature = "vertex")]
pub mod vertex;

pub mod studio;

/// The unified contract that both Studio and Vertex must fulfill.
/// This ensures calling code never needs to know which backend is active.
#[async_trait]
pub trait GeminiBackend: Send + Sync + std::fmt::Debug {
    /// Get the model name associated with this backend
    fn model(&self) -> &str;

    /// Generate content (unary)
    async fn generate_content(
        &self,
        req: GenerateContentRequest,
    ) -> Result<GenerationResponse, Error>;

    /// Generate content (streaming)
    /// Returns a type-erased stream so implementations can use different libraries (reqwest vs tonic)
    async fn stream_generate_content(
        &self,
        req: GenerateContentRequest,
    ) -> Result<BoxStream<'static, Result<GenerationResponse, Error>>, Error>;

    /// Count tokens
    async fn count_tokens(&self, req: GenerateContentRequest) -> Result<u32, Error>;

    /// Embed content
    async fn embed_content(
        &self,
        req: EmbedContentRequest,
    ) -> Result<ContentEmbeddingResponse, Error>;

    /// Batch embed content
    async fn batch_embed_content(
        &self,
        req: BatchEmbedContentsRequest,
    ) -> Result<BatchContentEmbeddingResponse, Error>;

    /// Create cached content
    async fn create_cached_content(
        &self,
        req: CreateCachedContentRequest,
    ) -> Result<CachedContent, Error>;

    /// Get cached content
    async fn get_cached_content(&self, name: &str) -> Result<CachedContent, Error>;

    /// List cached contents
    async fn list_cached_contents(
        &self,
        page_size: Option<i32>,
        page_token: Option<String>,
    ) -> Result<ListCachedContentsResponse, Error>;

    /// Update cached content
    async fn update_cached_content(
        &self,
        name: &str,
        expiration: CacheExpirationRequest,
    ) -> Result<CachedContent, Error>;

    /// Delete cached content
    async fn delete_cached_content(&self, name: &str) -> Result<(), Error>;

    /// Create batch operation (Generation)
    async fn create_batch(
        &self,
        req: BatchGenerateContentRequest,
    ) -> Result<BatchGenerateContentResponse, Error>;

    /// Get batch operation
    async fn get_batch(&self, name: &str) -> Result<BatchOperation, Error>;

    /// List batch operations
    async fn list_batches(
        &self,
        page_size: Option<u32>,
        page_token: Option<String>,
    ) -> Result<ListBatchesResponse, Error>;

    /// Cancel batch operation
    async fn cancel_batch(&self, name: &str) -> Result<(), Error>;

    /// Delete batch operation
    async fn delete_batch(&self, name: &str) -> Result<(), Error>;

    /// Upload file
    async fn upload_file(
        &self,
        display_name: Option<String>,
        file_bytes: Vec<u8>,
        mime_type: Mime,
    ) -> Result<File, Error>;

    /// Get file
    async fn get_file(&self, name: &str) -> Result<File, Error>;

    /// List files
    async fn list_files(
        &self,
        page_size: Option<u32>,
        page_token: Option<String>,
    ) -> Result<ListFilesResponse, Error>;

    /// Delete file
    async fn delete_file(&self, name: &str) -> Result<(), Error>;

    /// Download file (returns raw bytes)
    async fn download_file(&self, name: &str) -> Result<Vec<u8>, Error>;
}
