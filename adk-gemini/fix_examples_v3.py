import os
import re

examples_dir = "examples"

def fix_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    changed = False

    # Fix imports
    if "GenerationResponse" not in content and (".generate_content()" in content or ".execute()" in content):
        if "use adk_gemini::{" in content:
            content = re.sub(r'use adk_gemini::\{', 'use adk_gemini::{GenerationResponse, ', content)
        else:
            # Try to add it near other adk_gemini imports
            if "use adk_gemini::" in content:
                 content = re.sub(r'use adk_gemini::(.*?);', r'use adk_gemini::\1;\nuse adk_gemini::GenerationResponse;', content)
            else:
                 content = "use adk_gemini::GenerationResponse;\n" + content
        changed = True

    if "Batch" not in content and "batch_generate_content" in content:
        if "use adk_gemini::{" in content:
            content = re.sub(r'use adk_gemini::\{', 'use adk_gemini::{Batch, ', content)
        else:
            content = "use adk_gemini::Batch;\n" + content
        changed = True

    # Generic replacement for any 'let var = ...execute().await?;'
    # We want to catch 'let response = client.generate_content().execute().await?;'
    # and turn it into 'let response: GenerationResponse = ...'
    
    # This pattern matches 'let [varname] = [anything]execute().await?;'
    # but only if it doesn't already have a colon (type annotation)
    pattern = r'let ([a-zA-Z0-9_]+) = ([^:]+?\.execute\(\)\.await\?);'
    
    # We need to decide whether it's GenerationResponse or Batch.
    # If the variable name is 'batch', use Batch. Otherwise use GenerationResponse.
    
    def replacement(match):
        varname = match.group(1)
        expr = match.group(2)
        if varname == "batch":
            return f"let {varname}: Batch = {expr};"
        else:
            return f"let {varname}: GenerationResponse = {expr};"

    if re.search(pattern, content):
        content = re.sub(pattern, replacement, content)
        changed = True

    if changed:
        with open(filepath, 'w') as f:
            f.write(content)
        print(f"Fixed {filepath}")

for filename in os.listdir(examples_dir):
    if filename.endswith(".rs"):
        fix_file(os.path.join(examples_dir, filename))
