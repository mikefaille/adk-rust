//! Integration tests for the example validator.

use adk_doc_audit::{AsyncValidationConfig, CodeExample, ExampleValidator};
use std::env;

#[tokio::test]
async fn test_validator_integration() {
    // Create a temporary workspace for testing
    let temp_workspace = env::temp_dir().join("validator_integration_test");
    tokio::fs::create_dir_all(&temp_workspace).await.unwrap();

    // Create validator
    let validator = ExampleValidator::new("0.1.0".to_string(), temp_workspace).await.unwrap();

    // Test simple Rust example
    let simple_example = CodeExample {
        content: r#"fn main() { println!("Hello, world!"); }"#.to_string(),
        language: "rust".to_string(),
        line_number: 1,
        is_runnable: true,
        attributes: Vec::new(),
    };

    let result = validator.validate_example(&simple_example).await.unwrap();
    assert!(result.success, "Simple example should compile successfully");

    // Test async example with proper setup
    let async_example = CodeExample {
        content: r#"
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Hello, async world!");
    Ok(())
}
"#
        .to_string(),
        language: "rust".to_string(),
        line_number: 1,
        is_runnable: true,
        attributes: Vec::new(),
    };

    let async_result = validator.validate_example(&async_example).await.unwrap();
    assert!(async_result.success, "Proper async example should compile successfully");

    // Test async pattern validation
    let config = AsyncValidationConfig::default();
    let pattern_result = validator.validate_async_patterns(&async_example, &config).await.unwrap();
    assert!(pattern_result.success, "Proper async patterns should validate successfully");
}

#[tokio::test]
async fn test_validator_error_detection() {
    let temp_workspace = env::temp_dir().join("validator_error_test");
    tokio::fs::create_dir_all(&temp_workspace).await.unwrap();

    let validator = ExampleValidator::new("0.1.0".to_string(), temp_workspace).await.unwrap();

    // Test example with async pattern issues
    let bad_async_example = CodeExample {
        content: r#"
async fn main() {
    println!("This needs tokio runtime setup");
}
"#
        .to_string(),
        language: "rust".to_string(),
        line_number: 1,
        is_runnable: true,
        attributes: Vec::new(),
    };

    let config = AsyncValidationConfig::default();
    let result = validator.validate_async_patterns(&bad_async_example, &config).await.unwrap();

    // Should detect missing runtime setup
    assert!(!result.success, "Should detect async pattern issues");
    assert!(!result.errors.is_empty(), "Should have errors");
    assert!(
        result.suggestions.iter().any(|s| s.contains("tokio::main")),
        "Should suggest tokio::main"
    );
}

#[tokio::test]
async fn test_validator_suggestion_system() {
    let temp_workspace = env::temp_dir().join("validator_suggestion_test");
    tokio::fs::create_dir_all(&temp_workspace).await.unwrap();

    let validator = ExampleValidator::new("0.1.0".to_string(), temp_workspace).await.unwrap();

    // Test blocking calls in async context
    let blocking_example = CodeExample {
        content: r#"
async fn read_file() {
    let content = std::fs::read_to_string("file.txt");
    println!("{}", content);
}
"#
        .to_string(),
        language: "rust".to_string(),
        line_number: 1,
        is_runnable: true,
        attributes: Vec::new(),
    };

    let config = AsyncValidationConfig::default();
    let result = validator.validate_async_patterns(&blocking_example, &config).await.unwrap();

    // Should detect blocking calls and provide suggestions
    assert!(!result.warnings.is_empty(), "Should have warnings about blocking calls");
    assert!(result.suggestions.iter().any(|s| s.contains("tokio::fs")), "Should suggest tokio::fs");
}
