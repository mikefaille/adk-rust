# adk-schema

A provider-neutral JSON Schema contract layer for LLM applications.

`adk-schema` helps an application define structured-data rules once, progressively fulfill those rules through an interaction, adapt them to different LLM provider dialects, and validate the final result against the original contract.

[![Crates.io](https://img.shields.io/crates/v/adk-schema.svg)](https://crates.io/crates/adk-schema)
[![Documentation](https://docs.rs/adk-schema/badge.svg)](https://docs.rs/adk-schema)
[![License](https://img.shields.io/crates/l/adk-schema.svg)](LICENSE)

> **Project provenance and non-affiliation**
>
> `adk-schema` is an independent extension maintained in this fork of [ADK-Rust](https://github.com/zavora-ai/adk-rust). It is not an upstream ADK-Rust crate and is not maintained, sponsored, or endorsed by Zavora Technologies Ltd. or Google. References to ADK-Rust, Google, and Agent Development Kit are descriptive and identify the ecosystem and compatibility context only.

## Why this exists

LLM applications increasingly exchange structured data rather than only free-form text. A model may need to collect or produce:

- a caller's name and callback number;
- an order or reservation;
- an appointment request;
- a support case;
- the arguments for a software tool;
- a typed structured response.

Those rules are often described with JSON Schema.

The hard part is not merely validating one finished JSON object. In a real application, the contract has a lifecycle:

```text
application rules
      ↓
canonical schema
      ↓
partial information collected over time
      ↓
requirements become satisfied, active, or still undecided
      ↓
provider-specific schema projection
      ↓
model/tool interaction
      ↓
final canonical validation
```

Two different forms of evolution matter:

1. **The interaction evolves.** More information is collected, so conditional requirements become satisfied or newly applicable.
2. **The contract evolves.** A later schema revision may change the rules, so the application needs a stable way to identify which contract it is using.

`adk-schema` keeps those concerns explicit instead of treating a provider's wire schema as the application's source of truth.

## A living contract during an interaction

Consider a restaurant assistant. The initial contract may only need to know what kind of request the caller has:

```text
request_kind = ?
```

The caller says:

```text
"I'd like to order two lasagnas."
```

The application now has:

```text
request_kind = order
order_details = two lasagnas
```

Because this is an order, the schema may activate additional obligations:

```text
caller_name       required
callback_number   required
```

After the caller provides their name, the same contract can answer:

```text
fulfilled:
  request_kind
  order_details
  caller_name

outstanding:
  callback_number
```

The application does not need a separate hard-coded conversational state machine for every combination of fields. The schema describes the obligations, and `adk-schema` can evaluate them against what has been interactively fulfilled so far.

This pattern is business-agnostic. A dentist, restaurant, plumber, consultant, lawyer, accountant, retailer, or support desk can use different schemas while the runtime keeps the same contract-evaluation mechanism.

## Provider-neutral authority

Different model providers support different JSON Schema subsets and dialects. A provider projection can therefore be weaker or stricter than the application's canonical contract.

That creates an important failure mode:

```text
canonical contract
      │
      ├──────────────► runtime still enforces the original rules
      │
      ▼
provider projection
      │
      └──────────────► model sees a modified subset of those rules
```

If the projection drops a meaningful constraint, the model can produce something that looks valid according to the schema it saw but is later rejected by the application.

`adk-schema` preserves the canonical document and treats provider schemas as projections of it. With the optional ADK adapter integration, a projection can be inspected for substantive differences and enforcement gaps instead of silently replacing the original contract.

## Contract evolution

The contract itself may also change over time.

For example:

```text
revision A
  order requires name + callback number

revision B
  delivery orders additionally require a service address
```

Canonical bytes and a content-derived digest give each canonical schema a stable identity. Equivalent serialization changes do not create a different identity just because object keys were reordered or formatting changed.

`adk-schema` does not decide application migration policy. It provides the stable document identity and structured comparison primitives an application can use to make version changes explicit instead of silently changing the rules underneath an interaction.

## What the crate provides

The public API follows one schema-contract lifecycle rather than a collection of unrelated helpers.

### 1. Safe ingestion

Schemas can come from local Rust types, configuration, remote tool servers, MCP servers, or other runtime sources.

`InputSchema::from_value`, `OutputSchema::from_value`, and `from_json_slice` ingest them under an `IngestionPolicy` that can bound:

- source bytes;
- canonical bytes;
- nesting depth;
- node count;
- reference count;
- reference behavior.

External file and network `$ref` values are rejected by the default local-reference policy rather than resolved implicitly.

### 2. Typed input and output roles

`InputSchema` and `OutputSchema` are different Rust types.

That lets APIs distinguish contracts describing data sent **into** a tool/model boundary from contracts describing structured data expected **out of** a model.

Passing one role where the other is required can fail at compile time instead of becoming a runtime convention.

### 3. Canonical identity

Every ingested schema carries:

- a canonical JSON value;
- canonical UTF-8 bytes;
- a `SchemaDigest`;
- structural metrics;
- its schema role and dialect.

`canonical_bytes()` and `digest()` make schema identity independent of irrelevant serialization differences such as key order or whitespace.

### 4. Interactive requirement analysis

`outstanding()` answers a different question from final validation:

> Given the information collected so far, what does this contract still require?

It understands requirements carried by constructs such as:

- `required`;
- `dependentRequired`;
- `dependentSchemas`;
- nested `allOf`;
- `if` / `then` / `else` conditions.

When a condition cannot yet be decided, the result reports that state rather than silently waiving the obligation.

### 5. Provider projection and semantic loss

With the optional `adapters` feature, `SchemaAdapterExt::project()` applies an ADK-Rust `SchemaAdapter` while retaining the relationship between the canonical document and the projected document.

A projection can then report:

- structured differences;
- substantive constraint losses;
- whether the projection may leave an enforcement gap.

This complements `SchemaAdapter::normalize_schema()`: normalization answers **"what should I send?"**, while projection analysis also asks **"what changed relative to what my application still enforces?"**

### 6. Runtime validation

With the `runtime-validation` feature, a canonical document can be compiled once into a `ValidatedSchemaDocument` and used to validate model- or caller-supplied instances.

Validation issues include JSON pointers to the failing locations. Instance values are masked by default so validation diagnostics do not unnecessarily echo sensitive caller- or model-supplied data.

### 7. Schema inspection and transformation

The crate also exposes supporting operations such as:

- `fields()` for flat field metadata;
- `diff()` for structured schema comparisons;
- `flatten_static_all_of()` for static `allOf` composition that can be safely merged;
- structural metrics for observability and policy decisions.

These operations support the same canonical contract rather than creating parallel schema representations.

## Quick start

### Constrain and validate an LLM response with a Rust type

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
# let model_json = serde_json::json!({
#     "accepted": true,
#     "time": null,
#     "explanation": "ok"
# });
let schema = OutputSchema::for_type::<ReservationDecision>()?.compile()?;

schema.validate(&model_json)?;
let decision: ReservationDecision = serde_json::from_value(model_json)?;
# let _ = decision;
# Ok(())
# }
```

The Rust type generates the schema and the same contract validates the model's structured result.

### Evaluate what is still owed in a multi-turn interaction

```rust
use adk_schema::{IngestionPolicy, InputSchema};
use serde_json::json;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let schema = InputSchema::from_value(
    json!({
        "type": "object",
        "properties": {
            "request_kind": {
                "type": "string",
                "enum": ["information", "order"]
            },
            "order_details": { "type": "string" },
            "caller_name": { "type": "string" },
            "callback_number": { "type": "string" }
        },
        "required": ["request_kind"],
        "allOf": [{
            "if": {
                "properties": {
                    "request_kind": { "const": "order" }
                },
                "required": ["request_kind"]
            },
            "then": {
                "required": [
                    "order_details",
                    "caller_name",
                    "callback_number"
                ]
            }
        }]
    }),
    &IngestionPolicy::default(),
)?;

let collected = json!({
    "request_kind": "order",
    "order_details": "two lasagnas",
    "caller_name": "Ada"
});

let state = schema.outstanding(collected.as_object().unwrap());

assert!(state.missing.contains("callback_number"));
assert!(!state.is_complete());
# Ok(())
# }
```

The schema remains the source of truth for what the interaction has fulfilled and what it still owes.

### Ingest a schema arriving at runtime

```rust
use adk_schema::{IngestionPolicy, InputSchema};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let schema = InputSchema::from_value(
    serde_json::json!({
        "type": "object",
        "properties": {
            "query": { "type": "string" }
        },
        "required": ["query"]
    }),
    &IngestionPolicy::default(),
)?;

println!("digest: {}", schema.digest());
println!("nodes: {}", schema.metrics().node_count);
# Ok(())
# }
```

### Detect what a provider projection changed

```rust
use adk_schema::SchemaAdapterExt;

# fn inspect(
#     adapter: &impl adk_core::SchemaAdapter,
#     canonical_schema: &adk_schema::InputSchema,
#     policy: &adk_schema::IngestionPolicy,
# ) -> Result<(), Box<dyn std::error::Error>> {
let projection = adapter.project(canonical_schema, policy)?;

for loss in projection.substantive_losses() {
    println!(
        "pointer={} keyword={} kind={:?}",
        loss.pointer,
        loss.keyword,
        loss.kind
    );
}

if projection.leaves_enforcement_gap() {
    println!("provider projection may be weaker than canonical enforcement");
}
# Ok(())
# }
```

## Installation

```toml
[dependencies]
adk-schema = "2.0.0"
```

Generate schemas from Rust types and validate instances:

```toml
adk-schema = {
    version = "2.0.0",
    features = ["schemars", "runtime-validation"]
}
```

Enable integration with ADK-Rust `SchemaAdapter` implementations:

```toml
adk-schema = {
    version = "2.0.0",
    features = ["adapters"]
}
```

| Feature | Purpose | Optional dependency |
| --- | --- | --- |
| *(none)* | bounded ingestion, canonicalization, digest, diff, fields, outstanding requirements | none beyond the crate's core dependencies |
| `schemars` | derive schemas from Rust types | `schemars` |
| `runtime-validation` | compile and validate JSON instances | `jsonschema` |
| `adapters` | project canonical schemas through ADK-Rust `SchemaAdapter` implementations | `adk-core` |

## Runnable examples

```bash
# Multi-turn interaction state: determine what information is still owed
cargo run -p adk-schema --example what_to_ask_next

# Provider projection: report rules removed from the canonical contract
cargo run -p adk-schema --features adapters --example what_the_provider_dropped
```

## Production motivation

This abstraction grew out of real downstream failures rather than only API design.

In a Voice Gateway pilot, a Gemini Live provider projection removed constraints that the runtime still enforced. The model was therefore shown a weaker contract than the application used to validate its result, which contributed to a caller-visible repair turn.

Keeping the canonical schema separate from the provider projection made that mismatch observable: the projection reported the constraints that had disappeared, while the canonical validator continued to represent the application's actual rules.

The lesson is broader than that provider-specific incident:

> A provider wire schema is a transport representation of a contract, not necessarily the contract itself.

`adk-schema` exists to preserve that distinction.

## Architecture

```text
                     Application / business rules
                               │
                               ▼
                     Canonical SchemaDocument
                    /          |           \
                   /           |            \
                  ▼            ▼             ▼
        partial interaction   stable       runtime
             evaluation       identity     validation
             outstanding()    digest()     compile()
                  │                            ▲
                  │                            │
                  ▼                            │
           information still owed             │
                                               │
                         provider projection   │
                               │               │
                               ▼               │
                        LLM wire contract      │
                               │               │
                               ▼               │
                          model/tool result ───┘
```

The application owns the canonical rules. Provider adapters translate those rules for a model. The final result is still judged against the authoritative contract.

## Ownership boundary

| `adk-schema` owns | `adk-schema` does not own |
| --- | --- |
| canonical JSON Schema documents | application business policy |
| bounded ingestion and reference policy | provider transport protocols |
| input/output schema roles | tool execution and side effects |
| canonical bytes and schema identity | authorization decisions |
| interactive outstanding-requirement analysis | conversation wording or prompting strategy |
| structured schema diffing and projection-loss analysis | retries and network lifecycle |
| optional runtime instance validation | persistence and domain state |

## Design rule

The core rule is simple:

> **Define the contract once, progressively fulfill it through interaction, adapt it safely to the provider, and validate the completed result against the original rules.**

This turns JSON Schema from only a final validation document into an inspectable contract that can be evaluated continuously as an LLM interaction evolves.

## License

`adk-schema` is distributed under the Apache License 2.0, consistent with the ADK-Rust workspace. See [LICENSE](LICENSE).

The license applies to the source code and does not grant rights to third-party names or branding. ADK-Rust, Google, and Agent Development Kit are referenced only to describe project provenance, compatibility, and ecosystem context; no affiliation, sponsorship, or endorsement is implied.
