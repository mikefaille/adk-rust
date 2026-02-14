import sys

with open('session_temp.rs', 'r') as f:
    content = f.read()

# Replace struct definition
struct_target = '    #[serde(skip_serializing_if = "Option::is_none")]\n    tools: Option<Vec<Value>>,\n}'
struct_replacement = '    #[serde(skip_serializing_if = "Option::is_none")]\n    tools: Option<Vec<Value>>,\n    #[serde(skip_serializing_if = "Option::is_none")]\n    cached_content: Option<String>,\n}'

if struct_target in content:
    content = content.replace(struct_target, struct_replacement)
else:
    print("Struct target not found")

# Replace initialization
# Note: Indentation might vary, so be careful.
# I'll try matching strictly.
init_target = '                generation_config: Some(generation_config),\n                tools,\n            }),'
init_replacement = '                generation_config: Some(generation_config),\n                tools,\n                cached_content: config.cached_content,\n            }),'

if init_target in content:
    content = content.replace(init_target, init_replacement)
else:
    print("Init target not found")

with open('session_temp.rs', 'w') as f:
    f.write(content)
