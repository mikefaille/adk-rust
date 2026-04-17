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

// ===== Streaming buffer for token-by-token tool call detection =====

/// Prefixes that indicate a potential tool call is starting.
const TOOL_CALL_PREFIXES: &[&str] = &["<tool_call", "<|python_tag|", "[TOOL_CALLS]"];

/// Closing tags that complete a tool call.
const TOOL_CALL_CLOSERS: &[&str] = &["</tool_call>", "</function>", "\n"];

/// Maximum buffer size before flushing as plain text (safety valve).
const MAX_BUFFER_SIZE: usize = 4096;

/// Streaming buffer that accumulates tokens and detects tool call boundaries.
///
/// Use this in streaming response handlers to buffer tokens when a tool call
/// prefix is detected, then parse and emit `Part::FunctionCall` when the
/// closing tag arrives.
///
/// # Example
///
/// ```rust,ignore
/// let mut buffer = ToolCallBuffer::new();
///
/// for chunk in stream {
///     match buffer.push(&chunk.text) {
///         BufferAction::Emit(parts) => {
///             for part in parts { yield part; }
///         }
///         BufferAction::Buffering => { /* still accumulating */ }
///     }
/// }
/// // Flush any remaining content at end of stream
/// for part in buffer.flush() { yield part; }
/// ```
pub struct ToolCallBuffer {
    buffer: String,
    buffering: bool,
}

/// Action returned by `ToolCallBuffer::push()`.
pub enum BufferAction {
    /// Emit these parts immediately (text or parsed tool calls).
    Emit(Vec<Part>),
    /// Still buffering — don't emit anything yet.
    Buffering,
}

impl ToolCallBuffer {
    /// Create a new empty buffer.
    pub fn new() -> Self {
        Self { buffer: String::new(), buffering: false }
    }

    /// Push a text chunk into the buffer.
    ///
    /// Returns `BufferAction::Emit` with parts to yield, or
    /// `BufferAction::Buffering` if we're accumulating a potential tool call.
    pub fn push(&mut self, text: &str) -> BufferAction {
        self.buffer.push_str(text);

        if self.buffering {
            // Check if we have a complete tool call
            if self.has_complete_tool_call() {
                return self.try_parse_and_emit();
            }
            // Safety valve: if buffer is too large, flush as text
            if self.buffer.len() > MAX_BUFFER_SIZE {
                return self.flush_as_emit();
            }
            BufferAction::Buffering
        } else {
            // Check if this chunk starts or contains a tool call prefix
            if self.starts_tool_call_prefix() {
                self.buffering = true;
                // Check if the complete tool call arrived in one chunk
                if self.has_complete_tool_call() {
                    return self.try_parse_and_emit();
                }
                BufferAction::Buffering
            } else if self.has_partial_prefix() {
                // Could be the start of a prefix split across chunks (e.g., "<tool" then "_call>")
                self.buffering = true;
                BufferAction::Buffering
            } else {
                // Normal text — emit immediately
                self.flush_as_emit()
            }
        }
    }

    /// Flush any remaining buffered content as parts.
    /// Call this when the stream ends.
    pub fn flush(&mut self) -> Vec<Part> {
        if self.buffer.is_empty() {
            return Vec::new();
        }

        // Try to parse as tool calls one last time
        if let Some(parts) = parse_text_tool_calls(&self.buffer) {
            self.buffer.clear();
            self.buffering = false;
            return parts;
        }

        // Otherwise emit as text
        let text = std::mem::take(&mut self.buffer);
        self.buffering = false;
        if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![Part::Text { text }]
        }
    }

    fn starts_tool_call_prefix(&self) -> bool {
        TOOL_CALL_PREFIXES.iter().any(|prefix| self.buffer.contains(prefix))
    }

    fn has_partial_prefix(&self) -> bool {
        // Check if the buffer ends with a partial prefix like "<tool" or "<|python"
        let buf = &self.buffer;
        for prefix in TOOL_CALL_PREFIXES {
            for i in 1..prefix.len() {
                if buf.ends_with(&prefix[..i]) {
                    return true;
                }
            }
        }
        false
    }

    fn has_complete_tool_call(&self) -> bool {
        (self.buffer.contains("<tool_call>") && self.buffer.contains("</tool_call>"))
            || (self.buffer.contains("<|python_tag|>")
                && self.buffer.contains('\n')
                && self.buffer.len() > "<|python_tag|>".len() + 5)
            || (self.buffer.contains("[TOOL_CALLS]")
                && self.buffer.contains(']')
                && self.buffer.rfind(']') > self.buffer.find("[TOOL_CALLS]").map(|i| i + 12))
    }

    fn try_parse_and_emit(&mut self) -> BufferAction {
        if let Some(parts) = parse_text_tool_calls(&self.buffer) {
            self.buffer.clear();
            self.buffering = false;
            BufferAction::Emit(parts)
        } else {
            // Couldn't parse — flush as text
            self.flush_as_emit()
        }
    }

    fn flush_as_emit(&mut self) -> BufferAction {
        let text = std::mem::take(&mut self.buffer);
        self.buffering = false;
        if text.trim().is_empty() {
            BufferAction::Emit(Vec::new())
        } else {
            BufferAction::Emit(vec![Part::Text { text }])
        }
    }
}

impl Default for ToolCallBuffer {
    fn default() -> Self {
        Self::new()
    }
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

    // ===== Streaming buffer tests =====

    #[test]
    fn test_buffer_normal_text_emits_immediately() {
        let mut buf = ToolCallBuffer::new();
        match buf.push("Hello world") {
            BufferAction::Emit(parts) => {
                assert_eq!(parts.len(), 1);
                assert!(matches!(&parts[0], Part::Text { text } if text == "Hello world"));
            }
            BufferAction::Buffering => panic!("should emit immediately"),
        }
    }

    #[test]
    fn test_buffer_complete_tool_call_in_one_chunk() {
        let mut buf = ToolCallBuffer::new();
        let text = r#"<tool_call>{"name": "search", "arguments": {"q": "rust"}}</tool_call>"#;
        match buf.push(text) {
            BufferAction::Emit(parts) => {
                assert_eq!(parts.len(), 1);
                assert!(matches!(&parts[0], Part::FunctionCall { name, .. } if name == "search"));
            }
            BufferAction::Buffering => panic!("should emit parsed tool call"),
        }
    }

    #[test]
    fn test_buffer_tool_call_split_across_chunks() {
        let mut buf = ToolCallBuffer::new();

        // Chunk 1: prefix starts
        assert!(matches!(buf.push("<tool_call>"), BufferAction::Buffering));

        // Chunk 2: JSON body
        assert!(matches!(
            buf.push(r#"{"name": "get_weather", "arguments": {"city": "Tokyo"}}"#),
            BufferAction::Buffering
        ));

        // Chunk 3: closing tag
        match buf.push("</tool_call>") {
            BufferAction::Emit(parts) => {
                assert_eq!(parts.len(), 1);
                assert!(
                    matches!(&parts[0], Part::FunctionCall { name, .. } if name == "get_weather")
                );
            }
            BufferAction::Buffering => panic!("should emit after closing tag"),
        }
    }

    #[test]
    fn test_buffer_text_then_tool_call() {
        let mut buf = ToolCallBuffer::new();

        // Normal text first
        match buf.push("Let me check. ") {
            BufferAction::Emit(parts) => {
                assert_eq!(parts.len(), 1);
                assert!(matches!(&parts[0], Part::Text { .. }));
            }
            BufferAction::Buffering => panic!("should emit text"),
        }

        // Then tool call
        let tc = r#"<tool_call>{"name": "search", "arguments": {}}</tool_call>"#;
        match buf.push(tc) {
            BufferAction::Emit(parts) => {
                assert_eq!(parts.len(), 1);
                assert!(matches!(&parts[0], Part::FunctionCall { name, .. } if name == "search"));
            }
            BufferAction::Buffering => panic!("should emit tool call"),
        }
    }

    #[test]
    fn test_buffer_flush_incomplete_as_text() {
        let mut buf = ToolCallBuffer::new();
        assert!(matches!(buf.push("<tool_call>partial"), BufferAction::Buffering));

        // Stream ends without closing tag
        let parts = buf.flush();
        assert_eq!(parts.len(), 1);
        assert!(matches!(&parts[0], Part::Text { text } if text.contains("<tool_call>")));
    }

    #[test]
    fn test_buffer_flush_empty() {
        let mut buf = ToolCallBuffer::new();
        let parts = buf.flush();
        assert!(parts.is_empty());
    }

    #[test]
    fn test_buffer_partial_prefix_detection() {
        let mut buf = ToolCallBuffer::new();
        // "<tool" could be the start of "<tool_call>"
        assert!(matches!(buf.push("<tool"), BufferAction::Buffering));
        // Complete it
        assert!(matches!(buf.push("_call>"), BufferAction::Buffering));
        // Add body and close
        match buf.push(r#"{"name":"x","arguments":{}}</tool_call>"#) {
            BufferAction::Emit(parts) => {
                assert_eq!(parts.len(), 1);
                assert!(matches!(&parts[0], Part::FunctionCall { name, .. } if name == "x"));
            }
            BufferAction::Buffering => panic!("should emit"),
        }
    }
}
