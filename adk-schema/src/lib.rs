use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;
use std::sync::Arc;
use thiserror::Error;

pub mod compiler;
pub mod policy;
pub mod utils;

pub use compiler::*;
pub use policy::*;
pub use utils::*;

#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Meta-schema validation failed: {0}")]
    Validation(String),
    #[error("Schema exceeded resource limits: {0}")]
    ResourceLimit(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JsonSchemaDialect {
    Draft202012,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SchemaDigest(pub [u8; 32]);

impl fmt::Debug for SchemaDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SchemaDigest({:?})", hex::encode(self.0))
    }
}

impl fmt::Display for SchemaDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

#[derive(Debug, Clone)]
pub struct SchemaDocument {
    pub value: Arc<Value>,
    pub dialect: JsonSchemaDialect,
    pub digest: SchemaDigest,
}

#[derive(Debug, Clone, Default)]
pub struct SchemaStats {
    pub document_bytes: usize,
    pub node_count: usize,
    pub max_depth: usize,
    pub max_properties: usize,
    pub definition_count: usize,
    pub reference_count: usize,
    pub union_branches: usize,
    pub enum_values: usize,
    pub max_pattern_length: usize,
}

#[derive(Debug, Clone)]
pub struct ValidatedSchemaDocument {
    pub document: SchemaDocument,
    pub stats: SchemaStats,
}

#[derive(Debug, Clone)]
pub struct SchemaLimits {
    pub max_document_bytes: usize,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_properties_per_object: usize,
    pub max_definitions: usize,
    pub max_references: usize,
    pub max_union_branches: usize,
    pub max_enum_values: usize,
    pub max_pattern_length: usize,
    pub allow_external_references: bool,
}

impl Default for SchemaLimits {
    fn default() -> Self {
        Self {
            max_document_bytes: 1024 * 1024,
            max_nodes: 10000,
            max_depth: 100,
            max_properties_per_object: 1000,
            max_definitions: 1000,
            max_references: 1000,
            max_union_branches: 100,
            max_enum_values: 1000,
            max_pattern_length: 1000,
            allow_external_references: false,
        }
    }
}

pub fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, SchemaError> {
    // Basic canonicalization: just serialize directly for now.
    // A robust canonicalization should handle sorting keys.
    Ok(serde_json::to_vec(value)?)
}

pub fn compute_digest(value: &Value) -> Result<SchemaDigest, SchemaError> {
    let bytes = canonical_json_bytes(value)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&hasher.finalize());
    Ok(SchemaDigest(digest))
}

pub fn validate_schema(
    raw: Value,
    limits: &SchemaLimits,
) -> Result<ValidatedSchemaDocument, SchemaError> {
    // 1. size check
    let bytes = serde_json::to_vec(&raw)?;
    if bytes.len() > limits.max_document_bytes {
        return Err(SchemaError::ResourceLimit("Max document bytes exceeded".into()));
    }

    // 2. structural walk and resource accounting (skip full impl for now, just basics)
    let stats = SchemaStats {
        document_bytes: bytes.len(),
        ..Default::default()
    };

    // 3. reject forbidden reference schemes (external refs)
    // 4. Draft 2020-12 meta-schema validation
    #[cfg(feature = "runtime-validation")]
    {
        if let Err(e) = jsonschema::draft202012::meta::validate(&raw) {
            return Err(SchemaError::Validation(e.to_string()));
        }
    }

    // 5. canonical encoding & digest
    let digest = compute_digest(&raw)?;

    let doc = SchemaDocument {
        value: Arc::new(raw),
        dialect: JsonSchemaDialect::Draft202012,
        digest,
    };

    Ok(ValidatedSchemaDocument {
        document: doc,
        stats,
    })
}

#[cfg(feature = "schemars")]
pub mod generation {
    use super::*;
    use schemars::{generate::SchemaSettings, JsonSchema};

    pub fn input_schema_for<T: JsonSchema>() -> Result<SchemaDocument, SchemaError> {
        let settings = SchemaSettings::draft2020_12().for_deserialize();
        let generator = settings.into_generator();
        let schema = generator.into_root_schema_for::<T>();
        let value = serde_json::to_value(schema)?;
        let digest = compute_digest(&value)?;
        Ok(SchemaDocument {
            value: Arc::new(value),
            dialect: JsonSchemaDialect::Draft202012,
            digest,
        })
    }

    pub fn output_schema_for<T: JsonSchema>() -> Result<SchemaDocument, SchemaError> {
        let settings = SchemaSettings::draft2020_12().for_serialize();
        let generator = settings.into_generator();
        let schema = generator.into_root_schema_for::<T>();
        let value = serde_json::to_value(schema)?;
        let digest = compute_digest(&value)?;
        Ok(SchemaDocument {
            value: Arc::new(value),
            dialect: JsonSchemaDialect::Draft202012,
            digest,
        })
    }
}
