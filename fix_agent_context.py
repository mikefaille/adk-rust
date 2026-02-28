import os

def fix_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    content = content.replace('SessionId::from("test-session".to_string())', 'SessionId::new("test-session").unwrap()')
    content = content.replace('UserId::from("test-user".to_string())', 'UserId::new("test-user").unwrap()')

    with open(filepath, 'w') as f:
        f.write(content)

fix_file('adk-agent/tests/test_context.rs')
