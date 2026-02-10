use futures::stream::BoxStream;
use futures::{Stream, StreamExt, TryStreamExt};
use mime::Mime;
use reqwest::Url;
use std::sync::Arc;

use crate::backend::GeminiBackend;
use crate::batch::{BatchBuilder, BatchHandle};
use crate::builder::GeminiBuilder;
use crate::cache::{CacheBuilder, CachedContentHandle};
use crate::common::Model;
use crate::embedding::{
    BatchContentEmbeddingResponse, BatchEmbedContentsRequest, ContentEmbeddingResponse,
    EmbedBuilder, EmbedContentRequest,
};
use crate::error::Error;
use crate::files::{
    handle::FileHandle,
    model::{File, ListFilesResponse},
};
use crate::generation::{ContentBuilder, GenerateContentRequest, GenerationResponse};

#[cfg(feature = "vertex")]
pub use crate::backend::vertex::extract_service_account_project_id;
#[cfg(feature = "vertex")]
use crate::types::VertexContext;

/// The main entry point for interacting with the Gemini API.
///
/// This client provides a high-level interface for generating content, managing files,
/// and working with other Gemini resources. It supports both the Google AI Studio API
/// and Vertex AI (Google Cloud).
#[derive(Clone, Debug)]
pub struct GeminiClient {
    backend: Arc<Box<dyn GeminiBackend>>,
}

impl GeminiClient {
    /// Internal constructor used by the Builder
    pub fn new(backend: Box<dyn GeminiBackend>) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }

    pub fn model(&self) -> Model {
        self.backend.model().to_string().into()
    }

    /// Create a new client with the given API key.
    pub fn new_with_api_key(key: impl Into<String>) -> Self {
        GeminiBuilder::new(key)
            .build()
            .expect("failed to build default client")
    }

    /// Create a new client with custom base URL
    pub fn with_model_and_base_url<K: Into<String>, M: Into<Model>>(
        api_key: K,
        model: M,
        base_url: Url,
    ) -> Result<Self, Error> {
        GeminiBuilder::new(api_key)
            .with_model(model)
            .with_base_url(base_url)
            .build()
    }

    /// Create a new client using Vertex AI (Google Cloud) endpoints.
    #[cfg(feature = "vertex")]
    pub fn with_google_cloud<K: Into<String>, P: Into<String>, L: Into<String>, M: Into<Model>>(
        api_key: K,
        project_id: P,
        location: L,
        model: M,
    ) -> Result<Self, Error> {
        GeminiBuilder::new(api_key)
            .with_model(model)
            .with_google_cloud(project_id, location)
            .build()
    }

    /// Create a new client using Vertex AI (Google Cloud) endpoints with Application Default Credentials (ADC).
    #[cfg(feature = "vertex")]
    pub fn with_google_cloud_adc<P: Into<String>, L: Into<String>>(
        project_id: P,
        location: L,
    ) -> Result<Self, Error> {
        Self::with_google_cloud_adc_model(project_id, location, Model::default())
    }

    /// Create a new client using Vertex AI (Google Cloud) endpoints and a specific model with ADC.
    #[cfg(feature = "vertex")]
    pub fn with_google_cloud_adc_model<P: Into<String>, L: Into<String>, M: Into<Model>>(
        project_id: P,
        location: L,
        model: M,
    ) -> Result<Self, Error> {
        GeminiBuilder::new_without_api_key()
            .with_model(model)
            .with_google_cloud(project_id, location)
            .with_google_cloud_adc()?
            .build()
    }

    /// Create a new client using Vertex AI (Google Cloud) endpoints and Workload Identity Federation JSON.
    #[cfg(feature = "vertex")]
    pub fn with_google_cloud_wif_json<P: Into<String>, L: Into<String>, M: Into<Model>>(
        wif_json: &str,
        project_id: P,
        location: L,
        model: M,
    ) -> Result<Self, Error> {
        GeminiBuilder::new_without_api_key()
            .with_model(model)
            .with_google_cloud(project_id, location)
            .with_google_cloud_wif_json(wif_json)?
            .build()
    }

    /// Create a new client using a service account JSON key.
    #[cfg(feature = "vertex")]
    pub fn with_service_account_json(service_account_json: &str) -> Result<Self, Error> {
        Self::with_service_account_json_model(service_account_json, Model::default())
    }

    /// Create a new client using a service account JSON key and a specific model.
    #[cfg(feature = "vertex")]
    pub fn with_service_account_json_model<M: Into<Model>>(
        service_account_json: &str,
        model: M,
    ) -> Result<Self, Error> {
        let project_id = extract_service_account_project_id(service_account_json)?;
        GeminiBuilder::new_without_api_key()
            .with_model(model)
            .with_service_account_json(service_account_json)?
            .with_google_cloud(project_id, "us-central1")
            .build()
    }

    /// Create a new client using Vertex AI (Google Cloud) endpoints and a service account JSON key.
    #[cfg(feature = "vertex")]
    pub fn with_google_cloud_service_account_json<M: Into<Model>>(
        service_account_json: &str,
        project_id: &str,
        location: &str,
        model: M,
    ) -> Result<Self, Error> {
        GeminiBuilder::new_without_api_key()
            .with_model(model)
            .with_service_account_json(service_account_json)?
            .with_google_cloud(project_id, location)
            .build()
    }

    // --- Public API ---

    /// Start building a content generation request
    pub fn generate_content(&self) -> ContentBuilder {
        ContentBuilder::new(Arc::new(self.clone()))
    }

    /// Generate content (raw request)
    pub(crate) async fn generate_content_raw(
        &self,
        req: GenerateContentRequest,
    ) -> Result<GenerationResponse, Error> {
        self.backend.generate_content(req).await
    }

    /// Generate content (streaming)
    pub(crate) async fn generate_content_stream(
        &self,
        req: GenerateContentRequest,
    ) -> Result<BoxStream<'static, Result<GenerationResponse, Error>>, Error> {
        self.backend.stream_generate_content(req).await
    }

    /// Count tokens
    pub async fn count_tokens(&self, req: GenerateContentRequest) -> Result<u32, Error> {
        self.backend.count_tokens(req).await
    }

    /// Start building a content embedding request
    pub fn embed_content(&self) -> EmbedBuilder {
        EmbedBuilder::new(Arc::new(self.clone()))
    }

    /// Embed content (raw)
    pub(crate) async fn embed_content_raw(
        &self,
        req: EmbedContentRequest,
    ) -> Result<ContentEmbeddingResponse, Error> {
        self.backend.embed_content(req).await
    }

    /// Batch Embed content
    pub(crate) async fn embed_content_batch(
        &self,
        req: BatchEmbedContentsRequest,
    ) -> Result<BatchContentEmbeddingResponse, Error> {
        self.backend.batch_embed_content(req).await
    }

    /// Start building a batch content generation request
    pub fn batch_generate_content(&self) -> BatchBuilder {
        BatchBuilder::new(Arc::new(self.clone()))
    }

    /// Batch generate content (raw)
    pub(crate) async fn batch_generate_content_raw(
        &self,
        req: crate::batch::model::BatchGenerateContentRequest,
    ) -> Result<crate::batch::model::BatchGenerateContentResponse, Error> {
         self.backend.create_batch(req).await
    }

    /// Get a handle to a batch operation by its name.
    pub fn get_batch(&self, name: &str) -> BatchHandle {
        BatchHandle::new(name.to_string(), Arc::new(self.clone()))
    }

    pub(crate) async fn get_batch_operation(&self, name: &str) -> Result<crate::batch::model::BatchOperation, Error> {
        self.backend.get_batch(name).await
    }

    /// Lists batch operations.
    pub fn list_batches(
        &self,
        page_size: impl Into<Option<u32>>,
    ) -> impl Stream<Item = Result<crate::batch::model::BatchOperation, Error>> + Send {
        let client = self.backend.clone();
        let page_size = page_size.into();
        async_stream::try_stream! {
            let mut page_token: Option<String> = None;
            loop {
                let response = client
                    .list_batches(page_size, page_token.clone())
                    .await?;

                for operation in response.operations {
                    yield operation;
                }

                if let Some(next_page_token) = response.next_page_token {
                    page_token = Some(next_page_token);
                } else {
                    break;
                }
            }
        }
    }

    pub(crate) async fn cancel_batch_operation(&self, name: &str) -> Result<(), Error> {
        self.backend.cancel_batch(name).await
    }

    pub(crate) async fn delete_batch_operation(&self, name: &str) -> Result<(), Error> {
        self.backend.delete_batch(name).await
    }

    /// Create cached content with a fluent API.
    pub fn create_cache(&self) -> CacheBuilder {
        CacheBuilder::new(Arc::new(self.clone()))
    }

    /// Get a handle to cached content by its name.
    pub fn get_cached_content(&self, name: &str) -> CachedContentHandle {
        CachedContentHandle::new(name.to_string(), Arc::new(self.clone()))
    }

    pub(crate) async fn create_cached_content_raw(&self, req: crate::cache::model::CreateCachedContentRequest) -> Result<crate::cache::model::CachedContent, Error> {
        self.backend.create_cached_content(req).await
    }

    pub(crate) async fn get_cached_content_raw(&self, name: &str) -> Result<crate::cache::model::CachedContent, Error> {
        self.backend.get_cached_content(name).await
    }

    pub(crate) async fn update_cached_content(&self, name: &str, expiration: crate::cache::model::CacheExpirationRequest) -> Result<crate::cache::model::CachedContent, Error> {
        self.backend.update_cached_content(name, expiration).await
    }

    pub(crate) async fn delete_cached_content(&self, name: &str) -> Result<(), Error> {
        self.backend.delete_cached_content(name).await
    }

    /// Lists cached contents.
    pub fn list_cached_contents(
        &self,
        page_size: impl Into<Option<i32>>,
    ) -> impl Stream<Item = Result<crate::cache::model::CachedContentSummary, Error>> + Send {
        let client = self.backend.clone();
        let page_size = page_size.into();
        async_stream::try_stream! {
            let mut page_token: Option<String> = None;
            loop {
                let response = client
                    .list_cached_contents(page_size, page_token.clone())
                    .await?;

                for cached_content in response.cached_contents {
                    yield cached_content;
                }

                if let Some(next_page_token) = response.next_page_token {
                    page_token = Some(next_page_token);
                } else {
                    break;
                }
            }
        }
    }

    /// Start building a file resource
    pub fn create_file<B: Into<Vec<u8>>>(&self, bytes: B) -> crate::files::builder::FileBuilder {
        crate::files::builder::FileBuilder::new(Arc::new(self.clone()), bytes)
    }

    /// Get a handle to a file by its name.
    pub async fn get_file(&self, name: &str) -> Result<FileHandle, Error> {
        let file = self.backend.get_file(name).await?;
        Ok(FileHandle::new(Arc::new(self.clone()), file))
    }

    pub(crate) async fn upload_file(&self, display_name: Option<String>, file_bytes: Vec<u8>, mime_type: Mime) -> Result<File, Error> {
        self.backend.upload_file(display_name, file_bytes, mime_type).await
    }

    pub(crate) async fn delete_file(&self, name: &str) -> Result<(), Error> {
        self.backend.delete_file(name).await
    }

    pub(crate) async fn download_file(&self, name: &str) -> Result<Vec<u8>, Error> {
        self.backend.download_file(name).await
    }

    /// Lists files.
    pub fn list_files(
        &self,
        page_size: impl Into<Option<u32>>,
    ) -> impl Stream<Item = Result<FileHandle, Error>> + Send {
        let client = Arc::new(self.clone());
        let backend = self.backend.clone();
        let page_size = page_size.into();
        async_stream::try_stream! {
            let mut page_token: Option<String> = None;
            loop {
                let response = backend
                    .list_files(page_size, page_token.clone())
                    .await?;

                for file in response.files {
                    yield FileHandle::new(client.clone(), file);
                }

                if let Some(next_page_token) = response.next_page_token {
                    page_token = Some(next_page_token);
                } else {
                    break;
                }
            }
        }
    }
}
