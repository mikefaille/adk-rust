//! Vertex AI specific types and configuration.

/// Context for Vertex AI authentication.
#[derive(Debug, Clone)]
pub struct VertexContext {
    /// Google Cloud Project ID.
    pub project: String,
    /// GCP Location (e.g., "us-central1").
    pub location: String,
    /// OAuth2 Access Token.
    pub token: String,
}

/// Re-export google_cloud_auth credentials for downstream crates (VertexADC)
pub use google_cloud_auth::credentials;
