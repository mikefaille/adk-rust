# Implementation Plan: ADK Spatial OS (One Task at a Time)

## How this plan is used

- Tasks are intentionally linear.
- Only one active task is executed at a time.
- A task is marked complete only after code + validation.

## Completed Foundation

- [x] F1. Create `adk-spatial-os` crate scaffold and server entrypoint.
- [x] F2. Implement v0 shell transport routes (`apps`, `session`, `stream`, `prompt`, `event`).
- [x] F3. Define SSE/inbound protocol envelope and event types.
- [x] F4. Add session manager with per-session sequence and broadcast channel.
- [x] F5. Add in-memory app manifests and keyword intent routing.
- [x] F6. Add risk tiers and approval-required flow.
- [x] F7. Add timeline + audit entry scaffolding.
- [x] F8. Build full-width shell layout with dock/workspace/sidebar alignment.
- [x] F9. Add practical spatial rendering for workspace, sidebar cards, and dock.

## Active Queue

- [x] T1. Surface interaction: drag, focus-to-front, z-order update (workspace windows)
  - [x] T1.1 Pointer drag support for surface windows.
  - [x] T1.2 Focused surface lifts and gets highest z-order.
  - [x] T1.3 Emit `workspace_layout_change` on pointer-up with updated layout snapshot.
  - [x] T1.4 Validate manually in shell and with `cargo check -p adk-spatial-os`.
  - _Requirements: 1, 4, 7, 8_

- [x] T2. Spatial constraints: snap grid + workspace bounds
  - [x] T2.1 Snap surface position to configurable grid.
  - [x] T2.2 Clamp surface movement to stage bounds.
  - [x] T2.3 Include snapped layout in outbound/inbound event logs.
  - _Requirements: 1, 4, 8_

- [x] T3. Layout continuity
  - [x] T3.1 Persist latest layout in session context.
  - [x] T3.2 Restore layout when stream reconnects or prompt reroutes apps.
  - [x] T3.3 Add regression test for layout continuity.
  - _Requirements: 4, 8, 9_

- [x] T4. Keyboard-first shell controls
  - [x] T4.1 Global shortcut for prompt focus (`Ctrl/Cmd+K`).
  - [x] T4.2 App focus cycling shortcut.
  - [x] T4.3 Visible focus rings and keyboard navigation verification.
  - _Requirements: 1, 10_

- [x] T5. Runtime hardening and contract tests
  - [x] T5.1 Add contract tests for outbound SSE event shape/version.
  - [x] T5.2 Add integration test for dangerous prompt approval flow.
  - [x] T5.3 Add integration test for `workspace_layout_change` handling.
  - _Requirements: 6, 7, 9_

- [x] T6. App runtime bridge expansion
  - [x] T6.1 Replace keyword-only routing with capability-scored routing.
  - [x] T6.2 Add app command dispatch path in host bridge.
  - [x] T6.3 Add explicit handoff allow/deny flow with timeline entries.
  - _Requirements: 2, 3, 5, 11_

## Execution Rule

Current active task: `Completed`.
Post-plan scope can now be proposed as Phase 2 extension tasks.

## Phase 2 Extensions

- [x] E1. Optional persistent session storage backend
  - [x] E1.1 Wire server startup to read `ADK_SPATIAL_OS_STATE_PATH`.
  - [x] E1.2 Route AppState session manager through persistence-path constructor.
  - [x] E1.3 Add regression test for context restore through AppState wiring.
  - _Requirements: 8, 9_

- [x] E2. Command palette discoverability pass
  - [x] E2.1 Add keyboard hinting and command suggestions.
  - [x] E2.2 Add command examples in shell UI for first-run.
  - _Requirements: 2, 10_

- [x] E3. Handoff policy controls
  - [x] E3.1 Add per-app handoff allowlist in manifest/runtime.
  - [x] E3.2 Surface policy decisions in trust panel.
  - _Requirements: 5, 6_
