# v0.8.0 Performance Validation Examples

The live validation example at `adk-rust/examples/performance_0_8_llm_agents.rs` runs 12 real LLM agents using provider credentials from the environment or repo `.env`. Each agent maps one v0.8.0 optimization to a user-friendly adoption scenario and validates the response internally without printing model output:

1. `scaffold_advisor` validates current `cargo-adk` templates for a small support bot.
2. `install_doctor` explains the rustls-only install path for OpenSSL-friction cases.
3. `starter_agent` validates the true minimal tier for a simple reminder agent.
4. `cli_provider_advisor` shows provider-specific CLI installs.
5. `telemetry_triage` separates local tracing from OTLP export.
6. `tooling_advisor` keeps MCP out of local function-tool agents.
7. `gemini_debug_advisor` keeps Gemini backtraces debug-only.
8. `session_budgeter` covers empty state-delta session writes.
9. `history_window_support` uses `RunConfig.history_max_events`.
10. `privacy_observer` caps trace payload bytes.
11. `operations_dispatcher` uses `RunConfig.max_tool_concurrency`.
12. `cache_operator` validates cache lifecycle work that does not hold the cache mutex across network calls.

Run it locally with:

```bash
cargo run -p adk-rust --example performance_0_8_llm_agents --features openrouter
```
