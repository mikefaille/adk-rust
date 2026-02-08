# adk-spatial-os

`adk-spatial-os` is the Phase 1 scaffold for an AI-native shell where apps are ADK-Rust agents.

## What Exists

- Master Prompt route and orchestration scaffold
- App catalog and in-memory routing host
- Session-scoped SSE stream and inbound event routes
- Shell UI skeleton with:
  - Master Prompt bar
  - Dock / app switcher
  - Workspace surfaces
  - Timeline
  - Trust panel with approval controls

## Run

```bash
cargo run -p adk-spatial-os
```

Open `http://127.0.0.1:8199`.

Environment variables:

- `ADK_SPATIAL_OS_HOST` (default `127.0.0.1`)
- `ADK_SPATIAL_OS_PORT` (default `8199`)
- `ADK_SPATIAL_OS_STATE_PATH` (optional JSON file path for session/context persistence across restarts)

## Endpoints

- `POST /api/os/session`
- `GET /api/os/stream/{session_id}`
- `POST /api/os/prompt/{session_id}`
- `POST /api/os/event/{session_id}`
- `GET /api/os/apps`

## Notes

This is a contract-first scaffold. Full app execution via `adk-runner` and richer compositor behavior are planned in subsequent phases.
