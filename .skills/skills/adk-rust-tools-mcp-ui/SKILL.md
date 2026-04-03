---
name: adk-rust-tools-mcp-ui
description: Add ADK-Rust tools, MCP integrations, and ADK UI protocol outputs safely. Use when building function tools, MCP toolsets, browser tools, or UI render tools.
---

# ADK Rust Tools MCP UI

## Overview
Implement tools with strict schema contracts and protocol-aware outputs.

## Workflow
1. Start with `FunctionTool` and explicit JSON schema.
2. Add MCP toolsets with auth/reconnect configuration.
3. Add ADK UI render tools with protocol compatibility checks.
4. Validate tool protocol outputs with existing test matrices.
5. Use MCP Resource API (list_resources, list_resource_templates, read_resource) for server-provided data.
6. Use ElicitationHandler for MCP servers that request user input during tool execution.

## Guardrails
1. Reject ambiguous argument schemas.
2. Preserve protocol behavior for legacy and MCP Apps compatibility.
3. Add tests for invalid inputs and auth failures.

## References
- Use `references/tools-mcp-ui-checklist.md`.
