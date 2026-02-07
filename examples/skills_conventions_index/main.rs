//! AgentSkills example: discover and match instruction conventions.
//!
//! Run:
//!   cargo run --manifest-path examples/Cargo.toml --example skills_conventions_index

use adk_skill::{SelectionPolicy, load_skill_index, select_skills};
use anyhow::Result;
use std::path::PathBuf;

fn setup_demo_root() -> Result<PathBuf> {
    let root = std::env::temp_dir().join("adk_skills_conventions_index_demo");
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    std::fs::create_dir_all(root.join(".skills"))?;
    std::fs::create_dir_all(root.join("submodule"))?;

    std::fs::write(
        root.join(".skills/code_search.md"),
        "---\nname: code_search\ndescription: Search repository source code\ntags: [code, search]\n---\nUse rg --files, then rg <pattern>.\n",
    )?;
    std::fs::write(
        root.join("AGENTS.md"),
        "# Repository Agent Rules\nAlways run cargo test before commit and keep changes scoped.\n",
    )?;
    std::fs::write(
        root.join("GEMINI.md"),
        "# Gemini Guidance\nUse GOOGLE_API_KEY or GEMINI_API_KEY and default to gemini-2.5-flash.\n",
    )?;
    std::fs::write(
        root.join("CLAUDE.md"),
        "# Claude Guidance\nPrefer concise summaries and clear safety constraints.\n",
    )?;
    std::fs::write(
        root.join("COPILOT.md"),
        "# Copilot Guidance\nGenerate minimal diffs and preserve existing project style.\n",
    )?;
    std::fs::write(
        root.join("SKILLS.md"),
        "# Shared Skills Catalog\nList reusable coding workflows and troubleshooting heuristics.\n",
    )?;
    std::fs::write(
        root.join("submodule/AGENTS.md"),
        "# Submodule Rules\nUse crate-local tests when editing submodule code.\n",
    )?;

    Ok(root)
}

fn main() -> Result<()> {
    let root = setup_demo_root()?;
    let index = load_skill_index(&root)?;

    println!("Discovered {} instruction files\n", index.len());
    for skill in index.summaries() {
        println!("- {} | tags={:?} | file={}", skill.name, skill.tags, skill.path.display());
    }

    let policy = SelectionPolicy { top_k: 1, min_score: 0.1, ..SelectionPolicy::default() };
    let queries = [
        "how should we configure gemini api key for this repo",
        "what are the repository test before commit rules",
        "find todo markers in code",
    ];

    println!("\nTop match per query:");
    for query in queries {
        let matches = select_skills(&index, query, &policy);
        if let Some(top) = matches.first() {
            println!("* {:?} -> {} ({:.2})", query, top.skill.name, top.score);
        } else {
            println!("* {:?} -> no match", query);
        }
    }

    Ok(())
}
