use serde::{Deserialize, Serialize};

/// Maximum allowed size for inline binary data (10 MB).
/// Prevents accidental or malicious embedding of oversized payloads in Content parts.
pub const MAX_INLINE_DATA_SIZE: usize = 10 * 1024 * 1024;

macro_rules! define_id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Result<Self, &'static str> {
                let id = id.into();
                if id.is_empty() {
                    return Err(concat!(stringify!($name), " cannot be empty"));
                }
                if id.contains(':') {
                    return Err(concat!(stringify!($name), " cannot contain ':'"));
                }
                Ok(Self(id))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    };
}

define_id_type!(UserId);
define_id_type!(SessionId);

/// Strongly typed Role to prevent "Stringly-Typed" logic errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Role {
    User,
    Model,
    System,
    Tool,
    Custom(String),
}

impl From<String> for Role {
    fn from(s: String) -> Self {
        match s.to_lowercase().as_str() {
            "user" | "human" => Role::User,
            "model" | "assistant" => Role::Model,
            "system" | "developer" => Role::System,
            "tool" | "function" => Role::Tool,
            _ => Role::Custom(s),
        }
    }
}

impl From<&str> for Role {
    fn from(s: &str) -> Self {
        Role::from(s.to_string())
    }
}

impl From<Role> for String {
    fn from(role: Role) -> Self {
        match role {
            Role::User => "user".to_string(),
            Role::Model => "model".to_string(),
            Role::System => "system".to_string(),
            Role::Tool => "tool".to_string(),
            Role::Custom(s) => s,
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s: String = self.clone().into();
        write!(f, "{}", s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionResponseData {
    pub name: String,
    pub response: serde_json::Value,
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
        // Ensures Vec<u8> serializes to a base64 string, not an integer array.
        #[serde(with = "base64_serde")]
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
    /// Safely instantiate Content with a strongly-typed Role
    pub fn new(role: impl Into<Role>) -> Self {
        Self { role: role.into(), parts: Vec::new() }
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.parts.push(Part::Text { text: text.into() });
        self
    }

    /// Add inline binary data (e.g., image bytes).
    ///
    /// # Panics
    /// Panics if `data` exceeds [`MAX_INLINE_DATA_SIZE`] (10 MB).
    ///
    /// # Deprecated
    /// Use `try_with_inline_data` instead.
    #[deprecated(note = "Use try_with_inline_data instead")]
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

    /// Add inline binary data safely.
    /// Returns an Error instead of panicking if the payload exceeds 10MB.
    pub fn try_with_inline_data(
        mut self,
        mime_type: impl Into<String>,
        data: Vec<u8>,
    ) -> Result<Self, &'static str> {
        if data.len() > MAX_INLINE_DATA_SIZE {
             return Err("Inline data size exceeds maximum allowed size of 10 MB");
        }
        self.parts.push(Part::InlineData { mime_type: mime_type.into(), data });
        Ok(self)
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
    ///
    /// # Deprecated
    /// Use `try_inline_data` instead.
    #[deprecated(note = "Use try_inline_data instead")]
    pub fn inline_data(mime_type: impl Into<String>, data: Vec<u8>) -> Self {
        assert!(
            data.len() <= MAX_INLINE_DATA_SIZE,
            "Inline data size {} exceeds maximum allowed size of {} bytes",
            data.len(),
            MAX_INLINE_DATA_SIZE
        );
        Part::InlineData { mime_type: mime_type.into(), data }
    }

    /// Create a new inline data part safely.
    pub fn try_inline_data(mime_type: impl Into<String>, data: Vec<u8>) -> Result<Self, &'static str> {
        if data.len() > MAX_INLINE_DATA_SIZE {
            return Err("Inline data size exceeds maximum allowed size of 10 MB");
        }
        Ok(Part::InlineData { mime_type: mime_type.into(), data })
    }

    /// Create a new file data part from URI
    pub fn file_data(mime_type: impl Into<String>, file_uri: impl Into<String>) -> Self {
        Part::FileData { mime_type: mime_type.into(), file_uri: file_uri.into() }
    }
}

/// Utility module to enforce Base64 Serialization for Vec<u8>
mod base64_serde {
    use serde::{Serializer, Deserialize, Deserializer};
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let b64 = String::deserialize(d)?;
        STANDARD.decode(b64.as_bytes()).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_id_validation() {
        assert!(UserId::new("user123").is_ok());
        assert!(UserId::new("").is_err());
        assert!(UserId::new("user:123").is_err());
    }

    #[test]
    fn test_session_id_validation() {
        assert!(SessionId::new("session123").is_ok());
        assert!(SessionId::new("").is_err());
        assert!(SessionId::new("session:123").is_err());
    }

    #[test]
    fn test_role_enum() {
        assert_eq!(Role::from("user"), Role::User);
        assert_eq!(Role::from("User"), Role::User);
        assert_eq!(Role::from("human"), Role::User);
        assert_eq!(Role::from("model"), Role::Model);
        assert_eq!(Role::from("assistant"), Role::Model);
        assert_eq!(Role::from("system"), Role::System);
        assert_eq!(Role::from("developer"), Role::System);
        assert_eq!(Role::from("tool"), Role::Tool);
        assert_eq!(Role::from("function"), Role::Tool);
        assert_eq!(Role::from("custom"), Role::Custom("custom".to_string()));
    }

    #[test]
    fn test_role_serialization() {
        let role = Role::User;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"user\"");

        let role = Role::Custom("foo".to_string());
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"foo\"");
    }

    #[test]
    fn test_role_deserialization() {
        let json = "\"user\"";
        let role: Role = serde_json::from_str(json).unwrap();
        assert_eq!(role, Role::User);

        let json = "\"assistant\"";
        let role: Role = serde_json::from_str(json).unwrap();
        assert_eq!(role, Role::Model);

        let json = "\"foo\"";
        let role: Role = serde_json::from_str(json).unwrap();
        assert_eq!(role, Role::Custom("foo".to_string()));
    }

    #[test]
    fn test_content_creation() {
        let content = Content::new("user").with_text("Hello");
        assert_eq!(content.role, Role::User);
        assert_eq!(content.parts.len(), 1);
    }

    #[test]
    fn test_content_with_inline_data() {
        #[allow(deprecated)]
        let content = Content::new("user")
            .with_text("Check this image")
            .with_inline_data("image/png", vec![0x89, 0x50, 0x4E, 0x47]);
        assert_eq!(content.parts.len(), 2);
        assert!(
            matches!(&content.parts[1], Part::InlineData { mime_type, .. } if mime_type == "image/png")
        );
    }

    #[test]
    fn test_try_with_inline_data() {
        let content = Content::new("user")
            .try_with_inline_data("image/png", vec![0x89, 0x50, 0x4E, 0x47])
            .unwrap();
        assert_eq!(content.parts.len(), 1);

        let large_data = vec![0u8; MAX_INLINE_DATA_SIZE + 1];
        let result = Content::new("user").try_with_inline_data("image/png", large_data);
        assert!(result.is_err());
    }

    #[test]
    fn test_inline_data_base64_serialization() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let data = vec![1, 2, 3, 4, 5];
        let part = Part::try_inline_data("application/octet-stream", data.clone()).unwrap();

        let json = serde_json::to_string(&part).unwrap();
        // Should contain the base64 string, not an array of numbers
        assert!(json.contains("data"));
        assert!(json.contains(&STANDARD.encode(&data)));
        assert!(!json.contains("[1,2,3,4,5]"));
    }

    #[test]
    fn test_content_with_file_uri() {
        let content = Content::new("user")
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

        #[allow(deprecated)]
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
    fn test_try_inline_data() {
        let part = Part::try_inline_data("image/png", vec![1, 2, 3]).unwrap();
        assert!(part.is_media());

        let large_data = vec![0u8; MAX_INLINE_DATA_SIZE + 1];
        let result = Part::try_inline_data("image/png", large_data);
        assert!(result.is_err());
    }

    #[test]
    fn test_inline_data_within_limit() {
        // Should succeed: small data
        let data = vec![0u8; 1024];
        #[allow(deprecated)]
        let content = Content::new("user").with_inline_data("image/png", data);
        assert_eq!(content.parts.len(), 1);
    }

    #[test]
    fn test_inline_data_at_limit() {
        // Should succeed: exactly at limit
        let data = vec![0u8; MAX_INLINE_DATA_SIZE];
        #[allow(deprecated)]
        let part = Part::inline_data("image/png", data);
        assert!(part.is_media());
    }

    #[test]
    #[should_panic(expected = "exceeds maximum allowed size")]
    fn test_inline_data_exceeds_limit_content() {
        let data = vec![0u8; MAX_INLINE_DATA_SIZE + 1];
        #[allow(deprecated)]
        let _ = Content::new("user").with_inline_data("image/png", data);
    }

    #[test]
    #[should_panic(expected = "exceeds maximum allowed size")]
    fn test_inline_data_exceeds_limit_part() {
        let data = vec![0u8; MAX_INLINE_DATA_SIZE + 1];
        #[allow(deprecated)]
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
        let content = Content::new("model").with_thinking("Let me reason about this");
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
