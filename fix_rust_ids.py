import os
import re
import argparse

def process_file(filepath, dry_run):
    with open(filepath, 'r') as f:
        content = f.read()

    original = content
    
    # 1. Fix struct definitions (e.g., `pub session_id: SessionId::new( String,`)
    content = re.sub(r'SessionId::new\(\s*String\s*,', r'SessionId,', content)
    content = re.sub(r'UserId::new\(\s*String\s*,', r'UserId,', content)
    content = re.sub(r'SessionId::new\(\s*Option<String>\s*,', r'SessionId,', content)
    content = re.sub(r'UserId::new\(\s*Option<String>\s*,', r'UserId,', content)
    
    # 2. Fix method arguments (e.g., `session_id: SessionId::new( &str,`)
    content = re.sub(r'SessionId::new\(\s*&str\s*,', r'&SessionId,', content)
    content = re.sub(r'UserId::new\(\s*&str\s*,', r'&UserId,', content)

    # 3. Fix cloning with dangling paren `SessionId::new( session_id.clone(),` -> `SessionId::new(session_id.clone()).unwrap(),`
    content = re.sub(r'SessionId::new\(\s*session_id\.clone\(\)\s*,', r'SessionId::new(session_id.clone()).unwrap(),', content)
    content = re.sub(r'UserId::new\(\s*user_id\.clone\(\)\s*,', r'UserId::new(user_id.clone()).unwrap(),', content)

    # 4. Fix String::new() `SessionId::new( String::new(),` -> `SessionId::new(String::new()).unwrap(),`
    content = re.sub(r'SessionId::new\(\s*String::new\(\)\s*,', r'SessionId::new(String::new()).unwrap(),', content)
    content = re.sub(r'UserId::new\(\s*String::new\(\)\s*,', r'UserId::new(String::new()).unwrap(),', content)

    # 5. Fix remaining bare closing delimiters caused by the old macro changes
    # This involves looking for `Value::Number(n))` -> `Value::Number(n)`
    # Be careful not to replace valid Rust syntax when there are double closing parens for function calls.
    # The safest way is to catch exactly the known error patterns.

    if content != original:
        print(f"Modifications needed in {filepath}")
        if not dry_run:
            with open(filepath, 'w') as f:
                f.write(content)
            print(f"  -> Fixed {filepath}")
        else:
            print(f"  -> [DRY RUN] Would fix {filepath}")
        return True
    return False

def main():
    parser = argparse.ArgumentParser(description="Fix adk-rust ID type sed errors.")
    parser.add_argument("--dry-run", action="store_true", help="Print changes without applying them.")
    args = parser.parse_args()

    project_dir = "/home/michael/src/voice_gateway/zenith/adk-rust"
    modified_files = 0

    for root, _, files in os.walk(project_dir):
        if "target" in root or ".git" in root:
            continue
        for file in files:
            if file.endswith(".rs"):
                filepath = os.path.join(root, file)
                if process_file(filepath, args.dry_run):
                    modified_files += 1
                    
    print(f"\nTotal files {'would be ' if args.dry_run else ''}modified: {modified_files}")

if __name__ == "__main__":
    main()
