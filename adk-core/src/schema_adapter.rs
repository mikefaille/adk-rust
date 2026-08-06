//! Schema normalization adapter for LLM provider function-calling APIs.
use crate::schema_utils;
use serde_json::Value;
use std::borrow::Cow;
use std::fmt;

/// Error returned when a tool schema cannot be compiled for a specific provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaCompileError {
    /// Human-readable error message explaining why compilation failed.
    pub message: String,
}

impl fmt::Display for SchemaCompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Schema compile error: {}", self.message)
    }
}

impl std::error::Error for SchemaCompileError {}

impl SchemaCompileError {
    /// Create a new schema compilation error.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

/// Normalizes JSON Schema for a specific LLM provider's function-calling API.
pub trait SchemaAdapter: Send + Sync + std::fmt::Debug {
    /// Returns a unique identifier for this adapter (e.g., "gemini", "openai").
    fn identifier(&self) -> &str {
        "generic"
    }

    /// Returns the version of this adapter (e.g., "1.0.0").
    fn version(&self) -> &str {
        "1.0.0"
    }

    /// Returns the target surface (e.g., "studio", "vertex") if applicable.
    fn surface(&self) -> Option<&str> {
        None
    }

    /// Name of the function-declaration field that carries the parameter schema.
    ///
    /// A provider that offers more than one schema dialect usually offers them
    /// under *different field names*, and the fields are mutually exclusive —
    /// Gemini's `parameters` (an OpenAPI subset) and `parametersJsonSchema`
    /// (JSON Schema) are the worked example. Sending a dialect under the wrong
    /// name is not a degraded request: the Live socket closes with WS 1007
    /// before any turn happens.
    ///
    /// Returning the field name from the adapter keeps the pair — "what did I
    /// reduce to?" and "what do I call it on the wire?" — in one place, so a
    /// caller cannot select a dialect and then post it under the other name.
    /// The default is the long-standing `parameters`, so existing adapters are
    /// unaffected.
    fn parameters_field(&self) -> &'static str {
        "parameters"
    }

    /// Normalize a raw JSON Schema for this provider (infallible).
    fn normalize_schema(&self, schema: Value) -> Value;

    /// Compiles a raw JSON Schema for this provider, returning an error if unsupported.
    fn compile_schema(&self, schema: &Value) -> Result<Value, SchemaCompileError> {
        Ok(self.normalize_schema(schema.clone()))
    }

    /// Validates a tool name for this provider's limits.
    ///
    /// The default implementation accepts all names. Providers with specific
    /// byte limits (e.g., Gemini's 64-byte limit) should override this to return
    /// a `SchemaCompileError`.
    fn validate_tool_name(&self, _name: &str) -> Result<(), SchemaCompileError> {
        Ok(())
    }

    /// Normalize a tool name for this provider's limits.
    fn normalize_tool_name<'a>(&self, name: &'a str) -> Cow<'a, str> {
        if name.len() <= 64 {
            Cow::Borrowed(name)
        } else {
            let mut end = 64;
            while end > 0 && !name.is_char_boundary(end) {
                end -= 1;
            }
            Cow::Owned(name[..end].to_string())
        }
    }

    /// Fallback schema when a tool provides no parameters.
    fn empty_schema(&self) -> Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
}

/// Default schema adapter for providers with no specific requirements.
#[derive(Debug)]
pub struct GenericSchemaAdapter;

const GENERIC_ALLOWED_FORMATS: &[&str] =
    &["date-time", "date", "time", "email", "uri", "uuid", "int32", "int64", "float", "double"];

impl SchemaAdapter for GenericSchemaAdapter {
    fn normalize_schema(&self, mut schema: Value) -> Value {
        schema_utils::strip_schema_keyword(&mut schema);
        schema_utils::strip_conditional_keywords(&mut schema);
        schema_utils::convert_const_to_enum(&mut schema);
        schema_utils::add_implicit_object_type(&mut schema);
        schema_utils::strip_unsupported_formats(&mut schema, GENERIC_ALLOWED_FORMATS);
        schema
    }
}

/// Schema adapter for Moonshot / Kimi OpenAI-compatible API endpoints.
#[derive(Debug)]
pub struct KimiSchemaAdapter;

impl SchemaAdapter for KimiSchemaAdapter {
    fn identifier(&self) -> &str {
        "kimi"
    }

    fn validate_tool_name(&self, name: &str) -> Result<(), SchemaCompileError> {
        if name.len() < 3 || name.len() > 64 {
            return Err(SchemaCompileError::new(format!(
                "Kimi tool name '{}' length {} is outside allowed range [3, 64]",
                name,
                name.len()
            )));
        }
        let mut chars = name.chars();
        if let Some(first) = chars.next()
            && !first.is_ascii_alphabetic()
            && first != '_'
        {
            return Err(SchemaCompileError::new(format!(
                "Kimi tool name '{}' must start with an ASCII letter or underscore",
                name
            )));
        }
        if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(SchemaCompileError::new(format!(
                "Kimi tool name '{}' contains invalid characters (must be alphanumeric, '_' or '-')",
                name
            )));
        }
        Ok(())
    }

    fn compile_schema(&self, schema: &Value) -> Result<Value, SchemaCompileError> {
        if !schema.is_object() {
            return Err(SchemaCompileError::new(
                "Kimi tool parameter schema must be a JSON object",
            ));
        }

        // Validate that schema does not contain unsupported keywords for Kimi
        validate_kimi_supported_keywords(schema)?;

        let mut normalized = self.normalize_schema(schema.clone());
        if let Some(obj) = normalized.as_object_mut()
            && !obj.contains_key("type")
        {
            obj.insert("type".to_string(), Value::String("object".to_string()));
        }
        Ok(normalized)
    }

    fn normalize_schema(&self, mut schema: Value) -> Value {
        schema_utils::strip_schema_keyword(&mut schema);
        schema_utils::strip_conditional_keywords(&mut schema);
        schema_utils::strip_unsupported_formats(&mut schema, GENERIC_ALLOWED_FORMATS);
        schema
    }
}

fn validate_kimi_supported_keywords(val: &Value) -> Result<(), SchemaCompileError> {
    if let Some(obj) = val.as_object() {
        for (key, child) in obj {
            match key.as_str() {
                "$ref" | "oneOf" | "anyOf" | "allOf" | "not" | "if" | "then" | "else" => {
                    return Err(SchemaCompileError::new(format!(
                        "Kimi function calling schema does not support keyword '{key}'"
                    )));
                }
                "properties" => {
                    if let Some(props) = child.as_object() {
                        for prop_val in props.values() {
                            validate_kimi_supported_keywords(prop_val)?;
                        }
                    }
                }
                "items" => {
                    validate_kimi_supported_keywords(child)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
