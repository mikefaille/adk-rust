use crate::ValidatedSchemaDocument;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchemaCompileError {
    #[error("Compilation failed: {0}")]
    Failure(String),
}

#[derive(Debug, Clone)]
pub struct CompiledSchema<T> {
    pub target_schema: T,
}

pub trait SchemaCompiler {
    type Target;

    fn compile(
        &self,
        schema: &ValidatedSchemaDocument,
    ) -> Result<CompiledSchema<Self::Target>, SchemaCompileError>;
}
