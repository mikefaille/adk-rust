import os

def fix_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    content = content.replace('ExecutionConfig::new("test".to_string())', 'ExecutionConfig::new(adk_core::types::SessionId::new("test").unwrap())')

    with open(filepath, 'w') as f:
        f.write(content)

fix_file('adk-graph/src/agent.rs')
fix_file('adk-graph/src/executor.rs')
