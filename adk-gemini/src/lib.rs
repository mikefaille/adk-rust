//! # adk-gemini
//!
//! A Rust client library for Google's Gemini 2.0 API, wrapping the official Google Cloud Vertex AI client.

pub mod provider;

/// The main Gemini API provider
pub use provider::GeminiProvider;

/// Re-export official types from googleapis-tonic-google-cloud-aiplatform-v1
pub use googleapis_tonic_google_cloud_aiplatform_v1::google::cloud::aiplatform::v1::*;
