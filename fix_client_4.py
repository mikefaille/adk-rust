import re

file_path = 'adk-gemini/src/client.rs'

with open(file_path, 'r') as f:
    content = f.read()

# Remove cfg guard before GoogleCloudUnsupported
# We look for:
#     #[cfg(feature = "vertex")]
#     GoogleCloudUnsupported
#
# But there might be other attributes like #[snafu(...)]
# Pattern:
# (\s+)#\[cfg\(feature = "vertex"\)\]\n(\s+)#\[snafu\(display\(.*?\)\)\]\n(\s+)GoogleCloudUnsupported

# Actually, my previous script put cfg right before the line defining the variant.
# The variant line usually has #[snafu(...)] before it.
# So I likely put cfg before #[snafu(...)].

# Let's handle it by reading lines again to be safe.
lines = content.splitlines(keepends=True)
new_lines = []
skip_next_cfg = False

for i, line in enumerate(lines):
    # If this line is GoogleCloudUnsupported variant definition
    if 'GoogleCloudUnsupported' in line and '{ operation:' in line:
        # Check if previous lines were cfg
        # We need to find the cfg guard we added and remove it.
        # It should be 1 or 2 lines above depending on snafu attribute.

        # Let's scan backwards from here
        j = len(new_lines) - 1
        while j >= 0:
            if 'cfg(feature = "vertex")' in new_lines[j]:
                # Found it, remove it
                del new_lines[j]
                break
            if 'snafu(display' in new_lines[j] or line.strip() == '':
                j -= 1
            else:
                # Found something else, stop
                break

    new_lines.append(line)

with open(file_path, 'w') as f:
    f.writelines(new_lines)
