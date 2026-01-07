//! Example validator for testing code compilation and async patterns.
//!
//! This module provides functionality to validate that code examples in documentation
//! compile correctly and follow proper patterns, especially for async code.

use crate::{AuditError, CodeExample, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use tokio::fs;
use tracing::{debug, info, instrument};

/// Validator for code examples that tests compilation and patterns.
#[derive(Debug)]
pub struct ExampleValidator {
    /// Temporary directory for creating test projects
    temp_dir: TempDir,
    /// Current workspace version for dependency resolution
    #[allow(dead_code)]
    workspace_version: String,
    /// Path to the workspace root for dependency resolution
    workspace_path: PathBuf,
    /// Cache of generated Cargo.toml templates
    #[allow(dead_code)]
    cargo_templates: HashMap<String, String>,
}

/// Result of validating a code example.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationResult {
    /// Whether the validation succeeded
    pub success: bool,
    /// Compilation errors encountered
    pub errors: Vec<CompilationError>,
    /// Warnings from compilation
    pub warnings: Vec<String>,
    /// Suggested fixes for issues
    pub suggestions: Vec<String>,
    /// Additional metadata about the validation
    pub metadata: ValidationMetadata,
}

/// Additional metadata about the validation process.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ValidationMetadata {
    /// Time taken for validation in milliseconds
    pub duration_ms: u64,
    /// Whether a temporary project was created
    pub used_temp_project: bool,
    /// Cargo command that was executed
    pub cargo_command: Option<String>,
    /// Exit code from cargo command
    pub exit_code: Option<i32>,
}

/// Represents a compilation error with detailed information.
#[derive(Debug, Clone, PartialEq)]
pub struct CompilationError {
    /// Error message from the compiler
    pub message: String,
    /// Line number where the error occurred (if available)
    pub line: Option<usize>,
    /// Column number where the error occurred (if available)
    pub column: Option<usize>,
    /// Type of error encountered
    pub error_type: ErrorType,
    /// Suggested fix for the error (if available)
    pub suggestion: Option<String>,
    /// Code snippet that caused the error
    pub code_snippet: Option<String>,
}

/// Types of compilation errors that can occur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorType {
    /// Syntax error in the code
    SyntaxError,
    /// Type mismatch or type checking error
    TypeMismatch,
    /// Unresolved import or module not found
    UnresolvedImport,
    /// Missing dependency in Cargo.toml
    MissingDependency,
    /// Use of deprecated API
    DeprecatedApi,
    /// Async/await pattern issue
    AsyncPatternError,
    /// Runtime setup issue (e.g., missing tokio runtime)
    RuntimeSetupError,
    /// Generic compilation error
    CompilationFailure,
}

/// Configuration for async pattern validation.
#[derive(Debug, Clone)]
pub struct AsyncValidationConfig {
    /// Whether to require explicit tokio runtime setup
    pub require_runtime_setup: bool,
    /// Whether to validate proper error handling in async code
    pub validate_error_handling: bool,
    /// Whether to check for proper async/await usage
    pub check_await_patterns: bool,
    /// Maximum allowed nesting depth for async blocks
    pub max_async_nesting: usize,
}

impl ExampleValidator {
    /// Creates a new example validator.
    ///
    /// # Arguments
    ///
    /// * `workspace_version` - Current version of the ADK-Rust workspace
    /// * `workspace_path` - Path to the workspace root for dependency resolution
    ///
    /// # Returns
    ///
    /// A new `ExampleValidator` instance or an error if setup fails.
    #[instrument(skip(workspace_path))]
    pub async fn new(workspace_version: String, workspace_path: PathBuf) -> Result<Self> {
        let temp_dir =
            TempDir::new().map_err(|e| AuditError::TempDirError { details: e.to_string() })?;

        info!("Created temporary directory for example validation: {:?}", temp_dir.path());

        Ok(Self { temp_dir, workspace_version, workspace_path, cargo_templates: HashMap::new() })
    }

    /// Validates a code example by attempting to compile it.
    ///
    /// # Arguments
    ///
    /// * `example` - The code example to validate
    ///
    /// # Returns
    ///
    /// A `ValidationResult` containing the outcome and any errors found.
    #[instrument(skip(self, example), fields(language = %example.language, runnable = %example.is_runnable))]
    pub async fn validate_example(&self, example: &CodeExample) -> Result<ValidationResult> {
        let start_time = std::time::Instant::now();

        // Only validate Rust examples for compilation
        if example.language != "rust" {
            return Ok(ValidationResult {
                success: true,
                errors: Vec::new(),
                warnings: vec!["Non-Rust code not validated for compilation".to_string()],
                suggestions: Vec::new(),
                metadata: ValidationMetadata {
                    duration_ms: start_time.elapsed().as_millis() as u64,
                    used_temp_project: false,
                    cargo_command: None,
                    exit_code: None,
                },
            });
        }

        // Skip non-runnable examples
        if !example.is_runnable {
            debug!("Skipping non-runnable example");
            return Ok(ValidationResult {
                success: true,
                errors: Vec::new(),
                warnings: vec!["Example marked as non-runnable, skipping compilation".to_string()],
                suggestions: Vec::new(),
                metadata: ValidationMetadata {
                    duration_ms: start_time.elapsed().as_millis() as u64,
                    used_temp_project: false,
                    cargo_command: None,
                    exit_code: None,
                },
            });
        }

        // Create temporary project and validate
        let project_path = self.create_temp_project(example).await?;
        let result = self.compile_example(&project_path, example).await?;

        Ok(ValidationResult {
            success: result.success,
            errors: result.errors,
            warnings: result.warnings,
            suggestions: result.suggestions,
            metadata: ValidationMetadata {
                duration_ms: start_time.elapsed().as_millis() as u64,
                used_temp_project: true,
                cargo_command: result.metadata.cargo_command,
                exit_code: result.metadata.exit_code,
            },
        })
    }

    /// Validates async patterns in a code example.
    ///
    /// # Arguments
    ///
    /// * `example` - The code example to validate for async patterns
    /// * `config` - Configuration for async validation
    ///
    /// # Returns
    ///
    /// A `ValidationResult` containing async pattern validation results.
    #[instrument(skip(self, example, config))]
    pub async fn validate_async_patterns(
        &self,
        example: &CodeExample,
        config: &AsyncValidationConfig,
    ) -> Result<ValidationResult> {
        let start_time = std::time::Instant::now();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut suggestions = Vec::new();

        // Only validate Rust examples
        if example.language != "rust" {
            return Ok(ValidationResult {
                success: true,
                errors,
                warnings: vec!["Non-Rust code not validated for async patterns".to_string()],
                suggestions,
                metadata: ValidationMetadata {
                    duration_ms: start_time.elapsed().as_millis() as u64,
                    used_temp_project: false,
                    cargo_command: None,
                    exit_code: None,
                },
            });
        }

        let content = &example.content;

        // Check for async/await usage
        if content.contains("async") || content.contains(".await") {
            debug!("Found async code, validating patterns");

            // Check for proper runtime setup
            if config.require_runtime_setup {
                self.validate_runtime_setup(content, &mut errors, &mut suggestions);
            }

            // Check for proper error handling
            if config.validate_error_handling {
                self.validate_async_error_handling(content, &mut errors, &mut suggestions);
            }

            // Check await patterns
            if config.check_await_patterns {
                self.validate_await_patterns(content, &mut errors, &mut warnings, &mut suggestions);
            }

            // Check nesting depth
            self.validate_async_nesting(
                content,
                config.max_async_nesting,
                &mut warnings,
                &mut suggestions,
            );

            // Additional async pattern validations
            self.validate_tokio_usage(content, &mut errors, &mut warnings, &mut suggestions);
            self.validate_async_closures(content, &mut warnings, &mut suggestions);
            self.validate_blocking_calls(content, &mut warnings, &mut suggestions);
            self.validate_async_traits(content, &mut warnings, &mut suggestions);
        }

        let success = errors.is_empty();

        Ok(ValidationResult {
            success,
            errors,
            warnings,
            suggestions,
            metadata: ValidationMetadata {
                duration_ms: start_time.elapsed().as_millis() as u64,
                used_temp_project: false,
                cargo_command: None,
                exit_code: None,
            },
        })
    }

    /// Suggests fixes for compilation errors.
    ///
    /// # Arguments
    ///
    /// * `example` - The code example that failed compilation
    /// * `errors` - The compilation errors encountered
    ///
    /// # Returns
    ///
    /// A vector of suggested fixes.
    pub async fn suggest_fixes(
        &self,
        example: &CodeExample,
        errors: &[CompilationError],
    ) -> Result<Vec<String>> {
        let mut suggestions = Vec::new();

        for error in errors {
            match error.error_type {
                ErrorType::UnresolvedImport => {
                    if let Some(suggestion) = self.suggest_import_fix(&error.message) {
                        suggestions.push(suggestion);
                    }
                }
                ErrorType::MissingDependency => {
                    if let Some(suggestion) = self.suggest_dependency_fix(&error.message) {
                        suggestions.push(suggestion);
                    }
                }
                ErrorType::AsyncPatternError => {
                    suggestions
                        .push("Consider using #[tokio::main] for async main functions".to_string());
                    suggestions.push("Ensure all async calls use .await".to_string());
                }
                ErrorType::RuntimeSetupError => {
                    suggestions.push(
                        "Add tokio runtime setup: #[tokio::main] or tokio::runtime::Runtime::new()"
                            .to_string(),
                    );
                }
                ErrorType::DeprecatedApi => {
                    suggestions.push(
                        "Update to use the current API - check the latest documentation"
                            .to_string(),
                    );
                }
                _ => {
                    // Generic suggestions based on error message
                    if error.message.contains("cannot find") {
                        suggestions
                            .push("Check if the module or type is properly imported".to_string());
                    }
                    if error.message.contains("async") {
                        suggestions
                            .push("Ensure async functions are called with .await".to_string());
                    }
                }
            }
        }

        // Add example-specific suggestions
        if example.content.contains("adk_") && !example.content.contains("use adk_") {
            suggestions.push("Add appropriate use statements for ADK crates".to_string());
        }

        if example.content.contains("async fn main") && !example.content.contains("#[tokio::main]")
        {
            suggestions.push("Add #[tokio::main] attribute to async main function".to_string());
        }

        Ok(suggestions)
    }

    /// Creates a temporary Rust project for testing the example.
    #[instrument(skip(self, example))]
    async fn create_temp_project(&self, example: &CodeExample) -> Result<PathBuf> {
        let project_name = format!("example_test_{}", uuid::Uuid::new_v4().simple());
        let project_path = self.temp_dir.path().join(&project_name);

        // Create project directory structure
        fs::create_dir_all(&project_path).await?;
        fs::create_dir_all(project_path.join("src")).await?;

        // Generate Cargo.toml
        let cargo_toml = self.generate_cargo_toml(&project_name, example).await?;
        fs::write(project_path.join("Cargo.toml"), cargo_toml).await?;

        // Generate main.rs or lib.rs
        let rust_code = self.prepare_rust_code(example)?;
        let target_file =
            if example.content.contains("fn main") { "src/main.rs" } else { "src/lib.rs" };
        fs::write(project_path.join(target_file), rust_code).await?;

        debug!("Created temporary project at: {:?}", project_path);
        Ok(project_path)
    }

    /// Generates a Cargo.toml file for the temporary project.
    async fn generate_cargo_toml(
        &self,
        project_name: &str,
        example: &CodeExample,
    ) -> Result<String> {
        let mut dependencies = HashMap::new();

        // Add ADK dependencies based on code content
        if example.content.contains("adk_core") {
            dependencies.insert(
                "adk-core",
                format!("{{ path = \"{}\" }}", self.workspace_path.join("adk-core").display()),
            );
        }
        if example.content.contains("adk_model") {
            dependencies.insert(
                "adk-model",
                format!("{{ path = \"{}\" }}", self.workspace_path.join("adk-model").display()),
            );
        }
        if example.content.contains("adk_agent") {
            dependencies.insert(
                "adk-agent",
                format!("{{ path = \"{}\" }}", self.workspace_path.join("adk-agent").display()),
            );
        }
        if example.content.contains("adk_tool") {
            dependencies.insert(
                "adk-tool",
                format!("{{ path = \"{}\" }}", self.workspace_path.join("adk-tool").display()),
            );
        }

        // Add tokio if async code is detected
        if example.content.contains("async") || example.content.contains(".await") {
            dependencies
                .insert("tokio", "{ version = \"1.0\", features = [\"full\"] }".to_string());
        }

        // Add common dependencies based on imports
        if example.content.contains("serde") {
            dependencies
                .insert("serde", "{ version = \"1.0\", features = [\"derive\"] }".to_string());
        }
        if example.content.contains("anyhow") {
            dependencies.insert("anyhow", "\"1.0\"".to_string());
        }
        if example.content.contains("thiserror") {
            dependencies.insert("thiserror", "\"1.0\"".to_string());
        }

        let mut cargo_toml = format!(
            r#"[package]
name = "{}"
version = "0.1.0"
edition = "2021"

[dependencies]
"#,
            project_name
        );

        for (name, version) in dependencies {
            cargo_toml.push_str(&format!("{} = {}\n", name, version));
        }

        Ok(cargo_toml)
    }

    /// Prepares the Rust code for compilation, adding necessary boilerplate.
    fn prepare_rust_code(&self, example: &CodeExample) -> Result<String> {
        let mut code = example.content.clone();

        // Add common imports if not present
        if !code.contains("use ") && (code.contains("adk_") || code.contains("tokio")) {
            let mut imports = Vec::new();

            if code.contains("adk_core") {
                imports.push("use adk_core::*;");
            }
            if code.contains("adk_model") {
                imports.push("use adk_model::*;");
            }
            if code.contains("tokio") && code.contains("async") {
                imports.push("use tokio;");
            }

            if !imports.is_empty() {
                code = format!("{}\n\n{}", imports.join("\n"), code);
            }
        }

        // Add tokio main attribute if needed
        if code.contains("async fn main") && !code.contains("#[tokio::main]") {
            code = code.replace("async fn main", "#[tokio::main]\nasync fn main");
        }

        // Wrap in a basic structure if it's just expressions
        if !code.contains("fn ") && !code.contains("struct ") && !code.contains("impl ") {
            code = format!("fn main() {{\n{}\n}}", code);
        }

        Ok(code)
    }

    /// Compiles the example in the temporary project.
    #[instrument(skip(self, example))]
    async fn compile_example(
        &self,
        project_path: &Path,
        example: &CodeExample,
    ) -> Result<ValidationResult> {
        let cargo_command = "cargo check";

        debug!("Running cargo check in: {:?}", project_path);

        let output = Command::new("cargo")
            .arg("check")
            .arg("--message-format=json")
            .current_dir(project_path)
            .output()
            .map_err(|e| AuditError::CargoError {
                command: cargo_command.to_string(),
                output: e.to_string(),
            })?;

        let exit_code = output.status.code();
        let success = output.status.success();

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        debug!("Cargo check exit code: {:?}", exit_code);
        debug!("Cargo check stdout: {}", stdout);
        debug!("Cargo check stderr: {}", stderr);

        let (errors, warnings) = self.parse_cargo_output(&stdout, &stderr)?;
        let suggestions = self.suggest_fixes(example, &errors).await?;

        Ok(ValidationResult {
            success,
            errors,
            warnings,
            suggestions,
            metadata: ValidationMetadata {
                duration_ms: 0, // Will be set by caller
                used_temp_project: true,
                cargo_command: Some(cargo_command.to_string()),
                exit_code,
            },
        })
    }

    /// Parses cargo output to extract errors and warnings.
    fn parse_cargo_output(
        &self,
        stdout: &str,
        stderr: &str,
    ) -> Result<(Vec<CompilationError>, Vec<String>)> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        // Parse JSON messages from cargo
        for line in stdout.lines() {
            if let Ok(message) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some("compiler-message") = message.get("reason").and_then(|r| r.as_str()) {
                    if let Some(msg) = message.get("message") {
                        self.parse_compiler_message(msg, &mut errors, &mut warnings)?;
                    }
                }
            }
        }

        // Also parse stderr for any additional errors
        if !stderr.is_empty() {
            for line in stderr.lines() {
                if line.contains("error:") {
                    errors.push(CompilationError {
                        message: line.to_string(),
                        line: None,
                        column: None,
                        error_type: ErrorType::CompilationFailure,
                        suggestion: None,
                        code_snippet: None,
                    });
                } else if line.contains("warning:") {
                    warnings.push(line.to_string());
                }
            }
        }

        Ok((errors, warnings))
    }

    /// Parses a compiler message from cargo JSON output.
    fn parse_compiler_message(
        &self,
        message: &serde_json::Value,
        errors: &mut Vec<CompilationError>,
        warnings: &mut Vec<String>,
    ) -> Result<()> {
        let level = message.get("level").and_then(|l| l.as_str()).unwrap_or("error");
        let text = message.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown error");

        if level == "error" {
            let error_type = self.classify_error_type(text);
            let (line, column) = self.extract_location(message);
            let suggestion = message
                .get("children")
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.first())
                .and_then(|child| child.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string());

            errors.push(CompilationError {
                message: text.to_string(),
                line,
                column,
                error_type,
                suggestion,
                code_snippet: None,
            });
        } else if level == "warning" {
            warnings.push(text.to_string());
        }

        Ok(())
    }

    /// Classifies the type of compilation error based on the message.
    fn classify_error_type(&self, message: &str) -> ErrorType {
        if message.contains("cannot find") || message.contains("unresolved import") {
            ErrorType::UnresolvedImport
        } else if message.contains("mismatched types") || message.contains("type mismatch") {
            ErrorType::TypeMismatch
        } else if message.contains("deprecated") {
            ErrorType::DeprecatedApi
        } else if message.contains("async") || message.contains("await") {
            ErrorType::AsyncPatternError
        } else if message.contains("runtime") || message.contains("tokio") {
            ErrorType::RuntimeSetupError
        } else if message.contains("syntax") || message.contains("unexpected token") {
            ErrorType::SyntaxError
        } else {
            ErrorType::CompilationFailure
        }
    }

    /// Extracts line and column information from a compiler message.
    fn extract_location(&self, message: &serde_json::Value) -> (Option<usize>, Option<usize>) {
        let spans = message.get("spans").and_then(|s| s.as_array());
        if let Some(spans) = spans {
            if let Some(span) = spans.first() {
                let line = span.get("line_start").and_then(|l| l.as_u64()).map(|l| l as usize);
                let column = span.get("column_start").and_then(|c| c.as_u64()).map(|c| c as usize);
                return (line, column);
            }
        }
        (None, None)
    }

    /// Validates runtime setup for async code.
    fn validate_runtime_setup(
        &self,
        content: &str,
        errors: &mut Vec<CompilationError>,
        suggestions: &mut Vec<String>,
    ) {
        if content.contains("async fn main") && !content.contains("#[tokio::main]") {
            errors.push(CompilationError {
                message: "Async main function requires runtime setup".to_string(),
                line: None,
                column: None,
                error_type: ErrorType::RuntimeSetupError,
                suggestion: Some("Add #[tokio::main] attribute".to_string()),
                code_snippet: None,
            });
            suggestions.push("Add #[tokio::main] attribute to async main function".to_string());
        }
    }

    /// Validates error handling patterns in async code.
    fn validate_async_error_handling(
        &self,
        content: &str,
        errors: &mut Vec<CompilationError>,
        suggestions: &mut Vec<String>,
    ) {
        // Check for .await without proper error handling
        if content.contains(".await")
            && !content.contains("?")
            && !content.contains("unwrap")
            && !content.contains("expect")
        {
            errors.push(CompilationError {
                message: "Async calls should handle errors properly".to_string(),
                line: None,
                column: None,
                error_type: ErrorType::AsyncPatternError,
                suggestion: Some("Use ? operator or explicit error handling".to_string()),
                code_snippet: None,
            });
            suggestions.push(
                "Consider using the ? operator for error propagation in async code".to_string(),
            );
        }
    }

    /// Validates await patterns in async code.
    fn validate_await_patterns(
        &self,
        content: &str,
        _errors: &mut [CompilationError],
        warnings: &mut Vec<String>,
        suggestions: &mut Vec<String>,
    ) {
        // Check for missing .await on async calls
        let lines: Vec<&str> = content.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.contains("async")
                && line.contains("(")
                && !line.contains(".await")
                && !line.contains("fn ")
            {
                warnings.push(format!("Line {}: Possible missing .await on async call", i + 1));
                suggestions.push("Ensure async function calls use .await".to_string());
            }
        }
    }

    /// Validates async nesting depth.
    fn validate_async_nesting(
        &self,
        content: &str,
        max_depth: usize,
        warnings: &mut Vec<String>,
        suggestions: &mut Vec<String>,
    ) {
        let mut depth = 0;
        let mut max_found = 0;

        for line in content.lines() {
            if line.contains("async {") || line.contains("async move {") {
                depth += 1;
                max_found = max_found.max(depth);
            }
            if line.contains('}') && depth > 0 {
                depth -= 1;
            }
        }

        if max_found > max_depth {
            warnings.push(format!(
                "Async nesting depth {} exceeds recommended maximum {}",
                max_found, max_depth
            ));
            suggestions.push(
                "Consider refactoring deeply nested async blocks into separate functions"
                    .to_string(),
            );
        }
    }

    /// Suggests fixes for import-related errors.
    fn suggest_import_fix(&self, error_message: &str) -> Option<String> {
        if error_message.contains("adk_core") {
            Some("Add: use adk_core::*; or specific imports".to_string())
        } else if error_message.contains("tokio") {
            Some("Add: use tokio; and ensure tokio is in dependencies".to_string())
        } else if error_message.contains("serde") {
            Some("Add: use serde::{Serialize, Deserialize}; and serde dependency".to_string())
        } else {
            None
        }
    }

    /// Suggests fixes for dependency-related errors.
    fn suggest_dependency_fix(&self, error_message: &str) -> Option<String> {
        if error_message.contains("adk") {
            Some("Add the appropriate ADK crate to Cargo.toml dependencies".to_string())
        } else if error_message.contains("tokio") {
            Some(
                "Add tokio = { version = \"1.0\", features = [\"full\"] } to dependencies"
                    .to_string(),
            )
        } else {
            None
        }
    }

    /// Validates proper tokio usage patterns.
    fn validate_tokio_usage(
        &self,
        content: &str,
        errors: &mut Vec<CompilationError>,
        warnings: &mut Vec<String>,
        suggestions: &mut Vec<String>,
    ) {
        // Check for proper tokio runtime attributes
        if content.contains("async fn main")
            && !content.contains("#[tokio::main]")
            && !content.contains("Runtime::new()")
        {
            errors.push(CompilationError {
                message: "Async main function requires tokio runtime setup".to_string(),
                line: None,
                column: None,
                error_type: ErrorType::RuntimeSetupError,
                suggestion: Some(
                    "Add #[tokio::main] attribute or create runtime manually".to_string(),
                ),
                code_snippet: None,
            });
            suggestions.push("Use #[tokio::main] for simple async main functions".to_string());
        }

        // Check for tokio::test usage in test functions
        if content.contains("#[test]") && content.contains("async fn") {
            warnings.push(
                "Async test functions should use #[tokio::test] instead of #[test]".to_string(),
            );
            suggestions
                .push("Replace #[test] with #[tokio::test] for async test functions".to_string());
        }

        // Check for proper spawn usage
        if content.contains("tokio::spawn") && !content.contains(".await") {
            warnings.push("Spawned tasks should typically be awaited or joined".to_string());
            suggestions.push("Consider awaiting spawned tasks or using JoinHandle".to_string());
        }
    }

    /// Validates async closure patterns.
    fn validate_async_closures(
        &self,
        content: &str,
        warnings: &mut Vec<String>,
        suggestions: &mut Vec<String>,
    ) {
        // Check for async closures without proper handling
        if (content.contains("async move |") || content.contains("async |"))
            && !content.contains("Box::pin")
            && !content.contains("futures::")
        {
            warnings.push("Async closures may need special handling for compilation".to_string());
            suggestions.push(
                "Consider using Box::pin for async closures or futures utilities".to_string(),
            );
        }

        // Check for closure capture issues
        if content.contains("move |") && content.contains(".await") {
            warnings.push(
                "Be careful with move closures and async - ensure proper lifetime management"
                    .to_string(),
            );
            suggestions
                .push("Verify that moved values live long enough for async operations".to_string());
        }
    }

    /// Validates blocking calls in async context.
    fn validate_blocking_calls(
        &self,
        content: &str,
        warnings: &mut Vec<String>,
        suggestions: &mut Vec<String>,
    ) {
        let blocking_patterns = [
            "std::thread::sleep",
            "std::fs::",
            "std::net::",
            ".read_to_string()",
            ".write_all(",
            "reqwest::blocking::",
        ];

        for pattern in &blocking_patterns {
            if content.contains(pattern) && content.contains("async") {
                warnings.push(format!("Potentially blocking call '{}' in async context", pattern));
                match *pattern {
                    "std::thread::sleep" => {
                        suggestions.push(
                            "Use tokio::time::sleep instead of std::thread::sleep".to_string(),
                        );
                    }
                    "std::fs::" => {
                        suggestions.push("Use tokio::fs for async file operations".to_string());
                    }
                    "std::net::" => {
                        suggestions.push("Use tokio::net for async networking".to_string());
                    }
                    "reqwest::blocking::" => {
                        suggestions.push(
                            "Use async reqwest client instead of blocking client".to_string(),
                        );
                    }
                    _ => {
                        suggestions.push(
                            "Consider using async alternatives for blocking operations".to_string(),
                        );
                    }
                }
            }
        }
    }

    /// Validates async trait usage patterns.
    fn validate_async_traits(
        &self,
        content: &str,
        warnings: &mut Vec<String>,
        suggestions: &mut Vec<String>,
    ) {
        // Check for async trait methods without async-trait
        if content.contains("trait ")
            && content.contains("async fn")
            && !content.contains("#[async_trait]")
        {
            warnings.push("Async methods in traits require the async-trait crate".to_string());
            suggestions.push("Add #[async_trait] attribute and use async-trait crate".to_string());
        }

        // Check for proper async trait implementation
        if content.contains("impl ")
            && content.contains("async fn")
            && !content.contains("#[async_trait]")
        {
            let lines: Vec<&str> = content.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                if line.contains("impl ")
                    && i + 1 < lines.len()
                    && lines[i + 1].contains("async fn")
                {
                    warnings.push(
                        "Implementing async trait methods requires #[async_trait]".to_string(),
                    );
                    suggestions
                        .push("Add #[async_trait] to impl blocks with async methods".to_string());
                    break;
                }
            }
        }
    }
}

impl Default for AsyncValidationConfig {
    fn default() -> Self {
        Self {
            require_runtime_setup: true,
            validate_error_handling: true,
            check_await_patterns: true,
            max_async_nesting: 3,
        }
    }
}

// Add uuid dependency for unique project names
use uuid;

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    async fn create_test_validator() -> ExampleValidator {
        let temp_workspace = env::temp_dir().join("test_workspace");
        tokio::fs::create_dir_all(&temp_workspace).await.unwrap();

        ExampleValidator::new("0.1.0".to_string(), temp_workspace).await.unwrap()
    }

    #[tokio::test]
    async fn test_validator_creation() {
        let validator = create_test_validator().await;
        assert_eq!(validator.workspace_version, "0.1.0");
    }

    #[tokio::test]
    async fn test_simple_rust_example_validation() {
        let validator = create_test_validator().await;

        let example = CodeExample {
            content: "fn main() { println!(\"Hello, world!\"); }".to_string(),
            language: "rust".to_string(),
            line_number: 1,
            is_runnable: true,
            attributes: Vec::new(),
        };

        let result = validator.validate_example(&example).await.unwrap();
        assert!(result.success);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn test_non_rust_example_skipped() {
        let validator = create_test_validator().await;

        let example = CodeExample {
            content: "console.log('Hello, world!');".to_string(),
            language: "javascript".to_string(),
            line_number: 1,
            is_runnable: true,
            attributes: Vec::new(),
        };

        let result = validator.validate_example(&example).await.unwrap();
        assert!(result.success);
        assert!(!result.warnings.is_empty());
        assert!(!result.metadata.used_temp_project);
    }

    #[tokio::test]
    async fn test_non_runnable_example_skipped() {
        let validator = create_test_validator().await;

        let example = CodeExample {
            content: "fn main() { println!(\"Hello, world!\"); }".to_string(),
            language: "rust".to_string(),
            line_number: 1,
            is_runnable: false,
            attributes: vec!["ignore".to_string()],
        };

        let result = validator.validate_example(&example).await.unwrap();
        assert!(result.success);
        assert!(!result.warnings.is_empty());
        assert!(!result.metadata.used_temp_project);
    }

    #[tokio::test]
    async fn test_async_pattern_validation() {
        let validator = create_test_validator().await;
        let config = AsyncValidationConfig::default();

        let example = CodeExample {
            content: r#"
async fn main() {
    println!("Hello, async world!");
}
"#
            .to_string(),
            language: "rust".to_string(),
            line_number: 1,
            is_runnable: true,
            attributes: Vec::new(),
        };

        let result = validator.validate_async_patterns(&example, &config).await.unwrap();

        // Should detect missing #[tokio::main]
        assert!(!result.success);
        assert!(!result.errors.is_empty());
        assert!(result.errors.iter().any(|e| e.error_type == ErrorType::RuntimeSetupError));
    }

    #[tokio::test]
    async fn test_proper_async_example() {
        let validator = create_test_validator().await;
        let config = AsyncValidationConfig::default();

        let example = CodeExample {
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

        let result = validator.validate_async_patterns(&example, &config).await.unwrap();
        assert!(result.success);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn test_error_classification() {
        let validator = create_test_validator().await;

        assert_eq!(
            validator.classify_error_type("cannot find type `UnknownType`"),
            ErrorType::UnresolvedImport
        );
        assert_eq!(validator.classify_error_type("mismatched types"), ErrorType::TypeMismatch);
        assert_eq!(
            validator.classify_error_type("use of deprecated function"),
            ErrorType::DeprecatedApi
        );
        assert_eq!(
            validator.classify_error_type("async function in sync context"),
            ErrorType::AsyncPatternError
        );
    }

    #[tokio::test]
    async fn test_suggestion_generation() {
        let validator = create_test_validator().await;

        let errors = vec![CompilationError {
            message: "cannot find adk_core in scope".to_string(),
            line: None,
            column: None,
            error_type: ErrorType::UnresolvedImport,
            suggestion: None,
            code_snippet: None,
        }];

        let example = CodeExample {
            content: "use adk_core::Agent;".to_string(),
            language: "rust".to_string(),
            line_number: 1,
            is_runnable: true,
            attributes: Vec::new(),
        };

        let suggestions = validator.suggest_fixes(&example, &errors).await.unwrap();
        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.contains("adk_core")));
    }

    #[tokio::test]
    async fn test_cargo_toml_generation() {
        let validator = create_test_validator().await;

        let example = CodeExample {
            content: r#"
use adk_core::Agent;
use tokio;

#[tokio::main]
async fn main() {
    println!("Hello!");
}
"#
            .to_string(),
            language: "rust".to_string(),
            line_number: 1,
            is_runnable: true,
            attributes: Vec::new(),
        };

        let cargo_toml = validator.generate_cargo_toml("test_project", &example).await.unwrap();

        assert!(cargo_toml.contains("adk-core"));
        assert!(cargo_toml.contains("tokio"));
        assert!(cargo_toml.contains("[package]"));
        assert!(cargo_toml.contains("[dependencies]"));
    }

    #[tokio::test]
    async fn test_rust_code_preparation() {
        let validator = create_test_validator().await;

        let example = CodeExample {
            content: "async fn main() { println!(\"Hello!\"); }".to_string(),
            language: "rust".to_string(),
            line_number: 1,
            is_runnable: true,
            attributes: Vec::new(),
        };

        let prepared = validator.prepare_rust_code(&example).unwrap();
        assert!(prepared.contains("#[tokio::main]"));
        assert!(prepared.contains("async fn main"));
    }

    #[tokio::test]
    async fn test_tokio_usage_validation() {
        let validator = create_test_validator().await;
        let config = AsyncValidationConfig::default();

        let example = CodeExample {
            content: r#"
#[test]
async fn test_something() {
    // This should trigger a warning about using #[test] with async
}
"#
            .to_string(),
            language: "rust".to_string(),
            line_number: 1,
            is_runnable: true,
            attributes: Vec::new(),
        };

        let result = validator.validate_async_patterns(&example, &config).await.unwrap();
        assert!(!result.warnings.is_empty());
        assert!(result.warnings.iter().any(|w| w.contains("tokio::test")));
    }

    #[tokio::test]
    async fn test_blocking_calls_validation() {
        let validator = create_test_validator().await;
        let config = AsyncValidationConfig::default();

        let example = CodeExample {
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

        let result = validator.validate_async_patterns(&example, &config).await.unwrap();
        assert!(!result.warnings.is_empty());
        assert!(result.warnings.iter().any(|w| w.contains("blocking call")));
        assert!(result.suggestions.iter().any(|s| s.contains("tokio::fs")));
    }

    #[tokio::test]
    async fn test_async_trait_validation() {
        let validator = create_test_validator().await;
        let config = AsyncValidationConfig::default();

        let example = CodeExample {
            content: r#"
trait MyTrait {
    async fn do_something(&self) -> Result<(), Error>;
}
"#
            .to_string(),
            language: "rust".to_string(),
            line_number: 1,
            is_runnable: true,
            attributes: Vec::new(),
        };

        let result = validator.validate_async_patterns(&example, &config).await.unwrap();
        assert!(!result.warnings.is_empty());
        assert!(result.warnings.iter().any(|w| w.contains("async-trait")));
    }

    #[tokio::test]
    async fn test_comprehensive_async_validation() {
        let validator = create_test_validator().await;
        let config = AsyncValidationConfig {
            require_runtime_setup: true,
            validate_error_handling: true,
            check_await_patterns: true,
            max_async_nesting: 2,
        };

        let example = CodeExample {
            content: r#"
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let result = async_operation().await?;
    
    tokio::spawn(async move {
        async_nested_operation().await
    }).await??;
    
    Ok(())
}

async fn async_operation() -> Result<String, std::io::Error> {
    tokio::fs::read_to_string("file.txt").await
}

async fn async_nested_operation() -> Result<(), std::io::Error> {
    Ok(())
}
"#
            .to_string(),
            language: "rust".to_string(),
            line_number: 1,
            is_runnable: true,
            attributes: Vec::new(),
        };

        let result = validator.validate_async_patterns(&example, &config).await.unwrap();
        // This should be a well-formed async example
        assert!(result.success);
        assert!(result.errors.is_empty());
    }
}
