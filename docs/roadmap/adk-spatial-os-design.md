# Design Document: ADK Spatial OS (Agentic App Shell)

## Overview

ADK Spatial OS is an AI-native shell that runs ADK-Rust agents as desktop-like apps.

Design goals:

- keep a familiar desktop flow,
- add practical spatial depth,
- make agent actions transparent and governable,
- keep runtime contracts deterministic.

## Current Implementation Baseline (February 8, 2026)

### Backend (`adk-spatial-os` crate)

Implemented:

- Axum routes:
  - `GET /api/os/apps`
  - `POST /api/os/session`
  - `GET /api/os/stream/{session_id}`
  - `POST /api/os/prompt/{session_id}`
  - `POST /api/os/event/{session_id}`
- SSE envelope + event taxonomy in `protocol.rs`.
- Session manager with per-session broadcast channels and sequence numbering.
- Master prompt routing via `InMemoryAgentHost`.
- Risk + approval flow with timeline and audit entries.
- Versioned SSE envelope (`v0`) with contract/integration tests.

### Frontend (`ui-shell/index.html`)

Implemented:

- full-width 3-column shell grid: dock, workspace, side panel.
- spatial workspace stage with depth-aware surfaces using `x/y/w/h/z_index` props.
- spatial side panel cards with layered depth and parallax highlights.
- spatial dock buttons with depth and pointer-reactive motion.
- keyboard-first controls (`Cmd/Ctrl+K`, app focus cycling with `Cmd/Ctrl+[ ]`).
- draggable windows with focus-to-front and layout snapshot emission.
- timeline, trust panel, runtime log wiring over SSE.
- prompt discoverability with shortcut hints and adaptive first-run suggestion chips.
- per-app handoff allowlists with runtime policy checks and trust-panel policy decision visibility.

Not yet implemented:

- richer command palette overlays (beyond inline hints/chips) and deeper discoverability flows.
- advanced policy controls for per-app handoff allowlists.

## Runtime Architecture

```text
User Input (prompt/click/approval/layout)
  -> Shell UI (HTML/CSS/JS spatial renderer)
  -> HTTP inbound event endpoint
  -> Shell Orchestrator + Session Manager
  -> App Host / ADK runtime bridge
  -> SSE outbound event stream
  -> Shell UI state update
```

## Data Contracts

### Outbound (SSE)

Event kinds:

- `shell_state`
- `app_surface_ops`
- `timeline_entry`
- `approval_required`
- `notification`
- `error`
- `done`
- `ping`

### Inbound (HTTP)

Event types:

- `master_prompt_submit`
- `app_focus`
- `app_command`
- `approval_decision`
- `workspace_layout_change`

## Spatial Composition Model

### Layout Zones

- Dock rail: left utility lane with app launch/focus controls.
- Workspace plane: primary task surfaces with depth sorting.
- Side panel stack: trust/timeline/log with layered card depth.

### Spatial Rules

- Depth is derived from deterministic values (`z_index`, card stack order).
- Motion is low-amplitude and assistive (orientation/context cues).
- Responsive fallback disables transforms and keeps the same information architecture.

## Safety and Explainability Model

- Risk tiers: `safe`, `controlled`, `dangerous`.
- Dangerous actions emit `approval_required` and block execution.
- Approval decisions feed timeline and audit log.
- Routing rationale is retained and shown in event/timeline payloads.

## Revised Design Decisions

1. Keep transport simple: SSE + HTTP for v0.
2. Keep UI deterministic: server emits semantic ops, client applies view transforms.
3. Keep shell framework-light for now: static shell is acceptable until runtime contracts stabilize.
4. Prioritize interaction quality next: drag/snap/persist before heavier visual effects.

## Task Execution Strategy (One-at-a-Time)

Implementation proceeds in strict order:

1. Surface drag/focus/z-order update loop.
2. Snap-to-grid and bounds constraints.
3. Persist and restore workspace layout per session.
4. Keyboard shortcuts for prompt and app focus.
5. Contract and integration tests for layout + approval flows.

Each task is implemented and validated before moving to the next.
