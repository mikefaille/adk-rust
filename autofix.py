import json
import subprocess
import os
import re

ID_TYPES = ("UserId", "SessionId", "InvocationId")

def simplify_id_constructors():
    """Pre-pass: Replace Idtype::new("literal").unwrap() -> Idtype::from("literal").
    Only for string literals (double-quoted) which are guaranteed colon-free by definition.
    Returns number of files changed."""
    import glob
    changed = 0
    files = [f for f in glob.glob("**/*.rs", recursive=True)
             if "/target/" not in f and f != "./autofix.py"]
    for path in files:
        with open(path, "r") as fh:
            original = fh.read()
        text = original
        for id_type in ID_TYPES:
            # Pattern: IdType::new("some literal").unwrap()
            text = re.sub(
                rf'{id_type}::new\(\s*("(?:[^"\\]|\\.)*?")\s*\)\.unwrap\(\)',
                rf'{id_type}::from(\1)',
                text,
            )
            # Pattern: IdType::new('literal').unwrap() (with single-quote bytes, unlikely but safe)
            text = re.sub(
                rf"{id_type}::new\(\s*('(?:[^'\\]|\\.)*?')\s*\)\.unwrap\(\)",
                rf"{id_type}::from(\1)",
                text,
            )
        if text != original:
            with open(path, "w") as fh:
                fh.write(text)
            changed += 1
            print(f"Simplified ID constructors in {path}")
    return changed


def run_cargo_check():
    print("Running cargo check --all --message-format=json ...")
    # Run cargo directly -- don't wrap in `devenv shell` as that blocks forever
    # when stdout/stderr are captured (no TTY). The user runs this script from
    # within the devenv env, so cargo is already on PATH.
    result = subprocess.run(
        ["cargo", "check", "--all", "--message-format=json"],
        capture_output=True,
        text=True,
        env={**os.environ, "RUSTC_WRAPPER": ""},  # bypass sccache if not available
    )
    return result.stdout.splitlines()

def apply_fixes():
    lines = run_cargo_check()
    fixes_applied = 0
    file_lines_modified = {}
    manual_fixes_needed = []

    def get_file_content(file_name):
        if file_name not in file_lines_modified:
            with open(file_name, "r") as f:
                file_lines_modified[file_name] = f.read().splitlines()
        return file_lines_modified[file_name]

    for line in lines:
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
            
        if msg.get("reason") != "compiler-message":
            continue
            
        message = msg.get("message", {})
        code = message.get("code", {})
        if not code:
            continue
            
        code_id = code.get("code")
        spans = message.get("spans", [])
        if not spans:
            continue
            
        primary_span = next((s for s in spans if s.get("is_primary")), spans[0])
        file_name = primary_span.get("file_name")
        line_num = primary_span.get("line_start")
        
        handled = False

        # Missing Imports
        if code_id in ["E0433", "E0425"]:
            msg_text = message.get("message", "")
            if "SessionId" in msg_text or "UserId" in msg_text:
                content_lines = get_file_content(file_name)
                str_content = '\n'.join(content_lines)
                if "adk_core::types::SessionId" not in str_content and "adk_core::types::{SessionId, UserId}" not in str_content:
                    content_lines.insert(0, "use adk_core::types::{SessionId, UserId};")
                    fixes_applied += 1
                    print(f"Added imports to {file_name}:{line_num}")
                    handled = True

        # Type Mismatches
        if code_id == "E0308":
            msg_text = message.get("message", "")
            content_lines = get_file_content(file_name)
            target_line = content_lines[line_num - 1]
            
            # expected `Option<SessionId>`, found `SessionId`
            if "expected enum `std::option::Option<SessionId>`" in msg_text and "found struct `SessionId`" in msg_text:
                if "SessionId::new(" in target_line and "Some(SessionId::new(" not in target_line:
                    new_line = re.sub(r'(SessionId::new\([^)]+\)\.unwrap\(\))', r'Some(\1)', target_line)
                    if new_line != target_line:
                        content_lines[line_num - 1] = new_line
                        fixes_applied += 1
                        print(f"Wrapped SessionId in Some() in {file_name}:{line_num}")
                        handled = True

            # expected `&SessionId`, found `&String`
            if "expected `&SessionId`" in msg_text and "found `&String`" in msg_text:
                new_line = re.sub(r'&\s*([a-zA-Z0-9_]+)\b', r'&SessionId::new(\1.clone()).unwrap()', target_line)
                if new_line != target_line:
                    content_lines[line_num - 1] = new_line
                    fixes_applied += 1
                    print(f"Converted &String to &SessionId in {file_name}:{line_num}")
                    handled = True
            
            # expected `Number`, found `i64` in adk-studio/src/server/runner.rs
            if "expected `Number`" in msg_text and "found `i64`" in msg_text:
                if "Value::Number(n)" in target_line:
                    content_lines[line_num - 1] = target_line.replace("Value::Number(n)", "Value::Number(n.into())")
                    fixes_applied += 1
                    print(f"Converted i64 to Number in {file_name}:{line_num}")
                    handled = True
                    
        # Trait Mismatches
        if code_id == "E0277":
            msg_text = message.get("message", "")
            content_lines = get_file_content(file_name)
            target_line = content_lines[line_num - 1]
            
            # `SessionId: AsRef<...>` is not satisfied -> Need .to_string()
            if "the trait bound `SessionId: AsRef" in msg_text or "the trait bound `adk_core::types::SessionId: AsRef" in msg_text:
                if "arg(" in target_line:
                    new_line = re.sub(r'arg\(([^)]+)\)', r'arg(\1.to_string())', target_line)
                    if new_line != target_line:
                        content_lines[line_num - 1] = new_line
                        fixes_applied += 1
                        print(f"Added .to_string() for AsRef bound in {file_name}:{line_num}")
                        handled = True

            # `String: Borrow<SessionId>` is not satisfied
            if "Borrow<SessionId>" in msg_text:
                if "contains_key(" in target_line:
                    new_line = re.sub(r'contains_key\([^)]*\b(session_id)\b\)', r'contains_key(&\1.to_string())', target_line)
                    new_line = new_line.replace("&&", "&")
                    if new_line != target_line:
                        content_lines[line_num - 1] = new_line
                        fixes_applied += 1
                        print(f"Fixed Borrow<SessionId> in contains_key for {file_name}:{line_num}")
                        handled = True
                if "get_mut(" in target_line:
                    new_line = re.sub(r'get_mut\([^)]*\b(session_id)\b\)', r'get_mut(&\1.to_string())', target_line)
                    new_line = new_line.replace("&&", "&")
                    if new_line != target_line:
                        content_lines[line_num - 1] = new_line
                        fixes_applied += 1
                        print(f"Fixed Borrow<SessionId> in get_mut for {file_name}:{line_num}")
                        handled = True

        # Method Not Found
        if code_id == "E0599":
            msg_text = message.get("message", "")
            content_lines = get_file_content(file_name)
            target_line = content_lines[line_num - 1]
            
            if "unwrap_or_else" in msg_text and "SessionId" in msg_text:
                new_line = re.sub(r'\.unwrap_or_else\([^)]+\)', '', target_line)
                if new_line != target_line:
                    content_lines[line_num - 1] = new_line
                    fixes_applied += 1
                    print(f"Removed unwrap_or_else on SessionId in {file_name}:{line_num}")
                    handled = True

        if not handled:
            short_msg = message.get("message", "").split('\n')[0]
            manual_fixes_needed.append(f"MANUAL FIX NEEDED: {file_name}:{line_num} - {code_id}: {short_msg}")

    # Process all file changes
    for file_name, lines_content in file_lines_modified.items():
        with open(file_name, "w") as f:
            f.write('\n'.join(lines_content) + '\n')

    if manual_fixes_needed:
        print("\n--- ATTENTION AI: Unhandled Compilation Errors Requiring Manual Fixes ---")
        for fix in dict.fromkeys(manual_fixes_needed): # Deduplicate
            print(fix)
        print("-------------------------------------------------------------------------")

    return fixes_applied

if __name__ == "__main__":
    pre = simplify_id_constructors()
    print(f"Pre-pass: simplified ID constructors in {pre} files.")
    count = apply_fixes()
    print(f"\nApplied {count} additional compiler-driven fixes.")
