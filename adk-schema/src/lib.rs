//! # adk-schema
//!
//! Canonical JSON Schema documents and validation for ADK.
//!
//! ## Role Separation Compile-Time Assertions
//!
//! ```compile_fail
//! use adk_schema::{InputSchema, OutputSchema};
//!
//! fn requires_output(_: OutputSchema) {}
//!
//! # fn example(input: InputSchema) {
//! requires_output(input);
//! # }
//! ```

#![deny(missing_docs)]

#[cfg(feature = "adapters")]
mod adapter;
mod canonical;
mod diff;
mod digest;
mod document;
mod error;
mod fields;
mod ingest;
mod outstanding;
mod pointer;
mod policy;
mod references;
mod role;
#[cfg(feature = "schemars")]
mod static_schema;
#[cfg(feature = "runtime-validation")]
mod validation;

#[cfg(feature = "adapters")]
pub use adapter::{InputProjection, Projection, SchemaAdapterExt};
pub use diff::{Difference, DifferenceKind};
pub use digest::SchemaDigest;
pub use document::{JsonSchemaDialect, SchemaDirection, SchemaDocument, SchemaMetrics};
pub use error::{LimitKind, ReferenceRejection, Result, SchemaError, ValidationIssue};
pub use fields::FieldEntry;
pub use outstanding::Outstanding;
pub use policy::{IngestionPolicy, ReferencePolicy, ValidationOptions};
pub use role::{Input, InputSchema, Output, OutputSchema, SchemaRole};

#[cfg(feature = "runtime-validation")]
pub use validation::ValidatedSchemaDocument;
