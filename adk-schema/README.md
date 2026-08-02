# adk-schema

Canonical JSON Schema documents and validation for ADK — bounded ingestion, compile-time input/output roles, and content-addressed identity.

[![Crates.io](https://img.shields.io/crates/v/adk-schema.svg)](https://crates.io/crates/adk-schema)
[![Documentation](https://docs.rs/adk-schema/badge.svg)](https://docs.rs/adk-schema)
[![License](https://img.shields.io/crates/l/adk-schema.svg)](LICENSE)

## Overview

`adk-schema` owns provider-neutral Draft 2020-12 schema documents for the [ADK-Rust](https://github.com/zavora-ai/adk-rust) framework:

- **`SchemaDocument<Input>` / `SchemaDocument<Output>`** — input and output schemas are distinct types, so passing one where the other belongs fails to compile.
- **Bounded ingestion** — source bytes, canonical bytes, depth, node count, and reference count are all capped, because schemas arrive from remote tool servers as well as from local code.
- **Canonical bytes and a stable digest** — key order and integral-float spelling do not change a schema's identity.
- **Local `$ref` only** — external file and network references are rejected, not resolved.
- **Optional Schemars generation** — derive a schema from a Rust type, deserialize direction for inputs and serialize direction for outputs.
- **Optional runtime validation** — compile once, validate many, with instance values masked out of error messages by default.

## Installation

```toml
[dependencies]
adk-schema = "2.0.0"

# Generate schemas from Rust types and validate instances against them
adk-schema = { version = "2.0.0", features = ["schemars", "runtime-validation"] }
```

`default-features = false` builds without either dependency.

| Feature | Enables |
| --- | --- |
| *(none)* | Ingestion, canonicalization, digests, reference policy |
| `schemars` | `SchemaDocument::for_type::<T>()` |
| `runtime-validation` | `SchemaDocument::compile()` and instance validation |

## Quick Start

### Constrain an LLM response with a Rust type

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

No separate hand-maintained JSON Schema is needed to constrain and validate the response.

### Ingest a schema that arrives at runtime

```rust
use adk_schema::{IngestionPolicy, InputSchema};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let schema = InputSchema::from_value(
    serde_json::json!({
        "type": "object",
        "properties": { "query": { "type": "string" } },
        "required": ["query"]
    }),
    &IngestionPolicy::default(),
)?;

// Equivalent schemas share an identity regardless of key order or number form.
println!("{}", schema.digest());
# Ok(())
# }
```

### Control what validation errors reveal

Instance values are masked by default, because the instance is usually caller- or model-supplied:

```rust
use adk_schema::ValidationOptions;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let schema = adk_schema::InputSchema::from_value(
#     serde_json::json!({"properties": {"age": {"type": "integer"}}}),
#     &adk_schema::IngestionPolicy::default(),
# )?.compile()?;
# let instance = serde_json::json!({"age": "not a number"});
// Default: at most 100 issues, values masked.
let masked = schema.validate(&instance);

// Opt in where the instance is known not to carry user data.
let verbose = schema.validate_with(
    &instance,
    &ValidationOptions { include_instance_values: true, ..Default::default() },
);
# let _ = (masked, verbose);
# Ok(())
# }
```

Every issue carries the JSON pointer of the failing location either way. `SchemaError::InvalidInstance` reports `truncated` when collection stopped at the limit.

## Ownership boundary

| `adk-schema` owns | `adk-schema` does not own |
| --- | --- |
| Canonical schema documents | Provider-specific projection rules |
| Ingestion limits and reference policy | Tool handler execution |
| Canonical bytes and digest identity | Retries, callbacks, authorization |
| Optional compilation and instance validation | Application policy, database or transport schemas |

## License

Licensed under the same terms as the ADK-Rust workspace. See [LICENSE](LICENSE).
