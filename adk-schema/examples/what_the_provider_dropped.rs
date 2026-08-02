//! Finding out what a provider silently removed from your schema — before
//! production does.
//!
//! Every LLM provider accepts a subset of JSON Schema, so every provider
//! adapter reduces the contract on the way out. The reduction is silent: you
//! hand over a schema, a smaller one is sent, and nothing tells you which rules
//! stopped being enforced. The runtime keeps enforcing the original, so the
//! model gets judged against a contract it was never shown.
//!
//! Run with:
//! `cargo run -p adk-schema --features adapters --example what_the_provider_dropped`

use adk_core::SchemaAdapter;
use adk_schema::{IngestionPolicy, InputSchema, SchemaAdapterExt};
use serde_json::{Value, json};

/// Stands in for a real provider adapter, doing one of the reductions they all
/// do: deleting conditional keywords it cannot express.
///
/// This is not a strawman. `adk_gemini::GeminiSchemaAdapter::normalize_schema`
/// strips `if`/`then`/`else` at step 6 of 13. The same measurement is made
/// against that shipping adapter in `tests/provider_projection_loss.rs`; this
/// example keeps the crate free of a provider dependency.
#[derive(Debug)]
struct StripsConditionals;

impl SchemaAdapter for StripsConditionals {
    fn identifier(&self) -> &str {
        "strips-conditionals"
    }

    fn normalize_schema(&self, mut schema: Value) -> Value {
        if let Some(object) = schema.as_object_mut() {
            object.remove("allOf");
            object.remove("if");
            object.remove("then");
            object.remove("else");
            object.remove("$schema");
        }
        schema
    }
}

/// Ordering requires a caller name. Asking a question does not.
fn contract() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "request_kind": { "type": "string", "enum": ["order", "information"] },
            "issue_description": { "type": "string" },
            "caller_name": { "type": "string" }
        },
        "required": ["request_kind", "issue_description"],
        "allOf": [{
            "if": {
                "properties": { "request_kind": { "const": "order" } },
                "required": ["request_kind"]
            },
            "then": { "required": ["caller_name"] }
        }]
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let policy = IngestionPolicy::default();
    let canonical = InputSchema::from_value(contract(), &policy)?;

    // Without this crate, the reduction is a `Value` in and a `Value` out.
    // Comparing them means writing a schema-aware differ: `serde_json` equality
    // is useless because key order and formatting differ, a textual diff reports
    // noise, and a naive recursive compare cannot tell "dropped `minimum`" from
    // "dropped `description`" — one is an enforcement change, the other is not.
    let reduced = StripsConditionals.normalize_schema(canonical.as_value().clone());
    println!("What the provider is sent:\n  {reduced}\n");
    println!("...and that is the entire signal you get. The `allOf` is gone.\n");

    // With it, the same call also reports the cost.
    let projection = StripsConditionals.project(&canonical, &policy)?;

    println!("What `project()` reports it cost:");
    for loss in projection.substantive_losses() {
        println!("  {:<24} keyword={:<8} {:?}", loss.pointer, loss.keyword, loss.kind);
    }

    if projection.leaves_enforcement_gap() {
        println!(
            "\n  -> enforcement gap: the runtime still requires a caller name for\n     \
             orders, and the model was never told."
        );
    }

    assert!(projection.leaves_enforcement_gap());

    // The compensation. The provider surface cannot carry the obligation, so the
    // runtime asks the canonical document instead and prompts for the field
    // itself. That is a two-line fallback rather than a production incident.
    let collected = json!({ "request_kind": "order", "issue_description": "two lasagnas" });
    let collected = collected.as_object().expect("object");

    let still_needed = canonical.outstanding(collected);
    println!("\nSo the runtime asks the canonical contract what is still owed:");
    println!("  {:?}  <- ask the caller for this", still_needed.missing);

    assert!(still_needed.missing.contains("caller_name"));

    // And the projection, asked the same question, does not know to.
    let projection_view = projection.projected.outstanding(collected);
    assert!(
        !projection_view.missing.contains("caller_name"),
        "the projection was expected to have lost the obligation",
    );
    println!("  the projected schema, asked the same thing: {:?}", projection_view.missing);

    Ok(())
}
