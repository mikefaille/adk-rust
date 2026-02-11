import os
import re

examples_dir = "examples"

# Patterns to match
# 1. client.generate_content()...execute().await?
# 2. client.batch_generate_content()...execute().await?
# 3. client.files()...execute().await?
# 4. client.cache()...execute().await?

# We want to catch things like:
# let response = client.generate_content()...execute().await?;
# let final_response = ...execute().await?;
# let batch = ...execute().await?;

def fix_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    changed = False

    # Fix imports
    if "GenerationResponse" not in content and "generate_content" in content:
        content = re.sub(r'use adk_gemini::\{', 'use adk_gemini::{GenerationResponse, ', content)
        if "GenerationResponse" not in content:
            content = "use adk_gemini::GenerationResponse;\n" + content
        changed = True

    if "Batch" not in content and "batch_generate_content" in content:
        content = re.sub(r'use adk_gemini::\{', 'use adk_gemini::{Batch, ', content)
        if "Batch" not in content:
            content = "use adk_gemini::Batch;\n" + content
        changed = True

    # Fix execute().await? for GenerationResponse
    # Match 'let [varname] = [anything]execute().await?;'
    # but avoid if it already has a type or if it's unlikely to be GenerationResponse
    
    # We'll be more specific: match calls that likely return GenerationResponse
    patterns = [
        r'let ([a-zA-Z0-9_]+) = (.+?\.generate_content\(.+?\.execute\(\)\.await\?);',
        r'let ([a-zA-Z0-9_]+) = (.+?\.execute\(\)\.await\?);'
    ]
    
    # Actually, the simplest is to match any 'let var = ...execute().await?;' 
    # and if it fails, we revert. But let's try to be smart.
    
    # For now, let's fix the ones we know are failing.
    
    # response, response1, response2, response3, final_response, complex_response, etc.
    res_vars = ["response", "response1", "response2", "response3", "final_response", "complex_response", "base_response", "edit_response1", "edit_response2", "edit_response3", "token_usage_response", "followup_response"]
    for var in res_vars:
        old_pattern = f'let {var} = (.*execute\(\)\.await\?);'
        new_replacement = f'let {var}: GenerationResponse = \\1;'
        if re.search(old_pattern, content):
            content = re.sub(old_pattern, new_replacement, content)
            changed = True

    # batch
    if re.search(r'let batch = (.*execute\(\)\.await\?);', content):
        content = re.sub(r'let batch = (.*execute\(\)\.await\?);', r'let batch: Batch = \1;', content)
        changed = True

    if changed:
        with open(filepath, 'w') as f:
            f.write(content)
        print(f"Fixed {filepath}")

for filename in os.listdir(examples_dir):
    if filename.endswith(".rs"):
        fix_file(os.path.join(examples_dir, filename))
