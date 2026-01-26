use googleapis_tonic_google_cloud_aiplatform_v1::google::cloud::aiplatform::v1::prediction_service_client::PredictionServiceClient;
use googleapis_tonic_google_cloud_aiplatform_v1::google::cloud::aiplatform::v1::{GenerateContentRequest, GenerateContentResponse};
use tonic::{transport::Channel, Status, metadata::MetadataValue};
use google_cloud_auth::project::{Config, create_token_source};
use google_cloud_auth::token_source::TokenSource;
use std::sync::Arc;
use std::fmt::Debug;
use std::task::{Context, Poll};
use tower::Service;
use http_body_util::BodyExt;
use bytes::Bytes;

#[derive(Clone)]
struct BodyAdapter(Channel);

impl<B> Service<http::Request<B>> for BodyAdapter
where B: http_body::Body<Data = Bytes> + Send + 'static,
      B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Response = <Channel as Service<http::Request<tonic::body::Body>>>::Response;
    type Error = <Channel as Service<http::Request<tonic::body::Body>>>::Error;
    type Future = <Channel as Service<http::Request<tonic::body::Body>>>::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        let (parts, body) = req.into_parts();
        let body = body.map_err(|e| Status::internal(e.into().to_string())).boxed_unsync();
        // Convert to tonic::body::Body. Assuming From impl exists for BoxBody?
        // Or if tonic::body::Body is not constructible, I am in trouble.
        // But the error said `expected tonic::body::Body, found UnsyncBoxBody`.
        // So I need to produce `tonic::body::Body`.
        // Let's try `tonic::body::Body::new(body)`.
        // Note: tonic 0.12 Body was different. tonic 0.14 Body is likely different.
        // Wait, check error again.
        // `trait Service<http::Request<tonic::body::Body>> is implemented for Channel`
        // So I must produce `Request<tonic::body::Body>`.
        let body = tonic::body::Body::new(body);
        let req = http::Request::from_parts(parts, body);
        self.0.call(req)
    }
}

#[derive(Clone)]
pub struct GeminiProvider {
    client: PredictionServiceClient<BodyAdapter>,
    token_source: Arc<dyn TokenSource>,
    project_id: String,
    location: String,
}

impl Debug for GeminiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiProvider")
            .field("project_id", &self.project_id)
            .field("location", &self.location)
            .finish()
    }
}

impl GeminiProvider {
    /// Connects using the official Google Cloud crate, automatically handling
    /// v1 endpoints and ADC authentication.
    pub async fn new(project_id: impl Into<String>, location: impl Into<String>) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let project_id = project_id.into();
        let location = location.into();

        let auth_config = Config {
            audience: Some("https://aiplatform.googleapis.com/"),
            ..Default::default()
        };
        let token_source = create_token_source(auth_config).await?;
        let token_source: Arc<dyn TokenSource> = Arc::from(token_source);

        // Endpoint: us-central1-aiplatform.googleapis.com for us-central1
        let endpoint = format!("https://{}-aiplatform.googleapis.com", location);

        let channel = Channel::from_shared(endpoint)?.connect().await?;

        let client = PredictionServiceClient::new(BodyAdapter(channel));

        Ok(Self {
            client,
            token_source,
            project_id,
            location,
        })
    }

    async fn get_token(&self) -> Result<MetadataValue<tonic::metadata::Ascii>, Status> {
        let token = self.token_source.token().await.map_err(|e| Status::unauthenticated(e.to_string()))?;
        let bearer = format!("Bearer {}", token.access_token);
        bearer.parse().map_err(|_| Status::internal("Invalid token"))
    }

    /// Generate content
    pub async fn generate_content(
        &self,
        mut request: GenerateContentRequest,
    ) -> Result<GenerateContentResponse, Status> {
        // Ensure the model resource name is correctly formatted if not fully qualified
        if !request.model.contains('/') {
             request.model = format!(
                "projects/{}/locations/{}/publishers/google/models/{}",
                self.project_id, self.location, request.model
            );
        }

        let token = self.get_token().await?;
        let mut req = tonic::Request::new(request);
        req.metadata_mut().insert("authorization", token);

        self.client.clone().generate_content(req).await.map(|r| r.into_inner())
    }

    /// Generate content stream
    pub async fn stream_generate_content(
        &self,
        mut request: GenerateContentRequest,
    ) -> Result<tonic::Streaming<GenerateContentResponse>, Status> {
         // Ensure the model resource name is correctly formatted if not fully qualified
        if !request.model.contains('/') {
             request.model = format!(
                "projects/{}/locations/{}/publishers/google/models/{}",
                self.project_id, self.location, request.model
            );
        }

        let token = self.get_token().await?;
        let mut req = tonic::Request::new(request);
        req.metadata_mut().insert("authorization", token);

        self.client.clone().stream_generate_content(req).await.map(|r| r.into_inner())
    }
}
