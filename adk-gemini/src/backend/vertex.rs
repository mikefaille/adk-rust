use crate::{
    backend::{BackendStream, GeminiBackend},
    batch::model::{BatchGenerateContentRequest, BatchOperation, ListBatchesResponse},
    cache::model::{
        CacheExpirationRequest, CachedContent, CreateCachedContentRequest,
        ListCachedContentsResponse,
    },
    embedding::model::{
        BatchContentEmbeddingResponse, BatchEmbedContentsRequest, ContentEmbeddingResponse,
        EmbedContentRequest,
    },
    error::{
        BadResponseSnafu, DecodeResponseSnafu, Error,
        GoogleCloudCredentialHeadersSnafu,
        GoogleCloudCredentialHeadersUnavailableSnafu, GoogleCloudCredentialParseSnafu,
        GoogleCloudRequestDeserializeSnafu,
        GoogleCloudRequestSerializeSnafu,
        GoogleCloudResponseDeserializeSnafu, GoogleCloudResponseSerializeSnafu,
        GoogleCloudUnsupportedSnafu, PerformRequestSnafu, UrlParseSnafu,
    },
    files::model::{File, ListFilesResponse},
    generation::model::{GenerateContentRequest, GenerationResponse},
};
use async_trait::async_trait;
use google_cloud_auth::credentials::Credentials;
use reqwest::{Client as HttpClient, Url};
use serde_json::Value;
use snafu::ResultExt;
use std::sync::Arc;

/// Configuration for Google Cloud Vertex AI
#[derive(Debug, Clone)]
pub struct GoogleCloudConfig {
    pub project_id: String,
    pub location: String,
}

pub fn extract_service_account_project_id(service_account_json: &str) -> std::result::Result<String, Error> {
    let json: serde_json::Value =
        serde_json::from_str(service_account_json).context(GoogleCloudCredentialParseSnafu)?;
    json.get("project_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or(Error::MissingGoogleCloudProjectId)
}

#[derive(Clone, Debug)]
pub enum GoogleCloudAuth {
    Credentials(Credentials),
    // Token(String), // Future optimization
}

#[derive(Debug, Clone)]
pub struct VertexBackend {
    #[allow(dead_code)]
    project: String,
    #[allow(dead_code)]
    location: String,
    model: String,
    endpoint: String,
    credentials: Arc<Credentials>,
}

impl VertexBackend {
    pub fn new(
        endpoint: String,
        project: String,
        location: String,
        auth: GoogleCloudAuth,
        model: String,
    ) -> std::result::Result<Self, Error> {
        let GoogleCloudAuth::Credentials(creds) = auth;
        let credentials = Arc::new(creds);

        Ok(Self {
            project,
            location,
            model,
            endpoint,
            credentials,
        })
    }

    async fn generate_content_vertex_rest(
        &self,
        request: &Value,
    ) -> std::result::Result<GenerationResponse, Error> {
        // Fallback implementation using REST API for Vertex
        let url = Url::parse(&format!(
            "{}/v1/{}:generateContent",
            self.endpoint.trim_end_matches('/'),
            self.model
        ))
        .context(UrlParseSnafu)?;

        let auth_headers = match self
            .credentials
            .headers(Default::default())
            .await
            .context(GoogleCloudCredentialHeadersSnafu)?
        {
            google_cloud_auth::credentials::CacheableResource::New { data, .. } => data,
            google_cloud_auth::credentials::CacheableResource::NotModified => {
                return GoogleCloudCredentialHeadersUnavailableSnafu.fail();
            }
        };

        let response = HttpClient::new()
            .post(url.clone())
            .headers(auth_headers)
            .json(request)
            .send()
            .await
            .context(PerformRequestSnafu { url: url.clone() })?;

        let response: reqwest::Response = check_response(response).await?;
        let response: google_cloud_aiplatform_v1::model::GenerateContentResponse =
             response.json().await.context(DecodeResponseSnafu)?;

        let response_value =
            serde_json::to_value(&response).context(GoogleCloudResponseSerializeSnafu)?;
        serde_json::from_value(response_value).context(GoogleCloudResponseDeserializeSnafu)
    }
}

async fn check_response(response: reqwest::Response) -> std::result::Result<reqwest::Response, Error> {
    if response.status().is_success() {
        Ok(response)
    } else {
        let code = response.status().as_u16();
        let text = response.text().await.ok();
        BadResponseSnafu {
            code,
            description: text,
        }
        .fail()
    }
}

#[async_trait]
impl GeminiBackend for VertexBackend {
    fn model(&self) -> &str {
        &self.model
    }

    async fn generate_content(
        &self,
        req: GenerateContentRequest,
    ) -> std::result::Result<GenerationResponse, Error> {
        let request_value =
            serde_json::to_value(&req).context(GoogleCloudRequestSerializeSnafu)?;
        self.generate_content_vertex_rest(&request_value).await
    }

    async fn stream_generate_content(&self, _req: GenerateContentRequest) -> std::result::Result<BackendStream<GenerationResponse>, Error> {
        GoogleCloudUnsupportedSnafu { operation: "streamGenerateContent" }.fail()
    }

    async fn count_tokens(&self, _req: GenerateContentRequest) -> std::result::Result<u32, Error> {
        GoogleCloudUnsupportedSnafu { operation: "countTokens" }.fail()
    }

    async fn embed_content(&self, request: EmbedContentRequest) -> std::result::Result<ContentEmbeddingResponse, Error> {
        let content_value =
            serde_json::to_value(&request.content).context(GoogleCloudRequestSerializeSnafu)?;
        let content: google_cloud_aiplatform_v1::model::Content =
            serde_json::from_value(content_value).context(GoogleCloudRequestDeserializeSnafu)?;

        let mut vertex_request =
            google_cloud_aiplatform_v1::model::EmbedContentRequest::new().set_content(content);

        if let Some(title) = request.title.clone() {
            vertex_request = vertex_request.set_title(title);
        }
        if let Some(task_type) = request.task_type.clone() {
            let task_type =
                google_cloud_aiplatform_v1::model::embed_content_request::EmbeddingTaskType::from(
                    task_type.as_ref(),
                );
            vertex_request = vertex_request.set_task_type(task_type);
        }
        if let Some(output_dimensionality) = request.output_dimensionality {
            vertex_request = vertex_request.set_output_dimensionality(output_dimensionality as i32);
        }

        let url = Url::parse(&format!(
            "{}/v1/{}:embedContent",
            self.endpoint.trim_end_matches('/'),
            self.model
        ))
        .context(UrlParseSnafu)?;

        let auth_headers = match self
            .credentials
            .headers(Default::default())
            .await
            .context(GoogleCloudCredentialHeadersSnafu)?
        {
            google_cloud_auth::credentials::CacheableResource::New { data, .. } => data,
            google_cloud_auth::credentials::CacheableResource::NotModified => {
                return GoogleCloudCredentialHeadersUnavailableSnafu.fail();
            }
        };

        let response = HttpClient::new()
            .post(url.clone())
            .headers(auth_headers)
            .json(&vertex_request)
            .send()
            .await
            .context(PerformRequestSnafu { url: url.clone() })?;

        let response: reqwest::Response = check_response(response).await?;
        let response: google_cloud_aiplatform_v1::model::EmbedContentResponse =
            response.json().await.context(DecodeResponseSnafu)?;

        let response_value =
            serde_json::to_value(&response).context(GoogleCloudResponseSerializeSnafu)?;
        serde_json::from_value(response_value).context(GoogleCloudResponseDeserializeSnafu)
    }

    async fn batch_embed_contents(&self, _req: BatchEmbedContentsRequest) -> std::result::Result<BatchContentEmbeddingResponse, Error> {
         GoogleCloudUnsupportedSnafu { operation: "batchEmbedContents" }.fail()
    }

    async fn create_batch(&self, _req: BatchGenerateContentRequest) -> std::result::Result<BatchOperation, Error> {
         GoogleCloudUnsupportedSnafu { operation: "createBatch" }.fail()
    }

    async fn get_batch(&self, _name: &str) -> std::result::Result<BatchOperation, Error> {
         GoogleCloudUnsupportedSnafu { operation: "getBatch" }.fail()
    }

    async fn list_batches(&self, _page_size: Option<u32>, _page_token: Option<String>) -> std::result::Result<ListBatchesResponse, Error> {
         GoogleCloudUnsupportedSnafu { operation: "listBatches" }.fail()
    }

    async fn cancel_batch(&self, _name: &str) -> std::result::Result<(), Error> {
        GoogleCloudUnsupportedSnafu { operation: "cancelBatch" }.fail()
    }

    async fn delete_batch(&self, _name: &str) -> std::result::Result<(), Error> {
        GoogleCloudUnsupportedSnafu { operation: "deleteBatch" }.fail()
    }

    async fn upload_file(
        &self,
        _display_name: Option<String>,
        _file_bytes: Vec<u8>,
        _mime_type: mime::Mime,
    ) -> std::result::Result<File, Error> {
        GoogleCloudUnsupportedSnafu { operation: "uploadFile" }.fail()
    }

    async fn get_file(&self, _name: &str) -> std::result::Result<File, Error> {
        GoogleCloudUnsupportedSnafu { operation: "getFile" }.fail()
    }

    async fn list_files(
        &self,
        _page_size: Option<u32>,
        _page_token: Option<String>,
    ) -> std::result::Result<ListFilesResponse, Error> {
        GoogleCloudUnsupportedSnafu { operation: "listFiles" }.fail()
    }

    async fn delete_file(&self, _name: &str) -> std::result::Result<(), Error> {
        GoogleCloudUnsupportedSnafu { operation: "deleteFile" }.fail()
    }

    async fn download_file(&self, _name: &str) -> std::result::Result<Vec<u8>, Error> {
        GoogleCloudUnsupportedSnafu { operation: "downloadFile" }.fail()
    }

    async fn create_cached_content(&self, _req: CreateCachedContentRequest) -> std::result::Result<CachedContent, Error> {
        GoogleCloudUnsupportedSnafu { operation: "createCachedContent" }.fail()
    }

    async fn get_cached_content(&self, _name: &str) -> std::result::Result<CachedContent, Error> {
         GoogleCloudUnsupportedSnafu { operation: "getCachedContent" }.fail()
    }

    async fn list_cached_contents(&self, _page_size: Option<i32>, _page_token: Option<String>) -> std::result::Result<ListCachedContentsResponse, Error> {
         GoogleCloudUnsupportedSnafu { operation: "listCachedContents" }.fail()
    }

    async fn update_cached_content(&self, _name: &str, _req: CacheExpirationRequest) -> std::result::Result<CachedContent, Error> {
        Err(Error::GoogleCloudUnsupported { operation: "updateCachedContent" })
    }

    async fn delete_cached_content(&self, _name: &str) -> std::result::Result<(), Error> {
         GoogleCloudUnsupportedSnafu { operation: "deleteCachedContent" }.fail()
    }
}
