//! Documentation parser for extracting and analyzing markdown content.
//!
//! This module provides functionality to parse markdown documentation files and extract
//! relevant information for validation including code blocks, API references, version
//! references, and internal links.

use crate::{AuditError, Result};
use regex::Regex;
use std::path::{Path, PathBuf};

/// Parser for documentation files that extracts validation-relevant content.
#[derive(Debug)]
pub struct DocumentationParser {
    /// Current workspace version for validation
    workspace_version: String,
    /// Current Rust version requirement
    #[allow(dead_code)]
    rust_version: String,
    /// Compiled regex patterns for efficient parsing
    patterns: ParserPatterns,
}

/// Compiled regex patterns used by the parser.
#[derive(Debug)]
struct ParserPatterns {
    /// Pattern for matching code blocks with language specification
    code_block: Regex,
    /// Pattern for matching API references (e.g., `adk_core::Agent`)
    api_reference: Regex,
    /// Pattern for matching version references in dependencies
    version_reference: Regex,
    /// Pattern for matching internal markdown links
    internal_link: Regex,
    /// Pattern for matching feature flag mentions
    feature_flag: Regex,
    /// Pattern for matching Rust version requirements
    rust_version: Regex,
    /// Pattern for matching TOML dependency specifications
    toml_dependency: Regex,
}

/// Represents a parsed documentation file with extracted content.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedDocument {
    /// Path to the documentation file
    pub file_path: PathBuf,
    /// Extracted code examples from the document
    pub code_examples: Vec<CodeExample>,
    /// API references found in the document
    pub api_references: Vec<ApiReference>,
    /// Version references found in the document
    pub version_references: Vec<VersionReference>,
    /// Internal links to other documentation files
    pub internal_links: Vec<InternalLink>,
    /// Feature flag mentions in the document
    pub feature_mentions: Vec<FeatureMention>,
}

/// Represents a code example extracted from documentation.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeExample {
    /// The code content
    pub content: String,
    /// Programming language (e.g., "rust", "toml", "bash")
    pub language: String,
    /// Line number where the code block starts
    pub line_number: usize,
    /// Whether this example should be runnable/compilable
    pub is_runnable: bool,
    /// Additional attributes from the code block (e.g., "ignore", "no_run")
    pub attributes: Vec<String>,
}

/// Represents an API reference found in documentation.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiReference {
    /// Name of the crate being referenced
    pub crate_name: String,
    /// Full path to the API item (e.g., "core::Agent::run")
    pub item_path: String,
    /// Type of API item being referenced
    pub item_type: ApiItemType,
    /// Line number where the reference appears
    pub line_number: usize,
    /// Context around the reference for better error reporting
    pub context: String,
}

/// Types of API items that can be referenced in documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiItemType {
    /// Struct definition
    Struct,
    /// Function or method
    Function,
    /// Method on a struct/trait
    Method,
    /// Trait definition
    Trait,
    /// Enum definition
    Enum,
    /// Constant value
    Constant,
    /// Module
    Module,
    /// Type alias
    TypeAlias,
    /// Unknown or unspecified type
    Unknown,
}

/// Represents a version reference found in documentation.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionReference {
    /// The version string found (e.g., "0.1.0", "1.85.0")
    pub version: String,
    /// Type of version reference
    pub version_type: VersionType,
    /// Line number where the version appears
    pub line_number: usize,
    /// Context around the version for better error reporting
    pub context: String,
}

/// Types of version references that can appear in documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionType {
    /// Crate version in Cargo.toml dependencies
    CrateVersion,
    /// Rust version requirement
    RustVersion,
    /// ADK-Rust workspace version
    WorkspaceVersion,
    /// Generic version reference
    Generic,
}

/// Represents an internal link to another documentation file.
#[derive(Debug, Clone, PartialEq)]
pub struct InternalLink {
    /// The link target (file path or anchor)
    pub target: String,
    /// Display text for the link
    pub text: String,
    /// Line number where the link appears
    pub line_number: usize,
    /// Whether this is a relative or absolute link
    pub is_relative: bool,
}

/// Represents a feature flag mention in documentation.
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureMention {
    /// Name of the feature flag
    pub feature_name: String,
    /// Crate that defines this feature
    pub crate_name: Option<String>,
    /// Line number where the feature is mentioned
    pub line_number: usize,
    /// Context around the mention
    pub context: String,
}

impl DocumentationParser {
    /// Creates a new documentation parser with the given workspace configuration.
    ///
    /// # Arguments
    ///
    /// * `workspace_version` - Current version of the ADK-Rust workspace
    /// * `rust_version` - Required Rust version for the workspace
    ///
    /// # Returns
    ///
    /// A new `DocumentationParser` instance or an error if regex compilation fails.
    pub fn new(workspace_version: String, rust_version: String) -> Result<Self> {
        let patterns = ParserPatterns::new()?;

        Ok(Self { workspace_version, rust_version, patterns })
    }

    /// Parses a markdown file and extracts all relevant content for validation.
    ///
    /// # Arguments
    ///
    /// * `file_path` - Path to the markdown file to parse
    ///
    /// # Returns
    ///
    /// A `ParsedDocument` containing all extracted content or an error if parsing fails.
    pub async fn parse_file(&self, file_path: &Path) -> Result<ParsedDocument> {
        let content = tokio::fs::read_to_string(file_path).await.map_err(|e| {
            AuditError::IoError { path: file_path.to_path_buf(), details: e.to_string() }
        })?;

        self.parse_content(file_path, &content)
    }

    /// Parses markdown content from a string.
    ///
    /// # Arguments
    ///
    /// * `file_path` - Path to the file (for error reporting)
    /// * `content` - Markdown content to parse
    ///
    /// # Returns
    ///
    /// A `ParsedDocument` containing all extracted content.
    pub fn parse_content(&self, file_path: &Path, content: &str) -> Result<ParsedDocument> {
        let lines: Vec<&str> = content.lines().collect();

        let code_examples = self.extract_code_examples(&lines)?;
        let api_references = self.extract_api_references(&lines)?;
        let version_references = self.extract_version_references(&lines)?;
        let internal_links = self.extract_internal_links(&lines)?;
        let feature_mentions = self.extract_feature_mentions(&lines)?;

        Ok(ParsedDocument {
            file_path: file_path.to_path_buf(),
            code_examples,
            api_references,
            version_references,
            internal_links,
            feature_mentions,
        })
    }

    /// Extracts Rust code blocks specifically for compilation testing.
    ///
    /// This method focuses on extracting Rust code examples that should be compilable,
    /// filtering out display-only examples and identifying runnable vs non-runnable code.
    pub fn extract_rust_examples(&self, content: &str) -> Result<Vec<CodeExample>> {
        let lines: Vec<&str> = content.lines().collect();
        let all_examples = self.extract_code_examples(&lines)?;

        // Filter to only Rust examples and enhance with compilation metadata
        let rust_examples: Vec<CodeExample> = all_examples
            .into_iter()
            .filter(|example| example.language == "rust")
            .map(|mut example| {
                // Enhance with additional compilation metadata
                example.is_runnable = self.should_compile_rust_example(&example);
                example
            })
            .collect();

        Ok(rust_examples)
    }

    /// Extracts configuration examples from TOML code blocks.
    ///
    /// This method specifically looks for Cargo.toml configuration examples
    /// and extracts feature flag and dependency information.
    pub fn extract_configuration_examples(&self, content: &str) -> Result<Vec<CodeExample>> {
        let lines: Vec<&str> = content.lines().collect();
        let all_examples = self.extract_code_examples(&lines)?;

        // Filter to configuration files (TOML, YAML, JSON)
        let config_examples: Vec<CodeExample> = all_examples
            .into_iter()
            .filter(|example| matches!(example.language.as_str(), "toml" | "yaml" | "yml" | "json"))
            .collect();

        Ok(config_examples)
    }

    /// Determines if a Rust code example should be compiled.
    ///
    /// This method uses more sophisticated logic to determine compilation eligibility
    /// based on code content, attributes, and context.
    fn should_compile_rust_example(&self, example: &CodeExample) -> bool {
        // Don't compile if explicitly marked as non-runnable
        if example.attributes.contains(&"ignore".to_string())
            || example.attributes.contains(&"no_run".to_string())
            || example.attributes.contains(&"compile_fail".to_string())
        {
            return false;
        }

        // Check for incomplete code patterns that shouldn't be compiled
        let content = &example.content;

        // Skip examples that are clearly incomplete
        if content.contains("// ...") 
            || content.contains("/* ... */")
            || content.trim().starts_with("use ")  // Just import statements
            || content.trim().starts_with("//")    // Just comments
            || content.lines().count() < 2
        {
            // Too short to be meaningful
            return false;
        }

        // Skip examples that look like they're showing syntax or partial code
        if content.contains("fn example(")
            || content.contains("struct Example")
            || content.contains("// Example:")
        {
            return false;
        }

        // Examples with main functions or test functions are usually runnable
        if content.contains("fn main(")
            || content.contains("#[test]")
            || content.contains("#[tokio::main]")
        {
            return true;
        }

        // Examples that use common ADK patterns are likely runnable
        if content.contains("adk_") && (content.contains(".await") || content.contains("async")) {
            return true;
        }

        // Default to runnable for other complete-looking Rust code
        true
    }

    /// Extracts code blocks from markdown content.
    fn extract_code_examples(&self, lines: &[&str]) -> Result<Vec<CodeExample>> {
        let mut examples = Vec::new();
        let mut in_code_block = false;
        let mut current_code = String::new();
        let mut current_language = String::new();
        let mut current_attributes = Vec::new();
        let mut start_line = 0;

        for (line_num, line) in lines.iter().enumerate() {
            if let Some(captures) = self.patterns.code_block.captures(line) {
                if line.starts_with("```") {
                    if in_code_block {
                        // End of code block
                        let is_runnable =
                            self.is_code_runnable(&current_language, &current_attributes);

                        examples.push(CodeExample {
                            content: current_code.trim().to_string(),
                            language: current_language.clone(),
                            line_number: start_line + 1, // 1-based line numbers
                            is_runnable,
                            attributes: current_attributes.clone(),
                        });

                        // Reset for next block
                        current_code.clear();
                        current_language.clear();
                        current_attributes.clear();
                        in_code_block = false;
                    } else {
                        // Start of code block
                        if let Some(lang_match) = captures.get(1) {
                            let lang_spec = lang_match.as_str();
                            let (language, attributes) = self.parse_language_spec(lang_spec);
                            current_language = language;
                            current_attributes = attributes;
                        }
                        start_line = line_num;
                        in_code_block = true;
                    }
                }
            } else if in_code_block {
                current_code.push_str(line);
                current_code.push('\n');
            }
        }

        Ok(examples)
    }

    /// Extracts API references from markdown content.
    fn extract_api_references(&self, lines: &[&str]) -> Result<Vec<ApiReference>> {
        let mut references = Vec::new();

        for (line_num, line) in lines.iter().enumerate() {
            for captures in self.patterns.api_reference.captures_iter(line) {
                if let Some(api_match) = captures.get(0) {
                    let full_path = api_match.as_str();
                    let (crate_name, item_path, item_type) = self.parse_api_path(full_path);

                    references.push(ApiReference {
                        crate_name,
                        item_path: item_path.to_string(),
                        item_type,
                        line_number: line_num + 1,
                        context: line.to_string(),
                    });
                }
            }
        }

        Ok(references)
    }

    /// Extracts version references from markdown content.
    fn extract_version_references(&self, lines: &[&str]) -> Result<Vec<VersionReference>> {
        let mut references = Vec::new();

        for (line_num, line) in lines.iter().enumerate() {
            // Check for Rust version requirements
            for captures in self.patterns.rust_version.captures_iter(line) {
                if let Some(version_match) = captures.get(1) {
                    references.push(VersionReference {
                        version: version_match.as_str().to_string(),
                        version_type: VersionType::RustVersion,
                        line_number: line_num + 1,
                        context: line.to_string(),
                    });
                }
            }

            // Check for general version references
            for captures in self.patterns.version_reference.captures_iter(line) {
                if let Some(version_match) = captures.get(1) {
                    let version_type = self.classify_version_type(line, version_match.as_str());

                    references.push(VersionReference {
                        version: version_match.as_str().to_string(),
                        version_type,
                        line_number: line_num + 1,
                        context: line.to_string(),
                    });
                }
            }
        }

        Ok(references)
    }

    /// Extracts internal links from markdown content.
    fn extract_internal_links(&self, lines: &[&str]) -> Result<Vec<InternalLink>> {
        let mut links = Vec::new();

        for (line_num, line) in lines.iter().enumerate() {
            for captures in self.patterns.internal_link.captures_iter(line) {
                if let (Some(text_match), Some(target_match)) = (captures.get(1), captures.get(2)) {
                    let target = target_match.as_str();
                    let is_relative = !target.starts_with("http") && !target.starts_with('#');

                    links.push(InternalLink {
                        target: target.to_string(),
                        text: text_match.as_str().to_string(),
                        line_number: line_num + 1,
                        is_relative,
                    });
                }
            }
        }

        Ok(links)
    }

    /// Extracts feature flag mentions from markdown content.
    fn extract_feature_mentions(&self, lines: &[&str]) -> Result<Vec<FeatureMention>> {
        let mut mentions = Vec::new();

        for (line_num, line) in lines.iter().enumerate() {
            for captures in self.patterns.feature_flag.captures_iter(line) {
                if let Some(feature_match) = captures.get(1) {
                    let feature_name = feature_match.as_str().to_string();
                    let crate_name = self.extract_crate_from_context(line);

                    mentions.push(FeatureMention {
                        feature_name,
                        crate_name,
                        line_number: line_num + 1,
                        context: line.to_string(),
                    });
                }
            }
        }

        Ok(mentions)
    }

    /// Determines if a code example should be runnable based on language and attributes.
    fn is_code_runnable(&self, language: &str, attributes: &[String]) -> bool {
        // Rust code is runnable unless explicitly marked otherwise
        if language == "rust" {
            !attributes.contains(&"ignore".to_string())
                && !attributes.contains(&"no_run".to_string())
                && !attributes.contains(&"compile_fail".to_string())
        } else {
            // Other languages are not runnable by default
            false
        }
    }

    /// Parses language specification from code block header.
    fn parse_language_spec(&self, lang_spec: &str) -> (String, Vec<String>) {
        let parts: Vec<&str> = lang_spec.split(',').map(|s| s.trim()).collect();

        if parts.is_empty() {
            return ("text".to_string(), Vec::new());
        }

        let language = parts[0].to_string();
        let attributes = parts[1..].iter().map(|s| s.to_string()).collect();

        (language, attributes)
    }

    /// Parses an API path to extract crate name, item path, and type.
    fn parse_api_path(&self, full_path: &str) -> (String, String, ApiItemType) {
        let parts: Vec<&str> = full_path.split("::").collect();

        if parts.is_empty() {
            return ("unknown".to_string(), full_path.to_string(), ApiItemType::Unknown);
        }

        let crate_name = parts[0].to_string();
        let item_path = full_path.to_string();

        // Infer item type from naming conventions and context
        let item_type = if let Some(last_part) = parts.last() {
            self.infer_api_item_type(last_part)
        } else {
            ApiItemType::Unknown
        };

        (crate_name, item_path, item_type)
    }

    /// Infers the type of an API item from its name and context.
    fn infer_api_item_type(&self, item_name: &str) -> ApiItemType {
        // Basic heuristics for inferring API item types
        if item_name.chars().next().is_some_and(|c| c.is_uppercase()) {
            // Starts with uppercase - likely struct, enum, or trait
            if item_name.ends_with("Error") || item_name.ends_with("Result") {
                ApiItemType::Enum
            } else {
                ApiItemType::Struct
            }
        } else if item_name.contains('(') || item_name.ends_with("()") {
            // Contains parentheses - likely function or method
            ApiItemType::Function
        } else if item_name.chars().all(|c| c.is_uppercase() || c == '_') {
            // All uppercase - likely constant
            ApiItemType::Constant
        } else {
            // Default to unknown for ambiguous cases
            ApiItemType::Unknown
        }
    }

    /// Classifies the type of version reference based on context.
    fn classify_version_type(&self, line: &str, version: &str) -> VersionType {
        if line.contains("rust-version") || line.contains("rustc") {
            VersionType::RustVersion
        } else if line.contains("adk-") || version == self.workspace_version {
            VersionType::WorkspaceVersion
        } else if line.contains("version") && line.contains("=") {
            VersionType::CrateVersion
        } else {
            VersionType::Generic
        }
    }

    /// Extracts crate name from the context of a feature mention.
    fn extract_crate_from_context(&self, line: &str) -> Option<String> {
        // First, look for TOML dependency specifications (e.g., "adk-rust = { ... }")
        if let Some(captures) = self.patterns.toml_dependency.captures(line) {
            if let Some(crate_match) = captures.get(1) {
                return Some(crate_match.as_str().to_string());
            }
        }

        // Fallback to API reference patterns (e.g., "adk_core::")
        if let Some(captures) = self.patterns.api_reference.captures(line) {
            if let Some(crate_match) = captures.get(1) {
                return Some(crate_match.as_str().to_string());
            }
        }

        None
    }
}

impl ParserPatterns {
    /// Creates new compiled regex patterns for parsing.
    fn new() -> Result<Self> {
        Ok(Self {
            code_block: Regex::new(r"^```(\w+(?:,\w+)*)?").map_err(|e| AuditError::RegexError {
                pattern: "code_block".to_string(),
                details: e.to_string(),
            })?,

            api_reference: Regex::new(r"\b(adk_\w+)::([\w:]+)").map_err(|e| {
                AuditError::RegexError {
                    pattern: "api_reference".to_string(),
                    details: e.to_string(),
                }
            })?,

            version_reference: Regex::new(r#"version\s*=\s*"([^"]+)""#).map_err(|e| {
                AuditError::RegexError {
                    pattern: "version_reference".to_string(),
                    details: e.to_string(),
                }
            })?,

            internal_link: Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").map_err(|e| {
                AuditError::RegexError {
                    pattern: "internal_link".to_string(),
                    details: e.to_string(),
                }
            })?,

            feature_flag: Regex::new(r#"features?\s*=\s*\[?"([^"\]]+)""#).map_err(|e| {
                AuditError::RegexError {
                    pattern: "feature_flag".to_string(),
                    details: e.to_string(),
                }
            })?,

            rust_version: Regex::new(r#"rust-version\s*=\s*"([^"]+)""#).map_err(|e| {
                AuditError::RegexError {
                    pattern: "rust_version".to_string(),
                    details: e.to_string(),
                }
            })?,

            toml_dependency: Regex::new(r#"^([a-zA-Z0-9_-]+)\s*=\s*\{"#).map_err(|e| {
                AuditError::RegexError {
                    pattern: "toml_dependency".to_string(),
                    details: e.to_string(),
                }
            })?,
        })
    }
}

impl Default for ParsedDocument {
    fn default() -> Self {
        Self {
            file_path: PathBuf::new(),
            code_examples: Vec::new(),
            api_references: Vec::new(),
            version_references: Vec::new(),
            internal_links: Vec::new(),
            feature_mentions: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn create_test_parser() -> DocumentationParser {
        DocumentationParser::new("0.1.0".to_string(), "1.85.0".to_string()).unwrap()
    }

    #[test]
    fn test_parser_creation() {
        let parser = create_test_parser();
        assert_eq!(parser.workspace_version, "0.1.0");
        assert_eq!(parser.rust_version, "1.85.0");
    }

    #[test]
    fn test_code_block_extraction() {
        let parser = create_test_parser();
        let content = r#"
# Example

Here's some Rust code:

```rust
fn main() {
    println!("Hello, world!");
}
```

And some TOML:

```toml
[dependencies]
serde = "1.0"
```
"#;

        let result = parser.parse_content(&PathBuf::from("test.md"), content).unwrap();

        assert_eq!(result.code_examples.len(), 2);

        let rust_example = &result.code_examples[0];
        assert_eq!(rust_example.language, "rust");
        assert!(rust_example.is_runnable);
        assert!(rust_example.content.contains("println!"));

        let toml_example = &result.code_examples[1];
        assert_eq!(toml_example.language, "toml");
        assert!(!toml_example.is_runnable);
    }

    #[test]
    fn test_api_reference_extraction() {
        let parser = create_test_parser();
        let content = r#"
Use `adk_core::Agent` for creating agents.
The `adk_model::Llm::generate` method is useful.
"#;

        let result = parser.parse_content(&PathBuf::from("test.md"), content).unwrap();

        assert_eq!(result.api_references.len(), 2);

        let first_ref = &result.api_references[0];
        assert_eq!(first_ref.crate_name, "adk_core");
        assert_eq!(first_ref.item_path, "adk_core::Agent");

        let second_ref = &result.api_references[1];
        assert_eq!(second_ref.crate_name, "adk_model");
        assert_eq!(second_ref.item_path, "adk_model::Llm::generate");
    }

    #[test]
    fn test_version_reference_extraction() {
        let parser = create_test_parser();
        let content = r#"
```toml
[dependencies]
adk-core = { version = "0.1.0" }
serde = { version = "1.0.195" }

[package]
rust-version = "1.85.0"
```
"#;

        let result = parser.parse_content(&PathBuf::from("test.md"), content).unwrap();

        // Should find version references in the content
        assert!(!result.version_references.is_empty());
    }

    #[test]
    fn test_internal_link_extraction() {
        let parser = create_test_parser();
        let content = r#"
See the [Getting Started](./getting-started.md) guide.
Check out [API Reference](../api/index.md) for details.
"#;

        let result = parser.parse_content(&PathBuf::from("test.md"), content).unwrap();

        assert_eq!(result.internal_links.len(), 2);

        let first_link = &result.internal_links[0];
        assert_eq!(first_link.text, "Getting Started");
        assert_eq!(first_link.target, "./getting-started.md");
        assert!(first_link.is_relative);
    }

    #[test]
    fn test_feature_mention_extraction() {
        let parser = create_test_parser();
        let content = r#"
```toml
[dependencies]
adk-core = { version = "0.1.0", features = ["async"] }
```

Enable the `cuda` feature for GPU acceleration.
"#;

        let result = parser.parse_content(&PathBuf::from("test.md"), content).unwrap();

        // Should find feature mentions
        assert!(!result.feature_mentions.is_empty());
    }

    #[test]
    fn test_code_attributes_parsing() {
        let parser = create_test_parser();
        let content = r#"
```rust,ignore
// This code is ignored
fn ignored_example() {}
```

```rust,no_run
// This code doesn't run
fn no_run_example() {}
```
"#;

        let result = parser.parse_content(&PathBuf::from("test.md"), content).unwrap();

        assert_eq!(result.code_examples.len(), 2);

        let ignored_example = &result.code_examples[0];
        assert!(!ignored_example.is_runnable);
        assert!(ignored_example.attributes.contains(&"ignore".to_string()));

        let no_run_example = &result.code_examples[1];
        assert!(!no_run_example.is_runnable);
        assert!(no_run_example.attributes.contains(&"no_run".to_string()));
    }

    #[test]
    fn test_rust_example_extraction() {
        let parser = create_test_parser();
        let content = r#"
```rust
fn main() {
    println!("This should be runnable");
}
```

```rust,ignore
fn ignored() {}
```

```toml
[dependencies]
serde = "1.0"
```

```rust
// Just a comment
```
"#;

        let rust_examples = parser.extract_rust_examples(content).unwrap();

        // Should have 3 Rust examples, but only some should be runnable
        assert_eq!(rust_examples.len(), 3);

        // First example with main() should be runnable
        assert!(rust_examples[0].is_runnable);
        assert!(rust_examples[0].content.contains("main"));

        // Second example is ignored
        assert!(!rust_examples[1].is_runnable);

        // Third example is just a comment, shouldn't be runnable
        assert!(!rust_examples[2].is_runnable);
    }

    #[test]
    fn test_configuration_example_extraction() {
        let parser = create_test_parser();
        let content = r#"
```toml
[dependencies]
adk-core = "0.1.0"
```

```yaml
version: "3.8"
services:
  app:
    image: rust:latest
```

```rust
fn main() {}
```
"#;

        let config_examples = parser.extract_configuration_examples(content).unwrap();

        // Should have 2 configuration examples (TOML and YAML)
        assert_eq!(config_examples.len(), 2);

        assert_eq!(config_examples[0].language, "toml");
        assert_eq!(config_examples[1].language, "yaml");
    }

    #[test]
    fn test_enhanced_feature_detection() {
        let parser = create_test_parser();
        let content = r#"
Enable the `cuda` feature for GPU acceleration:

```toml
[dependencies]
adk-mistralrs = { version = "0.1.0", features = ["cuda", "flash-attn"] }
```

You can also use the `async` feature with adk-core.
"#;

        let result = parser.parse_content(&PathBuf::from("test.md"), content).unwrap();

        // Should detect feature mentions
        assert!(!result.feature_mentions.is_empty());

        // Should also detect them in code examples
        let config_examples = parser.extract_configuration_examples(content).unwrap();
        assert_eq!(config_examples.len(), 1);
        assert!(config_examples[0].content.contains("features"));
    }
}
