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
        // Version 5 of the digest algorithm with stable length-prefixed framing.
        // Domain separation: version, len(dialect) + dialect, len(direction) + direction, len(content) + content.
        hasher.update([5u8]);

        let dialect_str = self.dialect.to_string();
        hasher.update((dialect_str.len() as u64).to_be_bytes());
        hasher.update(dialect_str.as_bytes());

        let direction_str = self.direction.to_string();
        hasher.update((direction_str.len() as u64).to_be_bytes());
        hasher.update(direction_str.as_bytes());

        hasher.update((canonical_bytes.len() as u64).to_be_bytes());
        hasher.update(&canonical_bytes);
        SchemaDigest(hasher.finalize().into())
    }

    /// Returns deterministic canonical bytes for the document.
    /// Object keys are sorted, array order is preserved.
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let canonical_value = make_canonical(self.document.clone());
        serde_json::to_vec(&canonical_value)
            .expect("canonicalization should never fail to serialize")
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

struct StackFrame<'a> {
    value: &'a Value,
    ref_path: Vec<String>,
    pointer: String,
    depth: usize,
}

fn validate_bounds_iterative(v: &Value, limits: &IngestionLimits) -> Result<()> {
    // Check pre-parsed value byte size by serializing to JSON bytes
    let serialized_len = serde_json::to_vec(v)?.len();
    if serialized_len > limits.max_bytes {
        return Err(SchemaError::Ingestion {
            pointer: "".to_string(),
            message: format!("schema size {} exceeds limit", serialized_len),
            limit: limits.max_bytes,
        });
    }

    let mut stack =
        vec![StackFrame { value: v, ref_path: Vec::new(), pointer: String::new(), depth: 0 }];
    let mut node_count = 0;

    while let Some(frame) = stack.pop() {
        node_count += 1;
        if node_count > limits.max_nodes {
            return Err(SchemaError::Ingestion {
                pointer: frame.pointer,
                message: "schema node count exceeds limit".to_string(),
                limit: limits.max_nodes,
            });
        }

        if frame.depth > limits.max_depth {
            return Err(SchemaError::Ingestion {
                pointer: frame.pointer,
                message: "schema nesting depth exceeds limit".to_string(),
                limit: limits.max_depth,
            });
        }

        match frame.value {
            Value::Object(map) => {
                for (key, val) in map {
                    let next_pointer = if frame.pointer.is_empty() {
                        format!("/{}", key)
                    } else {
                        format!("{}/{}", frame.pointer, key)
                    };

                    if key == "$anchor" || key == "$dynamicAnchor" {
                        return Err(SchemaError::Ingestion {
                            pointer: next_pointer.clone(),
                            message: format!("anchor keywords are forbidden: {}", key),
                            limit: 0,
                        });
                    }

                    if key == "$ref" {
                        if let Some(s) = val.as_str() {
                            if !s.starts_with('#') {
                                return Err(SchemaError::Ingestion {
                                    pointer: next_pointer.clone(),
                                    message: format!("external references are forbidden: {}", s),
                                    limit: 0,
                                });
                            }
                            // Explicit anchor policy: only allow empty ref (#) or JSON Pointer refs (#/)
                            if s != "#" && !s.starts_with("#/") {
                                return Err(SchemaError::Ingestion {
                                    pointer: next_pointer.clone(),
                                    message: format!("named anchors are forbidden: {}", s),
                                    limit: 0,
                                });
                            }
                            if !is_valid_local_ref(v, s) {
                                return Err(SchemaError::Ingestion {
                                    pointer: next_pointer.clone(),
                                    message: format!("invalid or missing local reference: {}", s),
                                    limit: 0,
                                });
                            }
                            // Cycle check:
                            if frame.ref_path.contains(&s.to_string()) {
                                return Err(SchemaError::Ingestion {
                                    pointer: next_pointer.clone(),
                                    message: format!("cyclic reference detected: {}", s),
                                    limit: 0,
                                });
                            }
                            // Resolve target and push to stack with updated ref_path
                            let target = resolve_local_ref(v, s)?;
                            let mut new_ref_path = frame.ref_path.clone();
                            new_ref_path.push(s.to_string());
                            stack.push(StackFrame {
                                value: target,
                                ref_path: new_ref_path,
                                pointer: next_pointer.clone(),
                                depth: frame.depth + 1,
                            });
                        }
                    } else if key == "$dynamicRef" {
                        return Err(SchemaError::Ingestion {
                            pointer: next_pointer,
                            message: "unsupported keyword: $dynamicRef".to_string(),
                            limit: 0,
                        });
                    } else {
                        stack.push(StackFrame {
                            value: val,
                            ref_path: frame.ref_path.clone(),
                            pointer: next_pointer,
                            depth: frame.depth + 1,
                        });
                    }
                }
            }
            Value::Array(arr) => {
                for (i, val) in arr.iter().enumerate() {
                    let next_pointer = format!("{}/{}", frame.pointer, i);
                    stack.push(StackFrame {
                        value: val,
                        ref_path: frame.ref_path.clone(),
                        pointer: next_pointer,
                        depth: frame.depth + 1,
                    });
                }
            }
            _ => {}
        }
    }

    Ok(())
}

fn is_valid_local_ref(root: &Value, ref_str: &str) -> bool {
    if ref_str == "#" {
        return true;
    }
    if !ref_str.starts_with("#/") {
        return false;
    }
    let pointer = &ref_str[1..];
    root.pointer(pointer).is_some()
}

fn resolve_local_ref<'a>(root: &'a Value, ref_str: &str) -> Result<&'a Value> {
    if ref_str == "#" {
        return Ok(root);
    }
    let pointer = &ref_str[1..];
    root.pointer(pointer).ok_or_else(|| SchemaError::Ingestion {
        pointer: pointer.to_string(),
        message: format!("missing local reference target: {}", ref_str),
        limit: 0,
    })
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
        f.debug_struct("ValidatedSchemaDocument").field("doc", &self.doc).finish()
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
        Self::try_new_with_limits(doc, IngestionLimits::default())
    }

    /// Create a new validated schema document with custom limits.
    pub fn try_new_with_limits(doc: SchemaDocument, limits: IngestionLimits) -> Result<Self> {
        // Enforce the ingestion limits bounds check (depth, nodes, cycles, bytes).
        // This ensures no unchecked paths can skip the validation.
        validate_bounds_iterative(doc.document(), &limits)?;

        use jsonschema::{Draft, Validator};
        let schema_node = doc.document();

        // Meta-schema validation: Validate the schema document itself against Draft 2020-12
        let validator = Validator::options()
            .with_draft(Draft::Draft202012)
            .build(schema_node)
            .map_err(|e| SchemaError::Validation(format!("invalid schema document: {}", e)))?;

        Ok(Self { doc, validator: Arc::new(validator) })
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
            obj.remove("title");
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
    fn test_oversized_bytes_fail_before_parsing() {
        let raw_bytes = vec![b'{'; 200];
        let limits = IngestionLimits { max_bytes: 10, max_depth: 5, max_nodes: 100 };
        let res = SchemaDocument::try_from_bytes(
            &raw_bytes,
            JsonSchemaDialect::Draft202012,
            SchemaDirection::Input,
            limits,
        );
        assert!(res.is_err());
        match res.unwrap_err() {
            SchemaError::Ingestion { message, .. } => {
                assert!(message.contains("exceeds limit"));
            }
            _ => panic!("Expected Ingestion error"),
        }
    }

    #[test]
    fn test_oversized_pre_parsed_values_fail() {
        let value = json!({
            "nested": {
                "very": {
                    "deeply": "structure"
                }
            }
        });
        // Set limits so that the serialized JSON representation exceeds the limit.
        let limits = IngestionLimits { max_bytes: 20, max_depth: 10, max_nodes: 100 };
        let res = SchemaDocument::try_from_value(
            value,
            JsonSchemaDialect::Draft202012,
            SchemaDirection::Input,
            limits,
        );
        assert!(res.is_err());
        match res.unwrap_err() {
            SchemaError::Ingestion { message, .. } => {
                assert!(message.contains("exceeds limit"));
            }
            _ => panic!("Expected Ingestion error"),
        }
    }

    #[test]
    fn test_unchecked_constructors_fail_validation_conversion() {
        // SchemaDocument can be created with unchecked constructors:
        let invalid_schema = json!({
            "type": "invalid_type_name_which_fails_meta_schema"
        });
        let doc = SchemaDocument::new(
            invalid_schema,
            JsonSchemaDialect::Draft202012,
            SchemaDirection::Input,
        );

        // But converting it to ValidatedSchemaDocument fails!
        #[cfg(feature = "runtime-validation")]
        {
            let res = ValidatedSchemaDocument::try_new(doc);
            assert!(res.is_err());
        }
    }

    #[test]
    fn test_malformed_schemas_fail_validation() {
        let malformed = json!({
            "properties": {
                "age": {
                    "minimum": "should_be_number_not_string"
                }
            }
        });
        let doc =
            SchemaDocument::new(malformed, JsonSchemaDialect::Draft202012, SchemaDirection::Input);
        #[cfg(feature = "runtime-validation")]
        {
            let res = ValidatedSchemaDocument::try_new(doc);
            assert!(res.is_err());
        }
    }

    #[test]
    fn test_external_and_missing_references_fail() {
        let external_ref = json!({
            "properties": {
                "foo": { "$ref": "http://example.com/schema.json" }
            }
        });
        let res1 = SchemaDocument::try_from_value(
            external_ref,
            JsonSchemaDialect::Draft202012,
            SchemaDirection::Input,
            IngestionLimits::default(),
        );
        assert!(res1.is_err());

        let missing_ref = json!({
            "properties": {
                "foo": { "$ref": "#/$defs/missing" }
            }
        });
        let res2 = SchemaDocument::try_from_value(
            missing_ref,
            JsonSchemaDialect::Draft202012,
            SchemaDirection::Input,
            IngestionLimits::default(),
        );
        assert!(res2.is_err());
    }

    #[test]
    fn test_cyclic_references_fail() {
        let cyclic = json!({
            "$defs": {
                "node": {
                    "properties": {
                        "parent": { "$ref": "#/$defs/node" }
                    }
                }
            },
            "properties": {
                "head": { "$ref": "#/$defs/node" }
            }
        });
        let res = SchemaDocument::try_from_value(
            cyclic,
            JsonSchemaDialect::Draft202012,
            SchemaDirection::Input,
            IngestionLimits::default(),
        );
        assert!(res.is_err());
        match res.unwrap_err() {
            SchemaError::Ingestion { message, .. } => {
                assert!(message.contains("cyclic reference detected"));
            }
            _ => panic!("Expected cycle detection error"),
        }
    }

    #[test]
    fn test_explicit_anchor_policy_fails_named_anchors() {
        let anchor = json!({
            "$defs": {
                "foo": {
                    "$anchor": "foo_anchor",
                    "type": "string"
                }
            },
            "properties": {
                "bar": { "$ref": "#foo_anchor" }
            }
        });
        let res = SchemaDocument::try_from_value(
            anchor,
            JsonSchemaDialect::Draft202012,
            SchemaDirection::Input,
            IngestionLimits::default(),
        );
        assert!(res.is_err());
        match res.unwrap_err() {
            SchemaError::Ingestion { message, .. } => {
                assert!(
                    message.contains("anchor keywords are forbidden")
                        || message.contains("named anchors are forbidden")
                );
            }
            _ => panic!("Expected anchor error"),
        }
    }

    #[test]
    fn test_key_order_digest_insensitive() {
        let v1 = json!({
            "z": 1,
            "y": 2,
            "x": 3
        });
        let v2 = json!({
            "x": 3,
            "y": 2,
            "z": 1
        });
        let doc1 = SchemaDocument::new(v1, JsonSchemaDialect::Draft202012, SchemaDirection::Input);
        let doc2 = SchemaDocument::new(v2, JsonSchemaDialect::Draft202012, SchemaDirection::Input);
        assert_eq!(doc1.digest(), doc2.digest());
    }

    #[test]
    fn test_array_order_digest_sensitive() {
        let v1 = json!([1, 2, 3]);
        let v2 = json!([1, 3, 2]);
        let doc1 = SchemaDocument::new(v1, JsonSchemaDialect::Draft202012, SchemaDirection::Input);
        let doc2 = SchemaDocument::new(v2, JsonSchemaDialect::Draft202012, SchemaDirection::Input);
        assert_ne!(doc1.digest(), doc2.digest());
    }

    #[test]
    fn test_input_and_output_identities_differ() {
        let v = json!({
            "type": "string"
        });
        let doc_in =
            SchemaDocument::new(v.clone(), JsonSchemaDialect::Draft202012, SchemaDirection::Input);
        let doc_out =
            SchemaDocument::new(v, JsonSchemaDialect::Draft202012, SchemaDirection::Output);
        assert_ne!(doc_in.digest(), doc_out.digest());
    }

    #[test]
    #[cfg(feature = "runtime-validation")]
    fn test_validators_compiled_once_and_reused() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            }
        });
        let doc =
            SchemaDocument::new(schema, JsonSchemaDialect::Draft202012, SchemaDirection::Input);
        let validated = ValidatedSchemaDocument::try_new(doc).unwrap();

        let validated_clone = validated.clone();

        // Assert that the validator pointer points to the same compiled jsonschema::Validator
        let ptr1 = Arc::as_ptr(&validated.validator);
        let ptr2 = Arc::as_ptr(&validated_clone.validator);
        assert_eq!(ptr1, ptr2);
    }

    #[cfg(feature = "schemars")]
    #[test]
    fn test_jsonschema_works_without_serialize() {
        use schemars::JsonSchema;

        // A struct that implements JsonSchema but does NOT implement Serialize or Deserialize
        #[derive(JsonSchema)]
        struct NonSerializable {
            #[allow(dead_code)]
            name: String,
        }

        let doc = static_schema::for_deserialize::<NonSerializable>();
        assert!(doc.is_ok());
        let doc = doc.unwrap();

        // Ensure root title and $schema were removed
        let doc_val = doc.document();
        assert!(!doc_val.as_object().unwrap().contains_key("$schema"));
        assert!(!doc_val.as_object().unwrap().contains_key("title"));
    }
}
