use reqwest::{ClientBuilder, Url};
use snafu::ResultExt;
use std::sync::LazyLock;

use crate::backend::studio::{AuthConfig, ServiceAccountKey, ServiceAccountTokenSource, StudioBackend};
#[cfg(feature = "vertex")]
use crate::backend::vertex::{GoogleCloudAuth, GoogleCloudConfig, VertexBackend};
use crate::client::GeminiClient;
use crate::common::Model;
use crate::error::*;

static DEFAULT_BASE_URL: LazyLock<Url> = LazyLock::new(|| {
    Url::parse("https://generativelanguage.googleapis.com/v1beta/")
        .expect("unreachable error: failed to parse default base URL")
});

/// A builder for the `Gemini` client.
pub struct GeminiBuilder {
    model: Model,
    client_builder: ClientBuilder,
    base_url: Url,
    #[cfg(feature = "vertex")]
    google_cloud: Option<GoogleCloudConfig>,
    api_key: Option<String>,
    #[cfg(feature = "vertex")]
    google_cloud_auth: Option<google_cloud_auth::credentials::Credentials>,
    service_account_json: Option<String>,
}

impl GeminiBuilder {
    /// Creates a new `GeminiBuilder` with the given API key.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            model: Model::default(),
            client_builder: ClientBuilder::default(),
            base_url: DEFAULT_BASE_URL.clone(),
            #[cfg(feature = "vertex")]
            google_cloud: None,
            api_key: Some(key.into()),
            #[cfg(feature = "vertex")]
            google_cloud_auth: None,
            service_account_json: None,
        }
    }

    /// Creates a new `GeminiBuilder` without an API key.
    pub fn new_without_api_key() -> Self {
        Self {
            model: Model::default(),
            client_builder: ClientBuilder::default(),
            base_url: DEFAULT_BASE_URL.clone(),
            #[cfg(feature = "vertex")]
            google_cloud: None,
            api_key: None,
            #[cfg(feature = "vertex")]
            google_cloud_auth: None,
            service_account_json: None,
        }
    }

    /// Sets the API key.
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Sets the model for the client.
    pub fn with_model(mut self, model: impl Into<Model>) -> Self {
        self.model = model.into();
        self
    }

    /// Alias for with_model to match some user patterns
    pub fn model(mut self, model: impl Into<Model>) -> Self {
        self.model = model.into();
        self
    }

    /// Sets a custom `reqwest::ClientBuilder`.
    pub fn with_http_client(mut self, client_builder: ClientBuilder) -> Self {
        self.client_builder = client_builder;
        self
    }

    /// Sets a custom base URL for the API.
    pub fn with_base_url(mut self, base_url: Url) -> Self {
        self.base_url = base_url;
        #[cfg(feature = "vertex")]
        {
            self.google_cloud = None;
        }
        self
    }

    /// Configures the client to use a service account JSON key for authentication.
    pub fn with_service_account_json(mut self, service_account_json: &str) -> Result<Self, Error> {
         let _ = serde_json::from_str::<serde_json::Value>(service_account_json).context(ServiceAccountKeyParseSnafu)?;
         self.service_account_json = Some(service_account_json.to_string());
         #[cfg(feature = "vertex")]
         {
             let value = serde_json::from_str(service_account_json).context(GoogleCloudCredentialParseSnafu)?;
             let credentials = google_cloud_auth::credentials::service_account::Builder::new(value)
                .build()
                .context(GoogleCloudAuthSnafu)?;
             self.google_cloud_auth = Some(credentials);
         }
         Ok(self)
    }

    /// Configures the client to use Vertex AI (Google Cloud) endpoints.
    #[cfg(feature = "vertex")]
    pub fn with_google_cloud(
        mut self,
        project_id: impl Into<String>,
        location: impl Into<String>,
    ) -> Self {
        self.google_cloud = Some(GoogleCloudConfig {
            project_id: project_id.into(),
            location: location.into(),
        });
        self
    }

    /// Alias for with_google_cloud to match user request
    #[cfg(feature = "vertex")]
    pub fn vertex_auth(mut self, project: impl Into<String>, location: impl Into<String>) -> Self {
        self.google_cloud = Some(GoogleCloudConfig {
            project_id: project.into(),
            location: location.into(),
        });
        self
    }

    /// Configures the client to use Vertex AI with ADC.
    #[cfg(feature = "vertex")]
    pub fn with_google_cloud_adc(self) -> Result<Self, Error> {
         return Err(Error::Configuration { message: "Vertex AI ADC not fully implemented in this refactor version".to_string() });
    }

    #[cfg(feature = "vertex")]
    pub fn with_google_cloud_wif_json(self, _wif_json: &str) -> Result<Self, Error> {
         Err(Error::Configuration { message: "WIF not supported in this refactor yet".to_string() })
    }

    pub fn build(self) -> Result<GeminiClient, Error> {
        // DECISION LOGIC:

        // 1. If Vertex config is present AND feature enabled, use Vertex
        #[cfg(feature = "vertex")]
        if let Some(config) = self.google_cloud {
            let credentials = if let Some(creds) = self.google_cloud_auth {
                creds
            } else if let Some(json) = &self.service_account_json {
                 let value = serde_json::from_str(json).context(GoogleCloudCredentialParseSnafu)?;
                 google_cloud_auth::credentials::service_account::Builder::new(value)
                    .build()
                    .context(GoogleCloudAuthSnafu)?
            } else {
                 return Err(Error::Configuration { message: "Vertex AI requires authentication (Service Account or ADC)".to_string() });
            };

            let endpoint = format!("https://{}-aiplatform.googleapis.com", config.location);
            let backend = VertexBackend::new(
                endpoint,
                config.project_id,
                config.location,
                GoogleCloudAuth::Credentials(credentials),
                self.model.to_string(),
            )?;

            // Use Box::new instead of Arc::new
            return Ok(GeminiClient::new(Box::new(backend)));
        }

        // 2. Otherwise, use Studio
        let auth = if let Some(key) = self.api_key {
            AuthConfig::ApiKey(key)
        } else if let Some(json) = self.service_account_json {
             let key: ServiceAccountKey = serde_json::from_str(&json).context(ServiceAccountKeyParseSnafu)?;
             let source = ServiceAccountTokenSource::new(key);
             AuthConfig::ServiceAccount(source)
        } else {
             return Err(Error::MissingApiKey);
        };

        let mut headers = reqwest::header::HeaderMap::new();
        if let AuthConfig::ApiKey(ref key) = auth {
             headers.insert("x-goog-api-key", reqwest::header::HeaderValue::from_str(key).context(InvalidApiKeySnafu)?);
        }

        let http_client = self.client_builder
             .default_headers(headers)
             .build()
             .context(PerformRequestNewSnafu)?;

        let backend = StudioBackend::new_with_client(
            http_client,
            self.base_url,
            self.model,
            auth
        );

        // Use Box::new instead of Arc::new
        Ok(GeminiClient::new(Box::new(backend)))
    }
}
