use crate::document::JsonSchemaDialect;

/// Limits for resource consumption during schema ingestion.
#[derive(Debug, Clone)]
pub struct IngestionPolicy {
    /// Dialect target.
    pub dialect: JsonSchemaDialect,
    /// Maximum allowed input size in bytes before parsing.
    pub max_source_bytes: usize,
    /// Maximum allowed canonical size in bytes.
    pub max_canonical_bytes: usize,
    /// Maximum allowed nesting depth.
    pub max_depth: usize,
    /// Maximum total node count (properties, array elements, primitives).
    pub max_nodes: usize,
    /// Maximum allowed reference resolutions.
    pub max_references: usize,
    /// Reference resolution and cycle rejection policy.
    pub references: ReferencePolicy,
}

impl Default for IngestionPolicy {
    fn default() -> Self {
        Self {
            dialect: JsonSchemaDialect::Draft202012,
            max_source_bytes: 1024 * 1024,
            max_canonical_bytes: 1024 * 1024,
            max_depth: 32,
            max_nodes: 5000,
            max_references: 500,
            references: ReferencePolicy::LocalJsonPointerAcyclic,
        }
    }
}

/// Supported reference resolution policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferencePolicy {
    /// Strict local JSON Pointer resolution with iterative cycle checks.
    LocalJsonPointerAcyclic,
}

/// Limits applied when validating an instance against a compiled schema.
///
/// [`IngestionPolicy`] bounds the schema; this bounds the instance, which is the
/// side carrying untrusted input from models, users, and remote tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationOptions {
    /// Stops collecting after this many issues.
    ///
    /// [`crate::SchemaError::InvalidInstance`] records that the limit was reached, so a
    /// capped list is distinguishable from a complete one.
    pub max_issues: usize,
    /// Includes the offending instance values in issue messages.
    ///
    /// Defaults to `false`: instances validated here routinely carry
    /// caller-supplied data, and the underlying error's `Display` embeds the
    /// failing value. The JSON pointer and failed keyword are retained either
    /// way.
    pub include_instance_values: bool,
}

impl Default for ValidationOptions {
    fn default() -> Self {
        Self { max_issues: 100, include_instance_values: false }
    }
}
