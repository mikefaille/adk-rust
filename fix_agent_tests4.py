import os

def fix_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    content = content.replace('SessionId::from("session-456".to_string())', 'adk_core::types::SessionId::new("session-456").unwrap()')
    content = content.replace('UserId::from("user-123".to_string())', 'adk_core::types::UserId::new("user-123").unwrap()')

    content = content.replace('"session-456".to_string().into()', 'adk_core::types::SessionId::new("session-456").unwrap()')
    content = content.replace('"user-123".to_string().into()', 'adk_core::types::UserId::new("user-123").unwrap()')
    content = content.replace('"inv-1".to_string().into()', 'adk_core::types::InvocationId::new("inv-1").unwrap()')

    with open(filepath, 'w') as f:
        f.write(content)

fix_file('adk-agent/tests/context_propagation_test.rs')
