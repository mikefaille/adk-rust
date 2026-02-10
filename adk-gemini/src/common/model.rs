use serde::{Deserialize, Serialize};
use std::fmt::{self, Formatter};

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
