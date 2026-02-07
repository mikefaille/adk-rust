# Implementation Plan: ADK-Go v0.4.0 Parity (Core Runtime)

## Overview

Prioritized checklist to close core runtime parity gaps between `adk-rust` and `adk-go` (v0.4.0 era) without breaking existing ADK-Rust APIs.

Scope focus:
- Runtime correctness and reliability
- Human-in-the-loop safety controls
- Plugin and MCP operational readiness
- Session and telemetry parity for enterprise workloads

Out of scope for this phase:
- UI redesign work
- Non-critical docs polish

## Checklist

- [x] 1. P0: Fix Database Session Persistence Correctness
  - [x] 1.1 Fix `DatabaseSessionService::append_event` to persist correct `app_name` and `user_id` (currently blank).
  - [x] 1.2 Update DB state on append (session/user/app state deltas) to match in-memory behavior.
  - [x] 1.3 Ensure `updated_at` is updated when events are appended.
  - [x] 1.4 Keep `temp:` state keys excluded from persistence.
  - [x] 1.5 Add regression tests for append behavior.
  - Files:
    - `adk-session/src/database.rs`
    - `adk-session/tests/database_tests.rs`

- [x] 2. P0: Enforce Session Delete Event Cascade Semantics
  - [x] 2.1 Make event cleanup explicit and deterministic on session delete.
  - [x] 2.2 Ensure migration/schema behavior is compatible with SQLite foreign key behavior.
  - [x] 2.3 Add regression test validating no orphaned events.
  - Files:
    - `adk-session/src/database.rs`
    - `adk-session/tests/database_tests.rs`

- [x] 3. P0: Wire Plugin Manager Into Runner Execution Path
  - [x] 3.1 Add `plugin_manager` to `RunnerConfig`.
  - [x] 3.2 Run `before_run`, `on_user_message`, `on_event`, and `after_run` in correct order.
  - [x] 3.3 Ensure plugin execution errors propagate consistently.
  - [x] 3.4 Add integration tests for plugin callback order and mutation behavior.
  - Files:
    - `adk-runner/src/runner.rs`
    - `adk-plugin/src/manager.rs`
    - `adk-runner/tests/*`

- [x] 4. P0: Activate Tool Callbacks in `LlmAgent`
  - [x] 4.1 Invoke `before_tool_callbacks` before each tool execution.
  - [x] 4.2 Allow callback short-circuit/override behavior.
  - [x] 4.3 Invoke `after_tool_callbacks` with tool result and apply modifications.
  - [x] 4.4 Add tests for callback order, override, and error handling.
  - Files:
    - `adk-agent/src/llm_agent.rs`
    - `adk-agent/tests/*`

- [x] 5. P0: Add Conversation-Level Telemetry Attribute
  - [x] 5.1 Add `gen_ai.conversation.id` to top-level and child spans.
  - [x] 5.2 Ensure value is stable across an invocation (`session_id`-anchored).
  - [x] 5.3 Add tests/assertions in telemetry exporter path.
  - Files:
    - `adk-runner/src/runner.rs`
    - `adk-agent/src/llm_agent.rs`
    - `adk-telemetry/src/span_exporter.rs`

- [x] 6. P1: First-Class Tool Confirmation (Human-in-the-Loop)
  - [x] 6.1 Add tool confirmation policy API (per tool/global).
  - [x] 6.2 Emit interrupt/confirmation-required event before guarded tools execute.
  - [x] 6.3 Add resume/deny flow and tests.
  - Files:
    - `adk-agent/src/llm_agent.rs`
    - `adk-core/src/*` (policy/event structures)
    - `adk-server/src/*` (resume flow if needed)

- [x] 7. P1: Make MCP Reconnection Operational by Default Path
  - [x] 7.1 Integrate `ConnectionRefresher` into primary MCP execution path.
  - [x] 7.2 Add EOF/connection-reset retry coverage.
  - [x] 7.3 Ensure behavior is configurable and observable.
  - Files:
    - `adk-tool/src/mcp/toolset.rs`
    - `adk-tool/src/mcp/reconnect.rs`
    - `adk-tool/tests/*`

- [x] 8. P1: Artifact Filename Hardening
  - [x] 8.1 Reject artifact names containing path separators or traversal patterns.
  - [x] 8.2 Add tests for valid and invalid names.
  - Files:
    - `adk-artifact/src/inmemory.rs`
    - `adk-artifact/tests/artifact_tests.rs`

- [x] 9. P1: Preserve Citation Metadata End-to-End
  - [x] 9.1 Extend core response model to carry citation metadata.
  - [x] 9.2 Map Gemini citation metadata into core response.
  - [x] 9.3 Add serialization and backward-compatibility tests.
  - Files:
    - `adk-core/src/model.rs`
    - `adk-model/src/gemini/client.rs`
    - `adk-model/tests/*`

- [x] 10. P2: Vertex AI Session Service
  - [x] 10.1 Add `VertexAiSessionService` implementation behind feature flag.
  - [x] 10.2 Support create/get/list/delete/append parity with existing session trait.
  - [x] 10.3 Add contract tests shared with current session service test suite (mock-backed transport).
  - [ ] 10.4 Add live Vertex integration acceptance test (ADC + real Reasoning Engine resources).
  - Files:
    - `adk-session/src/*`
    - `adk-session/tests/*`

## Acceptance Tests

### P0 Acceptance Tests

- [x] AT-P0-DB-01: `append_event` persists scoped identity correctly
  - Setup: Create session `(app_name=test_app, user_id=user1, session_id=session1)`.
  - Action: Append event with `state_delta` containing `app:`, `user:`, session, and `temp:` keys.
  - Verify:
    - Event row is linked to correct `app_name`/`user_id`/`session_id`.
    - Retrieved session state includes `app:` and `user:` merged values.
    - `temp:` keys are absent from persisted state.
    - `updated_at` advanced.

- [x] AT-P0-DB-02: deleting a session leaves zero orphan events
  - Setup: Create session and append at least one event.
  - Action: Delete session.
  - Verify:
    - Session no longer exists.
    - No events remain for deleted session key.

- [x] AT-P0-PLUG-01: plugin callbacks run in expected order
  - Setup: Runner configured with plugin manager containing deterministic callbacks.
  - Action: Execute one user run.
  - Verify:
    - Observed order: `before_run -> on_user_message -> agent execution -> on_event(s) -> after_run`.
    - Any plugin content mutation is reflected in downstream execution.

- [x] AT-P0-TOOL-01: before/after tool callbacks are active and effective
  - Setup: Agent with one tool and callback hooks.
  - Action: Model emits tool call.
  - Verify:
    - `before_tool` is invoked before tool execution.
    - Callback can short-circuit or modify tool args/result.
    - `after_tool` receives execution result and can transform it.

- [x] AT-P0-OTEL-01: spans include `gen_ai.conversation.id`
  - Setup: Run one full invocation with telemetry exporter enabled.
  - Verify:
    - `agent.execute`, `call_llm`, and `execute_tool*` spans carry `gen_ai.conversation.id`.
    - Value is stable for all spans in same invocation/session.

### P1 Acceptance Tests

- [x] AT-P1-CONF-01: guarded tool requires confirmation before execution
  - Setup: Tool marked as confirmation-required.
  - Action: Model emits that tool call.
  - Verify:
    - System emits interrupt/confirmation-required signal.
    - Tool does not execute until approval.
    - Reject path returns controlled non-execution outcome.

- [x] AT-P1-MCP-01: MCP call recovers from EOF/connection reset
  - Setup: MCP connection forced to fail with reconnectable error.
  - Action: Execute MCP tool call.
  - Verify:
    - One or more reconnect attempts occur.
    - Call succeeds or fails with bounded retries according to config.

- [x] AT-P1-ART-01: artifact filename validation blocks path traversal
  - Setup: Save artifact using names like `../secret`, `a/b.txt`, `a\\b.txt`.
  - Verify:
    - Invalid names are rejected with artifact validation error.
    - Normal names (e.g., `report.pdf`) still work.

- [x] AT-P1-CITE-01: citation metadata survives provider-to-core mapping
  - Setup: Gemini response includes citation metadata.
  - Verify:
    - Core response carries citation metadata with source URI/title/indexes.
    - Serialization/deserialization round-trip preserves fields.

### P2 Acceptance Tests

- [x] AT-P2-VERTEX-CONTRACT-01: Vertex session service passes shared contract tests (mock transport)
  - Setup: Run shared session service test suite against Vertex-backed implementation using local mock API.
  - Verify:
    - `create/get/list/delete/append_event` behavior matches trait expectations.
    - Isolation by `(app_name, user_id, session_id)` holds.

- [ ] AT-P2-VERTEX-LIVE-01: Vertex session service passes live contract tests
  - Setup: Run `adk-session/tests/session_contract_vertex_live.rs` with ADC and two real reasoning engines.
  - Verify:
    - `create/get/list/delete/append_event` succeeds against live Vertex Session Service.
    - Isolation by `(app_name, user_id, session_id)` holds in live backend.

## Verification Commands (Target)

- `cargo test -p adk-session --features database`
- `cargo test -p adk-session --features vertex-session`
- `cargo test -p adk-session --features vertex-session --test session_contract_vertex_live -- --ignored`
- `cargo test -p adk-runner`
- `cargo test -p adk-agent`
- `cargo test -p adk-tool`
- `cargo test -p adk-artifact`
- `cargo test -p adk-model`
- `cargo test -p adk-telemetry`
