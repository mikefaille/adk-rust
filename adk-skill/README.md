# adk-skill

AgentSkills parser, index, matcher, and runtime injection helpers for ADK-Rust.

[![Crates.io](https://img.shields.io/crates/v/adk-skill.svg)](https://crates.io/crates/adk-skill)
[![Documentation](https://docs.rs/adk-skill/badge.svg)](https://docs.rs/adk-skill)
[![License](https://img.shields.io/crates/l/adk-skill.svg)](LICENSE)

## Overview

`adk-skill` provides the core building blocks to load markdown instruction files, select relevant skills for a user query, and inject selected instructions into user prompts.

This crate is provider-agnostic and can be used through:

- `adk-agent` (`LlmAgentBuilder::with_skills*`)
- `adk-runner` (`Runner::with_auto_skills`)
- Direct API calls from custom runtimes

## Supported File Conventions

### Frontmatter Skill Files (`.skills/**/*.md`)

Each skill is a markdown file with YAML frontmatter:

```md
---
name: code_search
description: Search source code quickly
tags: [code, search]
---
Use `rg --files` first, then `rg <pattern>`.
```

Rules enforced by parser:

- Opening and closing `---` delimiters are required.
- `name` is required and must be non-empty after trim.
- `description` is required and must be non-empty after trim.
- `tags` is optional and empty/whitespace tags are dropped.

### Instruction Convention Files

The index loader also discovers and ingests these markdown files:

- `AGENTS.md` and `AGENT.md`
- `CLAUDE.md`
- `GEMINI.md`
- `COPILOT.md`
- `SKILLS.md`

For these files, frontmatter is optional:

- If valid frontmatter is present, it is used.
- Otherwise the file is parsed as plain markdown instructions and converted into a skill document with convention tags (for example `agents-md`, `claude-md`).

## What The Crate Does

### 1. Discovery

- Scans `<root>/.skills/` recursively for frontmatter skills.
- Scans `<root>` recursively for supported convention files.
- Skips common heavy directories (`.git`, `target`, `node_modules`, etc.).
- Returns deterministic sorted file paths with de-duplication.

API: `discover_skill_files(root)`
API: `discover_instruction_files(root)`

### 2. Parsing + Validation

- Strict path (`.skills/**`): parses required frontmatter as YAML with validation.
- Convention path (`AGENTS.md`, `CLAUDE.md`, etc.): parses plain markdown (or frontmatter if provided).
- Returns actionable errors with file path context for strict frontmatter paths.

API: `parse_skill_markdown(path, content)`
API: `parse_instruction_markdown(path, content)`

### 3. Indexing

- Builds `SkillIndex` from discovered files.
- Computes:
  - content hash (`SHA-256`)
  - `last_modified` (Unix timestamp seconds when available)
  - stable document id: `normalized-name + first-12-hash-chars`
- Sorts documents deterministically by `(name, path)`.

API: `load_skill_index(root)`

### 4. Selection

Selection is lexical and deterministic (no embeddings yet).

Scoring weights:

- `name`: `+4.0` per token hit
- `description`: `+2.5`
- `tags`: `+2.0`
- `body`: `+1.0`

Score is normalized by `sqrt(body_token_count)` to reduce long-body bias.

Tie-break order:

1. Higher score first
2. Lexicographically smaller `name`
3. Lexicographically smaller `path`

API: `select_skills(index, query, policy)`

`SelectionPolicy` defaults:

- `top_k = 1`
- `min_score = 1.0`
- `include_tags = []`
- `exclude_tags = []`

### 5. Injection

Injection helpers prepend the selected skill body to user content using:

```text
[skill:<name>]
<skill body>
[/skill]
```

Then original user text follows.

Behavior:

- Injection runs only when `Content.role == "user"`.
- Query text is extracted from text parts and joined with newlines.
- Only the top match is injected.
- Injected body is truncated to `max_injected_chars`.

APIs:

- `select_skill_prompt_block(...)`
- `apply_skill_injection(...)`
- `SkillInjector` / `SkillInjectorConfig`
- `SkillInjector::build_plugin(...)`
- `SkillInjector::build_plugin_manager(...)`

## Quick Start

### Load and Match Skills

```rust
use adk_skill::{SelectionPolicy, load_skill_index, select_skills};

let index = load_skill_index(".")?;
let policy = SelectionPolicy {
    top_k: 1,
    min_score: 0.1,
    include_tags: vec![],
    exclude_tags: vec![],
};

let matches = select_skills(&index, "find TODO markers in code", &policy);
for m in matches {
    println!("{} ({:.2})", m.skill.name, m.score);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Inject Into User Content

```rust
use adk_core::Content;
use adk_skill::{SelectionPolicy, apply_skill_injection, load_skill_index};

let index = load_skill_index(".")?;
let policy = SelectionPolicy { min_score: 0.1, ..SelectionPolicy::default() };
let mut content = Content::new("user").with_text("Search this repository for TODO markers");

let matched = apply_skill_injection(&mut content, &index, &policy, 1500);
if let Some(m) = matched {
    println!("Injected skill: {}", m.skill.name);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Build A Plugin Manager

```rust
use adk_skill::{SkillInjector, SkillInjectorConfig};

let injector = SkillInjector::from_root(".", SkillInjectorConfig::default())?;
let plugin_manager = injector.build_plugin_manager("skills");
# let _ = plugin_manager;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Error Model

Main error type: `SkillError`

- `Io`
- `Yaml`
- `InvalidFrontmatter { path, message }`
- `MissingField { path, field }`
- `InvalidSkillsRoot(path)`

Type alias: `SkillResult<T> = Result<T, SkillError>`

## Current Limits

- No embedding/vector retrieval (lexical matching only).
- No incremental file reload API yet.
- No remote catalog (`skills-ref`/MCP) in this crate yet.
- No script/file reference execution layer in this crate (selection + injection only).

## Related Examples

From this repository:

- `examples/skills_llm_minimal`
- `examples/skills_auto_discovery`
- `examples/skills_policy_filters`
- `examples/skills_runner_injector`
- `examples/skills_workflow_minimal`

## Development

```bash
cargo test -p adk-skill
```

## License

Apache-2.0
