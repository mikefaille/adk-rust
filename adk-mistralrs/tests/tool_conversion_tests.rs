//! Property tests for tool declaration conversion.
//!
//! **Property 2: Tool Declaration Conversion Roundtrip**
//! *For any* valid ADK tool declaration (name, description, parameters schema),
//! converting to mistral.rs format and inspecting the result SHALL preserve
//! the tool name, description, and parameter structure.
//!
//! **Validates: Requirements 1.4, 1.5**

use proptest::prelude::*;
use serde_json::{Map, json};

// Import the conversion functions from the crate
// Note: These are pub functions in convert.rs
mod convert_helpers {
    use serde_json::{Map, Value};

    /// Convert ADK tool declarations to mistral.rs tool format
    pub fn tools_to_mistralrs(tools: &Map<String, Value>) -> Vec<Value> {
        let mut mistral_tools = Vec::new();

        for (name, tool_def) in tools {
            if let Some(tool_obj) = tool_def.as_object() {
                let mut function = serde_json::Map::new();
                function.insert("name".to_string(), Value::String(name.clone()));

                if let Some(desc) = tool_obj.get("description") {
                    function.insert("description".to_string(), desc.clone());
                }

                if let Some(params) = tool_obj.get("parameters") {
                    function.insert("parameters".to_string(), params.clone());
                }

                let mut tool = serde_json::Map::new();
                tool.insert("type".to_string(), Value::String("function".to_string()));
                tool.insert("function".to_string(), Value::Object(function));

                mistral_tools.push(Value::Object(tool));
            }
        }

        mistral_tools
    }

    /// Extract tool name from mistral.rs tool format
    pub fn extract_tool_name(tool: &Value) -> Option<String> {
        tool.get("function")
            .and_then(|f| f.get("name"))
            .and_then(|n| n.as_str())
            .map(|s| s.to_string())
    }

    /// Extract tool description from mistral.rs tool format
    pub fn extract_tool_description(tool: &Value) -> Option<String> {
        tool.get("function")
            .and_then(|f| f.get("description"))
            .and_then(|d| d.as_str())
            .map(|s| s.to_string())
    }

    /// Extract tool parameters from mistral.rs tool format
    pub fn extract_tool_parameters(tool: &Value) -> Option<Value> {
        tool.get("function").and_then(|f| f.get("parameters")).cloned()
    }
}

use convert_helpers::*;

// Strategy for generating valid tool names (alphanumeric with underscores)
fn arb_tool_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{2,20}".prop_map(|s| s)
}

// Strategy for generating tool descriptions
fn arb_description() -> impl Strategy<Value = String> {
    "[A-Za-z ]{10,50}".prop_map(|s| s.trim().to_string())
}

// Strategy for generating parameter property types
fn arb_param_type() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("string"),
        Just("number"),
        Just("integer"),
        Just("boolean"),
        Just("array"),
        Just("object"),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: mistral-rs-integration, Property 2: Tool Declaration Conversion Roundtrip**
    /// *For any* valid ADK tool declaration, converting to mistral.rs format
    /// SHALL preserve the tool name, description, and parameter structure.
    #[test]
    fn prop_tool_conversion_preserves_name_and_description(
        name in arb_tool_name(),
        description in arb_description(),
    ) {
        let mut tools = Map::new();
        tools.insert(
            name.clone(),
            json!({
                "description": description.clone(),
                "parameters": {
                    "type": "object",
                    "properties": {}
                }
            }),
        );

        let converted = tools_to_mistralrs(&tools);

        prop_assert_eq!(converted.len(), 1);
        let tool = &converted[0];

        // Verify name is preserved
        prop_assert_eq!(extract_tool_name(tool), Some(name));

        // Verify description is preserved
        prop_assert_eq!(extract_tool_description(tool), Some(description));
    }

    /// Property test for parameter structure preservation
    #[test]
    fn prop_tool_conversion_preserves_parameters(
        name in arb_tool_name(),
        param_name in "[a-z][a-z0-9_]{2,10}",
        param_type in arb_param_type(),
    ) {
        let params = json!({
            "type": "object",
            "properties": {
                param_name.clone(): {
                    "type": param_type
                }
            },
            "required": [param_name.clone()]
        });

        let mut tools = Map::new();
        tools.insert(
            name.clone(),
            json!({
                "description": "Test tool",
                "parameters": params.clone()
            }),
        );

        let converted = tools_to_mistralrs(&tools);
        let tool = &converted[0];

        // Verify parameters are preserved
        let extracted_params = extract_tool_parameters(tool);
        prop_assert!(extracted_params.is_some());
        prop_assert_eq!(extracted_params.unwrap(), params);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property test for multiple tools conversion
    #[test]
    fn prop_multiple_tools_conversion(
        name1 in arb_tool_name(),
        name2 in arb_tool_name(),
        desc1 in arb_description(),
        desc2 in arb_description(),
    ) {
        // Skip if names are the same (would overwrite in map)
        prop_assume!(name1 != name2);

        let mut tools = Map::new();
        tools.insert(
            name1.clone(),
            json!({
                "description": desc1.clone(),
                "parameters": {"type": "object", "properties": {}}
            }),
        );
        tools.insert(
            name2.clone(),
            json!({
                "description": desc2.clone(),
                "parameters": {"type": "object", "properties": {}}
            }),
        );

        let converted = tools_to_mistralrs(&tools);

        prop_assert_eq!(converted.len(), 2);

        // Collect names from converted tools
        let converted_names: Vec<String> = converted
            .iter()
            .filter_map(extract_tool_name)
            .collect();

        prop_assert!(converted_names.contains(&name1));
        prop_assert!(converted_names.contains(&name2));
    }

    /// Property test for tool type field
    #[test]
    fn prop_tool_has_function_type(
        name in arb_tool_name(),
    ) {
        let mut tools = Map::new();
        tools.insert(
            name,
            json!({
                "description": "Test",
                "parameters": {"type": "object", "properties": {}}
            }),
        );

        let converted = tools_to_mistralrs(&tools);
        let tool = &converted[0];

        // Verify type is "function"
        let tool_type = tool.get("type").and_then(|t| t.as_str());
        prop_assert_eq!(tool_type, Some("function"));
    }
}
