import os

def fix_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    content = content.replace('ExecutionConfig::new("test-thread".to_string())', 'ExecutionConfig::new(adk_core::types::SessionId::new("test-thread").unwrap())')
    content = content.replace('ExecutionConfig::new("thread-123".to_string())', 'ExecutionConfig::new(adk_core::types::SessionId::new("thread-123").unwrap())')

    with open(filepath, 'w') as f:
        f.write(content)

fix_file('adk-graph/tests/node_tests.rs')
