---
name: adk-rust-realtime-voice
description: Implement realtime voice agents with ADK-Rust, including audio formats, event handling, and streaming reliability. Use when building or debugging voice interactions in adk-realtime.
---

# ADK Rust Realtime Voice

## Overview
Build low-latency bidirectional audio flows with explicit session/event handling.

## Workflow
1. Select provider backend and audio format early.
2. Validate session configuration and VAD settings.
3. Handle client/server events with explicit match branches.
4. Verify tool call lifecycle in realtime paths.
5. Configure interruption detection (Manual or Automatic).
6. Use SessionUpdateConfig for mid-session context mutation (swap instructions/tools without dropping the call).

## Guardrails
1. Fail fast on unsupported modalities.
2. Keep audio encoding assumptions explicit.
3. Add coverage for cancellation, interruptions, and error events.
4. Use ContextMutationOutcome to handle provider differences (OpenAI: Applied, Gemini: RequiresResumption).

## References
- Use `references/realtime-voice-playbook.md`.
