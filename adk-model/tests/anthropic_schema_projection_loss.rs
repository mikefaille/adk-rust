//! What the real Anthropic schema adapter drops, measured with `adk_schema::diff()`.
//!
//! `adk-schema/tests/provider_projection_loss.rs` used to claim to characterise
//! this projection while only ever constructing `adk_core::GenericSchemaAdapter`
//! as a stand-in — harmless for OpenAI (same five transforms, same allowed
//! format list) but materially wrong for Anthropic: `AnthropicSchemaAdapter`
//! (`adk-model/src/anthropic/schema_adapter.rs`) runs only
//! `strip_schema_keyword`, `strip_conditional_keywords`, and
//! `add_implicit_object_type` — it omits `convert_const_to_enum` and
//! `strip_unsupported_formats`, so its real loss profile is narrower than the
//! stand-in reported. This file exercises the real adapter, next to the code
//! it characterises.

#![cfg(feature = "anthropic")]

use adk_core::{GenericSchemaAdapter, SchemaAdapter};
use adk_model::anthropic::AnthropicSchemaAdapter;
use adk_schema::{IngestionPolicy, InputSchema};
use serde_json::{Value, json};

/// A contract with a conditional obligation, identical to the fixture in
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

/// A schema whose only interesting content is `const` and a non-allowlisted
/// `format` — the two keywords where Anthropic's real support diverges from
/// the generic/OpenAI adapters.
fn const_and_format_source() -> Value {
    json!({
        "type": "object",
        "properties": {
            "status": { "type": "string", "const": "active" },
            "reference": { "type": "string", "format": "internal-ref" }
        }
    })
}

fn ingest(value: Value) -> InputSchema {
    InputSchema::from_value(value, &IngestionPolicy::default()).expect("fixture ingests")
}

/// Anthropic still strips `if`/`then`/`else` — `strip_conditional_keywords`
/// is one of its three real transforms — so the dropped conditional
/// obligation is a genuine loss, not a stand-in artifact.
#[test]
fn reports_that_anthropic_projection_drops_conditional_constraints() {
    let projected = AnthropicSchemaAdapter.normalize_schema(source());
    let losses = ingest(source()).diff(&ingest(projected));

    assert!(!losses.is_empty(), "anthropic projection reported no differences");
    assert!(
        losses.iter().any(|loss| loss.pointer.starts_with("/allOf") || loss.keyword == "allOf"),
        "the dropped if/then obligation was not reported for anthropic: {losses:#?}"
    );
}

/// The defect this file exists to fix. `GenericSchemaAdapter`, used as an
/// accidental Anthropic stand-in, reports `const` and a non-allowlisted
/// `format` as dropped (first two assertions — this is what the old test
/// silently relied on). The real `AnthropicSchemaAdapter` does not run
/// `convert_const_to_enum` or `strip_unsupported_formats`, so it preserves
/// both and the diff against it is empty (final assertion). Swap
/// `AnthropicSchemaAdapter` for `GenericSchemaAdapter` in the final block and
/// it fails — that failure is exactly the gap between the stand-in and the
/// real adapter that made the original test materially wrong.
#[test]
fn anthropic_projection_preserves_const_and_unlisted_format_the_stand_in_would_drop() {
    let canonical = ingest(const_and_format_source());

    let stand_in_losses =
        canonical.diff(&ingest(GenericSchemaAdapter.normalize_schema(const_and_format_source())));
    assert!(
        stand_in_losses.iter().any(|loss| loss.keyword == "const"),
        "expected the generic stand-in to report a dropped const: {stand_in_losses:#?}"
    );
    assert!(
        stand_in_losses.iter().any(|loss| loss.keyword == "format"),
        "expected the generic stand-in to report a dropped format: {stand_in_losses:#?}"
    );

    let real_losses =
        canonical.diff(&ingest(AnthropicSchemaAdapter.normalize_schema(const_and_format_source())));
    assert!(
        real_losses.is_empty(),
        "AnthropicSchemaAdapter does not strip const or non-allowlisted formats, \
         but diffing against it reported losses: {real_losses:#?}"
    );
}
