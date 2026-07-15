use adk_core::{SchemaAdapter, ToolContract, ToolSchema};
use adk_gemini::schema_adapter::GeminiSchemaAdapter;
use adk_model::anthropic::AnthropicSchemaAdapter;
use adk_model::openai::{
    OpenAiRealtimeSchemaAdapter, OpenAiSchemaAdapter, OpenAiStrictSchemaAdapter,
};
use serde_json::json;

#[test]
fn test_golden_contract_parity() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer", "minimum": 0 },
            "tags": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["name"]
    });

    let contract = ToolContract::new(
        "test_tool",
        "A test tool for golden parity.",
        ToolSchema::new(Some(schema), None),
    );

    let adapters: Vec<Box<dyn SchemaAdapter>> = vec![
        Box::new(GeminiSchemaAdapter::new()),
        Box::new(GeminiSchemaAdapter::vertex_ai()),
        Box::new(OpenAiSchemaAdapter),
        Box::new(OpenAiStrictSchemaAdapter),
        Box::new(OpenAiRealtimeSchemaAdapter),
        Box::new(AnthropicSchemaAdapter),
    ];

    for adapter in adapters {
        let compiled = adapter
            .compile_schema(contract.schema.parameters.as_ref().unwrap())
            .expect(&format!("{} compile failed", adapter.identifier()));

        // Verify basic structure
        assert!(compiled.schema.get("type").is_some());

        // Verify identifier and version are present
        assert!(!adapter.identifier().is_empty());
        assert!(!adapter.version().is_empty());

        match (adapter.identifier(), adapter.surface()) {
            ("gemini", Some("studio")) => {
                assert!(compiled.schema.get("additionalProperties").is_none());
            }
            ("gemini", Some("vertex")) => {
                assert_eq!(compiled.schema["additionalProperties"], false);
            }
            ("openai", Some("strict")) => {
                assert_eq!(compiled.schema["additionalProperties"], false);
            }
            _ => {}
        }
    }
}

#[test]
fn test_tool_name_validation_parity() {
    let long_name = "a".repeat(100);
    let invalid_name = "test tool!";

    let adapters: Vec<Box<dyn SchemaAdapter>> = vec![
        Box::new(GeminiSchemaAdapter::new()),
        Box::new(OpenAiSchemaAdapter),
        Box::new(OpenAiStrictSchemaAdapter),
        Box::new(AnthropicSchemaAdapter),
    ];

    for adapter in adapters {
        assert!(
            adapter.validate_tool_name(&long_name).is_err(),
            "{} should reject long name",
            adapter.identifier()
        );

        if adapter.identifier().starts_with("openai") || adapter.identifier() == "gemini" {
            assert!(
                adapter.validate_tool_name(invalid_name).is_err(),
                "{} should reject invalid name '{}'",
                adapter.identifier(),
                invalid_name
            );
        }
    }
}
