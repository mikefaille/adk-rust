# Ralph - Autonomous Agent Loop

An autonomous agent loop that runs continuously until all PRD items are complete. No bash scripts needed — everything runs within ADK-Rust.

## Overview

This example demonstrates ADK-Rust's native loop capabilities for building autonomous development agents. It uses a PRD-driven approach where the agent reads tasks from a JSON file, executes them using custom tools, and tracks progress until completion.

## Why Autonomous Loop? (vs Standard Worker)

Standard agents (like the base `ralph` example) typically follow a **Worker** pattern: they receive a specific prompt, execute a chain of thoughts, and return a result. This is excellent for task-specific automation but requires an external driver (human or script) to manage the workflow.

This **Autonomous Loop** structure improves upon the standard pattern by:

1.  **Self-Correction**: The agent runs in a continuous loop, allowing it to inspect the output of its own tools (e.g., `cargo test` failures) and attempt fixes immediately without human intervention.
2.  **State Persistence**: By maintaining a `PRD` and progress log, the agent has long-term memory of what has been accomplished, rather than just context-window memory.
3.  **Dynamic Prioritization**: The agent can re-read the PRD and adjust its focus based on what tasks are remaining, rather than following a rigid script.

This architecture shifts the role of the developer from "driver" to "architect" — defining the goal (PRD) and tools, then letting the agent navigate the path to completion.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        Ralph                                 │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────────────────────────────────────────────┐   │
│  │                   LoopAgent                          │   │
│  │  (Wraps the orchestrator for continuous execution)  │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                  │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              Loop Agent (Orchestrator)               │   │
│  │  - Checks PRD stats                                  │   │
│  │  - Gets next task                                    │   │
│  │  - Marks tasks complete                              │   │
│  │  - Signals exit when done                            │   │
│  └─────────────────────────────────────────────────────┘   │
│                           │                                  │
│  ┌─────────────────────────────────────────────────────┐   │
│  │                   Custom Tools                       │   │
│  ├─────────────┬─────────────┬─────────────┬───────────┤   │
│  │  PrdTool    │  GitTool    │  TestTool   │ FileTool  │   │
│  │  - get_next │  - add      │  - check    │ - read    │   │
│  │  - complete │  - commit   │  - test     │ - write   │   │
│  │  - stats    │  - learning │  - clippy   │ - append  │   │
│  │             │  - diff     │  - fmt      │ - list    │   │
│  └─────────────┴─────────────┴─────────────┴───────────┘   │
└─────────────────────────────────────────────────────────────┘
```

## Quick Start

### Prerequisites

- Rust 1.85+
- Google API key (Gemini)

### Setup

1. Set your API key:
```bash
export GOOGLE_API_KEY=your-api-key-here
```

2. Run Ralph:
```bash
cargo run -p ralph_autonomous_agent
```

## Configuration

| Environment Variable | Default | Description |
|---------------------|---------|-------------|
| `GOOGLE_API_KEY` | (required) | Gemini API key |
| `RALPH_PRD_PATH` | `prd.json` | Path to the PRD file |
| `RALPH_MAX_ITERATIONS` | `100` | Maximum loop iterations |
| `RALPH_MODEL` | `gemini-2.5-flash` | Model to use |

## Project Structure

```
examples/ralph_autonomous_agent/
├── Cargo.toml              # Package dependencies
├── prd.json                # Example PRD with user stories
└── src/
    ├── main.rs             # Entry point
    ├── agents/             # Agent implementations
    ├── tools/              # Custom tools
    └── models/             # Data models
```
