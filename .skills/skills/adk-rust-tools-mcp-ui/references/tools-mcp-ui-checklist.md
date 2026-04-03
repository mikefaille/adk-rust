# Tools MCP UI Checklist

## Function tools
- name, description, parameter schema, deterministic result shape

## MCP
- auth mode configured
- retry/reconnect bounded
- long-running behavior documented
- Resource API: list_resources, list_resource_templates, read_resource for server-provided data
- ElicitationHandler: configured for MCP servers that request user input during tool execution

## UI tools
- protocol capability selected explicitly
- response validated against protocol schema tests
- compatibility behavior covered for legacy payloads
