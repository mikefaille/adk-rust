//! # adk-schema
//!
//! Canonical JSON Schema documents and validation for ADK.

#![deny(missing_docs)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;
use thiserror::Error;

/// Errors that can occur during schema operations.
#[derive(Debug, Error)]
pub enum SchemaError {
    /// Serialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// Validation error.
    #[error("validation error: {0}")]
    Validation(String),
    /// Ingestion error.
    #[error("ingestion error: {0}")]
    Ingestion(String),
}

/// Result type for schema operations.
pub type Result<T> = std::result::Result<T, SchemaError>;

/// Supported JSON Schema dialects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum JsonSchemaDialect {
    /// Draft 2020-12
    #[serde(rename = "https://json-schema.org/draft/2020-12/schema")]
    Draft202012,
}

impl Default for JsonSchemaDialect {
    fn default() -> Self {
        Self::Draft202012
    }
}

impl fmt::Display for JsonSchemaDialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Draft202012 => write!(f, "draft202012"),
        }
    }
}

/// The direction of the schema (input or output).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaDirection {
    /// Schema for tool inputs (deserialization).
    Input,
    /// Schema for tool outputs (serialization).
    Output,
}

impl fmt::Display for SchemaDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input => write!(f, "input"),
            Self::Output => write!(f, "output"),
        }
    }
}

/// A canonical JSON Schema document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaDocument {
    dialect: JsonSchemaDialect,
    direction: SchemaDirection,
    document: Value,
}

impl SchemaDocument {
    /// Create a new schema document.
    pub fn new(document: Value, dialect: JsonSchemaDialect, direction: SchemaDirection) -> Self {
        Self { document, dialect, direction }
    }

    /// Access the canonical document.
    pub fn document(&self) -> &Value {
        &self.document
    }

    /// Access the dialect.
    pub fn dialect(&self) -> JsonSchemaDialect {
        self.dialect
    }

    /// Access the direction.
    pub fn direction(&self) -> SchemaDirection {
        self.direction
    }

    /// Generate a deterministic digest for this schema document.
    pub fn digest(&self) -> SchemaDigest {
        let canonical_bytes = self.to_canonical_bytes();
        let mut hasher = Sha256::new();
        // Version 1 of the digest algorithm
        hasher.update([1u8]);
        hasher.update(self.dialect.to_string().as_bytes());
        hasher.update(self.direction.to_string().as_bytes());
        hasher.update(&canonical_bytes);
        SchemaDigest(hex::encode(hasher.finalize()))
    }

    /// Returns deterministic canonical bytes for the document.
    /// Object keys are sorted, array order is preserved.
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let canonical_value = make_canonical(self.document.clone());
        serde_json::to_vec(&canonical_value).expect("canonicalization should never fail to serialize")
    }

    /// Ingest a schema from a byte slice with bounds.
    pub fn try_from_bytes(
        bytes: &[u8],
        dialect: JsonSchemaDialect,
        direction: SchemaDirection,
        limits: IngestionLimits,
    ) -> Result<Self> {
        if bytes.len() > limits.max_bytes {
            return Err(SchemaError::Ingestion(format!(
                "schema size {} exceeds limit of {}",
                bytes.len(),
                limits.max_bytes
            )));
        }

        let value: Value = serde_json::from_slice(bytes)?;
        Self::try_from_value(value, dialect, direction, limits)
    }

    /// Ingest a schema from a `serde_json::Value` with bounds.
    pub fn try_from_value(
        value: Value,
        dialect: JsonSchemaDialect,
        direction: SchemaDirection,
        limits: IngestionLimits,
    ) -> Result<Self> {
        validate_bounds_iterative(&value, &limits)?;
        Ok(Self::new(value, dialect, direction))
    }

    /// Create a new schema document from a raw Value for input.
    pub fn for_input(document: Value) -> Self {
        Self::new(document, JsonSchemaDialect::Draft202012, SchemaDirection::Input)
    }

    /// Create a new schema document from a raw Value for output.
    pub fn for_output(document: Value) -> Self {
        Self::new(document, JsonSchemaDialect::Draft202012, SchemaDirection::Output)
    }
}

fn make_canonical(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut sorted_map = serde_json::Map::with_capacity(map.len());
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            for key in keys {
                let val = map.get(key).unwrap().clone();
                sorted_map.insert(key.clone(), make_canonical(val));
            }
            Value::Object(sorted_map)
        }
        Value::Array(arr) => {
            Value::Array(arr.into_iter().map(make_canonical).collect())
        }
        _ => v,
    }
}

/// Limits for schema ingestion to prevent resource exhaustion.
#[derive(Debug, Clone, Copy)]
pub struct IngestionLimits {
    /// Maximum size of the schema in bytes.
    pub max_bytes: usize,
    /// Maximum nesting depth.
    pub max_depth: usize,
    /// Maximum total number of nodes (keys, array elements).
    pub max_nodes: usize,
}

impl Default for IngestionLimits {
    fn default() -> Self {
        Self {
            max_bytes: 1024 * 1024, // 1MB
            max_depth: 32,
            max_nodes: 5000,
        }
    }
}

fn validate_bounds_iterative(v: &Value, limits: &IngestionLimits) -> Result<()> {
    let mut stack = vec![(v, 0)];
    let mut node_count = 0;

    while let Some((current, depth)) = stack.pop() {
        node_count += 1;
        if node_count > limits.max_nodes {
            return Err(SchemaError::Ingestion(format!(
                "schema node count exceeds limit of {}",
                limits.max_nodes
            )));
        }

        if depth > limits.max_depth {
            return Err(SchemaError::Ingestion(format!(
                "schema nesting depth exceeds limit of {}",
                limits.max_depth
            )));
        }

        match current {
            Value::Object(map) => {
                for (key, val) in map {
                    if key == "$ref" {
                        if let Some(s) = val.as_str() {
                            if !s.starts_with('#') {
                                return Err(SchemaError::Ingestion(format!(
                                    "external references are forbidden: {}",
                                    s
                                )));
                            }
                        }
                    }
                    stack.push((val, depth + 1));
                }
            }
            Value::Array(arr) => {
                for val in arr {
                    stack.push((val, depth + 1));
                }
            }
            _ => {}
        }
    }

    Ok(())
}

/// A validated JSON Schema document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidatedSchemaDocument(SchemaDocument);

impl ValidatedSchemaDocument {
    /// Create a new validated schema document.
    ///
    /// # Errors
    ///
    /// If `runtime-validation` is enabled, this will check against the meta-schema.
    pub fn try_new(doc: SchemaDocument) -> Result<Self> {
        #[cfg(feature = "runtime-validation")]
        {
            use jsonschema::{Validator, Draft};
            let schema_node = doc.document();

            // Build validator with Draft 2020-12 specifically
            let _validator = Validator::options()
                .with_draft(Draft::Draft202012)
                .build(schema_node)
                .map_err(|e| SchemaError::Validation(format!("invalid schema document: {}", e)))?;
        }
        Ok(Self(doc))
    }

    /// Access the underlying schema document.
    pub fn as_document(&self) -> &SchemaDocument {
        &self.0
    }

    /// Access the canonical document.
    pub fn document(&self) -> &Value {
        self.0.document()
    }

    /// Generate a deterministic digest for this schema document.
    pub fn digest(&self) -> SchemaDigest {
        self.0.digest()
    }

    /// Validate a JSON value against this schema.
    #[cfg(feature = "runtime-validation")]
    pub fn validate(&self, instance: &Value) -> Result<()> {
        use jsonschema::{Validator, Draft};
        let validator = Validator::options()
            .with_draft(Draft::Draft202012)
            .build(self.document())
            .map_err(|e| SchemaError::Validation(format!("failed to compile validator: {}", e)))?;

        let mut errors = validator.iter_errors(instance);
        if let Some(error) = errors.next() {
            return Err(SchemaError::Validation(error.to_string()));
        }
        Ok(())
    }
}

/// A versioned SHA-256 digest of a schema.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SchemaDigest(String);

impl fmt::Display for SchemaDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Static schema generation.
#[cfg(feature = "schemars")]
pub mod static_schema {
    use super::*;
    use schemars::{JsonSchema, generate::{SchemaGenerator, SchemaSettings}};

    /// Generate a schema document for deserialization (input).
    pub fn for_deserialize<T: JsonSchema>() -> Result<SchemaDocument> {
        generate_schema::<T>(SchemaDirection::Input)
    }

    /// Generate a schema document for serialization (output).
    pub fn for_serialize<T: JsonSchema>() -> Result<SchemaDocument> {
        generate_schema::<T>(SchemaDirection::Output)
    }

    fn generate_schema<T: JsonSchema>(direction: SchemaDirection) -> Result<SchemaDocument> {
        let settings = SchemaSettings::draft2020_12().with(|s| {
            s.inline_subschemas = false;
        });
        let generator = SchemaGenerator::new(settings);
        let root = generator.into_root_schema_for::<T>();

        let mut value = serde_json::to_value(root)?;

        // Remove titles and $schema to keep it clean
        if let Some(obj) = value.as_object_mut() {
            obj.remove("$schema");
            if let Some(metadata) = obj.get_mut("metadata").and_then(|m| m.as_object_mut()) {
                metadata.remove("title");
            }
        }

        Ok(SchemaDocument::new(value, JsonSchemaDialect::Draft202012, direction))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_canonicalization_keys_sorted() {
        let v1 = json!({
            "b": 2,
            "a": 1,
            "c": {
                "y": 2,
                "x": 1
            }
        });
        let v2 = json!({
            "a": 1,
            "b": 2,
            "c": {
                "x": 1,
                "y": 2
            }
        });

        let doc1 = SchemaDocument::new(v1, JsonSchemaDialect::Draft202012, SchemaDirection::Input);
        let doc2 = SchemaDocument::new(v2, JsonSchemaDialect::Draft202012, SchemaDirection::Input);

        assert_eq!(doc1.to_canonical_bytes(), doc2.to_canonical_bytes());
        assert_eq!(doc1.digest(), doc2.digest());
    }

    #[test]
    fn test_canonicalization_array_order_preserved() {
        let v1 = json!([1, 2, 3]);
        let v2 = json!([3, 2, 1]);

        let doc1 = SchemaDocument::new(v1, JsonSchemaDialect::Draft202012, SchemaDirection::Input);
        let doc2 = SchemaDocument::new(v2, JsonSchemaDialect::Draft202012, SchemaDirection::Input);

        assert_ne!(doc1.to_canonical_bytes(), doc2.to_canonical_bytes());
        assert_ne!(doc1.digest(), doc2.digest());
    }

    #[test]
    fn test_ingestion_limits_depth() {
        let deep = json!({"a": {"a": {"a": {"a": 1}}}});
        let limits = IngestionLimits {
            max_bytes: 1000,
            max_depth: 2,
            max_nodes: 1000,
        };
        let res = SchemaDocument::try_from_value(deep, JsonSchemaDialect::Draft202012, SchemaDirection::Input, limits);
        assert!(res.is_err());
    }

    #[test]
    fn test_ingestion_forbidden_external_ref() {
        let external_ref = json!({
            "properties": {
                "foo": { "$ref": "http://example.com/schema.json" }
            }
        });
        let limits = IngestionLimits::default();
        let res = SchemaDocument::try_from_value(external_ref, JsonSchemaDialect::Draft202012, SchemaDirection::Input, limits);
        assert!(res.is_err());
    }

    #[cfg(feature = "schemars")]
    #[test]
    fn test_static_schema_generation() {
        use schemars::JsonSchema;
        #[derive(JsonSchema)]
        struct MyStruct {
            #[allow(dead_code)]
            field: String,
        }

        let doc = static_schema::for_deserialize::<MyStruct>().unwrap();
        assert_eq!(doc.direction(), SchemaDirection::Input);
        assert!(doc.document().as_object().unwrap().contains_key("properties"));
    }
}
