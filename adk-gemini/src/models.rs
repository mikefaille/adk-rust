//! # Core Gemini API Primitives
//!
//! This module contains the fundamental building blocks used across the Gemini API.
//! These core data structures are shared by multiple modules and form the foundation
//! for constructing requests and parsing responses.
//!
//! ## Core Types
//!
//! - [`Role`] - Represents the speaker in a conversation (User or Model)
//! - [`Part`] - Content fragments that make up messages (text, images, function calls)
//! - [`Blob`] - Binary data with MIME type for inline content
//! - [`Content`] - Container for parts with optional role assignment
//! - [`Message`] - Complete message with content and explicit role
//! - [`Modality`] - Output format types (text, image, audio)
//!
//! ## Usage
//!
//! These types are typically used in combination with the domain-specific modules:
//! - `generation` - For content generation requests and responses
//! - `embedding` - For text embedding operations
//! - `safety` - For content moderation settings
//! - `tools` - For function calling capabilities
//! - `batch` - For batch processing operations
//! - `cache` - For content caching
//! - `files` - For file management

#![allow(clippy::enum_variant_names)]

use std::fmt::{self, Formatter};
use serde::{Deserialize, Serialize};

/// Role of a message in a conversation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Message from the user
    User,
    /// Message from the model
    Model,
}

/// Content part that can be included in a message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Part {
    /// Text content
    Text {
        /// The text content
        text: String,
        /// Whether this is a thought summary (Gemini 2.5 series only)
        #[serde(skip_serializing_if = "Option::is_none")]
        thought: Option<bool>,
        /// The thought signature for the text (Gemini 2.5 series only)
        #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    InlineData {
        /// The blob data
        #[serde(rename = "inlineData")]
        inline_data: Blob,
    },
    /// Function call from the model
    FunctionCall {
        /// The function call details
        #[serde(rename = "functionCall")]
        function_call: super::tools::FunctionCall,
        /// The thought signature for the function call (Gemini 2.5 series only)
        #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    /// Function response (results from executing a function call)
    FunctionResponse {
        /// The function response details
        #[serde(rename = "functionResponse")]
        function_response: super::tools::FunctionResponse,
    },
}

/// Blob for a message part
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Blob {
    /// The MIME type of the data
    pub mime_type: String,
    /// Base64 encoded data
    pub data: String,
}

impl Blob {
    /// Create a new blob with mime type and data
    pub fn new(mime_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self { mime_type: mime_type.into(), data: data.into() }
    }
}

/// Content of a message
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    /// Parts of the content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<Part>>,
    /// Role of the content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
}

impl Content {
    /// Create a new text content
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            parts: Some(vec![Part::Text {
                text: text.into(),
                thought: None,
                thought_signature: None,
            }]),
            role: None,
        }
    }

    /// Create a new content with a function call
    pub fn function_call(function_call: super::tools::FunctionCall) -> Self {
        Self {
            parts: Some(vec![Part::FunctionCall { function_call, thought_signature: None }]),
            role: None,
        }
    }

    /// Create a new content with a function call and thought signature
    pub fn function_call_with_thought(
        function_call: super::tools::FunctionCall,
        thought_signature: impl Into<String>,
    ) -> Self {
        Self {
            parts: Some(vec![Part::FunctionCall {
                function_call,
                thought_signature: Some(thought_signature.into()),
            }]),
            role: None,
        }
    }

    /// Create a new text content with thought signature
    pub fn text_with_thought_signature(
        text: impl Into<String>,
        thought_signature: impl Into<String>,
    ) -> Self {
        Self {
            parts: Some(vec![Part::Text {
                text: text.into(),
                thought: None,
                thought_signature: Some(thought_signature.into()),
            }]),
            role: None,
        }
    }

    /// Create a new thought content with thought signature
    pub fn thought_with_signature(
        text: impl Into<String>,
        thought_signature: impl Into<String>,
    ) -> Self {
        Self {
            parts: Some(vec![Part::Text {
                text: text.into(),
                thought: Some(true),
                thought_signature: Some(thought_signature.into()),
            }]),
            role: None,
        }
    }

    /// Create a new content with a function response
    pub fn function_response(function_response: super::tools::FunctionResponse) -> Self {
        Self { parts: Some(vec![Part::FunctionResponse { function_response }]), role: None }
    }

    /// Create a new content with a function response from name and JSON value
    pub fn function_response_json(name: impl Into<String>, response: serde_json::Value) -> Self {
        Self {
            parts: Some(vec![Part::FunctionResponse {
                function_response: super::tools::FunctionResponse::new(name, response),
            }]),
            role: None,
        }
    }

    /// Create a new content with inline data (blob data)
    pub fn inline_data(mime_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            parts: Some(vec![Part::InlineData { inline_data: Blob::new(mime_type, data) }]),
            role: None,
        }
    }

    /// Add a role to this content
    pub fn with_role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }
}

/// Message in a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Content of the message
    pub content: Content,
    /// Role of the message
    pub role: Role,
}

impl Message {
    /// Create a new user message with text content
    pub fn user(text: impl Into<String>) -> Self {
        Self { content: Content::text(text).with_role(Role::User), role: Role::User }
    }

    /// Create a new model message with text content
    pub fn model(text: impl Into<String>) -> Self {
        Self { content: Content::text(text).with_role(Role::Model), role: Role::Model }
    }

    /// Create a new embedding message with text content
    pub fn embed(text: impl Into<String>) -> Self {
        Self { content: Content::text(text), role: Role::Model }
    }

    /// Create a new function message with function response content from JSON
    pub fn function(name: impl Into<String>, response: serde_json::Value) -> Self {
        Self {
            content: Content::function_response_json(name, response).with_role(Role::Model),
            role: Role::Model,
        }
    }

    /// Create a new function message with function response from a JSON string
    pub fn function_str(
        name: impl Into<String>,
        response: impl Into<String>,
    ) -> Result<Self, serde_json::Error> {
        let response_str = response.into();
        let json = serde_json::from_str(&response_str)?;
        Ok(Self {
            content: Content::function_response_json(name, json).with_role(Role::Model),
            role: Role::Model,
        })
    }
}

/// Content modality type - specifies the format of model output
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Modality {
    /// Default value.
    ModalityUnspecified,
    /// Indicates the model should return text.
    Text,
    /// Indicates the model should return images.
    Image,
    /// Indicates the model should return audio.
    Audio,
    /// Indicates the model should return video.
    Video,
    /// Indicates document content (PDFs, etc.)
    Document,
    /// Unknown or future modality types
    #[serde(other)]
    Unknown,
}

/// Available Gemini models
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct Model(String);

impl Default for Model {
    fn default() -> Self {
        Self::GEMINI_2_5_FLASH.into()
    }
}

impl Model {
    pub const GEMINI_2_5_FLASH: &'static str = "models/gemini-2.5-flash";
    pub const GEMINI_2_5_FLASH_LITE: &'static str = "models/gemini-2.5-flash-lite";
    pub const GEMINI_2_5_PRO: &'static str = "models/gemini-2.5-pro";
    pub const TEXT_EMBEDDING_004: &'static str = "models/text-embedding-004";

    pub fn new(model: impl Into<String>) -> Self {
        Self(model.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn vertex_model_path(&self, project_id: &str, location: &str) -> String {
        let model = self.as_str();
        if model.starts_with("projects/") {
            return model.to_string();
        }
        if model.starts_with("publishers/") {
            return format!("projects/{project_id}/locations/{location}/{model}");
        }
        let model_id = model.strip_prefix("models/").unwrap_or(model);
        format!("projects/{project_id}/locations/{location}/publishers/google/models/{model_id}")
    }
}

impl From<String> for Model {
    fn from(model: String) -> Self {
        Self(model)
    }
}

impl From<&str> for Model {
    fn from(model: &str) -> Self {
        Self(model.to_string())
    }
}

impl fmt::Display for Model {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

