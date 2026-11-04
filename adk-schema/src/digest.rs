use crate::document::JsonSchemaDialect;
use crate::role::SchemaRole;
use sha2::{Digest, Sha256};

/// Structured cryptographic identity digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SchemaDigest([u8; 32]);

impl SchemaDigest {
    /// Access the raw digest bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Display for SchemaDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

pub(crate) fn calculate_digest<R: SchemaRole>(
    dialect: JsonSchemaDialect,
    bytes: &[u8],
) -> SchemaDigest {
    let mut hasher = Sha256::new();
    hasher.update(b"adk-schema\0");
    let version: u32 = 1;
    hasher.update(version.to_be_bytes());
    hasher.update([dialect.digest_tag()]);
    hasher.update([R::DIGEST_TAG]);
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    SchemaDigest(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    /// Role is part of a document's identity. An input contract and an output
    /// contract that happen to share a shape are different things, and a digest
    /// used as a cache key must not conflate them.
    #[test]
    fn input_and_output_documents_do_not_share_a_digest() {
        use crate::{IngestionPolicy, InputSchema, OutputSchema};

        let policy = IngestionPolicy::default();
        let shape = serde_json::json!({ "type": "string" });

        let input = InputSchema::from_value(shape.clone(), &policy).expect("ingests");
        let output = OutputSchema::from_value(shape, &policy).expect("ingests");

        assert_ne!(input.digest(), output.digest());
    }
}
