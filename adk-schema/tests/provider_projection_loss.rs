//! What a real provider projection drops, measured with `diff()`.
//!
//! The projected fixture below is not hand-written. It is the verbatim output of
//! `adk_gemini::GeminiSchemaAdapter::new().normalize_schema(SOURCE)` captured
//! from the shipping adapter, so this file pins the behaviour of real code
//! without making `adk-schema` depend on a provider client.
//!
//! Re-capture it by running that call on [`source`] and pasting the result.

use adk_core::{GenericSchemaAdapter, KimiSchemaAdapter, SchemaAdapter};
use adk_gemini::GeminiSchemaAdapter;
use adk_schema::{IngestionPolicy, InputSchema};
use serde_json::{Value, json};

/// A contract with a conditional obligation: ordering requires a name and a
/// fulfillment method, asking a question does not.
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

/// Verbatim invocation of `GeminiSchemaAdapter::new().normalize_schema(source())`.
fn gemini_projection() -> Value {
    GeminiSchemaAdapter::new().normalize_schema(source())
}

/// Verbatim invocation of `GenericSchemaAdapter.normalize_schema(source())` for OpenAI.
fn openai_projection() -> Value {
    GenericSchemaAdapter.normalize_schema(source())
}

/// Verbatim invocation of `GenericSchemaAdapter.normalize_schema(source())` for Anthropic.
fn anthropic_projection() -> Value {
    GenericSchemaAdapter.normalize_schema(source())
}

/// Verbatim invocation of `KimiSchemaAdapter.normalize_schema(source())` for Kimi.
fn kimi_projection() -> Value {
    KimiSchemaAdapter.normalize_schema(source())
}

fn ingest(value: Value) -> InputSchema {
    InputSchema::from_value(value, &IngestionPolicy::default()).expect("fixture ingests")
}

/// The projection is lossy, and `diff()` says so rather than reporting parity.
#[test]
fn reports_that_the_gemini_projection_drops_constraints() {
    let losses = ingest(source()).diff(&ingest(gemini_projection()));

    assert!(!losses.is_empty(), "a lossy projection reported no differences");
}

/// The conditional obligation is the loss that matters: the model is never told
/// that ordering requires a name, while the runtime still enforces it.
#[test]
fn reports_the_dropped_conditional_obligation() {
    let losses = ingest(source()).diff(&ingest(gemini_projection()));

    assert!(
        losses.iter().any(|loss| loss.pointer.starts_with("/allOf") || loss.keyword == "allOf"),
        "the dropped if/then obligation was not reported: {losses:#?}",
    );
}

/// A dropped obligation is an enforcement gap, not a cosmetic difference. This
/// is the classification a caller filters on to decide whether a projection is
/// safe to ship.
#[test]
fn classifies_the_loss_as_an_enforcement_gap() {
    let losses = ingest(source()).diff(&ingest(gemini_projection()));

    assert!(
        losses.iter().any(|loss| loss.leaves_enforcement_gap()),
        "no loss was classified as an enforcement gap: {losses:#?}",
    );
}

/// At least one reported loss changes which instances are accepted, so the
/// report is not purely cosmetic. (It does not claim *every* difference is
/// substantive — the projection also drops `$schema`, which is not a rule.)
#[test]
fn reports_at_least_one_substantive_loss() {
    let losses = ingest(source()).diff(&ingest(gemini_projection()));

    assert!(
        losses.iter().any(|loss| loss.affects_validation()),
        "every reported difference was cosmetic: {losses:#?}",
    );
}

/// The recovery, and the reason `outstanding()` exists.
///
/// The projected schema cannot express the obligation, so a model working from
/// it has no way to know a name is needed. Asking the canonical document the
/// same question still reports it, which is what lets the runtime prompt for
/// the field the provider surface silently dropped.
#[test]
fn outstanding_recovers_the_obligation_the_projection_dropped() {
    let collected = json!({ "request_kind": "order", "issue_description": "one lasagna" });
    let collected = collected.as_object().expect("object");

    let from_canonical = ingest(source()).outstanding(collected);
    let from_projection = ingest(gemini_projection()).outstanding(collected);

    assert!(
        from_canonical.missing.contains("caller_name"),
        "canonical document lost the obligation: {from_canonical:#?}",
    );
    assert!(
        !from_projection.missing.contains("caller_name"),
        "projection was expected to have dropped the obligation: {from_projection:#?}",
    );
}

#[test]
fn reports_that_openai_projection_drops_conditional_constraints() {
    let losses = ingest(source()).diff(&ingest(openai_projection()));
    assert!(!losses.is_empty(), "openai projection reported no differences");
    assert!(
        losses.iter().any(|loss| loss.pointer.starts_with("/allOf") || loss.keyword == "allOf"),
        "the dropped if/then obligation was not reported for openai: {losses:#?}"
    );
}

#[test]
fn reports_that_anthropic_projection_drops_conditional_constraints() {
    let losses = ingest(source()).diff(&ingest(anthropic_projection()));
    assert!(!losses.is_empty(), "anthropic projection reported no differences");
    assert!(
        losses.iter().any(|loss| loss.pointer.starts_with("/allOf") || loss.keyword == "allOf"),
        "the dropped if/then obligation was not reported for anthropic: {losses:#?}"
    );
}

#[test]
fn reports_that_kimi_projection_drops_conditional_constraints() {
    let losses = ingest(source()).diff(&ingest(kimi_projection()));
    assert!(!losses.is_empty(), "kimi projection reported no differences");
    assert!(
        losses.iter().any(|loss| loss.pointer.starts_with("/allOf") || loss.keyword == "allOf"),
        "the dropped if/then obligation was not reported for kimi: {losses:#?}"
    );
}
