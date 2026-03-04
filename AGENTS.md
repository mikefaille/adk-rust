# ADK-Rust

**Context:** Modular workspace for building AI agents in Rust.

## Environment & Tooling

**Setup:**
- Use `devenv shell` to enter the reproducible development environment.
- Run `make setup` to install `sccache` and system dependencies if `devenv` is unavailable.
- Copy `.env.example` to `.env` for local API keys. **Never commit `.env` or secrets.**

**Quality Gates (Run before commit):**
1. **Format:** `cargo fmt --all` (or `devenv shell fmt`)
2. **Lint:** `cargo clippy --workspace --all-targets -- -D warnings` (or `devenv shell clippy`)
3. **Test:** `cargo test --workspace` (or `devenv shell test`)

**Build:**
- Standard: `cargo build --workspace`
- **Exception:** `adk-mistralrs` is excluded from the workspace due to heavy GPU dependencies. Build explicitly:
  ```bash
  cargo build -p adk-mistralrs
  ```

## Preconditions & Constraints

- **Do NOT commit `.env` files.**
- **Do NOT use `println!` or `eprintln!` in library code.** Use `tracing` macros (`info!`, `warn!`, `error!`, `debug!`).
- **Do NOT disable strict linting.** Warnings are treated as errors (`-D warnings`). Fix them.
- **Do NOT mix async runtimes.** Use `tokio` exclusively.
- **Must use `sccache`.** Ensure `RUSTC_WRAPPER=sccache` is set (handled automatically in `devenv shell`).
- **Must use `async-trait`.** Apply `#[async_trait]` to all async trait definitions and implementations.
- **Must use `thiserror`.** Define library errors using `thiserror::Error` (except in legacy `adk-gemini` code using `snafu`).
- **Mark external tests.** Tests requiring API keys or network access must be marked `#[ignore]`.

## Code Standards & Examples

### Serialization
- **REST APIs:** `#[serde(rename_all = "camelCase")]`
- **Internal/Rust:** `snake_case` (default)

### Feature Gating
When adding optional dependencies, gate both the module and the re-export:

```rust
// lib.rs
#[cfg(feature = "my-feature")]
pub mod my_module;

#[cfg(feature = "my-feature")]
pub use my_module::MyType;
```

### Error Handling
Use `thiserror` for library errors to ensure proper display and source chaining:

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MyError {
    #[error("network request failed: {0}")]
    Network(#[from] reqwest::Error),

    #[error("invalid configuration: {details}")]
    Config { details: String },
}
```

### Logging
Structured logging with `tracing`:

```rust
// Preferred: structured fields
tracing::info!(user_id = %user.id, "processing request");

// Avoid: strict string formatting
// tracing::info!("processing request for user {}", user.id);
```
