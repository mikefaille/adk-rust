import os

def fix_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    content = content.replace('SessionId::from("mcp-session".to_string())', 'adk_core::types::SessionId::new("mcp-session").unwrap()')
    content = content.replace('UserId::from("mcp-user".to_string())', 'adk_core::types::UserId::new("mcp-user").unwrap()')

    content = content.replace('SessionId::from("session-1".to_string())', 'adk_core::types::SessionId::new("session-1").unwrap()')
    content = content.replace('UserId::from("user-1".to_string())', 'adk_core::types::UserId::new("user-1").unwrap()')

    content = content.replace('"session-1".to_string().into()', 'adk_core::types::SessionId::new("session-1").unwrap()')
    content = content.replace('"user-1".to_string().into()', 'adk_core::types::UserId::new("user-1").unwrap()')
    content = content.replace('"inv-1".to_string().into()', 'adk_core::types::InvocationId::new("inv-1").unwrap()')

    with open(filepath, 'w') as f:
        f.write(content)

fix_file('adk-agent/tests/mcp_integration_test.rs')
fix_file('adk-agent/tests/callback_integration_test.rs')
