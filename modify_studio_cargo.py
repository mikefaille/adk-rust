import sys

try:
    with open('adk-studio/Cargo.toml', 'r') as f:
        content = f.read()

    # Add [features] if not present
    if "[features]" not in content:
        # Insert after [package] block end, or before [[bin]].
        # Or just before [dependencies]
        if "[dependencies]" in content:
            content = content.replace("[dependencies]", "[features]\ndefault = []\nvertex = [\"dep:adk-gemini\", \"adk-gemini/vertex\"]\nfull = [\"vertex\"]\n\n[dependencies]")
        else:
            print("Error: [dependencies] not found")
            sys.exit(1)

    # Add adk-gemini dependency
    if "adk-gemini" not in content:
        # Insert under # ADK crates
        if "# ADK crates" in content:
             content = content.replace("adk-graph.workspace = true", "adk-graph.workspace = true\nadk-gemini = { workspace = true, optional = true }")
        else:
             print("Error: # ADK crates section not found")
             sys.exit(1)

    with open('adk-studio/Cargo.toml', 'w') as f:
        f.write(content)
    print("Updated adk-studio/Cargo.toml")

except Exception as e:
    print(f"Error: {e}")
    sys.exit(1)
