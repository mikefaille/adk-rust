use crate::generation::model::ValidationError;
use crate::{
    batch::{BatchBuilder, BatchHandle},
    cache::{CacheBuilder, CachedContentHandle},
    embedding::{
        EmbedBuilder,
    },
    files::{
        handle::FileHandle,
        model::File,
    },
    generation::{ContentBuilder, GenerateContentRequest, GenerationResponse},
    backend::GeminiBackend,
    batch::model::{BatchOperation, BatchGenerateContentRequest, ListBatchesResponse},
    cache::model::{CachedContent, CachedContentSummary, CreateCachedContentRequest, ListCachedContentsResponse, CacheExpirationRequest},
    embedding::model::{BatchContentEmbeddingResponse, BatchEmbedContentsRequest, ContentEmbeddingResponse, EmbedContentRequest},
    files::model::{ListFilesResponse},
};
#[cfg(feature = "vertex")]
use crate::backend::vertex::{VertexBackend, GoogleCloudAuth};
use crate::backend::studio::{StudioBackend, AuthConfig};

use eventsource_stream::EventStreamError;
use futures::{Stream, StreamExt, TryStreamExt};
use reqwest::{
    ClientBuilder,
    header::InvalidHeaderValue,
};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, Snafu};
use std::{
    fmt::{self, Formatter},
    sync::{Arc, LazyLock},
};
use url::Url;

static DEFAULT_BASE_URL: LazyLock<Url> = LazyLock::new(|| {
    Url::parse("https://generativelanguage.googleapis.com/v1beta/")
        .expect("unreachable error: failed to parse default base URL")
});
static V1_BASE_URL: LazyLock<Url> = LazyLock::new(|| {
    Url::parse("https://generativelanguage.googleapis.com/v1/")
        .expect("unreachable error: failed to parse v1 base URL")
});

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
            Model::Gemini25Flash => "models/gemini-2.5-flash",
            Model::Gemini25FlashLite => "models/gemini-2.5-flash-lite",
            Model::Gemini25Pro => "models/gemini-2.5-pro",
            Model::TextEmbedding004 => "models/text-embedding-004",
            Model::Custom(model) => model,
        }
    }

    pub fn vertex_model_path(&self, project_id: &str, location: &str) -> String {
        let model_id = match self {
            Model::Gemini25Flash => "gemini-2.5-flash",
            Model::Gemini25FlashLite => "gemini-2.5-flash-lite",
            Model::Gemini25Pro => "gemini-2.5-pro",
            Model::TextEmbedding004 => "text-embedding-004",
            Model::Custom(model) => {
                if model.starts_with("projects/") {
                    return model.clone();
                }
                if model.starts_with("publishers/") {
                    return format!("projects/{project_id}/locations/{location}/{model}");
                }
                model.strip_prefix("models/").unwrap_or(model)
            }
        };

        format!("projects/{project_id}/locations/{location}/publishers/google/models/{model_id}")
    }
}

impl From<String> for Model {
    fn from(model: String) -> Self {
        Self::Custom(model)
    }
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Model::Gemini25Flash => write!(f, "models/gemini-2.5-flash"),
            Model::Gemini25FlashLite => write!(f, "models/gemini-2.5-flash-lite"),
            Model::Gemini25Pro => write!(f, "models/gemini-2.5-pro"),
            Model::TextEmbedding004 => write!(f, "models/text-embedding-004"),
            Model::Custom(model) => write!(f, "{}", model),
        }
    }
}

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("failed to parse API key"))]
    InvalidApiKey {
        source: InvalidHeaderValue,
    },

    #[snafu(display("failed to construct URL (probably incorrect model name): {suffix}"))]
    ConstructUrl {
        source: url::ParseError,
        suffix: String,
    },

    PerformRequestNew {
        source: reqwest::Error,
    },

    #[snafu(display("failed to perform request to '{url}'"))]
    PerformRequest {
        source: reqwest::Error,
        url: Url,
    },

    #[snafu(display(
        "bad response from server; code {code}; description: {}",
        description.as_deref().unwrap_or("none")
    ))]
    BadResponse {
        /// HTTP status code
        code: u16,
        /// HTTP error description
        description: Option<String>,
    },

    #[snafu(display("validation error"))]
    Validation {
        source: ValidationError,
    },

    MissingResponseHeader {
        header: String,
    },

    #[snafu(display("failed to obtain stream SSE part"))]
    BadPart {
        source: EventStreamError<reqwest::Error>,
    },

    #[snafu(display("failed to deserialize JSON response"))]
    Deserialize {
        source: serde_json::Error,
    },

    #[snafu(display("failed to generate content"))]
    DecodeResponse {
        source: reqwest::Error,
    },

    #[snafu(display("failed to parse URL"))]
    UrlParse {
        source: url::ParseError,
    },

    #[cfg(feature = "vertex")]
    #[snafu(display("failed to build google cloud credentials"))]
    GoogleCloudAuth {
        source: google_cloud_auth::build_errors::Error,
    },

    #[cfg(feature = "vertex")]
    #[snafu(display("failed to obtain google cloud auth headers"))]
    GoogleCloudCredentialHeaders {
        source: google_cloud_auth::errors::CredentialsError,
    },

    #[cfg(feature = "vertex")]
    #[snafu(display("google cloud credentials returned NotModified without cached headers"))]
    GoogleCloudCredentialHeadersUnavailable,

    #[cfg(feature = "vertex")]
    #[snafu(display("failed to parse google cloud credentials JSON"))]
    GoogleCloudCredentialParse {
        source: serde_json::Error,
    },

    #[cfg(feature = "vertex")]
    #[snafu(display("failed to build google cloud vertex client"))]
    GoogleCloudClientBuild {
        source: google_cloud_gax::client_builder::Error,
    },

    #[cfg(feature = "vertex")]
    #[snafu(display("failed to send google cloud vertex request"))]
    GoogleCloudRequest {
        source: google_cloud_aiplatform_v1::Error,
    },

    #[snafu(display("failed to serialize google cloud request"))]
    GoogleCloudRequestSerialize {
        source: serde_json::Error,
    },

    #[snafu(display("failed to deserialize google cloud request"))]
    GoogleCloudRequestDeserialize {
        source: serde_json::Error,
    },

    #[snafu(display("failed to serialize google cloud response"))]
    GoogleCloudResponseSerialize {
        source: serde_json::Error,
    },

    #[snafu(display("failed to deserialize google cloud response"))]
    GoogleCloudResponseDeserialize {
        source: serde_json::Error,
    },

    #[snafu(display("google cloud request payload is not an object"))]
    GoogleCloudRequestNotObject,

    #[snafu(display("google cloud configuration is required for this authentication mode"))]
    MissingGoogleCloudConfig,

    #[snafu(display("google cloud authentication is required for this configuration"))]
    MissingGoogleCloudAuth,

    #[snafu(display("service account JSON is missing required field 'project_id'"))]
    MissingGoogleCloudProjectId,

    #[snafu(display("api key is required for this configuration"))]
    MissingApiKey,

    #[snafu(display(
        "operation '{operation}' is not supported with the google cloud sdk backend (PredictionService currently exposes generateContent/embedContent only)"
    ))]
    GoogleCloudUnsupported {
        operation: &'static str,
    },

    #[snafu(display("failed to create tokio runtime for google cloud client"))]
    TokioRuntime {
        source: std::io::Error,
    },

    #[snafu(display("google cloud client initialization thread panicked"))]
    GoogleCloudInitThreadPanicked,

    #[snafu(display("failed to parse service account JSON"))]
    ServiceAccountKeyParse {
        source: serde_json::Error,
    },

    #[snafu(display("failed to sign service account JWT"))]
    ServiceAccountJwt {
        source: jsonwebtoken::errors::Error,
    },

    #[snafu(display("failed to request service account token from '{url}'"))]
    ServiceAccountToken {
        source: reqwest::Error,
        url: String,
    },

    #[snafu(display("failed to deserialize service account token response"))]
    ServiceAccountTokenDeserialize {
        source: serde_json::Error,
    },
    #[snafu(display("I/O error during file operations"))]
    Io {
        source: std::io::Error,
    },
}

pub struct GeminiClient {
    pub model: Model,
    backend: Box<dyn GeminiBackend>,
}

impl GeminiClient {
    pub(crate) fn new(model: Model, backend: Box<dyn GeminiBackend>) -> Self {
        Self { model, backend }
    }

    pub(crate) async fn generate_content_raw(
        &self,
        request: GenerateContentRequest,
    ) -> Result<GenerationResponse, Error> {
        self.backend.generate_content(request).await
    }

    pub(crate) async fn generate_content_stream(
        &self,
        request: GenerateContentRequest,
    ) -> Result<impl TryStreamExt<Ok = GenerationResponse, Error = Error> + Send + use<>, Error>
    {
        self.backend.generate_content_stream(request).await
    }

    pub(crate) async fn embed_content(
        &self,
        request: EmbedContentRequest,
    ) -> Result<ContentEmbeddingResponse, Error> {
        self.backend.embed_content(request).await
    }

    pub(crate) async fn embed_content_batch(
        &self,
        request: BatchEmbedContentsRequest,
    ) -> Result<BatchContentEmbeddingResponse, Error> {
        self.backend.batch_embed_contents(request).await
    }

    pub(crate) async fn batch_generate_content(
        &self,
        request: BatchGenerateContentRequest,
    ) -> Result<BatchOperation, Error> {
        self.backend.create_batch(request).await
    }

    pub(crate) async fn get_batch_operation(
        &self,
        name: &str,
    ) -> Result<BatchOperation, Error>
    {
        self.backend.get_batch(name).await
    }

    pub(crate) async fn list_batch_operations(
        &self,
        page_size: Option<u32>,
        page_token: Option<String>,
    ) -> Result<ListBatchesResponse, Error> {
        self.backend.list_batches(page_size, page_token).await
    }

    pub(crate) async fn cancel_batch_operation(&self, name: &str) -> Result<(), Error> {
        self.backend.cancel_batch(name).await
    }

    pub(crate) async fn delete_batch_operation(&self, name: &str) -> Result<(), Error> {
        self.backend.delete_batch(name).await
    }

    pub(crate) async fn get_file(&self, name: &str) -> Result<File, Error> {
        self.backend.get_file(name).await
    }

    pub(crate) async fn list_files(&self, page_size: Option<u32>, page_token: Option<String>) -> Result<ListFilesResponse, Error> {
        self.backend.list_files(page_size, page_token).await
    }

    pub(crate) async fn upload_file(&self, display_name: Option<String>, bytes: Vec<u8>, mime_type: mime::Mime) -> Result<File, Error> {
        self.backend.upload_file(display_name, bytes, mime_type.to_string()).await
    }

    pub(crate) async fn download_file(&self, name: &str) -> Result<Vec<u8>, Error> {
        self.backend.download_file(name).await
    }

    pub(crate) async fn delete_file(&self, name: &str) -> Result<(), Error> {
        self.backend.delete_file(name).await
    }

    pub(crate) async fn create_cached_content(&self, req: CreateCachedContentRequest) -> Result<CachedContent, Error> {
        self.backend.create_cached_content(req).await
    }

    pub(crate) async fn get_cached_content(&self, name: &str) -> Result<CachedContent, Error> {
        self.backend.get_cached_content(name).await
    }

    pub(crate) async fn list_cached_contents(&self, page_size: Option<i32>, page_token: Option<String>) -> Result<ListCachedContentsResponse, Error> {
        self.backend.list_cached_contents(page_size, page_token).await
    }

    pub(crate) async fn update_cached_content(&self, name: &str, req: CacheExpirationRequest) -> Result<CachedContent, Error> {
        self.backend.update_cached_content(name, req).await
    }

    pub(crate) async fn delete_cached_content(&self, name: &str) -> Result<(), Error> {
        self.backend.delete_cached_content(name).await
    }
}

struct GoogleCloudConfig {
    project_id: String,
    location: String,
}

#[cfg(feature = "vertex")]
impl GoogleCloudConfig {
    fn endpoint(&self) -> String {
        format!("https://{}-aiplatform.googleapis.com", self.location)
    }
}

#[cfg(feature = "vertex")]
fn extract_service_account_project_id(service_account_json: &str) -> Result<String, Error> {
    let value: serde_json::Value =
        serde_json::from_str(service_account_json).context(GoogleCloudCredentialParseSnafu)?;

    let project_id = value
        .get("project_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(Error::MissingGoogleCloudProjectId)?;

    Ok(project_id.to_string())
}

pub struct GeminiBuilder {
    model: Model,
    client_builder: ClientBuilder,
    base_url: Url,
    #[cfg(feature = "vertex")]
    google_cloud: Option<GoogleCloudConfig>,
    api_key: Option<String>,
    #[cfg(feature = "vertex")]
    google_cloud_auth: Option<GoogleCloudAuth>,
}

impl GeminiBuilder {
    pub fn new<K: Into<String>>(key: K) -> Self {
        Self {
            model: Model::default(),
            client_builder: ClientBuilder::default(),
            base_url: DEFAULT_BASE_URL.clone(),
            #[cfg(feature = "vertex")]
            google_cloud: None,
            api_key: Some(key.into()),
            #[cfg(feature = "vertex")]
            google_cloud_auth: None,
        }
    }

    pub fn with_model<M: Into<Model>>(mut self, model: M) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_http_client(mut self, client_builder: ClientBuilder) -> Self {
        self.client_builder = client_builder;
        self
    }

    pub fn with_base_url(mut self, base_url: Url) -> Self {
        self.base_url = base_url;
        #[cfg(feature = "vertex")]
        {
            self.google_cloud = None;
            self.google_cloud_auth = None;
        }
        self
    }

    #[cfg(feature = "vertex")]
    pub fn with_service_account_json(mut self, service_account_json: &str) -> Result<Self, Error> {
        let value =
            serde_json::from_str(service_account_json).context(GoogleCloudCredentialParseSnafu)?;
        let credentials = google_cloud_auth::credentials::service_account::Builder::new(value)
            .build()
            .context(GoogleCloudAuthSnafu)?;
        self.google_cloud_auth = Some(GoogleCloudAuth::Credentials(credentials));
        Ok(self)
    }

    #[cfg(feature = "vertex")]
    pub fn with_google_cloud<P: Into<String>, L: Into<String>>(
        mut self,
        project_id: P,
        location: L,
    ) -> Self {
        self.google_cloud =
            Some(GoogleCloudConfig { project_id: project_id.into(), location: location.into() });
        self
    }

    #[cfg(feature = "vertex")]
    pub fn with_google_cloud_adc(mut self) -> Result<Self, Error> {
        let credentials = google_cloud_auth::credentials::Builder::default()
            .build()
            .context(GoogleCloudAuthSnafu)?;
        self.google_cloud_auth = Some(GoogleCloudAuth::Credentials(credentials));
        Ok(self)
    }

    #[cfg(feature = "vertex")]
    pub fn with_google_cloud_wif_json(mut self, wif_json: &str) -> Result<Self, Error> {
        let value = serde_json::from_str(wif_json).context(GoogleCloudCredentialParseSnafu)?;
        let credentials = google_cloud_auth::credentials::external_account::Builder::new(value)
            .build()
            .context(GoogleCloudAuthSnafu)?;
        self.google_cloud_auth = Some(GoogleCloudAuth::Credentials(credentials));
        Ok(self)
    }

    pub fn build(self) -> Result<Gemini, Error> {
        #[cfg(feature = "vertex")]
        if self.google_cloud.is_none() && self.google_cloud_auth.is_some() {
             return MissingGoogleCloudConfigSnafu.fail();
        }

        #[cfg(feature = "vertex")]
        if let Some(config) = &self.google_cloud {
              let model =
                   Model::Custom(self.model.vertex_model_path(&config.project_id, &config.location));

              let google_cloud_auth = match self.google_cloud_auth {
                  Some(auth) => auth,
                  None => match self.api_key {
                      Some(api_key) if !api_key.is_empty() => GoogleCloudAuth::ApiKey(api_key),
                      _ => return MissingGoogleCloudAuthSnafu.fail(),
                  },
              };

              let credentials = google_cloud_auth.credentials()?;
              let endpoint = config.endpoint();

              let backend = Box::new(VertexBackend::new(endpoint, credentials, model.clone())?);
              return Ok(Gemini {
                  client: Arc::new(GeminiClient::new(model, backend)),
              });
        }

        let api_key = self.api_key.ok_or(Error::MissingApiKey)?;
        if api_key.is_empty() {
             return MissingApiKeySnafu.fail();
        }

        // StudioBackend needs api_key, base_url, model.
        let backend = Box::new(StudioBackend::new(api_key, Some(self.base_url), self.model.clone())?);

        Ok(Gemini {
            client: Arc::new(GeminiClient::new(self.model, backend)),
        })
    }
}

#[derive(Clone)]
pub struct Gemini {
    client: Arc<GeminiClient>,
}

impl Gemini {
    pub fn new<K: AsRef<str>>(api_key: K) -> Result<Self, Error> {
        Self::with_model(api_key, Model::default())
    }

    pub fn pro<K: AsRef<str>>(api_key: K) -> Result<Self, Error> {
        Self::with_model(api_key, Model::Gemini25Pro)
    }

    pub fn with_model<K: AsRef<str>, M: Into<Model>>(api_key: K, model: M) -> Result<Self, Error> {
        GeminiBuilder::new(api_key.as_ref())
            .with_model(model)
            .build()
    }

    pub fn generate_content(&self) -> ContentBuilder {
        ContentBuilder::new(self.client.clone())
    }

    pub fn embed_content(&self) -> EmbedBuilder {
        EmbedBuilder::new(self.client.clone())
    }

    pub fn batch_generate_content(&self) -> BatchBuilder {
        BatchBuilder::new(self.client.clone())
    }

    pub fn get_batch(&self, name: &str) -> BatchHandle {
        BatchHandle::new(name.to_string(), self.client.clone())
    }

    pub fn list_batches(
        &self,
        page_size: impl Into<Option<u32>>,
    ) -> impl Stream<Item = Result<BatchOperation, Error>> + Send {
        let client = self.client.clone();
        let page_size = page_size.into();
        async_stream::try_stream! {
            let mut page_token: Option<String> = None;
            loop {
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

    pub fn create_cache(&self) -> CacheBuilder {
        CacheBuilder::new(self.client.clone())
    }

    pub fn get_cached_content(&self, name: &str) -> CachedContentHandle {
        CachedContentHandle::new(name.to_string(), self.client.clone())
    }

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

    pub fn create_file<B: Into<Vec<u8>>>(&self, bytes: B) -> crate::files::builder::FileBuilder {
        crate::files::builder::FileBuilder::new(self.client.clone(), bytes)
    }

    pub async fn get_file(&self, name: &str) -> Result<FileHandle, Error> {
        let file = self.client.get_file(name).await?;
        Ok(FileHandle::new(self.client.clone(), file))
    }

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

#[cfg(test)]
#[cfg(feature = "vertex")]
mod client_tests {
    use super::{Error, extract_service_account_project_id};

    #[test]
    fn extract_service_account_project_id_reads_project_id() {
        let json = r#"{
            "type": "service_account",
            "project_id": "test-project-123",
            "private_key_id": "key-id"
        }"#;

        let project_id = extract_service_account_project_id(json).expect("project id should parse");
        assert_eq!(project_id, "test-project-123");
    }

    #[test]
    fn extract_service_account_project_id_missing_field_errors() {
        let json = r#"{
            "type": "service_account",
            "private_key_id": "key-id"
        }"#;

        let err =
            extract_service_account_project_id(json).expect_err("missing project_id should fail");
        assert!(matches!(err, Error::MissingGoogleCloudProjectId));
    }

    #[test]
    fn extract_service_account_project_id_invalid_json_errors() {
        let err =
            extract_service_account_project_id("not-json").expect_err("invalid json should fail");
        assert!(matches!(err, Error::GoogleCloudCredentialParse { .. }));
    }
}
