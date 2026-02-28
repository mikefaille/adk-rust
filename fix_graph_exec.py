import os
import re

def fix_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    # Replaces everything like ExecutionConfig::new("string".to_string()) -> ExecutionConfig::new(adk_core::types::SessionId::new("string").unwrap())
    content = re.sub(r'ExecutionConfig::new\("([^"]+)".to_string\(\)\)', r'ExecutionConfig::new(adk_core::types::SessionId::new("\1").unwrap())', content)

    with open(filepath, 'w') as f:
        f.write(content)

fix_file('adk-graph/tests/execution_tests.rs')
