use async_trait::async_trait;
use futures::stream::BoxStream;
use google_cloud_aiplatform_v1::client::PredictionService;
use google_cloud_auth::credentials::{self, Credentials};
use mime::Mime;
use reqwest::Client;
use serde_json::json;
use snafu::{OptionExt, ResultExt};
use url::Url;

use crate::backend::GeminiBackend;
use crate::batch::model::{
    BatchGenerateContentRequest, BatchGenerateContentResponse, BatchOperation, ListBatchesResponse,
};
use crate::cache::model::{
    CacheExpirationRequest, CachedContent, CreateCachedContentRequest, ListCachedContentsResponse,
};
use crate::common::Model;
use crate::embedding::{
    BatchContentEmbeddingResponse, BatchEmbedContentsRequest, ContentEmbeddingResponse,
    EmbedContentRequest,
};
use crate::error::*;
use crate::files::model::{File, ListFilesResponse};
use crate::generation::{GenerateContentRequest, GenerationResponse};

#[derive(Debug, Clone)]
pub struct GoogleCloudConfig {
    pub project_id: String,
    pub location: String,
}

impl GoogleCloudConfig {
    pub fn endpoint(&self) -> String {
        format!("https://{}-aiplatform.googleapis.com", self.location)
    }
}

pub fn extract_service_account_project_id(service_account_json: &str) -> Result<String, Error> {
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

pub fn build_vertex_prediction_service(
    endpoint: String,
    credentials: Credentials,
) -> Result<PredictionService, Error> {
    let build_in_runtime =
        |endpoint: String, credentials: Credentials| -> Result<PredictionService, Error> {
            let runtime = tokio::runtime::Runtime::new().context(TokioRuntimeSnafu)?;
            runtime
                .block_on(
                    PredictionService::builder()
                        .with_endpoint(endpoint)
                        .with_credentials(credentials)
                        .build(),
                )
                .context(GoogleCloudClientBuildSnafu)
        };

    if tokio::runtime::Handle::try_current().is_ok() {
        let worker = std::thread::Builder::new()
            .name("adk-gemini-vertex-init".to_string())
            .spawn(move || build_in_runtime(endpoint, credentials))
            .map_err(|source| Error::TokioRuntime { source })?;

        return worker.join().map_err(|_| Error::GoogleCloudInitThreadPanicked)?;
    }

    build_in_runtime(endpoint, credentials)
}

#[derive(Debug)]
pub struct VertexBackend {
    pub prediction: PredictionService,
    pub credentials: Credentials,
    pub endpoint: String,
    pub project_id: String,
    pub location: String,
    pub model: String,
}

impl VertexBackend {
    pub fn new(
        project_id: String,
        location: String,
        model: impl Into<Model>,
        credentials: Credentials,
    ) -> Result<Self, Error> {
        let config = GoogleCloudConfig {
            project_id: project_id.clone(),
            location: location.clone(),
        };
        let endpoint = config.endpoint();
        let prediction = build_vertex_prediction_service(endpoint.clone(), credentials.clone())?;

        Ok(Self {
            prediction,
            credentials,
            endpoint,
            project_id,
            location,
            model: model.into().as_str().to_string(),
        })
    }

    fn is_vertex_transport_error_message(message: &str) -> bool {
        let normalized = message.to_ascii_lowercase();
        normalized.contains("transport reports an error")
            || normalized.contains("http2 error")
            || normalized.contains("client error (sendrequest)")
            || normalized.contains("stream error")
    }

    async fn generate_content_vertex_rest(
        &self,
        request: GenerateContentRequest,
    ) -> Result<GenerationResponse, Error> {
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

        let response = Client::new()
            .post(url.clone())
            .headers(auth_headers)
            .query(&[("$alt", "json;enum-encoding=int")])
            .json(&request)
            .send()
            .await
            .map_err(|source| Error::PerformRequest { source, url })?;

        let response = Self::check_response(response).await?;
        let response: google_cloud_aiplatform_v1::model::GenerateContentResponse =
            response.json().await.context(DecodeResponseSnafu)?;
        let response_value =
            serde_json::to_value(&response).context(GoogleCloudResponseSerializeSnafu)?;
        serde_json::from_value(response_value).context(GoogleCloudResponseDeserializeSnafu)
    }

    async fn check_response(response: reqwest::Response) -> Result<reqwest::Response, Error> {
        let status = response.status();
        if !status.is_success() {
            let description = response.text().await.ok();
            BadResponseSnafu { code: status.as_u16(), description }.fail()
        } else {
            Ok(response)
        }
    }
}

#[async_trait]
impl GeminiBackend for VertexBackend {
    fn model(&self) -> &str {
        &self.model
    }

    async fn generate_content(
        &self,
        request: GenerateContentRequest,
    ) -> Result<GenerationResponse, Error> {
        let rest_request = request.clone();
        let mut request_value =
            serde_json::to_value(&request).context(GoogleCloudRequestSerializeSnafu)?;

        let request_object =
            request_value.as_object_mut().context(GoogleCloudRequestNotObjectSnafu)?;
        request_object.insert("model".to_string(), serde_json::Value::String(self.model.clone()));

        let request: google_cloud_aiplatform_v1::model::GenerateContentRequest =
            serde_json::from_value(request_value).context(GoogleCloudRequestDeserializeSnafu)?;

        let response = match self.prediction.generate_content().with_request(request).send().await
        {
            Ok(response) => response,
            Err(source) => {
                if Self::is_vertex_transport_error_message(&source.to_string()) {
                    tracing::warn!(
                        error = %source,
                        "Vertex SDK transport error on generateContent; falling back to REST"
                    );
                    return self.generate_content_vertex_rest(rest_request).await;
                }
                return Err(Error::GoogleCloudRequest { source });
            }
        };
        let response_value =
            serde_json::to_value(&response).context(GoogleCloudResponseSerializeSnafu)?;
        serde_json::from_value(response_value).context(GoogleCloudResponseDeserializeSnafu)
    }

    async fn stream_generate_content(
        &self,
        _req: GenerateContentRequest,
    ) -> Result<BoxStream<'static, Result<GenerationResponse, Error>>, Error> {
        GoogleCloudUnsupportedSnafu { operation: "streamGenerateContent" }.fail()
    }

    async fn count_tokens(&self, _req: GenerateContentRequest) -> Result<u32, Error> {
         GoogleCloudUnsupportedSnafu { operation: "countTokens" }.fail()
    }

    async fn embed_content(
        &self,
        request: EmbedContentRequest,
    ) -> Result<ContentEmbeddingResponse, Error> {
        let content_value =
            serde_json::to_value(&request.content).context(GoogleCloudRequestSerializeSnafu)?;
        let content: google_cloud_aiplatform_v1::model::Content =
            serde_json::from_value(content_value).context(GoogleCloudRequestDeserializeSnafu)?;

        let mut vertex_request =
            google_cloud_aiplatform_v1::model::EmbedContentRequest::new().set_content(content);

        if let Some(title) = request.title {
            vertex_request = vertex_request.set_title(title);
        }
        if let Some(task_type) = request.task_type {
            let task_type =
                google_cloud_aiplatform_v1::model::embed_content_request::EmbeddingTaskType::from(
                    task_type.as_ref(),
                );
            vertex_request = vertex_request.set_task_type(task_type);
        }
        if let Some(output_dimensionality) = request.output_dimensionality {
            vertex_request = vertex_request.set_output_dimensionality(output_dimensionality);
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

        let response = Client::new()
            .post(url.clone())
            .headers(auth_headers)
            .query(&[("$alt", "json;enum-encoding=int")])
            .json(&vertex_request)
            .send()
            .await
            .map_err(|source| Error::PerformRequest { source, url })?;
        let response = Self::check_response(response).await?;
        let response: google_cloud_aiplatform_v1::model::EmbedContentResponse =
            response.json().await.context(DecodeResponseSnafu)?;
        let response_value =
            serde_json::to_value(&response).context(GoogleCloudResponseSerializeSnafu)?;
        serde_json::from_value(response_value).context(GoogleCloudResponseDeserializeSnafu)
    }

    async fn batch_embed_content(
        &self,
        _req: BatchEmbedContentsRequest,
    ) -> Result<BatchContentEmbeddingResponse, Error> {
        GoogleCloudUnsupportedSnafu { operation: "batchEmbedContent" }.fail()
    }

    async fn create_cached_content(
        &self,
        _req: CreateCachedContentRequest,
    ) -> Result<CachedContent, Error> {
        GoogleCloudUnsupportedSnafu { operation: "createCachedContent" }.fail()
    }

    async fn get_cached_content(&self, _name: &str) -> Result<CachedContent, Error> {
        GoogleCloudUnsupportedSnafu { operation: "getCachedContent" }.fail()
    }

    async fn list_cached_contents(
        &self,
        _page_size: Option<i32>,
        _page_token: Option<String>,
    ) -> Result<ListCachedContentsResponse, Error> {
        GoogleCloudUnsupportedSnafu { operation: "listCachedContents" }.fail()
    }

    async fn update_cached_content(
        &self,
        _name: &str,
        _expiration: CacheExpirationRequest,
    ) -> Result<CachedContent, Error> {
        GoogleCloudUnsupportedSnafu { operation: "updateCachedContent" }.fail()
    }

    async fn delete_cached_content(&self, _name: &str) -> Result<(), Error> {
        GoogleCloudUnsupportedSnafu { operation: "deleteCachedContent" }.fail()
    }

    async fn create_batch(
        &self,
        _req: BatchGenerateContentRequest,
    ) -> Result<BatchGenerateContentResponse, Error> {
        GoogleCloudUnsupportedSnafu { operation: "createBatch" }.fail()
    }

    async fn get_batch(&self, _name: &str) -> Result<BatchOperation, Error> {
        GoogleCloudUnsupportedSnafu { operation: "getBatch" }.fail()
    }

    async fn list_batches(
        &self,
        _page_size: Option<u32>,
        _page_token: Option<String>,
    ) -> Result<ListBatchesResponse, Error> {
        GoogleCloudUnsupportedSnafu { operation: "listBatches" }.fail()
    }

    async fn cancel_batch(&self, _name: &str) -> Result<(), Error> {
        GoogleCloudUnsupportedSnafu { operation: "cancelBatch" }.fail()
    }

    async fn delete_batch(&self, _name: &str) -> Result<(), Error> {
        GoogleCloudUnsupportedSnafu { operation: "deleteBatch" }.fail()
    }

    async fn upload_file(
        &self,
        _display_name: Option<String>,
        _file_bytes: Vec<u8>,
        _mime_type: Mime,
    ) -> Result<File, Error> {
        GoogleCloudUnsupportedSnafu { operation: "uploadFile" }.fail()
    }

    async fn get_file(&self, _name: &str) -> Result<File, Error> {
        GoogleCloudUnsupportedSnafu { operation: "getFile" }.fail()
    }

    async fn list_files(
        &self,
        _page_size: Option<u32>,
        _page_token: Option<String>,
    ) -> Result<ListFilesResponse, Error> {
        GoogleCloudUnsupportedSnafu { operation: "listFiles" }.fail()
    }

    async fn delete_file(&self, _name: &str) -> Result<(), Error> {
        GoogleCloudUnsupportedSnafu { operation: "deleteFile" }.fail()
    }

    async fn download_file(&self, _name: &str) -> Result<Vec<u8>, Error> {
        GoogleCloudUnsupportedSnafu { operation: "downloadFile" }.fail()
    }
}
