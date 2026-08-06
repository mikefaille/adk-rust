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

`default-features = false` builds without any optional dependencies for zero-overhead, pure ingestion environments.

| Feature | Description & Utility | Dependencies |
| --- | --- | --- |
| *(none)* | Ingestion, canonicalization, SHA-256 digests, reference policy | `serde`, `serde_json`, `sha2` |
| `schemars` | Derive `SchemaDocument::for_type::<T>()` directly from Rust structs | `schemars` |
| `runtime-validation` | High-performance zero-allocation instance validation via `compile()` | `jsonschema` |
| `adapters` | Trait adapters for automatic projection onto provider dialects | *(none)* |

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

### Determine "What To Ask Next" in Multi-Turn Conversations

```rust
use adk_schema::{IngestionPolicy, InputSchema};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let schema = InputSchema::from_value(
#     serde_json::json!({
#         "properties": {
#             "kind": { "type": "string" },
#             "name": { "type": "string" }
#         },
#         "required": ["kind"],
#         "allOf": [{
#             "if": { "properties": { "kind": { "const": "order" } }, "required": ["kind"] },
#             "then": { "required": ["name"] }
#         }]
#     }),
#     &IngestionPolicy::default(),
# )?;
let collected = serde_json::json!({ "kind": "order" });
let collected_obj = collected.as_object().unwrap();

// Evaluate missing obligations (including conditional ones)
let state = schema.outstanding(collected_obj);

assert!(!state.is_complete());
assert!(state.missing.contains("name")); // Asks for caller name because kind=="order"
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

## Runnable Examples

`adk-schema` includes executable examples demonstrating real-world agent orchestration and provider loss detection:

```bash
# 1. Multi-turn conversation state: determining what fields to ask the caller next
cargo run -p adk-schema --example what_to_ask_next

# 2. Provider projection loss: detecting what rules an LLM adapter silently stripped
cargo run -p adk-schema --features adapters --example what_the_provider_dropped
```

## Key Public API & Capabilities Reference

`adk-schema` exposes a rich set of strongly-typed public methods on `SchemaDocument<R>` (`InputSchema` / `OutputSchema`):

### 1. Ingestion & Document Construction
- **`InputSchema::from_value(value, &policy)`** / **`OutputSchema::from_value(value, &policy)`**  
  Ingests raw JSON Schema values into a bounded, canonical document. Validates recursion depth, node count, byte budget, and rejects unsafe external `$ref` pointers.
- **`SchemaDocument::for_type::<T>()`** *(requires feature `schemars`)*  
  Generates a compile-time type-safe schema directly from any Rust struct implementing `schemars::JsonSchema`.

### 2. Schema Flattening & Normalization
- **`schema.flatten_static_all_of()`**  
  Infallibly flattens top-level static `allOf` composite branches into merged `properties` and `required` arrays. Essential for compatibility with strict models (e.g. Gemini Live, OpenAI Strict) that fail when given `allOf` arrays:
  ```rust
  let flattened_schema = input_schema.flatten_static_all_of();
  ```
- **`schema.canonical_bytes()`**  
  Returns canonicalized UTF-8 JSON bytes with deterministic key ordering and standardized numeric representations.

### 3. Identity & Diffing
- **`schema.digest()`**  
  Computes a 32-byte content-addressed SHA-256 digest (`SchemaDigest`). Equivalent schemas produce identical digests regardless of whitespace, formatting, or key ordering.
- **`schema_a.diff(&schema_b)`**  
  Compares two schemas and produces structured diffs (`DifferenceKind`), identifying added/removed constraints, tightened types, or dropped required fields:
  ```rust
  let diffs = schema_a.diff(&schema_b);
  for diff in diffs {
      println!("Path: {}, Kind: {:?}", diff.path, diff.kind);
  }
  ```

### 4. Compilation & Runtime Validation
- **`schema.compile()`** *(requires feature `runtime-validation`)*  
  Compiles the schema document into a high-performance `ValidatedSchemaDocument` for zero-allocation instance validation.
- **`compiled.validate(&instance)`** / **`compiled.validate_with(&instance, &options)`**  
  Validates a JSON instance against the compiled schema. Supports value masking (to protect sensitive PII) and custom issue limits.

### 5. Field Analysis & Outstanding Requirements
- **`schema.fields()`**  
  Extracts a flat list of `FieldEntry` items describing every property path, type, and unconditional requirement:
  ```rust
  let fields = schema.fields();
  for field in fields {
      println!("Field: {}, Type: {:?}", field.path, field.types);
  }
  ```
- **`schema.outstanding(&present_fields)`**  
  Calculates unfulfilled required fields given a set of currently provided keys, accounting for conditional `if/then/else` dependencies:
  ```rust
  let present = serde_json::json!({ "request_kind": "order" });
  let state = schema.outstanding(present.as_object().unwrap());
  println!("Missing: {:?}, Undecided: {:?}", state.missing, state.undecided);
  ```

### 6. Adapter Projection & Loss Detection
*(requires feature `adapters`)*
- **`adapter.project(&schema, &policy)`**  
  Projects a canonical `InputSchema` through a provider adapter and detects dropped constraints:
  ```rust
  use adk_schema::SchemaAdapterExt;

  let projection = adapter.project(&canonical_schema, &policy)?;
  for loss in projection.substantive_losses() {
      println!("Path: {}, Keyword: {}, Loss: {:?}", loss.pointer, loss.keyword, loss.kind);
  }

  if projection.leaves_enforcement_gap() {
      println!("Warning: Provider adapter stripped enforcement rules!");
  }
  ```

## Architecture & Design Rationale

### The Core Problem: Multi-LLM Schema Divergence

In multi-provider agent systems, LLMs treat tool parameter specifications and structured output schemas with wildly conflicting constraints:

- **OpenAI Strict Mode**: Demands `additionalProperties: false` on every nested object, exact required arrays, and rejects conditional keywords like `allOf` or `oneOf`.
- **Anthropic (Claude)**: Requires standard JSON Schema Draft 2020-12, but fails on specific OpenAPI format strings or top-level `$schema` keywords.
- **Gemini (Studio & Vertex)**: Enforces an **OpenAPI subset** (or `parametersJsonSchema`) with a strict 64-byte tool name limit, rejecting `$schema` and unsupported formats like `int32`.
- **Moonshot (Kimi)**: Exposes an OpenAI-compatible REST endpoint (`https://api.moonshot.ai/v1`), but enforces tool name length `3..=64` and rejects invalid starting characters.

Without a centralized schema contract, agent frameworks degrade into fragile, ad-hoc `if provider == "gemini"` hacks scattered throughout tool handlers, execution loops, and network transport layers.

---

### Design Principles of `adk-schema`

`adk-schema` solves this by introducing a **three-tier schema abstraction**:

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                          1. Canonical Domain Schema                         │
│   adk-schema::InputSchema / OutputSchema (Draft 2020-12 + Digest + Role)    │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼  (Zero-Copy / Infallible Translation)
┌─────────────────────────────────────────────────────────────────────────────┐
│                           2. Pluggable SchemaAdapter                         │
│    GenericSchemaAdapter  │  GeminiSchemaAdapter  │  KimiSchemaAdapter, etc. │
└─────────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼  (Wire Projection)
┌─────────────────────────────────────────────────────────────────────────────┐
│                         3. LLM Provider Wire Payload                         │
│     OpenAI tools  │  Anthropic tool_choice  │  Gemini Live WebSocket        │
└─────────────────────────────────────────────────────────────────────────────┘
```

#### 1. Zero Mandatory Coupling, Modular & Fully Optional
`adk-schema` sits as a pure, focused utility crate with **zero mandatory dependencies on transport or models**. It is **100% optional and configurable**:
- **Feature Flags**: Heavy features like `schemars` (type derivation) and `runtime-validation` (`jsonschema` engine) are optional feature flags. With `default-features = false`, it compiles down to lightweight, zero-overhead ingestion and canonicalization using only core `serde_json` and `sha2`.
- **Pluggable Adapters**: Schema normalization policies are fully configurable. Applications can use `GenericSchemaAdapter`, provider presets (`GeminiSchemaAdapter`, `KimiSchemaAdapter`), or define custom domain adapters via `impl SchemaAdapter`.

#### 2. Universal Schema Neutrality
Instead of treating schema normalization as provider-specific hackery, `adk-schema` elevates schema handling to a first-class framework primitive—just like error handling, tracing, or context propagation.

#### 3. Static `allOf` Flattening (`flatten_static_all_of`)
Tool definitions derived from complex composite models or inheritance hierarchies often generate top-level `allOf` arrays. Models like Gemini Live or OpenAI Strict Mode fail or degrade when given `allOf` conditional structures.
`adk-schema` provides static property and required-field merging (`InputSchema::flatten_static_all_of`), flattening nested static rules into clean, top-level `properties` and `required` arrays without losing field constraints or descriptions.

#### 4. Content-Addressed Schema Identity (Canonical Digest)
Remote tool servers (e.g. MCP / ADK Remote) and local tool registries dynamically register tool parameters. `adk-schema` computes a deterministic SHA-256 digest over canonicalized JSON bytes, guaranteeing that two schemas with different key orders or whitespace share identical content-addressed digests (`schema.digest()`).

---

### Architectural Q&A: Framework Design & Maintainer Review

#### Q1: "Is there tight coupling for non-standard functionality? Should cross-crate functionality live in `adk-core`?"
**No tight coupling exists.** `adk-core` provides cross-crate interfaces like the `SchemaAdapter` trait, while `adk-schema` acts as a pure, lightweight utility crate. It does not import `tokio`, HTTP clients, WebSocket engines, or model implementations.
- **Pure Utility**: `adk-schema` only parses, canonicalizes, flattens, and validates JSON Schema Draft 2020-12 documents.
- **Configurable Cargo Features**: Heavy features (`schemars` type derivation, `runtime-validation` validation engine) are optional. With `default-features = false`, `adk-schema` compiles to zero-overhead ingestion with zero model or transport dependencies.
- **Pluggable Integration**: Model clients (`adk-model`, `adk-realtime`, `adk-gemini`) consume `adk-schema` on an opt-in basis, keeping `adk-core` clean and preventing cross-crate coupling.

#### Q2: "The Gemini serialization issue was real because Gemini changed spec goalposts twice and had difficult tool calling schemas. Is `adk-schema` just a Gemini workaround?"
**No.** While Gemini's spec changes and strict WebSocket protocol exposed wire sensitivities first (abruptly closing live audio connections with WS 1007 on unhandled keywords), **schema handling is a universal requirement across all major providers**:
- **OpenAI Strict Mode**: Rejects nested objects without `additionalProperties: false` or incomplete `required` arrays.
- **Anthropic (Claude)**: Requires specific Draft 2020-12 keyword stripping and OpenAPI format sanitization.
- **Moonshot (Kimi)**: Enforces strict 64-character tool name limits and character set validation.
- **Ollama / Local LLMs**: Demand simplified OpenAPI schemas without `$schema` headers.

Without `adk-schema`, tool definitions would be forced to carry fragile `if provider == "gemini"` hacks throughout application code.

#### Q3: "Should schema handling be a standard framework primitive, just like error handling and tracing?"
**Yes.** Schema handling is a foundational framework primitive. Just as `tracing` standardizes logs and `AdkError` standardizes error states, `adk-schema` standardizes tool parameter definitions. Tool authors write parameter contracts **once** in standard JSON Schema / Rust types, and `adk-schema` safely translates them onto whichever LLM provider is active at runtime.

#### Q4: "Has this design been validated across all major LLM providers to avoid duplicate functionality?"
**Yes.** `adk-schema` and `SchemaAdapter` have been validated across:
1. **Gemini** (Studio REST & Vertex Multimodal Live WebSockets)
2. **OpenAI** (Chat Completions & Realtime API)
3. **Anthropic** (Messages API tool choice)
4. **Moonshot / Kimi** (OpenAI-compatible endpoints)
5. **Ollama / vLLM** (Local model execution)

This single standard prevents duplicated schema translation logic across `adk-core`, `adk-model`, `adk-realtime`, and downstream application crates.

---

### Critical Value in `adk-realtime` & Gemini Multimodal Live

The integration of `adk-schema` into `adk-realtime` solves **three catastrophic operational risks** specifically for Gemini Live real-time audio streams:

#### 1. Avoidance of Silent WebSocket Protocol Disconnects (WS 1007)
The Gemini Multimodal Live WebSocket API rejects invalid tool parameter dialects. If a JSON Schema containing `$schema`, `allOf`, or forbidden formats (e.g., `int32`, `int64`) is transmitted inside a `session.update` frame, the Gemini server does not return a readable JSON error: **it abruptly closes the WebSocket connection with error code 1007 (Invalid Frame Payload Data)**.
`adk-schema` cleans and normalizes tool parameter schemas before transmission, ensuring WebSocket connections remain open and stable.

#### 2. Wire Field Name Purity (`parameters` vs `parametersJsonSchema`)
Gemini supports two distinct schema dialects on the wire (`parameters` for OpenAPI subset, `parametersJsonSchema` for JSON Schema). Sending a dialect under the wrong JSON field name triggers an immediate WS 1007 disconnect.
`adk-schema` binds the field name directly to the adapter via `adapter.parameters_field()`. This guarantees that the dialect selection and the wire field key are paired atomically in a single place.

#### 3. Preserving Structured Obligations Across `allOf` Composition
In complex agent workflows (e.g. Zenith call confirmation nodes), tool declarations use composite schemas (`allOf` containing properties, required lists, and conditional rules). Without `adk-schema`'s `flatten_static_all_of()`, sending `allOf` to Gemini causes Gemini Live to ignore tool arguments or omit parameter collection entirely. `adk-schema` flattens static `allOf` branches into top-level properties and required fields while preserving verbatim conditional obligations for runtime verification.

---

## Ownership boundary

| `adk-schema` owns | `adk-schema` does not own |
| --- | --- |
| Canonical schema documents & Draft 2020-12 ingestion | Provider-specific wire transport |
| Ingestion limits, depth bounds, reference policy | Tool execution loops |
| Canonical bytes and content-addressed digests | Retries, callbacks, authorization |
| Static `allOf` flattening & runtime validation | Application business logic |

## License

Licensed under the same terms as the ADK-Rust workspace. See [LICENSE](LICENSE).
