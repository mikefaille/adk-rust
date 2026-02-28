import os

def fix_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    content = content.replace('SessionId::from("test-session".to_string())', 'adk_core::types::SessionId::new("test-session").unwrap()')
    content = content.replace('UserId::from("test-user".to_string())', 'adk_core::types::UserId::new("test-user").unwrap()')

    content = content.replace('SessionId::from("session-1".to_string())', 'adk_core::types::SessionId::new("session-1").unwrap()')
    content = content.replace('UserId::from("user-1".to_string())', 'adk_core::types::UserId::new("user-1").unwrap()')

    content = content.replace('"session-1".to_string().into()', 'adk_core::types::SessionId::new("session-1").unwrap()')
    content = content.replace('"user-1".to_string().into()', 'adk_core::types::UserId::new("user-1").unwrap()')
    content = content.replace('"inv-1".to_string().into()', 'adk_core::types::InvocationId::new("inv-1").unwrap()')

    with open(filepath, 'w') as f:
        f.write(content)

fix_file('adk-agent/tests/workflow_tests.rs')
fix_file('adk-agent/examples/state_management_demo.rs')
fix_file('adk-agent/tests/tool_callback_tests.rs')
