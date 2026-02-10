#![cfg(feature = "vertex")]

use crate::{
    backend::{GeminiBackend, BackendStream},
    client::{
        BadResponseSnafu, DecodeResponseSnafu, DeserializeSnafu, GoogleCloudAuthSnafu,
        GoogleCloudClientBuildSnafu, GoogleCloudCredentialHeadersSnafu,
        GoogleCloudCredentialHeadersUnavailableSnafu, GoogleCloudCredentialParseSnafu,
        GoogleCloudInitThreadPanicked, GoogleCloudRequestDeserializeSnafu,
        GoogleCloudRequestNotObjectSnafu, GoogleCloudRequestSerializeSnafu, GoogleCloudRequestSnafu,
        GoogleCloudResponseDeserializeSnafu, GoogleCloudResponseSerializeSnafu, GoogleCloudUnsupportedSnafu,
        MissingGoogleCloudProjectId, PerformRequestSnafu, TokioRuntimeSnafu, UrlParseSnafu,
        Error, Model,
    },
    generation::model::{GenerateContentRequest, GenerationResponse},
    embedding::model::{EmbedContentRequest, ContentEmbeddingResponse, BatchEmbedContentsRequest, BatchContentEmbeddingResponse},
    batch::model::{BatchOperation, BatchGenerateContentRequest, ListBatchesResponse},
    cache::model::{CachedContent, CreateCachedContentRequest, ListCachedContentsResponse, CacheExpirationRequest},
    files::model::{File, ListFilesResponse},
};
use async_trait::async_trait;
use google_cloud_aiplatform_v1::client::PredictionService;
use google_cloud_auth::credentials::{self, Credentials};
use reqwest::{Client, Url, Response};
use serde_json::Value;
use snafu::{OptionExt, ResultExt};
use std::sync::Arc;
use tracing::instrument;

#[derive(Debug, Clone)]
pub enum GoogleCloudAuth {
    ApiKey(String),
    Credentials(Credentials),
}

impl GoogleCloudAuth {
    pub fn credentials(&self) -> Result<Credentials, Error> {
        match self {
            GoogleCloudAuth::ApiKey(api_key) => {
                Ok(credentials::api_key_credentials::Builder::new(api_key.clone()).build())
            }
            GoogleCloudAuth::Credentials(credentials) => Ok(credentials.clone()),
        }
    }
}

#[derive(Debug)]
pub struct VertexBackend {
    prediction: PredictionService,
    credentials: Credentials,
    endpoint: String,
    model: Model,
}

impl VertexBackend {
    pub fn new(
        endpoint: String,
        credentials: Credentials,
        model: Model,
    ) -> Result<Self, Error> {
        let prediction = build_vertex_prediction_service(endpoint.clone(), credentials.clone())?;

        Ok(Self {
            prediction,
            credentials,
            endpoint,
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
}
