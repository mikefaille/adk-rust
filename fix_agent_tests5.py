import os

def fix_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    content = content.replace('SessionId::from("multi-agent-session".to_string())', 'adk_core::types::SessionId::new("multi-agent-session").unwrap()')
    content = content.replace('UserId::from("multi-agent-user".to_string())', 'adk_core::types::UserId::new("multi-agent-user").unwrap()')

    content = content.replace('SessionId::from("session-real".to_string())', 'adk_core::types::SessionId::new("session-real").unwrap()')
    content = content.replace('UserId::from("user-real".to_string())', 'adk_core::types::UserId::new("user-real").unwrap()')

    content = content.replace('"session-real".to_string().into()', 'adk_core::types::SessionId::new("session-real").unwrap()')
    content = content.replace('"user-real".to_string().into()', 'adk_core::types::UserId::new("user-real").unwrap()')
    content = content.replace('"inv-real".to_string().into()', 'adk_core::types::InvocationId::new("inv-real").unwrap()')

    content = content.replace('id.to_string().into()', 'adk_core::types::InvocationId::new(id.to_string()).unwrap()')

    with open(filepath, 'w') as f:
        f.write(content)

fix_file('adk-agent/tests/multi_agent_test.rs')
fix_file('adk-agent/tests/integration_test.rs')
fix_file('adk-agent/tests/callback_integration_tests.rs')
