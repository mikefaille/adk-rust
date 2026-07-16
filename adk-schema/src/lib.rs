//! # adk-schema
//!
//! Canonical JSON Schema documents and validation for ADK.

#![deny(missing_docs)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;
use thiserror::Error;

#[cfg(feature = "runtime-validation")]
use std::sync::Arc;

/// Errors that can occur during schema operations.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// Serialization error.
    #[error("serialization error: {0}")]
    Serialization(String),
    /// Validation error.
    #[error("validation error: {0}")]
    Validation(String),
    /// Ingestion error at {pointer}: {message} (limit: {limit})
    #[error("ingestion error at {pointer}: {message} (limit: {limit})")]
    Ingestion {
        /// JSON Pointer to the location of the error.
        pointer: String,
        /// Description of the error.
        message: String,
        /// The limit that was exceeded (if applicable).
        limit: usize,
    },
}

impl From<serde_json::Error> for SchemaError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialization(e.to_string())
    }
}

/// Result type for schema operations.
pub type Result<T> = std::result::Result<T, SchemaError>;

/// Supported JSON Schema dialects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash, Default)]
#[non_exhaustive]
pub enum JsonSchemaDialect {
    /// Draft 2020-12
    #[serde(rename = "https://json-schema.org/draft/2020-12/schema")]
    #[default]
    Draft202012,
}

impl fmt::Display for JsonSchemaDialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Draft202012 => write!(f, "draft202012"),
        }
    }
}

/// The direction of the schema (input or output).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
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
    /// Create a new schema document without additional validation.
    /// Internal use mostly; public ingestion should use `try_from_*` variants.
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
        // Version 3 of the digest algorithm
        // Domain separation: version, dialect, direction, content
        hasher.update([4u8]);
        hasher.update(self.dialect.to_string().as_bytes());
        hasher.update(self.direction.to_string().as_bytes());
        hasher.update(&canonical_bytes);
        SchemaDigest(hasher.finalize().into())
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
            return Err(SchemaError::Ingestion {
                pointer: "".to_string(),
                message: format!("schema size {} exceeds limit", bytes.len()),
                limit: limits.max_bytes,
            });
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

    /// Convenience: Create a new schema document from a raw Value for input.
    /// Does NOT perform bounded ingestion checks.
    pub fn for_input(document: Value) -> Self {
        Self::new(document, JsonSchemaDialect::Draft202012, SchemaDirection::Input)
    }

    /// Convenience: Create a new schema document from a raw Value for output.
    /// Does NOT perform bounded ingestion checks.
    pub fn for_output(document: Value) -> Self {
        Self::new(document, JsonSchemaDialect::Draft202012, SchemaDirection::Output)
    }
}

fn make_canonical(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut sorted_map = serde_json::Map::with_capacity(map.len());
            let mut keys: Vec<_> = map.keys().cloned().collect();
            keys.sort();
            for key in keys {
                let val = map.get(&key).unwrap().clone();
                sorted_map.insert(key, make_canonical(val));
            }
            Value::Object(sorted_map)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(make_canonical).collect()),
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
    let mut stack = vec![(v, 0, String::new())];
    let mut node_count = 0;

    while let Some((current, depth, pointer)) = stack.pop() {
        node_count += 1;
        if node_count > limits.max_nodes {
            return Err(SchemaError::Ingestion {
                pointer,
                message: "schema node count exceeds limit".to_string(),
                limit: limits.max_nodes,
            });
        }

        if depth > limits.max_depth {
            return Err(SchemaError::Ingestion {
                pointer,
                message: "schema nesting depth exceeds limit".to_string(),
                limit: limits.max_depth,
            });
        }

        match current {
            Value::Object(map) => {
                for (key, val) in map {
                    let next_pointer = if pointer.is_empty() {
                        format!("/{}", key)
                    } else {
                        format!("{}/{}", pointer, key)
                    };

                    if key == "$ref" {
                        if let Some(s) = val.as_str() {
                            if !s.starts_with('#') {
                                return Err(SchemaError::Ingestion {
                                    pointer: next_pointer,
                                    message: format!("external references are forbidden: {}", s),
                                    limit: 0,
                                });
                            }
                            if !is_valid_local_ref(v, s) {
                                return Err(SchemaError::Ingestion {
                                    pointer: next_pointer,
                                    message: format!("invalid or missing local reference: {}", s),
                                    limit: 0,
                                });
                            }
                        }
                    } else if key == "$dynamicRef" {
                        return Err(SchemaError::Ingestion {
                            pointer: next_pointer,
                            message: "unsupported keyword: $dynamicRef".to_string(),
                            limit: 0,
                        });
                    }
                    stack.push((val, depth + 1, next_pointer));
                }
            }
            Value::Array(arr) => {
                for (i, val) in arr.iter().enumerate() {
                    let next_pointer = format!("{}/{}", pointer, i);
                    stack.push((val, depth + 1, next_pointer));
                }
            }
            _ => {}
        }
    }

    Ok(())
}

fn is_valid_local_ref(root: &Value, ref_str: &str) -> bool {
    if !ref_str.starts_with('#') {
        return false;
    }
    if ref_str == "#" {
        return true;
    }
    let pointer = &ref_str[1..];
    root.pointer(pointer).is_some()
}

/// A validated JSON Schema document.
#[cfg(feature = "runtime-validation")]
#[derive(Clone)]
pub struct ValidatedSchemaDocument {
    doc: SchemaDocument,
    validator: Arc<jsonschema::Validator>,
}

#[cfg(feature = "runtime-validation")]
impl fmt::Debug for ValidatedSchemaDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidatedSchemaDocument")
            .field("doc", &self.doc)
            .finish()
    }
}

#[cfg(feature = "runtime-validation")]
impl PartialEq for ValidatedSchemaDocument {
    fn eq(&self, other: &Self) -> bool {
        self.doc == other.doc
    }
}

#[cfg(feature = "runtime-validation")]
impl ValidatedSchemaDocument {
    /// Create a new validated schema document.
    pub fn try_new(doc: SchemaDocument) -> Result<Self> {
        use jsonschema::{Draft, Validator};
        let schema_node = doc.document();

        // Meta-schema validation: Validate the schema document itself against Draft 2020-12
        let validator = Validator::options()
            .with_draft(Draft::Draft202012)
            .build(schema_node)
            .map_err(|e| SchemaError::Validation(format!("invalid schema document: {}", e)))?;

        Ok(Self {
            doc,
            validator: Arc::new(validator),
        })
    }

    /// Access the underlying schema document.
    pub fn as_document(&self) -> &SchemaDocument {
        &self.doc
    }

    /// Access the canonical document.
    pub fn document(&self) -> &Value {
        self.doc.document()
    }

    /// Generate a deterministic digest for this schema document.
    pub fn digest(&self) -> SchemaDigest {
        self.doc.digest()
    }

    /// Validate a JSON value against this schema.
    pub fn validate(&self, instance: &Value) -> Result<()> {
        let mut errors = self.validator.iter_errors(instance);
        if let Some(error) = errors.next() {
            return Err(SchemaError::Validation(error.to_string()));
        }
        Ok(())
    }
}

/// A versioned SHA-256 digest of a schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SchemaDigest([u8; 32]);

impl fmt::Display for SchemaDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Static schema generation.
#[cfg(feature = "schemars")]
pub mod static_schema {
    use super::*;
    use schemars::{
        JsonSchema,
        generate::{SchemaGenerator, SchemaSettings},
    };

    /// Generate a schema document for deserialization (input).
    pub fn for_deserialize<T: JsonSchema>() -> Result<SchemaDocument> {
        generate_schema::<T>(SchemaDirection::Input)
    }

    /// Generate a schema document for serialization (output).
    pub fn for_serialize<T: JsonSchema>() -> Result<SchemaDocument> {
        generate_schema::<T>(SchemaDirection::Output)
    }

    fn generate_schema<T: JsonSchema>(direction: SchemaDirection) -> Result<SchemaDocument> {
        let mut settings = SchemaSettings::draft2020_12().with(|s| {
            s.inline_subschemas = false;
        });

        settings = match direction {
            SchemaDirection::Input => settings.for_deserialize(),
            SchemaDirection::Output => settings.for_serialize(),
        };

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
        let limits = IngestionLimits { max_bytes: 1000, max_depth: 2, max_nodes: 1000 };
        let res = SchemaDocument::try_from_value(
            deep,
            JsonSchemaDialect::Draft202012,
            SchemaDirection::Input,
            limits,
        );
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
        let res = SchemaDocument::try_from_value(
            external_ref,
            JsonSchemaDialect::Draft202012,
            SchemaDirection::Input,
            limits,
        );
        assert!(res.is_err());
    }

    #[test]
    fn test_local_ref_validation() {
        let valid = json!({
            "$defs": {
                "foo": { "type": "string" }
            },
            "properties": {
                "bar": { "$ref": "#/$defs/foo" }
            }
        });
        let limits = IngestionLimits::default();
        let res = SchemaDocument::try_from_value(
            valid,
            JsonSchemaDialect::Draft202012,
            SchemaDirection::Input,
            limits,
        );
        assert!(res.is_ok());

        let invalid = json!({
            "properties": {
                "bar": { "$ref": "#/$defs/missing" }
            }
        });
        let res2 = SchemaDocument::try_from_value(
            invalid,
            JsonSchemaDialect::Draft202012,
            SchemaDirection::Input,
            limits,
        );
        assert!(res2.is_err());
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
