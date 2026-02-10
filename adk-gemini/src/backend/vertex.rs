#![cfg(feature = "vertex")]

use crate::{
    backend::{GeminiBackend, BackendStream},
    client::{
        BadResponseSnafu, DecodeResponseSnafu, DeserializeSnafu, GoogleCloudAuthSnafu,
        GoogleCloudClientBuildSnafu, GoogleCloudCredentialHeadersSnafu,
        GoogleCloudCredentialHeadersUnavailableSnafu, GoogleCloudCredentialParseSnafu,
         GoogleCloudRequestDeserializeSnafu,
        GoogleCloudRequestNotObjectSnafu, GoogleCloudRequestSerializeSnafu, GoogleCloudRequestSnafu,
        GoogleCloudResponseDeserializeSnafu, GoogleCloudResponseSerializeSnafu, GoogleCloudUnsupportedSnafu,
         PerformRequestSnafu, TokioRuntimeSnafu, UrlParseSnafu,
        Error, Model,
    },
    generation::model::{GenerateContentRequest, GenerationResponse},
    embedding::model::{EmbedContentRequest, ContentEmbeddingResponse, BatchEmbedContentsRequest, BatchContentEmbeddingResponse},
    batch::model::{BatchOperation, BatchGenerateContentRequest, ListBatchesResponse},
    cache::model::{CachedContent, CreateCachedContentRequest, ListCachedContentsResponse, CacheExpirationRequest},
    files::model::{File, ListFilesResponse},
};
use async_trait::async_trait;
use google_cloud_aiplatform_v1::client::{PredictionService, JobService, GenAiCacheService};
use google_cloud_auth::credentials::{self, Credentials};
use reqwest::{Client, Url, Response};
use serde_json::Value;
use snafu::{OptionExt, ResultExt};
use std::sync::Arc;
use tracing::instrument;

#[derive(Debug, Clone)]
pub enum GoogleCloudAuth {
    ApiKey(String),
    Adc,
    ServiceAccountJson(String),
    WifJson(String),
    Credentials(Credentials),
}

#[derive(Debug)]
pub struct VertexBackend {
    prediction: PredictionService,
    job: JobService,
    cache: GenAiCacheService,
    credentials: Credentials,
    endpoint: String,
    project: String,
    location: String,
    model: Model,
}

impl VertexBackend {
    pub fn new(
        endpoint: String,
        project: String,
        location: String,
        auth: GoogleCloudAuth,
        model: Model,
    ) -> Result<Self, Error> {
        let (prediction, job, cache, credentials) = build_vertex_prediction_service(endpoint.clone(), auth)?;

        Ok(Self {
            prediction,
            job,
            cache,
            credentials,
            endpoint,
            project,
            location,
            model,
        })
    }

    fn is_vertex_transport_error_message(message: &str) -> bool {
        message.contains("client error (SendRequest): http2 error")
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
            .query(&[("alt", "json;enum-encoding=int")])
            .json(&request)
            .send()
            .await
            .context(PerformRequestSnafu { url: url.clone() })?;

        let response = check_response(response).await?;
        let response: google_cloud_aiplatform_v1::model::GenerateContentResponse =
            response.json().await.context(DecodeResponseSnafu)?;
        let response_value =
            serde_json::to_value(&response).context(GoogleCloudResponseSerializeSnafu)?;
        serde_json::from_value(response_value).context(GoogleCloudResponseDeserializeSnafu)
    }
}

async fn check_response(response: Response) -> Result<Response, Error> {
    let status = response.status();
    if !status.is_success() {
        let description = response.text().await.ok();
        BadResponseSnafu { code: status.as_u16(), description }.fail()
    } else {
        Ok(response)
    }
}

fn build_vertex_prediction_service(
    endpoint: String,
    auth: GoogleCloudAuth,
) -> Result<(PredictionService, JobService, GenAiCacheService, Credentials), Error> {
    let build_in_runtime =
        |endpoint: String, auth: GoogleCloudAuth| -> Result<(PredictionService, JobService, GenAiCacheService, Credentials), Error> {
            let runtime = tokio::runtime::Runtime::new().context(TokioRuntimeSnafu)?;
            runtime.block_on(async {
                let credentials = match auth {
                    GoogleCloudAuth::ApiKey(api_key) => {
                         credentials::api_key_credentials::Builder::new(api_key).build()
                    }
                    GoogleCloudAuth::Adc => {
                        let scopes = ["https://www.googleapis.com/auth/cloud-platform"];
                        credentials::Builder::default().with_scopes(scopes).build().context(GoogleCloudAuthSnafu)?
                    }
                    GoogleCloudAuth::ServiceAccountJson(json) => {
                        let value: serde_json::Value = serde_json::from_str(&json).context(GoogleCloudCredentialParseSnafu)?;
                        credentials::service_account::Builder::new(value).build().context(GoogleCloudAuthSnafu)?
                    }
                    GoogleCloudAuth::WifJson(json) => {
                         let value: serde_json::Value = serde_json::from_str(&json).context(GoogleCloudCredentialParseSnafu)?;
                         credentials::external_account::Builder::new(value).build().context(GoogleCloudAuthSnafu)?
                    }
                    GoogleCloudAuth::Credentials(c) => c,
                };

                let prediction = PredictionService::builder()
                        .with_endpoint(endpoint.clone())
                        .with_credentials(credentials.clone())
                        .build().await.context(GoogleCloudClientBuildSnafu)?;

                let job = JobService::builder()
                        .with_endpoint(endpoint.clone())
                        .with_credentials(credentials.clone())
                        .build().await.context(GoogleCloudClientBuildSnafu)?;

                let cache = GenAiCacheService::builder()
                        .with_endpoint(endpoint)
                        .with_credentials(credentials.clone())
                        .build().await.context(GoogleCloudClientBuildSnafu)?;

                Ok((prediction, job, cache, credentials))
            })
        };

    if tokio::runtime::Handle::try_current().is_ok() {
        let worker = std::thread::Builder::new()
            .name("adk-gemini-vertex-init".to_string())
            .spawn(move || build_in_runtime(endpoint, auth))
            .map_err(|source| Error::TokioRuntime { source })?;

        return worker.join().map_err(|_| Error::GoogleCloudInitThreadPanicked)?;
    }

    build_in_runtime(endpoint, auth)
}

#[async_trait]
impl GeminiBackend for VertexBackend {
    async fn generate_content(&self, request: GenerateContentRequest) -> Result<GenerationResponse, Error> {
        let rest_request = request.clone();
        let mut request_value =
            serde_json::to_value(&request).context(GoogleCloudRequestSerializeSnafu)?;
        let model = self.model.to_string();
        let request_object =
            request_value.as_object_mut().context(GoogleCloudRequestNotObjectSnafu)?;
        request_object.insert("model".to_string(), serde_json::Value::String(model));

        let request: google_cloud_aiplatform_v1::model::GenerateContentRequest =
            serde_json::from_value(request_value).context(GoogleCloudRequestDeserializeSnafu)?;

        let response = match self.prediction.generate_content().with_request(request).send().await
        {
            Ok(response) => response,
            Err(source) => {
                if VertexBackend::is_vertex_transport_error_message(&source.to_string()) {
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

    async fn generate_content_stream(&self, _req: GenerateContentRequest) -> Result<BackendStream<GenerationResponse>, Error> {
        GoogleCloudUnsupportedSnafu { operation: "streamGenerateContent" }.fail()
    }

    async fn embed_content(&self, request: EmbedContentRequest) -> Result<ContentEmbeddingResponse, Error> {
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

        let response = Client::new()
            .post(url.clone())
            .headers(auth_headers)
            .json(&request)
            .send()
            .await
            .context(PerformRequestSnafu { url: url.clone() })?;

        let response = check_response(response).await?;
        let response: google_cloud_aiplatform_v1::model::EmbedContentResponse =
            response.json().await.context(DecodeResponseSnafu)?;

        let response_value =
            serde_json::to_value(&response).context(GoogleCloudResponseSerializeSnafu)?;
        serde_json::from_value(response_value).context(GoogleCloudResponseDeserializeSnafu)
    }

    async fn batch_embed_contents(&self, _req: BatchEmbedContentsRequest) -> Result<BatchContentEmbeddingResponse, Error> {
         GoogleCloudUnsupportedSnafu { operation: "batchEmbedContents" }.fail()
    }

    async fn create_batch(&self, req: BatchGenerateContentRequest) -> Result<BatchOperation, Error> {
        let request_value = serde_json::to_value(&req).context(GoogleCloudRequestSerializeSnafu)?;
        let request: google_cloud_aiplatform_v1::model::CreateBatchPredictionJobRequest =
            serde_json::from_value(request_value).context(GoogleCloudRequestDeserializeSnafu)?;

        let response = self.job.create_batch_prediction_job().with_request(request).send().await.map_err(|source| Error::GoogleCloudRequest { source })?;

        let response_value = serde_json::to_value(&response).context(GoogleCloudResponseSerializeSnafu)?;
        serde_json::from_value(response_value).context(GoogleCloudResponseDeserializeSnafu)
    }

    async fn get_batch(&self, name: &str) -> Result<BatchOperation, Error> {
         let request = google_cloud_aiplatform_v1::model::GetBatchPredictionJobRequest::new().set_name(name.to_string());
         let response = self.job.get_batch_prediction_job().with_request(request).send().await.map_err(|source| Error::GoogleCloudRequest { source })?;

         let response_value = serde_json::to_value(&response).context(GoogleCloudResponseSerializeSnafu)?;
         serde_json::from_value(response_value).context(GoogleCloudResponseDeserializeSnafu)
    }

    async fn list_batches(&self, page_size: Option<u32>, page_token: Option<String>) -> Result<ListBatchesResponse, Error> {
         let parent = format!("projects/{}/locations/{}", self.project, self.location);
         let request = google_cloud_aiplatform_v1::model::ListBatchPredictionJobsRequest::new()
             .set_parent(parent)
             .set_page_size(page_size.unwrap_or(10) as i32)
             .set_page_token(page_token.unwrap_or_default());

         let response = self.job.list_batch_prediction_jobs().with_request(request).send().await.map_err(|source| Error::GoogleCloudRequest { source })?;

         let response_value = serde_json::to_value(&response).context(GoogleCloudResponseSerializeSnafu)?;
         serde_json::from_value(response_value).context(GoogleCloudResponseDeserializeSnafu)
    }

    async fn create_cached_content(&self, req: CreateCachedContentRequest) -> Result<CachedContent, Error> {
        let request_value = serde_json::to_value(&req).context(GoogleCloudRequestSerializeSnafu)?;
        let request: google_cloud_aiplatform_v1::model::CreateCachedContentRequest =
            serde_json::from_value(request_value).context(GoogleCloudRequestDeserializeSnafu)?;

        let response = self.cache.create_cached_content().with_request(request).send().await.map_err(|source| Error::GoogleCloudRequest { source })?;

        let response_value = serde_json::to_value(&response).context(GoogleCloudResponseSerializeSnafu)?;
        serde_json::from_value(response_value).context(GoogleCloudResponseDeserializeSnafu)
    }

    async fn get_cached_content(&self, name: &str) -> Result<CachedContent, Error> {
         let request = google_cloud_aiplatform_v1::model::GetCachedContentRequest::new().set_name(name.to_string());
         let response = self.cache.get_cached_content().with_request(request).send().await.map_err(|source| Error::GoogleCloudRequest { source })?;

         let response_value = serde_json::to_value(&response).context(GoogleCloudResponseSerializeSnafu)?;
         serde_json::from_value(response_value).context(GoogleCloudResponseDeserializeSnafu)
    }

    async fn list_cached_contents(&self, page_size: Option<i32>, page_token: Option<String>) -> Result<ListCachedContentsResponse, Error> {
         let parent = format!("projects/{}/locations/{}", self.project, self.location);
         let request = google_cloud_aiplatform_v1::model::ListCachedContentsRequest::new()
             .set_parent(parent)
             .set_page_size(page_size.unwrap_or(10))
             .set_page_token(page_token.unwrap_or_default());
         let response = self.cache.list_cached_contents().with_request(request).send().await.map_err(|source| Error::GoogleCloudRequest { source })?;

         let response_value = serde_json::to_value(&response).context(GoogleCloudResponseSerializeSnafu)?;
         serde_json::from_value(response_value).context(GoogleCloudResponseDeserializeSnafu)
    }

    async fn update_cached_content(&self, _name: &str, _req: CacheExpirationRequest) -> Result<CachedContent, Error> {
        Err(Error::GoogleCloudUnsupported { operation: "updateCachedContent" })
    }

    async fn delete_cached_content(&self, name: &str) -> Result<(), Error> {
         let request = google_cloud_aiplatform_v1::model::DeleteCachedContentRequest::new().set_name(name.to_string());
         self.cache.delete_cached_content().with_request(request).send().await.map_err(|source| Error::GoogleCloudRequest { source })?;
         Ok(())
    }
}
