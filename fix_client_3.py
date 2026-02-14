import sys

file_path = 'adk-gemini/src/client.rs'

with open(file_path, 'r') as f:
    lines = f.readlines()

new_lines = []
in_error_enum = False
for line in lines:
    if 'pub enum Error {' in line:
        in_error_enum = True
    elif in_error_enum and line.strip() == '}':
        in_error_enum = False

    if in_error_enum:
        # Check if line defines a variant using google_cloud types or naming
        if ('google_cloud_' in line or 'GoogleCloud' in line) and 'cfg(feature' not in line:
            # Check if previous line already has cfg
            if new_lines and 'cfg(feature' in new_lines[-1]:
                pass # Already guarded
            else:
                # Add guard with same indentation
                indent = line[:len(line) - len(line.lstrip())]
                new_lines.append(f'{indent}#[cfg(feature = "vertex")]\n')

    new_lines.append(line)

with open(file_path, 'w') as f:
    f.writelines(new_lines)
