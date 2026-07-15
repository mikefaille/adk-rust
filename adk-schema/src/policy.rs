use crate::ValidatedSchemaDocument;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchemaPolicyError {
    #[error("Schema policy violation: {0}")]
    Violation(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SchemaPath(pub String);

impl std::fmt::Display for SchemaPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone)]
pub enum SchemaDiagnostic {
    MissingDescription { path: SchemaPath },
    Warning(String),
}

pub struct PolicyReport {
    pub warnings: Vec<SchemaDiagnostic>,
    pub runtime_only_constraints: Vec<SchemaPath>,
}

pub trait SchemaPolicy: Send + Sync {
    fn id(&self) -> &str;
    fn version(&self) -> u32;

    fn evaluate(
        &self,
        _schema: &ValidatedSchemaDocument,
    ) -> Result<PolicyReport, SchemaPolicyError>;
}

// Portable Tool Profile
pub struct PortableToolLimits {
    pub max_union_branches: usize,
    pub allow_any_of: bool,
    pub allow_all_of: bool,
    // Add other limits
}

impl Default for PortableToolLimits {
    fn default() -> Self {
        Self {
            max_union_branches: 5,
            allow_any_of: false,
            allow_all_of: false,
        }
    }
}

pub struct PortableToolSchemaPolicy {
    pub limits: PortableToolLimits,
}

impl SchemaPolicy for PortableToolSchemaPolicy {
    fn id(&self) -> &str {
        "portable-tool-profile"
    }

    fn version(&self) -> u32 {
        1
    }

    fn evaluate(
        &self,
        _schema: &ValidatedSchemaDocument,
    ) -> Result<PolicyReport, SchemaPolicyError> {
        // Implement portable tool policy checks here (no external ref, limited unions, etc)
        // This relies on traversing the canonical document

        Ok(PolicyReport {
            warnings: vec![],
            runtime_only_constraints: vec![],
        })
    }
}
