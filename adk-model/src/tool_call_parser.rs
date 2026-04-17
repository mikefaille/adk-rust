//! Text-based tool call parser for models that emit tool calls as XML tags.
//!
//! Some models (Qwen, Llama, Mistral Nemo, DeepSeek) emit tool calls as
//! text tags instead of structured `tool_calls` JSON when served through
//! endpoints that don't support native tool calling (e.g., HuggingFace TGI
//! without `--enable-auto-tool-choice`).
//!
//! This module detects and parses these text-based tool calls, converting
//! them to proper `Part::FunctionCall` entries so the agent pipeline works
//! regardless of the serving backend.
//!
//! ## Supported Formats
//!
//! - **Qwen/Hermes**: `<tool_call>{"name":"...", "arguments":{...}}</tool_call>`
//! - **Qwen function tag**: `<tool_call><function=NAME>ARGS</function></tool_call>`
//! - **Llama**: `<|python_tag|>{"name":"...", "parameters":{...}}`
//! - **Mistral Nemo**: `[TOOL_CALLS][{"name":"...", "arguments":{...}}]`

use adk_core::Part;

/// Check if text contains a tool call tag that should be parsed.
pub fn contains_tool_call_tag(text: &str) -> bool {
    text.contains("<tool_call>")
        || text.contains("<|python_tag|>")
        || text.contains("[TOOL_CALLS]")
}

/// Parse text-based tool calls from model output.
///
/// Returns `Some(parts)` if tool calls were found and parsed, where `parts`
/// contains `Part::FunctionCall` entries (and optionally `Part::Text` for
/// any non-tool-call text before/after the tags).
///
/// Returns `None` if no tool call tags were detected.
pub fn parse_text_tool_calls(text: &str) -> Option<Vec<Part>> {
    if !contains_tool_call_tag(text) {
        return None;
    }

    let mut parts = Vec::new();

    // Try Qwen/Hermes format: <tool_call>JSON</tool_call>
    if let Some(parsed) = parse_qwen_format(text, &mut parts) {
        return Some(parsed);
    }

    // Try Llama format: <|python_tag|>JSON
    if let Some(parsed) = parse_llama_format(text, &mut parts) {
        return Some(parsed);
    }

    // Try Mistral Nemo format: [TOOL_CALLS][JSON]
    if let Some(parsed) = parse_mistral_nemo_format(text, &mut parts) {
        return Some(parsed);
    }

    None
}

/// Parse Qwen/Hermes format tool calls.
///
/// Handles two sub-formats:
/// 1. JSON body: `<tool_call>{"name":"fn", "arguments":{...}}</tool_call>`
/// 2. Function tag: `<tool_call><function=fn>ARGS</function></tool_call>`
fn parse_qwen_format(text: &str, _parts: &mut Vec<Part>) -> Option<Vec<Part>> {
    let mut result = Vec::new();
    let mut remaining = text;

    loop {
        let start = remaining.find("<tool_call>")?;

        // Add any text before the tool call
        let before = remaining[..start].trim();
        if !before.is_empty() {
            result.push(Part::Text { text: before.to_string() });
        }

        let after_open = &remaining[start + "<tool_call>".len()..];
        let end = after_open.find("</tool_call>")?;
        let inner = after_open[..end].trim();

        // Try JSON format first: {"name":"...", "arguments":{...}}
        if let Some(part) = parse_json_tool_call(inner) {
            result.push(part);
        }
        // Try function tag format: <function=NAME>ARGS</function>
        else if let Some(part) = parse_function_tag(inner) {
            result.push(part);
        } else {
            // Couldn't parse — keep as text
            result.push(Part::Text { text: remaining[start..start + "<tool_call>".len() + end + "</tool_call>".len()].to_string() });
        }

        remaining = &after_open[end + "</tool_call>".len()..];
        if remaining.trim().is_empty() || !remaining.contains("<tool_call>") {
            let trailing = remaining.trim();
            if !trailing.is_empty() {
                result.push(Part::Text { text: trailing.to_string() });
            }
            break;
        }
    }

    if result.is_empty() { None } else { Some(result) }
}

/// Parse `<function=NAME>ARGS</function>` tag.
fn parse_function_tag(inner: &str) -> Option<Part> {
    let func_start = inner.find("<function=")?;
    let after_eq = &inner[func_start + "<function=".len()..];
    let name_end = after_eq.find('>')?;
    let name = after_eq[..name_end].trim().to_string();

    let body_start = name_end + 1;
    let func_end = after_eq.find("</function>")?;
    let body = after_eq[body_start..func_end].trim();

    let args = if body.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(body).unwrap_or_else(|_| serde_json::json!({}))
    };

    Some(Part::FunctionCall { name, args, id: None, thought_signature: None })
}

/// Parse JSON tool call: `{"name":"...", "arguments":{...}}`
/// Also handles `{"function":"...", "parameters":{...}}` variant.
fn parse_json_tool_call(json_str: &str) -> Option<Part> {
    let value: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let obj = value.as_object()?;

    let name = obj
        .get("name")
        .or_else(|| obj.get("function"))
        .and_then(|v| v.as_str())?
        .to_string();

    let args = obj
        .get("arguments")
        .or_else(|| obj.get("parameters"))
        .cloned()
        .unwrap_or(serde_json::json!({}));

    Some(Part::FunctionCall { name, args, id: None, thought_signature: None })
}

/// Parse Llama format: `<|python_tag|>{"name":"...", "parameters":{...}}`
fn parse_llama_format(text: &str, _parts: &mut Vec<Part>) -> Option<Vec<Part>> {
    let tag = "<|python_tag|>";
    let start = text.find(tag)?;

    let mut result = Vec::new();
    let before = text[..start].trim();
    if !before.is_empty() {
        result.push(Part::Text { text: before.to_string() });
    }

    let json_str = text[start + tag.len()..].trim();
    if let Some(part) = parse_json_tool_call(json_str) {
        result.push(part);
    } else {
        return None;
    }

    Some(result)
}

/// Parse Mistral Nemo format: `[TOOL_CALLS][{"name":"...", "arguments":{...}}]`
fn parse_mistral_nemo_format(text: &str, _parts: &mut Vec<Part>) -> Option<Vec<Part>> {
    let tag = "[TOOL_CALLS]";
    let start = text.find(tag)?;

    let mut result = Vec::new();
    let before = text[..start].trim();
    if !before.is_empty() {
        result.push(Part::Text { text: before.to_string() });
    }

    let json_str = text[start + tag.len()..].trim();
    // Expect a JSON array of tool calls
    let arr: Vec<serde_json::Value> = serde_json::from_str(json_str).ok()?;
    for item in &arr {
        let obj = item.as_object()?;
        let name = obj.get("name").and_then(|v| v.as_str())?.to_string();
        let args = obj
            .get("arguments")
            .or_else(|| obj.get("parameters"))
            .cloned()
            .unwrap_or(serde_json::json!({}));
        result.push(Part::FunctionCall { name, args, id: None, thought_signature: None });
    }

    if result.is_empty() { None } else { Some(result) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qwen_json_format() {
        let text = r#"<tool_call>{"name": "get_weather", "arguments": {"city": "Tokyo"}}</tool_call>"#;
        let parts = parse_text_tool_calls(text).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            Part::FunctionCall { name, args, .. } => {
                assert_eq!(name, "get_weather");
                assert_eq!(args["city"], "Tokyo");
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn test_qwen_function_tag_format() {
        let text = r#"<tool_call><function=screenshot></function></tool_call>"#;
        let parts = parse_text_tool_calls(text).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            Part::FunctionCall { name, args, .. } => {
                assert_eq!(name, "screenshot");
                assert_eq!(*args, serde_json::json!({}));
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn test_qwen_function_tag_with_args() {
        let text = r#"<tool_call><function=get_weather>{"city": "Tokyo"}</function></tool_call>"#;
        let parts = parse_text_tool_calls(text).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            Part::FunctionCall { name, args, .. } => {
                assert_eq!(name, "get_weather");
                assert_eq!(args["city"], "Tokyo");
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn test_text_before_tool_call() {
        let text = r#"Let me check that for you.
<tool_call>{"name": "search", "arguments": {"query": "rust"}}</tool_call>"#;
        let parts = parse_text_tool_calls(text).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], Part::Text { text } if text.contains("check that")));
        assert!(matches!(&parts[1], Part::FunctionCall { name, .. } if name == "search"));
    }

    #[test]
    fn test_multiple_tool_calls() {
        let text = r#"<tool_call>{"name": "a", "arguments": {}}</tool_call>
<tool_call>{"name": "b", "arguments": {"x": 1}}</tool_call>"#;
        let parts = parse_text_tool_calls(text).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], Part::FunctionCall { name, .. } if name == "a"));
        assert!(matches!(&parts[1], Part::FunctionCall { name, .. } if name == "b"));
    }

    #[test]
    fn test_llama_format() {
        let text = r#"<|python_tag|>{"name": "get_weather", "parameters": {"city": "NYC"}}"#;
        let parts = parse_text_tool_calls(text).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            Part::FunctionCall { name, args, .. } => {
                assert_eq!(name, "get_weather");
                assert_eq!(args["city"], "NYC");
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn test_mistral_nemo_format() {
        let text = r#"[TOOL_CALLS][{"name": "search", "arguments": {"q": "rust"}}]"#;
        let parts = parse_text_tool_calls(text).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            Part::FunctionCall { name, args, .. } => {
                assert_eq!(name, "search");
                assert_eq!(args["q"], "rust");
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn test_no_tool_call_returns_none() {
        assert!(parse_text_tool_calls("Hello, how can I help?").is_none());
        assert!(parse_text_tool_calls("").is_none());
    }

    #[test]
    fn test_contains_tool_call_tag() {
        assert!(contains_tool_call_tag("<tool_call>"));
        assert!(contains_tool_call_tag("text <tool_call> more"));
        assert!(contains_tool_call_tag("<|python_tag|>"));
        assert!(contains_tool_call_tag("[TOOL_CALLS]"));
        assert!(!contains_tool_call_tag("normal text"));
    }
}
