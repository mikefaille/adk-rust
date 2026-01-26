use crate::{
    batch::{BatchBuilder, BatchHandle},
    cache::{CacheBuilder, CachedContentHandle},
    embedding::{
        BatchContentEmbeddingResponse, BatchEmbedContentsRequest, ContentEmbeddingResponse,
        EmbedBuilder, EmbedContentRequest,
    },
    files::{
        handle::FileHandle,
        model::{File, ListFilesResponse},
    },
    generation::{ContentBuilder, GenerateContentRequest, GenerationResponse},
};
use futures::{Stream, TryStreamExt};
use mime::Mime;
use serde::{Deserialize, Serialize};
use snafu::Snafu;
use std::{
    fmt::{self, Formatter},
    sync::Arc,
};
use tracing::{Level, instrument};

use google_cloud_aiplatform_v1::client::LlmUtilityService;
use google_cloud_aiplatform_v1::client::PredictionService;
use google_cloud_aiplatform_v1::model as vertex;

use crate::batch::model::*;
use crate::cache::model::*;

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum Model {
    #[default]
    #[serde(rename = "models/gemini-2.5-flash")]
    Gemini25Flash,
    #[serde(rename = "models/gemini-2.5-flash-lite")]
    Gemini25FlashLite,
    #[serde(rename = "models/gemini-2.5-pro")]
    Gemini25Pro,
    #[serde(rename = "models/text-embedding-004")]
    TextEmbedding004,
    #[serde(untagged)]
    Custom(String),
}

impl Model {
    pub fn as_str(&self) -> &str {
        match self {
            Model::Gemini25Flash => "gemini-2.5-flash-001",
            Model::Gemini25FlashLite => "gemini-2.5-flash-lite-001",
            Model::Gemini25Pro => "gemini-2.5-pro-001",
            Model::TextEmbedding004 => "text-embedding-004",
            Model::Custom(model) => model,
        }
    }
}

impl From<String> for Model {
    fn from(model: String) -> Self {
        Self::Custom(model)
    }
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("Google Cloud GAX error: {}", source))]
    Gax {
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[snafu(display("Google Cloud Builder error: {}", source))]
    Builder {
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[snafu(display("feature not implemented in Vertex AI client yet"))]
    NotImplemented,

    #[snafu(display("failed to deserialize JSON response"))]
    Deserialize {
        source: serde_json::Error,
    },

    // Legacy errors kept to minimize breakage in other files temporarily
    #[snafu(display("I/O error during file operations"))]
    Io {
        source: std::io::Error,
    },
}

/// Internal client for making requests to the Gemini API via Vertex AI
pub struct GeminiClient {
    pub prediction_client: PredictionService,
    pub llm_utility_client: LlmUtilityService,
    pub project_id: String,
    pub location: String,
    pub model: Model,
}

impl GeminiClient {
    /// Create a new client
    pub async fn new(
        project_id: String,
        location: String,
        model: Model,
    ) -> Result<Self, Error> {
        let endpoint = format!("https://{}-aiplatform.googleapis.com", location);

        let prediction_client = PredictionService::builder()
            .with_endpoint(endpoint.clone())
            .build()
            .await
            .map_err(|e| Error::Builder { source: Box::new(e) })?;

        let llm_utility_client = LlmUtilityService::builder()
            .with_endpoint(endpoint)
            .build()
            .await
            .map_err(|e| Error::Builder { source: Box::new(e) })?;

        Ok(Self {
            prediction_client,
            llm_utility_client,
            project_id,
            location,
            model,
        })
    }

    // Helper to get the resource path for a model
    pub fn model_path(&self) -> String {
        format!("projects/{}/locations/{}/publishers/google/models/{}", self.project_id, self.location, self.model.as_str())
    }

    pub fn endpoint_path(&self) -> String {
        format!("projects/{}/locations/{}/publishers/google", self.project_id, self.location)
    }

    /// Generate content
    #[instrument(skip_all, ret(level = Level::TRACE), err)]
    pub(crate) async fn generate_content_raw(
        &self,
        request: GenerateContentRequest,
    ) -> Result<GenerationResponse, Error> {
        let vertex_req: vertex::GenerateContentRequest = request.into();

        let response = self.prediction_client.generate_content()
            .set_model(self.model_path())
            .with_request(vertex_req)
            .send()
            .await
            .map_err(|e| Error::Gax { source: Box::new(e) })?;

        Ok(response.into())
    }

    /// Generate content with streaming
    #[instrument(skip_all, err)]
    pub(crate) async fn generate_content_stream(
        &self,
        _request: GenerateContentRequest,
    ) -> Result<impl TryStreamExt<Ok = GenerationResponse, Error = Error> + Send + use<>, Error>
    {
        // Streaming is not yet supported in the generated client for GenerateContent
        let stream = futures::stream::iter(vec![Err(Error::NotImplemented)]);
        Ok(stream)
    }

    /// Embed content
    #[instrument(skip_all)]
    pub(crate) async fn embed_content(
        &self,
        request: EmbedContentRequest,
    ) -> Result<ContentEmbeddingResponse, Error> {
        let vertex_req: vertex::EmbedContentRequest = request.into();

        let response = self.prediction_client.embed_content()
            .set_model(self.model_path())
            .with_request(vertex_req)
            .send()
            .await
            .map_err(|e| Error::Gax { source: Box::new(e) })?;

        Ok(response.into())
    }

    /// Count tokens
    #[instrument(skip_all)]
    pub(crate) async fn count_tokens(
        &self,
        request: GenerateContentRequest,
    ) -> Result<i32, Error> {
        let vertex_req: vertex::GenerateContentRequest = request.into();

        let response = self.llm_utility_client.count_tokens()
            .set_endpoint(self.endpoint_path())
            .set_model(self.model_path())
            .set_contents(vertex_req.contents)
            .send()
            .await
            .map_err(|e| Error::Gax { source: Box::new(e) })?;

        Ok(response.total_tokens)
    }

    /// Batch Embed content
    #[instrument(skip_all)]
    pub(crate) async fn embed_content_batch(
        &self,
        _request: BatchEmbedContentsRequest,
    ) -> Result<BatchContentEmbeddingResponse, Error> {
        Err(Error::NotImplemented)
    }

    /// Batch generate content
    #[instrument(skip_all)]
    pub(crate) async fn batch_generate_content(
        &self,
        _request: BatchGenerateContentRequest,
    ) -> Result<BatchGenerateContentResponse, Error> {
        Err(Error::NotImplemented)
    }

    /// Get a batch operation
    #[instrument(skip_all)]
    pub(crate) async fn get_batch_operation<T: serde::de::DeserializeOwned>(
        &self,
        _name: &str,
    ) -> Result<T, Error> {
        Err(Error::NotImplemented)
    }

    /// List batch operations
    #[instrument(skip_all)]
    pub(crate) async fn list_batch_operations(
        &self,
        _page_size: Option<u32>,
        _page_token: Option<String>,
    ) -> Result<ListBatchesResponse, Error> {
        Err(Error::NotImplemented)
    }

    /// List files
    #[instrument(skip_all)]
    pub(crate) async fn list_files(
        &self,
        _page_size: Option<u32>,
        _page_token: Option<String>,
    ) -> Result<ListFilesResponse, Error> {
        Err(Error::NotImplemented)
    }

    /// Cancel a batch operation
    #[instrument(skip_all)]
    pub(crate) async fn cancel_batch_operation(&self, _name: &str) -> Result<(), Error> {
        Err(Error::NotImplemented)
    }

    /// Delete a batch operation
    #[instrument(skip_all)]
    pub(crate) async fn delete_batch_operation(&self, _name: &str) -> Result<(), Error> {
        Err(Error::NotImplemented)
    }

    /// Upload a file
    #[instrument(skip_all)]
    pub(crate) async fn upload_file(
        &self,
        _display_name: Option<String>,
        _file_bytes: Vec<u8>,
        _mime_type: Mime,
    ) -> Result<File, Error> {
        Err(Error::NotImplemented)
    }

    /// Get a file resource
    #[instrument(skip_all)]
    pub(crate) async fn get_file(&self, _name: &str) -> Result<File, Error> {
        Err(Error::NotImplemented)
    }

    /// Delete a file resource
    #[instrument(skip_all)]
    pub(crate) async fn delete_file(&self, _name: &str) -> Result<(), Error> {
        Err(Error::NotImplemented)
    }

    /// Download a file resource
    #[instrument(skip_all)]
    pub(crate) async fn download_file(&self, _name: &str) -> Result<Vec<u8>, Error> {
        Err(Error::NotImplemented)
    }

    /// Create cached content
    pub(crate) async fn create_cached_content(
        &self,
        _cached_content: CreateCachedContentRequest,
    ) -> Result<CachedContent, Error> {
        Err(Error::NotImplemented)
    }

    /// Get cached content
    pub(crate) async fn get_cached_content(&self, _name: &str) -> Result<CachedContent, Error> {
        Err(Error::NotImplemented)
    }

    /// Update cached content
    pub(crate) async fn update_cached_content(
        &self,
        _name: &str,
        _expiration: CacheExpirationRequest,
    ) -> Result<CachedContent, Error> {
        Err(Error::NotImplemented)
    }

    /// Delete cached content
    pub(crate) async fn delete_cached_content(&self, _name: &str) -> Result<(), Error> {
        Err(Error::NotImplemented)
    }

    /// List cached contents
    pub(crate) async fn list_cached_contents(
        &self,
        _page_size: Option<i32>,
        _page_token: Option<String>,
    ) -> Result<ListCachedContentsResponse, Error> {
        Err(Error::NotImplemented)
    }
}

/// A builder for the `Gemini` client.
pub struct GeminiBuilder {
    project_id: Option<String>,
    location: Option<String>,
    model: Model,
}

impl GeminiBuilder {
    /// Creates a new `GeminiBuilder`.
    pub fn new() -> Self {
        Self {
            project_id: None,
            location: None, // Default to us-central1 maybe?
            model: Model::default(),
        }
    }

    /// Sets the project ID.
    pub fn with_project_id<S: Into<String>>(mut self, project_id: S) -> Self {
        self.project_id = Some(project_id.into());
        self
    }

    /// Sets the location (region).
    pub fn with_location<S: Into<String>>(mut self, location: S) -> Self {
        self.location = Some(location.into());
        self
    }

    /// Sets the model for the client.
    pub fn with_model<M: Into<Model>>(mut self, model: M) -> Self {
        self.model = model.into();
        self
    }

    /// Builds the `Gemini` client.
    pub async fn build(self) -> Result<Gemini, Error> {
        let project_id = self.project_id.ok_or_else(|| Error::NotImplemented /* Using NotImplemented as placeholder for MissingProjectID */)?;
        let location = self.location.unwrap_or_else(|| "us-central1".to_string());

        let client = GeminiClient::new(project_id, location, self.model).await?;
        Ok(Gemini {
            client: Arc::new(client),
        })
    }
}

/// Client for the Gemini API
#[derive(Clone)]
pub struct Gemini {
    client: Arc<GeminiClient>,
}

impl Gemini {
    /// Create a new client
    ///
    /// Note: This is a breaking change from previous versions.
    /// It now requires Project ID and Location, or assumes defaults/environment.
    /// For migration, we might need a way to support the old API or fail gracefully.
    pub async fn new(project_id: impl Into<String>, location: impl Into<String>) -> Result<Self, Error> {
        GeminiBuilder::new()
            .with_project_id(project_id)
            .with_location(location)
            .build()
            .await
    }

    /// Start building a content generation request
    pub fn generate_content(&self) -> ContentBuilder {
        ContentBuilder::new(self.client.clone())
    }

    /// Start building a content embedding request
    pub fn embed_content(&self) -> EmbedBuilder {
        EmbedBuilder::new(self.client.clone())
    }

    /// Count tokens for a request
    pub async fn count_tokens(&self, request: GenerateContentRequest) -> Result<i32, Error> {
        self.client.count_tokens(request).await
    }

    /// Start building a batch content generation request
    pub fn batch_generate_content(&self) -> BatchBuilder {
        BatchBuilder::new(self.client.clone())
    }

    /// Get a handle to a batch operation by its name.
    pub fn get_batch(&self, name: &str) -> BatchHandle {
        BatchHandle::new(name.to_string(), self.client.clone())
    }

    /// Lists batch operations.
    pub fn list_batches(
        &self,
        page_size: impl Into<Option<u32>>,
    ) -> impl Stream<Item = Result<BatchOperation, Error>> + Send {
        let client = self.client.clone();
        let page_size = page_size.into();
        async_stream::try_stream! {
            let mut page_token: Option<String> = None;
            loop {
                // TODO: Implement using client.list_batch_operations
                let response = client
                    .list_batch_operations(page_size, page_token.clone())
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

    /// Create cached content with a fluent API.
    pub fn create_cache(&self) -> CacheBuilder {
        CacheBuilder::new(self.client.clone())
    }

    /// Get a handle to cached content by its name.
    pub fn get_cached_content(&self, name: &str) -> CachedContentHandle {
        CachedContentHandle::new(name.to_string(), self.client.clone())
    }

    /// Lists cached contents.
    pub fn list_cached_contents(
        &self,
        page_size: impl Into<Option<i32>>,
    ) -> impl Stream<Item = Result<CachedContentSummary, Error>> + Send {
        let client = self.client.clone();
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
        crate::files::builder::FileBuilder::new(self.client.clone(), bytes)
    }

    /// Get a handle to a file by its name.
    pub async fn get_file(&self, name: &str) -> Result<FileHandle, Error> {
        let file = self.client.get_file(name).await?;
        Ok(FileHandle::new(self.client.clone(), file))
    }

    /// Lists files.
    pub fn list_files(
        &self,
        page_size: impl Into<Option<u32>>,
    ) -> impl Stream<Item = Result<FileHandle, Error>> + Send {
        let client = self.client.clone();
        let page_size = page_size.into();
        async_stream::try_stream! {
            let mut page_token: Option<String> = None;
            loop {
                let response = client
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
