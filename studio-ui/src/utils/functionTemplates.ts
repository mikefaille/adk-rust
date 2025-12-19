import type { FunctionToolConfig } from '../types/project';

export const FUNCTION_TEMPLATES = [
  { 
    name: 'Calculator', 
    icon: '🧮', 
    template: { 
      name: 'calculator', 
      description: 'Performs arithmetic operations (add, subtract, multiply, divide)', 
      parameters: [
        { name: 'operation', param_type: 'string' as const, description: 'Operation: add, subtract, multiply, divide', required: true },
        { name: 'a', param_type: 'number' as const, description: 'First number', required: true },
        { name: 'b', param_type: 'number' as const, description: 'Second number', required: true }
      ], 
      code: `let result = match operation {
    "add" => a + b,
    "subtract" => a - b,
    "multiply" => a * b,
    "divide" if b != 0.0 => a / b,
    "divide" => return Err(AdkError::Tool("Division by zero".into())),
    _ => return Err(AdkError::Tool(format!("Unknown operation: {}", operation))),
};
Ok(json!({ "result": result }))` 
    }
  },
  { 
    name: 'HTTP Request', 
    icon: '🌐', 
    template: { 
      name: 'http_request', 
      description: 'Make HTTP GET/POST requests to URLs', 
      parameters: [
        { name: 'url', param_type: 'string' as const, description: 'URL to request', required: true },
        { name: 'method', param_type: 'string' as const, description: 'HTTP method (GET or POST)', required: false },
        { name: 'body', param_type: 'string' as const, description: 'Request body for POST', required: false }
      ], 
      code: `let client = reqwest::Client::new();
let method = if method.is_empty() { "GET" } else { method };
let response = match method.to_uppercase().as_str() {
    "GET" => client.get(url).send().await,
    "POST" => client.post(url).body(body.to_string()).send().await,
    _ => return Err(AdkError::Tool(format!("Unsupported method: {}", method))),
};
match response {
    Ok(resp) => {
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        Ok(json!({ "status": status, "body": text }))
    }
    Err(e) => Err(AdkError::Tool(format!("Request failed: {}", e)))
}` 
    }
  },
  { 
    name: 'Read File', 
    icon: '📄', 
    template: { 
      name: 'read_file', 
      description: 'Read contents of a file', 
      parameters: [
        { name: 'path', param_type: 'string' as const, description: 'File path to read', required: true }
      ], 
      code: `match std::fs::read_to_string(path) {
    Ok(content) => Ok(json!({ "content": content })),
    Err(e) => Err(AdkError::Tool(format!("Failed to read file: {}", e)))
}` 
    }
  },
  { 
    name: 'Write File', 
    icon: '💾', 
    template: { 
      name: 'write_file', 
      description: 'Write content to a file', 
      parameters: [
        { name: 'path', param_type: 'string' as const, description: 'File path to write', required: true },
        { name: 'content', param_type: 'string' as const, description: 'Content to write', required: true }
      ], 
      code: `match std::fs::write(path, content) {
    Ok(_) => Ok(json!({ "status": "success", "path": path })),
    Err(e) => Err(AdkError::Tool(format!("Failed to write file: {}", e)))
}` 
    }
  },
  { 
    name: 'JSON Parser', 
    icon: '📋', 
    template: { 
      name: 'parse_json', 
      description: 'Parse JSON string and extract a field', 
      parameters: [
        { name: 'json_str', param_type: 'string' as const, description: 'JSON string to parse', required: true },
        { name: 'field', param_type: 'string' as const, description: 'Field to extract (dot notation)', required: false }
      ], 
      code: `let parsed: Value = serde_json::from_str(json_str)
    .map_err(|e| AdkError::Tool(format!("Invalid JSON: {}", e)))?;
if field.is_empty() {
    Ok(parsed)
} else {
    let value = field.split('.').fold(Some(&parsed), |acc, key| {
        acc.and_then(|v| v.get(key))
    });
    Ok(value.cloned().unwrap_or(Value::Null))
}` 
    }
  },
  { 
    name: 'Shell Command', 
    icon: '⚡', 
    template: { 
      name: 'run_command', 
      description: 'Execute a shell command', 
      parameters: [
        { name: 'command', param_type: 'string' as const, description: 'Command to execute', required: true },
        { name: 'args', param_type: 'string' as const, description: 'Space-separated arguments', required: false }
      ], 
      code: `use std::process::Command;
let args_vec: Vec<&str> = if args.is_empty() { vec![] } else { args.split_whitespace().collect() };
match Command::new(command).args(&args_vec).output() {
    Ok(output) => Ok(json!({
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
        "exit_code": output.status.code()
    })),
    Err(e) => Err(AdkError::Tool(format!("Command failed: {}", e)))
}` 
    }
  },
];

export const MCP_TEMPLATES = [
  { name: 'Time', icon: '🕐', command: 'uvx', args: ['mcp-server-time'], desc: 'Get current time and timezone info' },
  { name: 'Fetch', icon: '🌐', command: 'uvx', args: ['mcp-server-fetch'], desc: 'Fetch and parse web pages' },
  { name: 'Filesystem', icon: '📁', command: 'npx', args: ['-y', '@modelcontextprotocol/server-filesystem', '/tmp'], desc: 'Read, write, and manage files' },
  { name: 'GitHub', icon: '🐙', command: 'npx', args: ['-y', '@modelcontextprotocol/server-github'], desc: 'GitHub repos, issues, PRs' },
  { name: 'SQLite', icon: '💾', command: 'uvx', args: ['mcp-server-sqlite', '--db-path', '/tmp/data.db'], desc: 'Query SQLite databases' },
  { name: 'Memory', icon: '🧠', command: 'npx', args: ['-y', '@modelcontextprotocol/server-memory'], desc: 'Persistent key-value store' },
  { name: 'Brave Search', icon: '🔍', command: 'npx', args: ['-y', '@anthropic/mcp-server-brave-search'], desc: 'Web search via Brave' },
  { name: 'Puppeteer', icon: '🎭', command: 'npx', args: ['-y', '@anthropic/mcp-server-puppeteer'], desc: 'Browser automation' },
  { name: 'Slack', icon: '💬', command: 'npx', args: ['-y', '@anthropic/mcp-server-slack'], desc: 'Slack messaging' },
  { name: 'Google Drive', icon: '📂', command: 'npx', args: ['-y', '@anthropic/mcp-server-gdrive'], desc: 'Google Drive files' },
  { name: 'PostgreSQL', icon: '🐘', command: 'npx', args: ['-y', '@anthropic/mcp-server-postgres'], desc: 'PostgreSQL queries' },
  { name: 'Everything', icon: '✨', command: 'npx', args: ['-y', '@modelcontextprotocol/server-everything'], desc: 'Demo server with all features' },
];

export function generateFunctionTemplate(config: FunctionToolConfig): string {
  const fnName = config.name || 'my_function';
  const params = config.parameters.map(p => {
    const extractor = p.param_type === 'number' 
      ? 'as_f64().unwrap_or(0.0)' 
      : p.param_type === 'boolean' 
        ? 'as_bool().unwrap_or(false)' 
        : 'as_str().unwrap_or("")';
    return `    let ${p.name} = args["${p.name}"].${extractor};`;
  }).join('\n');
  const code = config.code || 'Ok(json!({"status": "ok"}))';
  return `async fn ${fnName}_fn(_ctx: Arc<dyn ToolContext>, args: Value) -> Result<Value, AdkError> {\n${params}\n    ${code}\n}`;
}

export function extractUserCode(fullCode: string, config: FunctionToolConfig): string {
  const lines = fullCode.split('\n');
  const startIdx = config.parameters.length + 1;
  const endIdx = lines.length - 1;
  if (startIdx >= endIdx) return config.code || '';
  return lines.slice(startIdx, endIdx).map(l => l.replace(/^    /, '')).join('\n');
}
