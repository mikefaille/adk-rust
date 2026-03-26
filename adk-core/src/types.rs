use serde::{Deserialize, Serialize};

/// Maximum allowed size for inline binary data (10 MB).
/// Prevents accidental or malicious embedding of oversized payloads in Content parts.
pub const MAX_INLINE_DATA_SIZE: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionResponseData {
    pub name: String,
    pub response: serde_json::Value,
}

/// Standardized message roles for LLM interactions.
///
/// Implements custom serialization/deserialization to cleanly map standard roles
/// (user, model, system, tool) while preserving extensibility via the `Other` variant.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Role {
    #[default]
    User,
    Model,
    System,
    Tool,
    Function,
    Other(String),
}

impl std::str::FromStr for Role {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "user" => Ok(Role::User),
            "model" | "assistant" => Ok(Role::Model),
            "system" | "developer" => Ok(Role::System),
            "tool" => Ok(Role::Tool),
            "function" => Ok(Role::Function),
            _ => Ok(Role::Other(s.to_string())),
        }
    }
}

impl TryFrom<&str> for Role {
    type Error = crate::error::AdkError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        if s.trim().is_empty() {
            return Err(crate::error::AdkError::Config(
                "Role cannot be an empty string".to_string(),
            ));
        }
        Ok(s.parse().unwrap_or(Role::Other(s.to_string())))
    }
}

impl TryFrom<String> for Role {
    type Error = crate::error::AdkError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        if s.trim().is_empty() {
            return Err(crate::error::AdkError::Config(
                "Role cannot be an empty string".to_string(),
            ));
        }
        Ok(s.as_str().parse().unwrap_or(Role::Other(s)))
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Model => write!(f, "model"),
            Role::System => write!(f, "system"),
            Role::Tool => write!(f, "tool"),
            Role::Function => write!(f, "function"),
            Role::Other(s) => write!(f, "{s}"),
        }
    }
}

impl serde::Serialize for Role {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for Role {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(s.parse().unwrap_or(Role::Other(s)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Content {
    pub role: Role,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Part {
    /// Thinking/reasoning trace from a thinking-capable model.
    ///
    /// Must be placed before `Text` in the enum so that `#[serde(untagged)]`
    /// deserialization matches `{"thinking": "..."}` before falling through to `Text`.
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    Text {
        text: String,
    },
    InlineData {
        mime_type: String,
        data: Vec<u8>,
    },
    /// File data referenced by URI (URL or cloud storage path).
    ///
    /// This allows referencing external files without embedding the data inline.
    /// Providers that don't support URI-based content can fetch and convert to InlineData.
    ///
    /// # Example
    ///
    /// ```rust
    /// use adk_core::Part;
    ///
    /// let image_url = Part::FileData {
    ///     mime_type: "image/jpeg".to_string(),
    ///     file_uri: "https://example.com/image.jpg".to_string(),
    /// };
    /// ```
    FileData {
        /// MIME type of the file (e.g., "image/jpeg", "audio/wav")
        mime_type: String,
        /// URI to the file (URL, gs://, etc.)
        file_uri: String,
    },
    FunctionCall {
        name: String,
        args: serde_json::Value,
        /// Tool call ID for OpenAI-style providers. None for Gemini.
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Thought signature for Gemini 3 series models.
        /// Must be preserved and relayed back in conversation history
        /// during multi-turn function calling.
        #[serde(skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    FunctionResponse {
        function_response: FunctionResponseData,
        /// Tool call ID for OpenAI-style providers. None for Gemini.
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
}

impl Content {
    pub fn new(role: impl TryInto<Role, Error = crate::error::AdkError>) -> Result<Self, crate::error::AdkError> {
        Ok(Self { role: role.try_into()?, parts: Vec::new() })
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.parts.push(Part::Text { text: text.into() });
        self
    }

    /// Add inline binary data (e.g., image bytes).
    ///
    /// # Panics
    /// Panics if `data` exceeds [`MAX_INLINE_DATA_SIZE`] (10 MB).
    pub fn with_inline_data(mut self, mime_type: impl Into<String>, data: Vec<u8>) -> Self {
        assert!(
            data.len() <= MAX_INLINE_DATA_SIZE,
            "Inline data size {} exceeds maximum allowed size of {} bytes",
            data.len(),
            MAX_INLINE_DATA_SIZE
        );
        self.parts.push(Part::InlineData { mime_type: mime_type.into(), data });
        self
    }

    /// Add a thinking/reasoning trace part.
    pub fn with_thinking(mut self, thinking: impl Into<String>) -> Self {
        self.parts.push(Part::Thinking { thinking: thinking.into(), signature: None });
        self
    }

    /// Add a file reference by URI (URL or cloud storage path).
    pub fn with_file_uri(
        mut self,
        mime_type: impl Into<String>,
        file_uri: impl Into<String>,
    ) -> Self {
        self.parts.push(Part::FileData { mime_type: mime_type.into(), file_uri: file_uri.into() });
        self
    }
}

impl Part {
    /// Returns the text content if this is a Text part, None otherwise
    pub fn text(&self) -> Option<&str> {
        match self {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        }
    }

    /// Returns true if this part is a Thinking variant
    pub fn is_thinking(&self) -> bool {
        matches!(self, Part::Thinking { .. })
    }

    /// Returns the thinking text content if this is a Thinking part, None otherwise
    pub fn thinking_text(&self) -> Option<&str> {
        match self {
            Part::Thinking { thinking, .. } => Some(thinking.as_str()),
            _ => None,
        }
    }

    /// Returns the MIME type if this part has one (InlineData or FileData)
    pub fn mime_type(&self) -> Option<&str> {
        match self {
            Part::InlineData { mime_type, .. } => Some(mime_type.as_str()),
            Part::FileData { mime_type, .. } => Some(mime_type.as_str()),
            _ => None,
        }
    }

    /// Returns the file URI if this is a FileData part
    pub fn file_uri(&self) -> Option<&str> {
        match self {
            Part::FileData { file_uri, .. } => Some(file_uri.as_str()),
            _ => None,
        }
    }

    /// Returns true if this part contains media (image, audio, video)
    pub fn is_media(&self) -> bool {
        matches!(self, Part::InlineData { .. } | Part::FileData { .. })
    }

    /// Create a new text part
    pub fn text_part(text: impl Into<String>) -> Self {
        Part::Text { text: text.into() }
    }

    /// Create a new inline data part
    ///
    /// # Panics
    /// Panics if `data` exceeds [`MAX_INLINE_DATA_SIZE`] (10 MB).
    pub fn inline_data(mime_type: impl Into<String>, data: Vec<u8>) -> Self {
        assert!(
            data.len() <= MAX_INLINE_DATA_SIZE,
            "Inline data size {} exceeds maximum allowed size of {} bytes",
            data.len(),
            MAX_INLINE_DATA_SIZE
        );
        Part::InlineData { mime_type: mime_type.into(), data }
    }

    /// Create a new file data part from URI
    pub fn file_data(mime_type: impl Into<String>, file_uri: impl Into<String>) -> Self {
        Part::FileData { mime_type: mime_type.into(), file_uri: file_uri.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_creation() {
        let content = Content::new("user").expect("Failed to parse role").with_text("Hello");
        assert_eq!(content.role, Role::User);
        assert_eq!(content.parts.len(), 1);
    }

    #[test]
    fn test_content_with_inline_data() {
        let content = Content::new("user").expect("Failed to parse role")
            .with_text("Check this image")
            .with_inline_data("image/png", vec![0x89, 0x50, 0x4E, 0x47]);
        assert_eq!(content.parts.len(), 2);
        assert!(
            matches!(&content.parts[1], Part::InlineData { mime_type, .. } if mime_type == "image/png")
        );
    }

    #[test]
    fn test_content_with_file_uri() {
        let content = Content::new("user").expect("Failed to parse role")
            .with_text("Check this image")
            .with_file_uri("image/jpeg", "https://example.com/image.jpg");
        assert_eq!(content.parts.len(), 2);
        assert!(
            matches!(&content.parts[1], Part::FileData { file_uri, .. } if file_uri == "https://example.com/image.jpg")
        );
    }

    #[test]
    fn test_part_serialization() {
        let part = Part::Text { text: "test".to_string() };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("test"));
    }

    #[test]
    fn test_part_file_data_serialization() {
        let part = Part::FileData {
            mime_type: "image/jpeg".to_string(),
            file_uri: "https://example.com/image.jpg".to_string(),
        };
        let json = serde_json::to_string(&part).unwrap();
        assert!(json.contains("image/jpeg"));
        assert!(json.contains("https://example.com/image.jpg"));
    }

    #[test]
    fn test_part_text_accessor() {
        let text_part = Part::Text { text: "hello".to_string() };
        assert_eq!(text_part.text(), Some("hello"));

        let data_part = Part::InlineData { mime_type: "image/png".to_string(), data: vec![] };
        assert_eq!(data_part.text(), None);
    }

    #[test]
    fn test_part_mime_type_accessor() {
        let text_part = Part::Text { text: "hello".to_string() };
        assert_eq!(text_part.mime_type(), None);

        let inline_part = Part::InlineData { mime_type: "image/png".to_string(), data: vec![] };
        assert_eq!(inline_part.mime_type(), Some("image/png"));

        let file_part = Part::FileData {
            mime_type: "image/jpeg".to_string(),
            file_uri: "https://example.com".to_string(),
        };
        assert_eq!(file_part.mime_type(), Some("image/jpeg"));
    }

    #[test]
    fn test_part_file_uri_accessor() {
        let text_part = Part::Text { text: "hello".to_string() };
        assert_eq!(text_part.file_uri(), None);

        let file_part = Part::FileData {
            mime_type: "image/jpeg".to_string(),
            file_uri: "https://example.com/img.jpg".to_string(),
        };
        assert_eq!(file_part.file_uri(), Some("https://example.com/img.jpg"));
    }

    #[test]
    fn test_part_is_media() {
        let text_part = Part::Text { text: "hello".to_string() };
        assert!(!text_part.is_media());

        let inline_part = Part::InlineData { mime_type: "image/png".to_string(), data: vec![] };
        assert!(inline_part.is_media());

        let file_part = Part::FileData {
            mime_type: "image/jpeg".to_string(),
            file_uri: "https://example.com".to_string(),
        };
        assert!(file_part.is_media());
    }

    #[test]
    fn test_part_constructors() {
        let text = Part::text_part("hello");
        assert!(matches!(text, Part::Text { text } if text == "hello"));

        let inline = Part::inline_data("image/png", vec![1, 2, 3]);
        assert!(
            matches!(inline, Part::InlineData { mime_type, data } if mime_type == "image/png" && data == vec![1, 2, 3])
        );

        let file = Part::file_data("image/jpeg", "https://example.com/img.jpg");
        assert!(
            matches!(file, Part::FileData { mime_type, file_uri } if mime_type == "image/jpeg" && file_uri == "https://example.com/img.jpg")
        );
    }

    #[test]
    fn test_inline_data_within_limit() {
        // Should succeed: small data
        let data = vec![0u8; 1024];
        let content = Content::new("user").expect("Failed to parse role").with_inline_data("image/png", data);
        assert_eq!(content.parts.len(), 1);
    }

    #[test]
    fn test_inline_data_at_limit() {
        // Should succeed: exactly at limit
        let data = vec![0u8; MAX_INLINE_DATA_SIZE];
        let part = Part::inline_data("image/png", data);
        assert!(part.is_media());
    }

    #[test]
    #[should_panic(expected = "exceeds maximum allowed size")]
    fn test_inline_data_exceeds_limit_content() {
        let data = vec![0u8; MAX_INLINE_DATA_SIZE + 1];
        let _ = Content::new("user").expect("Failed to parse role").with_inline_data("image/png", data);
    }

    #[test]
    #[should_panic(expected = "exceeds maximum allowed size")]
    fn test_inline_data_exceeds_limit_part() {
        let data = vec![0u8; MAX_INLINE_DATA_SIZE + 1];
        let _ = Part::inline_data("image/png", data);
    }

    #[test]
    fn test_thinking_variant_accessors() {
        let part = Part::Thinking {
            thinking: "step by step".to_string(),
            signature: Some("sig123".to_string()),
        };
        assert!(part.is_thinking());
        assert_eq!(part.thinking_text(), Some("step by step"));
        assert_eq!(part.text(), None);
    }

    #[test]
    fn test_non_thinking_variant_accessors() {
        let text = Part::Text { text: "hello".to_string() };
        assert!(!text.is_thinking());
        assert_eq!(text.thinking_text(), None);

        let data = Part::InlineData { mime_type: "image/png".to_string(), data: vec![] };
        assert!(!data.is_thinking());
        assert_eq!(data.thinking_text(), None);
    }

    #[test]
    fn test_content_with_thinking() {
        let content = Content::new("model").expect("Failed to parse role").with_thinking("Let me reason about this");
        assert_eq!(content.parts.len(), 1);
        assert!(matches!(
            &content.parts[0],
            Part::Thinking { thinking, signature } if thinking == "Let me reason about this" && signature.is_none()
        ));
    }

    #[test]
    fn test_thinking_serialization_round_trip() {
        let part = Part::Thinking {
            thinking: "reasoning here".to_string(),
            signature: Some("sig".to_string()),
        };
        let json = serde_json::to_string(&part).unwrap();
        let deserialized: Part = serde_json::from_str(&json).unwrap();
        assert_eq!(part, deserialized);
    }

    #[test]
    fn test_thinking_without_signature_serialization() {
        let part = Part::Thinking { thinking: "reasoning".to_string(), signature: None };
        let json = serde_json::to_string(&part).unwrap();
        // signature should be omitted from JSON
        assert!(!json.contains("signature"));
        let deserialized: Part = serde_json::from_str(&json).unwrap();
        assert_eq!(part, deserialized);
    }

    #[test]
    fn test_thinking_does_not_deserialize_as_text() {
        let json = r#"{"thinking": "some reasoning"}"#;
        let part: Part = serde_json::from_str(json).unwrap();
        assert!(part.is_thinking());
        assert_eq!(part.thinking_text(), Some("some reasoning"));
        assert_eq!(part.text(), None);
    }

    #[test]
    fn test_text_does_not_deserialize_as_thinking() {
        let json = r#"{"text": "hello world"}"#;
        let part: Part = serde_json::from_str(json).unwrap();
        assert!(!part.is_thinking());
        assert_eq!(part.text(), Some("hello world"));
    }
}
