//! A phone agent collecting a tool call over several turns.
//!
//! The caller does not answer in one utterance, so the agent needs an answer to
//! "what do I still have to ask for?" after every turn. Validation cannot answer
//! it: validation says the instance is invalid, not which question comes next.
//!
//! Run with `cargo run -p adk-schema --example what_to_ask_next`.

use adk_schema::{IngestionPolicy, InputSchema};
use serde_json::{Map, Value, json};

/// The tool contract. Ordering requires a name and a fulfillment method;
/// answering a question requires neither.
///
/// The conditional matters commercially: demanding a caller's name before
/// answering "what time do you close?" is the difference between a useful agent
/// and one people hang up on.
fn contract() -> Value {
    json!({
        "type": "object",
        "properties": {
            "request_kind": { "type": "string", "enum": ["order", "information"] },
            "issue_description": { "type": "string" },
            "caller_name": { "type": "string" },
            "fulfillment_method": { "type": "string", "enum": ["pickup", "delivery"] }
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

/// The check almost everyone writes first: read the top-level `required` array
/// and diff it against the keys collected so far.
///
/// It is three lines and it is wrong. `required` holds the *unconditional*
/// obligations only, so once `request_kind` and `issue_description` are in hand
/// this reports "nothing outstanding" for an order that still has no name
/// attached to it. The agent then submits a nameless order and the shift
/// supervisor has a ticket nobody can act on.
///
/// Getting it right by hand means walking `allOf`, evaluating each `if` against
/// a *partial* instance, deciding what an unknown gate means, and repeating the
/// work for `dependentRequired`, `dependentSchemas`, and nested `allOf`. That is
/// a schema evaluator, and it is the thing people quietly reimplement per
/// project.
fn required_keys_only(schema: &Value, collected: &Map<String, Value>) -> Vec<String> {
    schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|key| !collected.contains_key(*key))
        .map(str::to_string)
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schema = InputSchema::from_value(contract(), &IngestionPolicy::default())?;

    // Turn one. "Hi, I'd like to place an order — two lasagnas."
    let after_turn_one = json!({ "request_kind": "order", "issue_description": "two lasagnas" });
    let after_turn_one = after_turn_one.as_object().expect("object");

    println!("After turn one, the caller has said they want to order.\n");

    let by_hand = required_keys_only(&contract(), after_turn_one);
    println!("  hand-rolled `required` check : {by_hand:?}");
    println!("      -> reads as complete, so the agent submits a nameless order\n");

    let outstanding = schema.outstanding(after_turn_one);
    println!("  adk-schema outstanding()     : {:?}", outstanding.missing);
    println!("      -> the conditional fired, so the agent asks for the name\n");

    assert!(by_hand.is_empty(), "the naive check sees nothing left to ask");
    assert!(outstanding.missing.contains("caller_name"));
    assert!(!outstanding.is_complete());

    // Turn two. The caller gives a name and picks up in person.
    let after_turn_two = json!({
        "request_kind": "order",
        "issue_description": "two lasagnas",
        "caller_name": "Ada",
        "fulfillment_method": "pickup"
    });
    let after_turn_two = after_turn_two.as_object().expect("object");

    let outstanding = schema.outstanding(after_turn_two);
    println!(
        "After turn two: {}",
        if outstanding.is_complete() { "complete" } else { "incomplete" }
    );
    assert!(outstanding.is_complete());

    // A different caller, only asking a question. The same conditional must now
    // stay quiet — this is the half people break when they hard-code the fields.
    let question = json!({ "request_kind": "information", "issue_description": "closing time?" });
    let question = question.as_object().expect("object");

    let outstanding = schema.outstanding(question);
    println!(
        "A caller only asking a question: {}",
        if outstanding.is_complete() { "complete, no name demanded" } else { "incomplete" }
    );
    assert!(outstanding.is_complete());

    // And the case that separates "not required" from "not known yet". Before
    // the caller has said what they want, the obligation is neither owed nor
    // waived — it is undecided, and `undecided` names the field whose value
    // settles it, which is exactly the question to ask next.
    let opening = json!({ "issue_description": "hi there" });
    let opening = opening.as_object().expect("object");

    let outstanding = schema.outstanding(opening);
    println!("\nBefore the caller says what they want:");
    println!("  missing   : {:?}", outstanding.missing);
    println!("  undecided : {:?}   <- ask this to settle the rest", outstanding.undecided);

    assert!(outstanding.undecided.contains("request_kind"));
    assert!(!outstanding.is_complete(), "an undecided obligation is not a satisfied one");

    Ok(())
}
