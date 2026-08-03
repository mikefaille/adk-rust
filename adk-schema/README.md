# adk-schema

Read a JSON Schema once, then ask it the questions your code actually has.

[![Crates.io](https://img.shields.io/crates/v/adk-schema.svg)](https://crates.io/crates/adk-schema)
[![Documentation](https://docs.rs/adk-schema/badge.svg)](https://docs.rs/adk-schema)
[![License](https://img.shields.io/crates/l/adk-schema.svg)](LICENSE)

## What this is for

If you already work with JSON Schema daily, skip to [What you can ask](#what-you-can-ask). Otherwise, the short version:

A **JSON Schema** is a machine-readable description of what a JSON value is allowed to look like — which fields exist, which are required, what type each one is. It is itself written in JSON:

```json
{
  "type": "object",
  "properties": {
    "issue_description": { "type": "string" },
    "service_address":   { "type": "string" }
  },
  "required": ["issue_description"]
}
```

That says: an object, with two optional string fields, of which `issue_description` must be present.

**Where this shows up when you build with language models.** You can give a model a set of functions it may call — booking an appointment, looking up an order. The model does not receive your Rust signature; it receives a JSON Schema describing the arguments. It then produces JSON that is supposed to match. So the schema is doing two jobs at once: it is the instructions the model reads, *and* the contract you check its answer against.

That dual role is where the bugs live, and it is what this crate exists to handle. Three things go wrong routinely:

1. **Every part of your system re-reads the schema by hand.** One place walks the JSON to list fields for a UI, another to decide what is still missing, a third to log it. Those hand-written walks disagree, and they disagree silently.
2. **Requirements are often conditional, and naive code flattens them.** "A delivery address is required *if* they are placing an order" gets read as "a delivery address is required", and now your agent demands an address from someone who only asked a question.
3. **Providers rewrite your schema before the model sees it.** Gemini, OpenAI and Anthropic each accept only a reduced subset, and they silently drop rules they cannot express. The model is shown looser rules than the ones its answer will be judged against — so it breaks a rule it was never told about, and cannot tell what it did wrong.

This crate reads a schema once, under limits you set, and answers those questions in one place.

## Installation

```toml
[dependencies]
adk-schema = "2.0.0"
```

Everything below works with no features enabled except where noted.

| Feature | Adds |
| --- | --- |
| *(none)* | Reading schemas, the fingerprint, and all the questions below |
| `runtime-validation` | Checking a JSON value against a schema |
| `schemars` | Deriving a schema from a Rust type |
| `adapters` | Reporting what an LLM provider dropped |

Without `adapters`, nothing from the ADK framework is compiled in — the crate works as a plain JSON Schema library.

## What you can ask

### What is still missing?

The one that motivated the crate. You have a half-filled form and want to know what to ask for next.

```rust
use adk_schema::{IngestionPolicy, InputSchema};
use serde_json::json;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
// "If they are placing an order, we also need an address."
let schema = InputSchema::from_value(
    json!({
        "type": "object",
        "properties": {
            "request_kind":      { "type": "string", "enum": ["order", "question"] },
            "issue_description": { "type": "string" },
            "service_address":   { "type": "string" }
        },
        "required": ["request_kind", "issue_description"],
        "allOf": [{
            "if":   { "properties": { "request_kind": { "const": "order" } } },
            "then": { "required": ["service_address"] }
        }]
    }),
    &IngestionPolicy::default(),
)?;

// What we have gathered so far, as a map of field name to value.
let asking = json!({ "request_kind": "question", "issue_description": "are you open?" });
let asking = asking.as_object().expect("an object");

// Someone asking a question: the address is not owed, because the condition
// that would require it is false.
assert!(schema.outstanding(asking).is_complete());

// Someone placing an order: now it is.
let ordering = json!({ "request_kind": "order", "issue_description": "leaking tap" });
let ordering = ordering.as_object().expect("an object");

assert!(schema.outstanding(ordering).missing.contains("service_address"));
# Ok(())
# }
```

The result separates two different situations. `missing` is what you can go and ask for. `undecided` names the fields whose *value* would settle a condition you cannot evaluate yet — if you do not yet know whether this is an order, you cannot know whether the address is needed, and the honest answer is to ask what they want rather than to demand an address. `is_complete()` is true when both are empty.

It walks conditional structure at any nesting depth.

### Just list the fields

For code with no typed model of its own: dynamic UIs, logging, generic policy checks.

```rust
# use adk_schema::{IngestionPolicy, InputSchema};
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let schema = InputSchema::from_value(serde_json::json!({
#     "properties": { "name": { "type": ["string", "null"] } },
#     "required": ["name"]
# }), &IngestionPolicy::default())?;
let fields = schema.fields();

assert_eq!(fields[0].name, "name");
assert_eq!(fields[0].types, ["string", "null"]);
assert!(fields[0].required);
# Ok(())
# }
```

Each entry carries the field's name, its declared types, whether it is required, and a pointer to where it was declared. Conditionally required fields are reported as *not* required — because that is what they are.

### What changed between two versions?

```rust
# use adk_schema::{IngestionPolicy, InputSchema};
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let old = InputSchema::from_value(serde_json::json!({"type": "object"}), &IngestionPolicy::default())?;
# let new = InputSchema::from_value(serde_json::json!({"type": "object", "required": ["id"]}), &IngestionPolicy::default())?;
for change in old.diff(&new) {
    // Each change says whether it affects which values are accepted,
    // or is only descriptive (a reworded `description`, say).
    println!("{} — affects validation: {}", change.pointer, change.affects_validation());
}
# Ok(())
# }
```

### Is this the same schema as before?

`digest()` is a fingerprint that ignores differences that carry no meaning — whitespace, the order keys happen to be written in, and `5` versus `5.0`. Two schemas that say the same thing get the same digest, so you can use it as a cache key or a change detector.

### What did the provider quietly delete?

*Requires the `adapters` feature.*

```rust,ignore
use adk_schema::SchemaAdapterExt;

let projection = gemini_adapter.project(&canonical, &IngestionPolicy::default())?;

if projection.leaves_enforcement_gap() {
    // The model is being shown looser rules than the ones we will judge its
    // answer against. Either relax our side or re-check it ourselves.
    for lost in projection.substantive_losses() {
        tracing::warn!(pointer = %lost.pointer, "constraint dropped by provider");
    }
}
```

This works for every provider adapter in the ADK workspace without any of them being modified.

## Generating a schema from a Rust type

*Requires `schemars` and `runtime-validation`.*

Rather than hand-maintaining a JSON Schema alongside a Rust struct, derive one:

```rust
use adk_schema::OutputSchema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct ReservationDecision {
    accepted: bool,
    time: Option<String>,
    explanation: String,
}

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let model_json = serde_json::json!({"accepted": true, "time": null, "explanation": "ok"});
let schema = OutputSchema::for_type::<ReservationDecision>()?.compile()?;

schema.validate(&model_json)?;
let decision: ReservationDecision = serde_json::from_value(model_json)?;
# Ok(())
# }
```

`InputSchema` and `OutputSchema` are different types on purpose: a schema describing a function's *arguments* cannot be passed where one describing its *results* is expected. That mix-up is easy to make and hard to see, so the compiler catches it.

## Safety defaults

Schemas do not only come from your own code — they arrive from remote tool servers and from model output. So both entry points refuse rather than guess.

**Reading a schema** enforces limits on input size, nesting depth, node count, and number of references, and will not fetch a remote `$ref` over the network. Exceeding a limit is an error naming which limit it was, not a truncated result.

**Checking a value** stops after a bounded number of problems, and its messages name the rule that failed *without quoting the value that failed it* — those values are whatever a caller typed or a model produced, and they end up in logs.

```rust
# use adk_schema::ValidationOptions;
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let schema = adk_schema::InputSchema::from_value(
#     serde_json::json!({"properties": {"age": {"type": "integer"}}}),
#     &adk_schema::IngestionPolicy::default(),
# )?.compile()?;
# let instance = serde_json::json!({"age": "not a number"});
// Default: at most 100 problems reported, values masked.
let masked = schema.validate(&instance);

// Opt in where you know the value carries no user data.
let verbose = schema.validate_with(
    &instance,
    &ValidationOptions { include_instance_values: true, ..Default::default() },
);
# let _ = (masked, verbose);
# Ok(())
# }
```

Either way, every problem carries a JSON Pointer to where it occurred, and `SchemaError::InvalidInstance` tells you via `truncated` whether the list was cut short.

## What this crate does not do

| Handled here | Not handled here |
| --- | --- |
| Reading schemas, and the limits on doing so | Executing tools |
| Canonical form and fingerprint identity | Retries, callbacks, authorization |
| Optional validation of values | Your application's own policy |
| Reporting what a provider dropped | Deciding what to do about it |

## License

Licensed under the same terms as the ADK-Rust workspace. See [LICENSE](LICENSE).
