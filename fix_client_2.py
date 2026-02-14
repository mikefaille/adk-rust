import re

file_path = 'adk-gemini/src/client.rs'

with open(file_path, 'r') as f:
    content = f.read()

# 1. Guard impl GoogleCloudAuth
content = content.replace(
    'impl GoogleCloudAuth {',
    '#[cfg(feature = "vertex")]\nimpl GoogleCloudAuth {'
)

# 2. Guard impl GoogleCloudConfig
content = content.replace(
    'impl GoogleCloudConfig {',
    '#[cfg(feature = "vertex")]\nimpl GoogleCloudConfig {'
)

with open(file_path, 'w') as f:
    f.write(content)
