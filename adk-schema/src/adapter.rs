//! Bridge to every `adk-core` schema adapter, without either crate changing.
//!
//! `adk_core::SchemaAdapter` is already a trait, and every provider in the
//! workspace implements it — `GenericSchemaAdapter`, `GeminiSchemaAdapter`,
//! `OpenAiSchemaAdapter`, `OpenAiStrictSchemaAdapter`, `AnthropicSchemaAdapter`.
//! An extension trait with a blanket implementation therefore reaches all of
//! them at once: no adapter is edited, and `adk-core` is not touched.
//!
//! The direction matters. Putting `project()` on the `adk-core` trait would
//! make Stable `adk-core` depend on Beta `adk-schema` and pin core's release
//! cadence to this crate's. Here the dependency points the other way — Beta on
//! Stable — and it is optional, so a consumer using this crate as a plain
//! JSON Schema library never compiles `adk-core` at all.

use crate::{Difference, IngestionPolicy, InputSchema, Result, SchemaRole};
use adk_core::SchemaAdapter;

/// A provider's reduced form of a schema, and what the reduction cost.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Projection<R: SchemaRole> {
    /// The schema as the provider will see it, re-ingested and canonicalized.
    pub projected: crate::SchemaDocument<R>,
    /// Constraints present in the canonical document and absent or altered in
    /// the projection.
    pub dropped: Vec<Difference>,
}

impl<R: SchemaRole> Projection<R> {
    /// Whether the provider is enforcing less than the runtime still will.
    ///
    /// The dangerous shape: the model is shown a looser contract than the one
    /// it will be judged against, so it cannot know which rule it broke.
    ///
    /// Deliberately conservative. A constraint present on both sides with a
    /// different value — `minimum: 1` becoming `minimum: 5` — counts, because
    /// ordering the two requires per-keyword semantics this crate does not
    /// model. It over-reports a projection that tightened rather than staying
    /// quiet about one that loosened.
    pub fn leaves_enforcement_gap(&self) -> bool {
        self.dropped.iter().any(Difference::leaves_enforcement_gap)
    }

    /// Losses that change which instances are accepted, ignoring annotations.
    pub fn substantive_losses(&self) -> impl Iterator<Item = &Difference> {
        self.dropped.iter().filter(|loss| loss.affects_validation())
    }
}

/// Projection with loss reporting, for any [`SchemaAdapter`].
///
/// Blanket-implemented, so implementing `SchemaAdapter` is the only thing a
/// provider ever has to do.
pub trait SchemaAdapterExt: SchemaAdapter {
    /// Reduces a canonical document to this provider's dialect and reports what
    /// the reduction dropped.
    ///
    /// `normalize_schema` alone answers "what do I send?". This also answers
    /// "what did I stop enforcing by sending it?", which is the question that
    /// decides whether the runtime has to compensate.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError`](crate::SchemaError) if the provider's output
    /// fails ingestion under `policy` — a projection that violates the caller's
    /// own limits is reported rather than trusted.
    fn project<R: SchemaRole>(
        &self,
        canonical: &crate::SchemaDocument<R>,
        policy: &IngestionPolicy,
    ) -> Result<Projection<R>> {
        let reduced = self.normalize_schema(canonical.as_value().clone());
        let projected = crate::SchemaDocument::<R>::from_value(reduced, policy)?;
        let dropped = canonical.diff(&projected);

        Ok(Projection { projected, dropped })
    }
}

impl<T: SchemaAdapter + ?Sized> SchemaAdapterExt for T {}

/// Convenience alias for the common input-side case.
pub type InputProjection = Projection<crate::Input>;

#[allow(dead_code)]
fn _assert_input_schema_is_projectable(schema: &InputSchema) -> &InputSchema {
    schema
}

#[cfg(test)]
mod tests {
    use super::*;
    use adk_core::GenericSchemaAdapter;
    use serde_json::json;

    fn ingest(value: serde_json::Value) -> InputSchema {
        InputSchema::from_value(value, &IngestionPolicy::default()).expect("fixture ingests")
    }

    /// The blanket implementation reaches an adapter that knows nothing about
    /// this crate.
    #[test]
    fn projects_through_an_unmodified_adk_core_adapter() {
        let canonical = ingest(json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": { "n": { "type": "integer" } }
        }));

        let projection = GenericSchemaAdapter
            .project(&canonical, &IngestionPolicy::default())
            .expect("projection ingests");

        assert!(projection.projected.as_value().get("$schema").is_none());
    }

    /// A lossless projection reports no enforcement gap, so the flag stays
    /// meaningful rather than firing on every provider.
    #[test]
    fn reports_no_enforcement_gap_when_only_an_annotation_moves() {
        let canonical = ingest(json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": { "n": { "type": "integer" } }
        }));

        let projection = GenericSchemaAdapter
            .project(&canonical, &IngestionPolicy::default())
            .expect("projection ingests");

        assert!(!projection.leaves_enforcement_gap(), "{:#?}", projection.dropped);
    }

    /// A projection that changed nothing reports nothing, so the signal stays
    /// worth reading.
    #[test]
    fn an_identity_projection_reports_no_losses() {
        #[derive(Debug)]
        struct Identity;
        impl SchemaAdapter for Identity {
            fn normalize_schema(&self, schema: serde_json::Value) -> serde_json::Value {
                schema
            }
        }

        let canonical = ingest(json!({ "type": "object", "minimum": 1 }));

        let projection =
            Identity.project(&canonical, &IngestionPolicy::default()).expect("projection ingests");

        assert!(projection.dropped.is_empty(), "{:#?}", projection.dropped);
    }

    /// Adapter output is re-ingested, not trusted. A provider that emits a
    /// remote reference must not get one past the reference policy just by
    /// being on the far side of a projection.
    #[test]
    fn adapter_output_that_violates_the_policy_is_rejected() {
        #[derive(Debug)]
        struct EmitsRemoteRef;
        impl SchemaAdapter for EmitsRemoteRef {
            fn normalize_schema(&self, _: serde_json::Value) -> serde_json::Value {
                json!({ "$ref": "https://example.invalid/schema.json" })
            }
        }

        let canonical = ingest(json!({ "type": "object" }));

        let result = EmitsRemoteRef.project(&canonical, &IngestionPolicy::default());

        assert!(result.is_err(), "a remote $ref survived projection");
    }

    /// A projection that tightens is still reported. The direction cannot be
    /// ordered without per-keyword semantics, so the conservative reading wins.
    #[test]
    fn a_tightening_projection_is_reported_rather_than_assumed_safe() {
        #[derive(Debug)]
        struct Tightens;
        impl SchemaAdapter for Tightens {
            fn normalize_schema(&self, mut schema: serde_json::Value) -> serde_json::Value {
                schema["minimum"] = json!(5);
                schema
            }
        }

        let canonical = ingest(json!({ "type": "object", "minimum": 1 }));

        let projection =
            Tightens.project(&canonical, &IngestionPolicy::default()).expect("projection ingests");

        assert!(projection.leaves_enforcement_gap());
    }
}
