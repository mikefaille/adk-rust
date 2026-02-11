import os
import re

examples_dir = "examples"

def fix_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    changed = False

    # Regex for finding assignments to update with type annotations
    # Matches: let var = ... .execute().await?;
    # capturing var name and the expression.
    # Uses DOTALL to match newlines in the expression, but stops at semicolon.
    # Updated to allow whitespace between .execute() and .await?
    assignment_pattern = re.compile(r'let\s+([a-zA-Z0-9_]+)\s*=\s*([^;]*?\.execute\(\)\s*\.await\?);', re.DOTALL)

    def replacement(match):
        varname = match.group(1)
        expr = match.group(2)

        # Determine type based on expression content or variable name
        new_type = None
        if "batch_generate_content" in expr or varname == "batch":
             new_type = "Batch"
        elif "generate_content" in expr and "GenerationResponse" not in expr:
             # Only add if it looks like a generation response
             # Check for variable name hints as well
             if varname in ["response", "final_response", "complex_response", "base_response", "edit_response1", "edit_response2", "edit_response3", "token_usage_response", "followup_response", "response1", "response2", "response3"]:
                new_type = "GenerationResponse"

        if new_type:
            return f"let {varname}: {new_type} = {expr};"
        return match.group(0) # No change

    # Apply type annotations first
    new_content = assignment_pattern.sub(replacement, content)
    if new_content != content:
        content = new_content
        changed = True

    # Check if a type is used and needs import
    def type_is_used(content, type_name):
        # : Type
        if re.search(r':\s*' + type_name + r'\b', content): return True
        # Type::
        if re.search(r'\b' + type_name + r'::', content): return True
        # <Type> or <Type, or Type>
        if re.search(r'<\s*' + type_name + r'\b', content): return True
        if re.search(r'\b' + type_name + r'\s*>', content): return True
        # Type {
        if re.search(r'\b' + type_name + r'\s*\{', content): return True
        # Type (tuple struct)
        if re.search(r'\b' + type_name + r'\s*\(', content): return True
        # Type)
        if re.search(r'\b' + type_name + r'\s*\)', content): return True
        # Type,
        if re.search(r'\b' + type_name + r'\s*,', content): return True

        return False

    # Helper to insert import safely
    def insert_import(content, import_line, item_name):
        if not type_is_used(content, item_name):
             return content, False

        # Check if already imported
        if re.search(r'use adk_gemini::\{[^}]*\b' + item_name + r'\b[^}]*\}', content):
            return content, False
        if re.search(r'use adk_gemini::' + item_name + r';', content):
            return content, False

        # Not imported, need to add
        if "use adk_gemini::{" in content:
            content = re.sub(r'use adk_gemini::\{', f'use adk_gemini::{{{item_name}, ', content)
            return content, True
        else:
            # Insert new line safely
            lines = content.splitlines()
            insert_idx = 0
            found_use = False
            for i, line in enumerate(lines):
                sline = line.strip()
                if sline.startswith("//!") or sline.startswith("#!"):
                    insert_idx = i + 1

            lines.insert(insert_idx, import_line)
            return "\n".join(lines) + "\n", True

    # 1. GenerationResponse
    content, c1 = insert_import(content, "use adk_gemini::GenerationResponse;", "GenerationResponse")
    if c1: changed = True

    # 2. Batch
    content, c2 = insert_import(content, "use adk_gemini::Batch;", "Batch")
    if c2: changed = True

    if changed:
        with open(filepath, 'w') as f:
            f.write(content)
        print(f"Fixed {filepath}")

for filename in os.listdir(examples_dir):
    if filename.endswith(".rs"):
        fix_file(os.path.join(examples_dir, filename))
