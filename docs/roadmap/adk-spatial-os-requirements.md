# Requirements Document: ADK Spatial OS (Agentic App Shell)

## Introduction

This specification defines an AI-native operating shell for ADK-Rust where:

- users work in a familiar desktop interaction model,
- one Master Prompt routes normal-language intent,
- apps are ADK-Rust agents,
- spatial depth improves context without adding complexity.

## Status Snapshot (February 8, 2026)

Current implementation already includes:

- `adk-spatial-os` server routes for app catalog, session, SSE stream, prompt submit, and inbound events.
- SSE + HTTP contract (`shell_state`, `app_surface_ops`, `timeline_entry`, `approval_required`, `notification`, `done`, `error`, `ping`).
- In-memory app catalog and keyword-based prompt routing.
- Risk classification (`safe`, `controlled`, `dangerous`) and approval gate flow.
- Session context, timeline events, and basic audit entries.
- Optional persisted session/context snapshots via `ADK_SPATIAL_OS_STATE_PATH`.
- Full-width shell UI with spatial workspace depth, spatial sidebar cards, and spatial dock interactions.
- Drag/focus/snap/bounds workspace interactions with layout-change emission and restore.
- Keyboard-first controls (`Cmd/Ctrl+K`, app focus cycling).
- Capability-scored routing, command dispatch bridge path, and explicit handoff approval flow.
- Prompt discoverability hints with adaptive first-run suggestion chips.
- Per-app handoff allowlists with runtime policy enforcement and trust-panel policy decision display.
- Contract and integration tests for SSE envelope shape, approvals, and workspace layout continuity.

The remaining work is focused on production hardening and deeper runtime behaviors, not on reinventing the baseline architecture.

## Scope

### In Scope

- AI-native shell with Master Prompt, dock, workspace, timeline, trust panel.
- Spatial desktop interactions grounded in mouse/keyboard conventions.
- ADK-Rust app runtime integration with manifest/capability metadata.
- Safety, approvals, auditability, and explainability.
- Reliability, accessibility, and developer adoption path.

### Out of Scope

- Headset-first XR product.
- Game mechanics, sci-fi metaphors, cinematic navigation.
- Full native OS process replacement.

## Product Principles

1. Familiar first.
2. AI-native second.
3. Spatial by utility, not spectacle.
4. Transparent agent behavior.
5. Explicit safety for risky actions.

## Requirements

### Requirement 1: Familiar Desktop Baseline

**User Story:** As a mainstream user, I want the shell to behave like a desktop I already know.

#### Acceptance Criteria

1. The shell SHALL provide launcher/dock, running app state, focused app state, timeline, and notifications.
2. The shell SHALL maintain a full-width aligned layout grid (dock, workspace, side panel) with consistent column edges.
3. Users SHALL be able to complete core workflows with mouse + keyboard only.

### Requirement 2: Master Prompt as Primary Intent Entry

**User Story:** As a user, I want one place to ask for what I need in normal language.

#### Acceptance Criteria

1. The shell SHALL expose a global Master Prompt accessible from any app context.
2. Prompt input SHALL not require protocol, code, or 3D wording.
3. The shell SHALL show routing rationale and selected app set for each prompt.

### Requirement 3: Agent Apps as First-Class OS Apps

**User Story:** As a developer, I want my ADK-Rust agent to run like a native app in the shell.

#### Acceptance Criteria

1. Each app SHALL have an app manifest with identity, capabilities, permissions, and default risk.
2. App surfaces SHALL mount/unmount through shell lifecycle events.
3. App runtime integration SHALL be compatible with ADK-Rust agent/runner primitives.

### Requirement 4: Practical Spatial Workspace

**User Story:** As a user, I want spatial context to help task flow without complexity.

#### Acceptance Criteria

1. Workspace SHALL support depth-aware surface composition from deterministic props (`x`, `y`, `w`, `h`, `z_index`).
2. Dock and sidebar SHALL support subtle spatial motion (depth, tilt, parallax) without requiring 3D camera controls.
3. A 2D-safe fallback mode SHALL preserve the same semantics on constrained/mobile layouts.

### Requirement 5: Multi-App Agent Collaboration

**User Story:** As a user, I want apps to collaborate without manually copying context.

#### Acceptance Criteria

1. The shell SHALL support explicit cross-app handoff events.
2. Handoffs SHALL be visible in timeline entries.
3. Users SHALL be able to allow/deny handoff decisions.

### Requirement 6: Safety, Trust, and Approval Gates

**User Story:** As an operator, I need assurance that high-risk actions cannot execute silently.

#### Acceptance Criteria

1. Actions SHALL be classified by risk tier (`safe`, `controlled`, `dangerous`).
2. Dangerous actions SHALL require explicit user approval in-shell.
3. Approval decisions SHALL be logged with action, app, risk, timestamp, and outcome.

### Requirement 7: Stable Shell Runtime Contract

**User Story:** As an integrator, I want a stable contract between shell and apps.

#### Acceptance Criteria

1. v0 transport SHALL use SSE outbound and HTTP POST inbound.
2. SSE payloads SHALL be versioned and contract-tested.
3. Inbound events SHALL include prompt submit, focus, command, approval, and layout changes.

### Requirement 8: Session Continuity and Explainability

**User Story:** As a user, I want to understand what happened and continue where I left off.

#### Acceptance Criteria

1. Session context SHALL persist active apps, focus, prompt history, and pending approvals.
2. Timeline SHALL include routing, focus changes, approvals, and important system events.
3. Users SHALL be able to inspect execution rationale from the shell UI.

### Requirement 9: Reliability and Latency

**User Story:** As a user, I need responsive interactions and reliable long-running sessions.

#### Acceptance Criteria

1. Stream transport SHALL include keepalive and bounded reconnect behavior.
2. Prompt-to-first-shell-update latency SHALL be instrumented and measured.
3. Stale/lagging stream clients SHALL degrade gracefully without crashing the session.

### Requirement 10: Accessibility and Inclusive UX

**User Story:** As a user, I need accessible interaction without losing capability.

#### Acceptance Criteria

1. Shell controls SHALL support keyboard navigation and visible focus states.
2. Critical actions SHALL be operable without pointer-only interactions.
3. Reduced-motion mode SHALL be respected for animated spatial effects.

### Requirement 11: Developer Experience for App Authors

**User Story:** As an ADK-Rust developer, I want a low-friction path to publish and run apps.

#### Acceptance Criteria

1. The repo SHALL provide app manifest and runtime integration docs.
2. Example apps SHALL demonstrate single-app and multi-app behavior.
3. Integration guidance SHALL cover permissions, risks, and approvals.

### Requirement 12: Migration and Compatibility

**User Story:** As a maintainer, I want to evolve from prototypes without discarding useful substrate.

#### Acceptance Criteria

1. Existing prototype components SHALL be documented as reusable or deprecated.
2. Migration mapping SHALL describe prototype-to-spatial-shell concept changes.
3. Contract compatibility SHALL be validated by tests before deprecating old pathways.
