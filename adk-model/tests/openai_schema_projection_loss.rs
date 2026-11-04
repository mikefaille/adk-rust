//! What the real OpenAI schema adapters drop, measured with `adk_schema::diff()`.
//!
//! `adk-schema/tests/provider_projection_loss.rs` used to claim to characterise
//! this projection while only ever constructing `adk_core::GenericSchemaAdapter`.
//! `OpenAiSchemaAdapter` and `OpenAiStrictSchemaAdapter` live here, in
//! `adk-model` — `adk-schema` cannot depend on them without dragging a
//! 51-dependency provider crate into a small Beta crate's dev graph, exactly
//! the tight-coupling concern the upstream maintainer raised on issue #550.
//! This file exercises the real adapters instead, next to the code they
//! characterise.

#![cfg(feature = "openai")]

use adk_core::SchemaAdapter;
use adk_model::openai::{OpenAiSchemaAdapter, OpenAiStrictSchemaAdapter};
use adk_schema::{IngestionPolicy, InputSchema, SchemaAdapterExt};
use serde_json::{Value, json};

/// A contract with a conditional obligation: ordering requires a name and a
/// fulfillment method, asking a question does not. Mirrors the fixture in
/// `adk-schema/tests/provider_projection_loss.rs`.
fn source() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "request_kind": { "type": "string", "enum": ["order", "information"] },
            "issue_description": { "type": "string" },
            "caller_name": { "type": ["string", "null"] },
            "fulfillment_method": { "type": "string" }
        },
        "required": ["request_kind", "issue_description"],
        "allOf": [{
            "if": {
                "properties": { "request_kind": { "const": "order" } },
                "required": ["request_kind"]
            },
            "then": { "required": ["caller_name", "fulfillment_method"] }
        }]
    })
}

fn ingest(value: Value) -> InputSchema {
    InputSchema::from_value(value, &IngestionPolicy::default()).expect("fixture ingests")
}

/// `OpenAiSchemaAdapter::normalize_schema` runs the same five transforms, in
/// the same order, over the same allowed-format list as
/// `GenericSchemaAdapter` (verified against
/// `adk-model/src/openai/schema_adapter.rs` and
/// `adk-core/src/schema_adapter.rs`), so it drops the same `if`/`then`
/// obligation the generic stand-in did.
#[test]
fn reports_that_openai_projection_drops_conditional_constraints() {
    let projected = OpenAiSchemaAdapter.normalize_schema(source());
    let losses = ingest(source()).diff(&ingest(projected));

    assert!(!losses.is_empty(), "openai projection reported no differences");
    assert!(
        losses.iter().any(|loss| loss.pointer.starts_with("/allOf") || loss.keyword == "allOf"),
        "the dropped if/then obligation was not reported for openai: {losses:#?}"
    );
}

/// `OpenAiStrictSchemaAdapter` is the one adapter in the workspace that
/// *tightens* a schema it already constrains, rather than only dropping
/// constraints: `set_additional_properties_false` overwrites an existing
/// `additionalProperties: true` with `false`. `Projection::leaves_enforcement_gap()`
/// is documented as deliberately conservative for exactly this shape — "a
/// constraint present on both sides with a different value... counts,
/// because ordering the two requires per-keyword semantics this crate does
/// not model. It over-reports a projection that tightened rather than
/// staying quiet about one that loosened" (`adk-schema/src/adapter.rs`).
/// Nothing in the workspace exercised that branch against a real adapter
/// before this test.
#[test]
fn openai_strict_projection_reports_narrowed_additional_properties_as_an_enforcement_gap() {
    let canonical = ingest(json!({
        "type": "object",
        "properties": { "name": { "type": "string" } },
        "additionalProperties": true
    }));

    let projection = OpenAiStrictSchemaAdapter
        .project(&canonical, &IngestionPolicy::default())
        .expect("projection ingests");

    assert!(
        projection.dropped.iter().any(|loss| loss.keyword == "additionalProperties"),
        "expected additionalProperties: true -> false to be reported: {:#?}",
        projection.dropped
    );
    assert!(
        projection.leaves_enforcement_gap(),
        "a tightening projection must still report as a gap (conservative over-report): {:#?}",
        projection.dropped
    );
}

/// The boundary the previous test relies on: when `additionalProperties` is
/// merely *added* (absent on the canonical side, `false` on the projected
/// side) rather than changed in place, the diff engine classifies it as
/// `ConstraintTightened` — a key present only on the "more restrictive" side
/// — which `Difference::leaves_enforcement_gap()` deliberately excludes
/// (`adk-schema/src/diff.rs`): a projection that only ever adds restrictions
/// cannot be the dangerous "runtime enforces a rule the model was never
/// shown" case `leaves_enforcement_gap()` exists to flag. This is still a
/// real, reportable difference; it is just not an enforcement gap.
#[test]
fn openai_strict_projection_reports_added_additional_properties_without_a_gap() {
    let canonical = ingest(json!({
        "type": "object",
        "properties": { "name": { "type": "string" } }
    }));

    let projection = OpenAiStrictSchemaAdapter
        .project(&canonical, &IngestionPolicy::default())
        .expect("projection ingests");

    assert!(
        projection.dropped.iter().any(|loss| loss.keyword == "additionalProperties"),
        "expected the added additionalProperties: false to be reported: {:#?}",
        projection.dropped
    );
    assert!(
        !projection.leaves_enforcement_gap(),
        "a purely additive tightening should not be classified as an enforcement gap: {:#?}",
        projection.dropped
    );
}
