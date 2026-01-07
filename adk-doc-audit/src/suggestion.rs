//! Automated fix suggestion engine for documentation audit issues.
//!
//! This module provides functionality to generate automated suggestions for fixing
//! various types of documentation issues including API signature corrections,
//! version inconsistencies, compilation errors, and diff-style updates.

use crate::{
    ApiItemType, ApiReference, CompilationError, CrateInfo, ErrorType, PublicApi, Result,
    VersionReference, VersionType,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::instrument;

/// Engine for generating automated fix suggestions.
#[derive(Debug)]
pub struct SuggestionEngine {
    /// Registry of available crates and their APIs
    crate_registry: HashMap<String, CrateInfo>,
    /// Current workspace version information
    workspace_version: String,
    /// Cache of generated suggestions to avoid duplicates
    suggestion_cache: HashMap<String, Vec<Suggestion>>,
}

/// Represents an automated fix suggestion.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// Type of suggestion
    pub suggestion_type: SuggestionType,
    /// Human-readable description of the suggestion
    pub description: String,
    /// Original text that needs to be changed
    pub original_text: String,
    /// Suggested replacement text
    pub suggested_text: String,
    /// File path where the change should be made
    pub file_path: PathBuf,
    /// Line number where the change should be made (if known)
    pub line_number: Option<usize>,
    /// Column number where the change should be made (if known)
    pub column_number: Option<usize>,
    /// Confidence level of the suggestion (0.0 to 1.0)
    pub confidence: f64,
    /// Additional context or explanation
    pub context: Option<String>,
    /// Diff-style representation of the change
    pub diff: Option<String>,
}

/// Types of automated suggestions that can be generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionType {
    /// API signature correction
    ApiSignatureCorrection,
    /// Version number update
    VersionUpdate,
    /// Compilation fix
    CompilationFix,
    /// Import statement fix
    ImportFix,
    /// Dependency addition
    DependencyAddition,
    /// Deprecated API replacement
    DeprecatedApiReplacement,
    /// Async pattern fix
    AsyncPatternFix,
    /// Link correction
    LinkCorrection,
    /// Feature flag correction
    FeatureFlagCorrection,
    /// Documentation structure improvement
    StructureImprovement,
}

/// Configuration for suggestion generation.
#[derive(Debug, Clone)]
pub struct SuggestionConfig {
    /// Minimum confidence threshold for suggestions
    pub min_confidence: f64,
    /// Maximum number of suggestions per issue
    pub max_suggestions_per_issue: usize,
    /// Whether to generate diff-style output
    pub generate_diffs: bool,
    /// Whether to include context in suggestions
    pub include_context: bool,
    /// Whether to cache suggestions
    pub enable_caching: bool,
}

impl SuggestionEngine {
    /// Creates a new suggestion engine.
    ///
    /// # Arguments
    ///
    /// * `crate_registry` - Registry of available crates and their APIs
    /// * `workspace_version` - Current workspace version
    ///
    /// # Returns
    ///
    /// A new `SuggestionEngine` instance.
    pub fn new(crate_registry: HashMap<String, CrateInfo>, workspace_version: String) -> Self {
        Self { crate_registry, workspace_version, suggestion_cache: HashMap::new() }
    }

    /// Creates a new suggestion engine with empty registry (for orchestrator use).
    pub fn new_empty() -> Self {
        Self {
            crate_registry: HashMap::new(),
            workspace_version: "0.1.0".to_string(),
            suggestion_cache: HashMap::new(),
        }
    }

    /// Generates API signature correction suggestions.
    ///
    /// # Arguments
    ///
    /// * `api_ref` - The API reference that needs correction
    /// * `file_path` - Path to the file containing the reference
    /// * `config` - Configuration for suggestion generation
    ///
    /// # Returns
    ///
    /// A vector of suggestions for correcting the API signature.
    #[instrument(skip(self, config))]
    pub fn suggest_api_signature_corrections(
        &self,
        api_ref: &ApiReference,
        file_path: &Path,
        config: &SuggestionConfig,
    ) -> Result<Vec<Suggestion>> {
        let mut suggestions = Vec::new();

        // Check cache first
        let cache_key = format!("api_{}_{}", api_ref.crate_name, api_ref.item_path);
        if config.enable_caching {
            if let Some(cached) = self.suggestion_cache.get(&cache_key) {
                return Ok(cached.clone());
            }
        }

        // Find the crate in our registry
        if let Some(crate_info) = self.crate_registry.get(&api_ref.crate_name) {
            // Look for exact matches first
            if let Some(exact_match) = self.find_exact_api_match(crate_info, api_ref) {
                let suggestion = self.create_api_correction_suggestion(
                    api_ref,
                    exact_match,
                    file_path,
                    1.0, // High confidence for exact matches
                    config,
                )?;
                suggestions.push(suggestion);
            } else {
                // Look for similar APIs (fuzzy matching)
                let similar_apis = self.find_similar_apis(crate_info, api_ref);
                for (api, confidence) in similar_apis {
                    if confidence >= config.min_confidence {
                        let suggestion = self.create_api_correction_suggestion(
                            api_ref, api, file_path, confidence, config,
                        )?;
                        suggestions.push(suggestion);
                    }
                }
            }

            // Check for deprecated APIs and suggest replacements
            if let Some(deprecated_replacement) =
                self.find_deprecated_replacement(crate_info, api_ref)
            {
                let suggestion =
                    Suggestion {
                        suggestion_type: SuggestionType::DeprecatedApiReplacement,
                        description: format!(
                            "Replace deprecated API '{}' with '{}'",
                            api_ref.item_path, deprecated_replacement.path
                        ),
                        original_text: api_ref.item_path.clone(),
                        suggested_text: deprecated_replacement.path.clone(),
                        file_path: file_path.to_path_buf(),
                        line_number: Some(api_ref.line_number),
                        column_number: None,
                        confidence: 0.9,
                        context: Some(format!(
                            "The API '{}' has been deprecated. Use '{}' instead.",
                            api_ref.item_path, deprecated_replacement.path
                        )),
                        diff: if config.generate_diffs {
                            Some(self.generate_simple_diff(
                                &api_ref.item_path,
                                &deprecated_replacement.path,
                            ))
                        } else {
                            None
                        },
                    };
                suggestions.push(suggestion);
            }
        } else {
            // Crate not found - suggest adding dependency
            let suggestion = Suggestion {
                suggestion_type: SuggestionType::DependencyAddition,
                description: format!("Add missing dependency '{}'", api_ref.crate_name),
                original_text: String::new(),
                suggested_text: format!("{} = \"{}\"", api_ref.crate_name, self.workspace_version),
                file_path: file_path.to_path_buf(),
                line_number: None,
                column_number: None,
                confidence: 0.8,
                context: Some(format!(
                    "The crate '{}' is not found in dependencies. Add it to Cargo.toml.",
                    api_ref.crate_name
                )),
                diff: None,
            };
            suggestions.push(suggestion);
        }

        // Limit suggestions per configuration
        suggestions.truncate(config.max_suggestions_per_issue);

        Ok(suggestions)
    }

    /// Generates version consistency correction suggestions.
    ///
    /// # Arguments
    ///
    /// * `version_ref` - The version reference that needs correction
    /// * `crate_name` - Name of the crate for this version reference
    /// * `file_path` - Path to the file containing the reference
    /// * `config` - Configuration for suggestion generation
    ///
    /// # Returns
    ///
    /// A vector of suggestions for correcting version inconsistencies.
    #[instrument(skip(self, config))]
    pub fn suggest_version_corrections(
        &self,
        version_ref: &VersionReference,
        crate_name: &str,
        file_path: &Path,
        config: &SuggestionConfig,
    ) -> Result<Vec<Suggestion>> {
        let mut suggestions = Vec::new();

        let correct_version = match version_ref.version_type {
            VersionType::CrateVersion => {
                // Get the correct version from crate registry
                if let Some(crate_info) = self.crate_registry.get(crate_name) {
                    crate_info.version.clone()
                } else {
                    self.workspace_version.clone()
                }
            }
            VersionType::RustVersion => {
                // Get Rust version from workspace
                self.get_workspace_rust_version().unwrap_or_else(|| "1.85.0".to_string())
            }
            VersionType::WorkspaceVersion => {
                // Use workspace version
                self.workspace_version.clone()
            }
            VersionType::Generic => {
                // Get dependency version from workspace
                self.get_dependency_version(crate_name)
                    .unwrap_or_else(|| self.workspace_version.clone())
            }
        };

        if version_ref.version != correct_version {
            let suggestion = Suggestion {
                suggestion_type: SuggestionType::VersionUpdate,
                description: format!(
                    "Update {} version from '{}' to '{}'",
                    crate_name, version_ref.version, correct_version
                ),
                original_text: version_ref.version.clone(),
                suggested_text: correct_version.clone(),
                file_path: file_path.to_path_buf(),
                line_number: Some(version_ref.line_number),
                column_number: None,
                confidence: 0.95,
                context: if config.include_context {
                    Some(format!(
                        "Version '{}' is outdated. Current version is '{}'.",
                        version_ref.version, correct_version
                    ))
                } else {
                    None
                },
                diff: if config.generate_diffs {
                    Some(self.generate_simple_diff(&version_ref.version, &correct_version))
                } else {
                    None
                },
            };
            suggestions.push(suggestion);
        }

        Ok(suggestions)
    }

    /// Generates compilation fix suggestions based on compilation errors.
    ///
    /// # Arguments
    ///
    /// * `errors` - Compilation errors to generate fixes for
    /// * `file_path` - Path to the file with compilation errors
    /// * `config` - Configuration for suggestion generation
    ///
    /// # Returns
    ///
    /// A vector of suggestions for fixing compilation errors.
    #[instrument(skip(self, config))]
    pub fn suggest_compilation_fixes(
        &self,
        errors: &[CompilationError],
        file_path: &Path,
        config: &SuggestionConfig,
    ) -> Result<Vec<Suggestion>> {
        let mut suggestions = Vec::new();

        for error in errors {
            match error.error_type {
                ErrorType::UnresolvedImport => {
                    if let Some(import_suggestion) = self.suggest_import_fix(&error.message) {
                        let suggestion = Suggestion {
                            suggestion_type: SuggestionType::ImportFix,
                            description: format!("Add missing import: {}", import_suggestion),
                            original_text: String::new(),
                            suggested_text: import_suggestion.clone(),
                            file_path: file_path.to_path_buf(),
                            line_number: error.line,
                            column_number: error.column,
                            confidence: 0.8,
                            context: if config.include_context {
                                Some(format!(
                                    "Import '{}' to resolve the unresolved reference.",
                                    import_suggestion
                                ))
                            } else {
                                None
                            },
                            diff: None,
                        };
                        suggestions.push(suggestion);
                    }
                }
                ErrorType::MissingDependency => {
                    if let Some(dep_suggestion) = self.suggest_dependency_addition(&error.message) {
                        let suggestion = Suggestion {
                            suggestion_type: SuggestionType::DependencyAddition,
                            description: format!("Add missing dependency: {}", dep_suggestion),
                            original_text: String::new(),
                            suggested_text: dep_suggestion.clone(),
                            file_path: file_path.to_path_buf(),
                            line_number: None,
                            column_number: None,
                            confidence: 0.85,
                            context: if config.include_context {
                                Some("Add this dependency to your Cargo.toml file.".to_string())
                            } else {
                                None
                            },
                            diff: None,
                        };
                        suggestions.push(suggestion);
                    }
                }
                ErrorType::AsyncPatternError => {
                    let async_suggestions = self.suggest_async_pattern_fixes(&error.message);
                    for async_fix in async_suggestions {
                        let suggestion = Suggestion {
                            suggestion_type: SuggestionType::AsyncPatternFix,
                            description: format!("Fix async pattern: {}", async_fix),
                            original_text: String::new(),
                            suggested_text: async_fix.clone(),
                            file_path: file_path.to_path_buf(),
                            line_number: error.line,
                            column_number: error.column,
                            confidence: 0.75,
                            context: if config.include_context {
                                Some(
                                    "Async code requires proper runtime setup and await usage."
                                        .to_string(),
                                )
                            } else {
                                None
                            },
                            diff: None,
                        };
                        suggestions.push(suggestion);
                    }
                }
                ErrorType::DeprecatedApi => {
                    if let Some(replacement) =
                        self.suggest_deprecated_api_replacement(&error.message)
                    {
                        let suggestion = Suggestion {
                            suggestion_type: SuggestionType::DeprecatedApiReplacement,
                            description: format!("Replace deprecated API with: {}", replacement),
                            original_text: String::new(),
                            suggested_text: replacement.clone(),
                            file_path: file_path.to_path_buf(),
                            line_number: error.line,
                            column_number: error.column,
                            confidence: 0.9,
                            context: if config.include_context {
                                Some(
                                    "This API has been deprecated. Use the suggested replacement."
                                        .to_string(),
                                )
                            } else {
                                None
                            },
                            diff: None,
                        };
                        suggestions.push(suggestion);
                    }
                }
                _ => {
                    // Generic compilation fix suggestions
                    if let Some(generic_fix) = self.suggest_generic_compilation_fix(&error.message)
                    {
                        let suggestion = Suggestion {
                            suggestion_type: SuggestionType::CompilationFix,
                            description: format!("Compilation fix: {}", generic_fix),
                            original_text: String::new(),
                            suggested_text: generic_fix.clone(),
                            file_path: file_path.to_path_buf(),
                            line_number: error.line,
                            column_number: error.column,
                            confidence: 0.6,
                            context: if config.include_context {
                                Some("General compilation fix suggestion.".to_string())
                            } else {
                                None
                            },
                            diff: None,
                        };
                        suggestions.push(suggestion);
                    }
                }
            }
        }

        // Limit suggestions per configuration
        suggestions.truncate(config.max_suggestions_per_issue);

        Ok(suggestions)
    }

    /// Generates diff-style update suggestions.
    ///
    /// # Arguments
    ///
    /// * `original_content` - Original file content
    /// * `suggestions` - List of suggestions to apply
    /// * `file_path` - Path to the file
    ///
    /// # Returns
    ///
    /// A diff-style string showing the proposed changes.
    pub fn generate_diff_suggestions(
        &self,
        original_content: &str,
        suggestions: &[Suggestion],
        file_path: &Path,
    ) -> Result<String> {
        let mut diff_output = String::new();

        diff_output.push_str(&format!("--- {}\n", file_path.display()));
        diff_output.push_str(&format!("+++ {}\n", file_path.display()));

        let lines: Vec<&str> = original_content.lines().collect();
        let mut modified_lines = lines.clone();

        // Apply suggestions to create modified content
        for suggestion in suggestions {
            if let Some(line_num) = suggestion.line_number {
                if line_num > 0 && line_num <= modified_lines.len() {
                    let line_index = line_num - 1;
                    let original_line = modified_lines[line_index];
                    let modified_line = original_line
                        .replace(&suggestion.original_text, &suggestion.suggested_text);
                    modified_lines[line_index] = Box::leak(modified_line.into_boxed_str());
                }
            }
        }

        // Generate unified diff format
        for (i, (original, modified)) in lines.iter().zip(modified_lines.iter()).enumerate() {
            if original != modified {
                diff_output.push_str(&format!("@@ -{},{} +{},{} @@\n", i + 1, 1, i + 1, 1));
                diff_output.push_str(&format!("-{}\n", original));
                diff_output.push_str(&format!("+{}\n", modified));
            }
        }

        Ok(diff_output)
    }

    // Private helper methods

    /// Finds an exact API match in the crate registry.
    fn find_exact_api_match<'a>(
        &self,
        crate_info: &'a CrateInfo,
        api_ref: &ApiReference,
    ) -> Option<&'a PublicApi> {
        crate_info.public_apis.iter().find(|api| {
            api.path == api_ref.item_path
                && self.api_types_match(&api.item_type, &api_ref.item_type)
        })
    }

    /// Finds similar APIs using fuzzy matching.
    fn find_similar_apis<'a>(
        &self,
        crate_info: &'a CrateInfo,
        api_ref: &ApiReference,
    ) -> Vec<(&'a PublicApi, f64)> {
        let mut similar_apis = Vec::new();

        for api in &crate_info.public_apis {
            let similarity = self.calculate_similarity(&api_ref.item_path, &api.path);
            if similarity > 0.6 {
                // Threshold for similarity
                similar_apis.push((api, similarity));
            }
        }

        // Sort by similarity (highest first)
        similar_apis.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        similar_apis
    }

    /// Finds replacement for deprecated API.
    fn find_deprecated_replacement<'a>(
        &self,
        crate_info: &'a CrateInfo,
        api_ref: &ApiReference,
    ) -> Option<&'a PublicApi> {
        // Look for non-deprecated APIs with similar names
        crate_info.public_apis.iter().find(|api| {
            !api.deprecated && self.calculate_similarity(&api_ref.item_path, &api.path) > 0.8
        })
    }

    /// Creates an API correction suggestion.
    fn create_api_correction_suggestion(
        &self,
        api_ref: &ApiReference,
        correct_api: &PublicApi,
        file_path: &Path,
        confidence: f64,
        config: &SuggestionConfig,
    ) -> Result<Suggestion> {
        Ok(Suggestion {
            suggestion_type: SuggestionType::ApiSignatureCorrection,
            description: format!(
                "Correct API signature from '{}' to '{}'",
                api_ref.item_path, correct_api.path
            ),
            original_text: api_ref.item_path.clone(),
            suggested_text: correct_api.path.clone(),
            file_path: file_path.to_path_buf(),
            line_number: Some(api_ref.line_number),
            column_number: None,
            confidence,
            context: if config.include_context {
                Some(format!("Current signature: {}", correct_api.signature))
            } else {
                None
            },
            diff: if config.generate_diffs {
                Some(self.generate_simple_diff(&api_ref.item_path, &correct_api.path))
            } else {
                None
            },
        })
    }

    /// Checks if API types match (with some flexibility).
    fn api_types_match(&self, type1: &ApiItemType, type2: &ApiItemType) -> bool {
        type1 == type2
    }

    /// Calculates similarity between two strings using Levenshtein distance.
    fn calculate_similarity(&self, s1: &str, s2: &str) -> f64 {
        let len1 = s1.len();
        let len2 = s2.len();

        if len1 == 0 && len2 == 0 {
            return 1.0;
        }

        if len1 == 0 || len2 == 0 {
            return 0.0;
        }

        let distance = self.levenshtein_distance(s1, s2);
        let max_len = len1.max(len2);

        1.0 - (distance as f64 / max_len as f64)
    }

    /// Calculates Levenshtein distance between two strings.
    fn levenshtein_distance(&self, s1: &str, s2: &str) -> usize {
        let chars1: Vec<char> = s1.chars().collect();
        let chars2: Vec<char> = s2.chars().collect();
        let len1 = chars1.len();
        let len2 = chars2.len();

        let mut matrix = vec![vec![0; len2 + 1]; len1 + 1];

        for (i, row) in matrix.iter_mut().enumerate().take(len1 + 1) {
            row[0] = i;
        }
        #[allow(clippy::needless_range_loop)]
        for j in 0..=len2 {
            matrix[0][j] = j;
        }

        for i in 1..=len1 {
            for j in 1..=len2 {
                let cost = if chars1[i - 1] == chars2[j - 1] { 0 } else { 1 };
                matrix[i][j] = (matrix[i - 1][j] + 1)
                    .min(matrix[i][j - 1] + 1)
                    .min(matrix[i - 1][j - 1] + cost);
            }
        }

        matrix[len1][len2]
    }

    /// Gets the workspace Rust version.
    fn get_workspace_rust_version(&self) -> Option<String> {
        // This would typically read from workspace Cargo.toml
        // For now, return a default
        Some("1.85.0".to_string())
    }

    /// Gets the version of a specific dependency.
    fn get_dependency_version(&self, crate_name: &str) -> Option<String> {
        self.crate_registry.get(crate_name).map(|info| info.version.clone())
    }

    /// Suggests import fixes based on error message.
    fn suggest_import_fix(&self, error_message: &str) -> Option<String> {
        if error_message.contains("adk_core") {
            Some("use adk_core::*;".to_string())
        } else if error_message.contains("adk_model") {
            Some("use adk_model::*;".to_string())
        } else if error_message.contains("adk_agent") {
            Some("use adk_agent::*;".to_string())
        } else if error_message.contains("tokio") {
            Some("use tokio;".to_string())
        } else if error_message.contains("serde") {
            Some("use serde::{Serialize, Deserialize};".to_string())
        } else if error_message.contains("anyhow") {
            Some("use anyhow::Result;".to_string())
        } else {
            None
        }
    }

    /// Suggests dependency additions based on error message.
    fn suggest_dependency_addition(&self, error_message: &str) -> Option<String> {
        if error_message.contains("adk_core") {
            Some("adk-core = { path = \"../adk-core\" }".to_string())
        } else if error_message.contains("adk_model") {
            Some("adk-model = { path = \"../adk-model\" }".to_string())
        } else if error_message.contains("tokio") {
            Some("tokio = { version = \"1.0\", features = [\"full\"] }".to_string())
        } else if error_message.contains("serde") {
            Some("serde = { version = \"1.0\", features = [\"derive\"] }".to_string())
        } else if error_message.contains("anyhow") {
            Some("anyhow = \"1.0\"".to_string())
        } else {
            None
        }
    }

    /// Suggests async pattern fixes.
    fn suggest_async_pattern_fixes(&self, error_message: &str) -> Vec<String> {
        let mut suggestions = Vec::new();

        if error_message.contains("async fn main") {
            suggestions.push("#[tokio::main]".to_string());
        }
        if error_message.contains("await") {
            suggestions.push("Add .await to async function calls".to_string());
        }
        if error_message.contains("runtime") {
            suggestions
                .push("Set up tokio runtime with #[tokio::main] or Runtime::new()".to_string());
        }

        suggestions
    }

    /// Suggests deprecated API replacements.
    fn suggest_deprecated_api_replacement(&self, _error_message: &str) -> Option<String> {
        // This would typically use a mapping of deprecated APIs to their replacements
        // For now, provide generic advice
        Some("Check the latest documentation for the current API".to_string())
    }

    /// Suggests generic compilation fixes.
    fn suggest_generic_compilation_fix(&self, error_message: &str) -> Option<String> {
        if error_message.contains("cannot find") {
            Some("Check imports and ensure the module is available".to_string())
        } else if error_message.contains("mismatched types") {
            Some("Check type annotations and ensure types match".to_string())
        } else if error_message.contains("borrow") {
            Some("Check borrowing rules and lifetime annotations".to_string())
        } else {
            None
        }
    }

    /// Generates documentation placement suggestions for new features.
    ///
    /// # Arguments
    ///
    /// * `undocumented_apis` - APIs that are not documented
    /// * `workspace_path` - Path to the workspace root
    /// * `docs_path` - Path to the documentation directory
    /// * `config` - Configuration for suggestion generation
    ///
    /// # Returns
    ///
    /// A vector of suggestions for where to document new features.
    #[instrument(skip(self, config))]
    pub fn suggest_documentation_placement(
        &self,
        undocumented_apis: &[PublicApi],
        workspace_path: &Path,
        docs_path: &Path,
        config: &SuggestionConfig,
    ) -> Result<Vec<Suggestion>> {
        let mut suggestions = Vec::new();

        for api in undocumented_apis {
            // Determine the best documentation file for this API
            let suggested_file = self.determine_documentation_file(api, docs_path)?;
            let suggested_section = self.determine_documentation_section(api);

            let suggestion = Suggestion {
                suggestion_type: SuggestionType::StructureImprovement,
                description: format!("Document '{}' in {}", api.path, suggested_file.display()),
                original_text: String::new(),
                suggested_text: self.generate_documentation_template(api),
                file_path: suggested_file,
                line_number: None,
                column_number: None,
                confidence: 0.8,
                context: if config.include_context {
                    Some(format!(
                        "Add documentation for {} in the {} section",
                        api.path, suggested_section
                    ))
                } else {
                    None
                },
                diff: None,
            };
            suggestions.push(suggestion);
        }

        // Suggest documentation structure improvements
        let structure_suggestions = self.suggest_structure_improvements(docs_path, config)?;
        suggestions.extend(structure_suggestions);

        // Limit suggestions per configuration
        suggestions.truncate(config.max_suggestions_per_issue);

        Ok(suggestions)
    }

    /// Suggests improvements to documentation structure.
    ///
    /// # Arguments
    ///
    /// * `docs_path` - Path to the documentation directory
    /// * `config` - Configuration for suggestion generation
    ///
    /// # Returns
    ///
    /// A vector of suggestions for improving documentation structure.
    #[instrument(skip(self, config))]
    pub fn suggest_structure_improvements(
        &self,
        docs_path: &Path,
        config: &SuggestionConfig,
    ) -> Result<Vec<Suggestion>> {
        let mut suggestions = Vec::new();

        // Check for missing essential documentation files
        let essential_files = [
            ("getting-started.md", "Getting Started Guide"),
            ("api-reference.md", "API Reference"),
            ("examples.md", "Examples and Tutorials"),
            ("migration-guide.md", "Migration Guide"),
            ("troubleshooting.md", "Troubleshooting Guide"),
            ("changelog.md", "Changelog"),
        ];

        for (filename, description) in &essential_files {
            let file_path = docs_path.join(filename);
            if !file_path.exists() {
                let suggestion = Suggestion {
                    suggestion_type: SuggestionType::StructureImprovement,
                    description: format!("Create missing {}", description),
                    original_text: String::new(),
                    suggested_text: self.generate_file_template(filename),
                    file_path,
                    line_number: None,
                    column_number: None,
                    confidence: 0.9,
                    context: if config.include_context {
                        Some(format!(
                            "{} is essential for comprehensive documentation",
                            description
                        ))
                    } else {
                        None
                    },
                    diff: None,
                };
                suggestions.push(suggestion);
            }
        }

        // Check for proper index/navigation structure
        let index_path = docs_path.join("index.md");
        if !index_path.exists() {
            let suggestion = Suggestion {
                suggestion_type: SuggestionType::StructureImprovement,
                description: "Create documentation index file".to_string(),
                original_text: String::new(),
                suggested_text: self.generate_index_template(docs_path)?,
                file_path: index_path,
                line_number: None,
                column_number: None,
                confidence: 0.95,
                context: if config.include_context {
                    Some("An index file helps users navigate the documentation".to_string())
                } else {
                    None
                },
                diff: None,
            };
            suggestions.push(suggestion);
        }

        // Suggest organizing documentation by feature/crate
        let crate_organization_suggestions =
            self.suggest_crate_based_organization(docs_path, config)?;
        suggestions.extend(crate_organization_suggestions);

        Ok(suggestions)
    }

    /// Suggests organizing documentation by crate/feature.
    fn suggest_crate_based_organization(
        &self,
        docs_path: &Path,
        config: &SuggestionConfig,
    ) -> Result<Vec<Suggestion>> {
        let mut suggestions = Vec::new();

        // Check if crate-specific directories exist
        for (crate_name, crate_info) in &self.crate_registry {
            let crate_docs_dir = docs_path.join(crate_name);
            if !crate_docs_dir.exists() && !crate_info.public_apis.is_empty() {
                let suggestion = Suggestion {
                    suggestion_type: SuggestionType::StructureImprovement,
                    description: format!("Create documentation directory for {}", crate_name),
                    original_text: String::new(),
                    suggested_text: format!("Create directory: {}/", crate_docs_dir.display()),
                    file_path: crate_docs_dir.clone(),
                    line_number: None,
                    column_number: None,
                    confidence: 0.85,
                    context: if config.include_context {
                        Some("Organize documentation by crate for better structure".to_string())
                    } else {
                        None
                    },
                    diff: None,
                };
                suggestions.push(suggestion);

                // Suggest creating crate-specific files
                let crate_files = [
                    ("README.md", format!("{} Overview", crate_name)),
                    ("api.md", format!("{} API Reference", crate_name)),
                    ("examples.md", format!("{} Examples", crate_name)),
                ];

                for (filename, description) in &crate_files {
                    let file_path = crate_docs_dir.join(filename);
                    let suggestion = Suggestion {
                        suggestion_type: SuggestionType::StructureImprovement,
                        description: format!("Create {}", description),
                        original_text: String::new(),
                        suggested_text: self
                            .generate_crate_file_template(crate_name, filename, crate_info),
                        file_path,
                        line_number: None,
                        column_number: None,
                        confidence: 0.8,
                        context: if config.include_context {
                            Some(format!("Dedicated {} documentation", description))
                        } else {
                            None
                        },
                        diff: None,
                    };
                    suggestions.push(suggestion);
                }
            }
        }

        Ok(suggestions)
    }

    // Private helper methods for documentation placement

    /// Determines the best documentation file for an API.
    fn determine_documentation_file(&self, api: &PublicApi, docs_path: &Path) -> Result<PathBuf> {
        // Extract crate name from API path
        let crate_name = self.extract_crate_name_from_api(&api.path);

        // Check if crate-specific documentation exists
        let crate_docs_dir = docs_path.join(&crate_name);
        if crate_docs_dir.exists() {
            match api.item_type {
                ApiItemType::Trait => Ok(crate_docs_dir.join("traits.md")),
                ApiItemType::Struct => Ok(crate_docs_dir.join("structs.md")),
                ApiItemType::Function => Ok(crate_docs_dir.join("functions.md")),
                ApiItemType::Enum => Ok(crate_docs_dir.join("enums.md")),
                ApiItemType::Constant => Ok(crate_docs_dir.join("constants.md")),
                ApiItemType::Method => Ok(crate_docs_dir.join("methods.md")),
                ApiItemType::Module => Ok(crate_docs_dir.join("modules.md")),
                ApiItemType::TypeAlias => Ok(crate_docs_dir.join("types.md")),
                ApiItemType::Unknown => Ok(crate_docs_dir.join("misc.md")),
            }
        } else {
            // Fall back to general API reference
            Ok(docs_path.join("api-reference.md"))
        }
    }

    /// Determines the appropriate documentation section for an API.
    fn determine_documentation_section(&self, api: &PublicApi) -> String {
        match api.item_type {
            ApiItemType::Trait => "Traits".to_string(),
            ApiItemType::Struct => "Structs".to_string(),
            ApiItemType::Function => "Functions".to_string(),
            ApiItemType::Enum => "Enums".to_string(),
            ApiItemType::Constant => "Constants".to_string(),
            ApiItemType::Method => "Methods".to_string(),
            ApiItemType::Module => "Modules".to_string(),
            ApiItemType::TypeAlias => "Type Aliases".to_string(),
            ApiItemType::Unknown => "Miscellaneous".to_string(),
        }
    }

    /// Generates a documentation template for an API.
    fn generate_documentation_template(&self, api: &PublicApi) -> String {
        let section = self.determine_documentation_section(api);

        format!(
            r#"## {}

### `{}`

{}

#### Signature

```rust
{}
```

#### Description

[Add description here]

#### Examples

```rust
// Add example usage here
```

#### See Also

- [Related documentation]

"#,
            section,
            api.path,
            api.documentation.as_deref().unwrap_or("[Add documentation here]"),
            api.signature
        )
    }

    /// Generates a template for a documentation file.
    fn generate_file_template(&self, filename: &str) -> String {
        match filename {
            "getting-started.md" => r#"# Getting Started

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
adk-rust = "0.1.0"
```

## Quick Start

[Add quick start guide here]

## Next Steps

- [API Reference](api-reference.md)
- [Examples](examples.md)
"#
            .to_string(),
            "api-reference.md" => r#"# API Reference

## Overview

This document provides a comprehensive reference for all public APIs.

## Modules

[List modules here]

## Traits

[List traits here]

## Structs

[List structs here]

## Functions

[List functions here]
"#
            .to_string(),
            "examples.md" => r#"# Examples and Tutorials

## Basic Examples

### Hello World

```rust
// Add basic example here
```

## Advanced Examples

### Complex Usage

```rust
// Add advanced example here
```

## Tutorials

- [Tutorial 1](tutorials/tutorial-1.md)
- [Tutorial 2](tutorials/tutorial-2.md)
"#
            .to_string(),
            "migration-guide.md" => r#"# Migration Guide

## Migrating from Previous Versions

### Version 0.0.x to 0.1.x

[Add migration instructions here]

## Breaking Changes

[List breaking changes here]

## Deprecated APIs

[List deprecated APIs and their replacements here]
"#
            .to_string(),
            "troubleshooting.md" => r#"# Troubleshooting

## Common Issues

### Issue 1

**Problem:** [Describe problem]

**Solution:** [Describe solution]

### Issue 2

**Problem:** [Describe problem]

**Solution:** [Describe solution]

## Getting Help

- [GitHub Issues](https://github.com/zavora-ai/adk-rust/issues)
- [Discussions](https://github.com/zavora-ai/adk-rust/discussions)
"#
            .to_string(),
            "changelog.md" => r#"# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- [New features]

### Changed
- [Changes in existing functionality]

### Deprecated
- [Soon-to-be removed features]

### Removed
- [Removed features]

### Fixed
- [Bug fixes]

### Security
- [Security improvements]
"#
            .to_string(),
            _ => format!(
                "# {}\n\n[Add content here]\n",
                filename.replace(".md", "").replace("-", " ").to_uppercase()
            ),
        }
    }

    /// Generates an index template for the documentation.
    fn generate_index_template(&self, docs_path: &Path) -> Result<String> {
        let mut index_content = String::from(
            r#"# ADK-Rust Documentation

Welcome to the ADK-Rust documentation!

## Getting Started

- [Installation and Setup](getting-started.md)
- [Quick Start Guide](getting-started.md#quick-start)

## Core Documentation

- [API Reference](api-reference.md)
- [Examples and Tutorials](examples.md)

## Crates

"#,
        );

        // Add crate-specific documentation links
        for crate_name in self.crate_registry.keys() {
            let crate_docs_dir = docs_path.join(crate_name);
            if crate_docs_dir.exists() {
                index_content.push_str(&format!("- [{}]({})\n", crate_name, crate_name));
            } else {
                // Include crate even if directory doesn't exist yet
                index_content.push_str(&format!("- [{}]({}/README.md)\n", crate_name, crate_name));
            }
        }

        index_content.push_str(
            r#"
## Additional Resources

- [Migration Guide](migration-guide.md)
- [Troubleshooting](troubleshooting.md)
- [Changelog](changelog.md)

## Contributing

- [Contributing Guidelines](../CONTRIBUTING.md)
- [Development Setup](development.md)
"#,
        );

        Ok(index_content)
    }

    /// Generates a template for crate-specific documentation files.
    fn generate_crate_file_template(
        &self,
        crate_name: &str,
        filename: &str,
        crate_info: &CrateInfo,
    ) -> String {
        match filename {
            "README.md" => {
                format!(
                    r#"# {}

## Overview

[Add crate overview here]

## Installation

```toml
[dependencies]
{} = "{}"
```

## Features

{}

## Quick Start

```rust
use {}::*;

// Add quick start example here
```

## API Reference

- [Traits](traits.md)
- [Structs](structs.md)
- [Functions](functions.md)

## Examples

See [examples.md](examples.md) for detailed usage examples.
"#,
                    crate_name,
                    crate_name,
                    crate_info.version,
                    crate_info
                        .feature_flags
                        .iter()
                        .map(|f| format!("- `{}`", f))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    crate_name.replace("-", "_")
                )
            }
            "api.md" => {
                let mut api_content = format!("# {} API Reference\n\n", crate_name);

                // Group APIs by type
                let mut traits = Vec::new();
                let mut structs = Vec::new();
                let mut functions = Vec::new();
                let mut enums = Vec::new();

                for api in &crate_info.public_apis {
                    match api.item_type {
                        ApiItemType::Trait => traits.push(api),
                        ApiItemType::Struct => structs.push(api),
                        ApiItemType::Function => functions.push(api),
                        ApiItemType::Enum => enums.push(api),
                        _ => {}
                    }
                }

                if !traits.is_empty() {
                    api_content.push_str("## Traits\n\n");
                    for trait_api in traits {
                        api_content.push_str(&format!("### `{}`\n\n", trait_api.path));
                        api_content.push_str(&format!("```rust\n{}\n```\n\n", trait_api.signature));
                        if let Some(doc) = &trait_api.documentation {
                            api_content.push_str(&format!("{}\n\n", doc));
                        }
                    }
                }

                if !structs.is_empty() {
                    api_content.push_str("## Structs\n\n");
                    for struct_api in structs {
                        api_content.push_str(&format!("### `{}`\n\n", struct_api.path));
                        api_content
                            .push_str(&format!("```rust\n{}\n```\n\n", struct_api.signature));
                        if let Some(doc) = &struct_api.documentation {
                            api_content.push_str(&format!("{}\n\n", doc));
                        }
                    }
                }

                if !functions.is_empty() {
                    api_content.push_str("## Functions\n\n");
                    for func_api in functions {
                        api_content.push_str(&format!("### `{}`\n\n", func_api.path));
                        api_content.push_str(&format!("```rust\n{}\n```\n\n", func_api.signature));
                        if let Some(doc) = &func_api.documentation {
                            api_content.push_str(&format!("{}\n\n", doc));
                        }
                    }
                }

                api_content
            }
            "examples.md" => {
                format!(
                    r#"# {} Examples

## Basic Usage

```rust
use {}::*;

// Add basic example here
```

## Advanced Usage

```rust
use {}::*;

// Add advanced example here
```

## Integration Examples

```rust
// Add integration examples here
```
"#,
                    crate_name,
                    crate_name.replace("-", "_"),
                    crate_name.replace("-", "_")
                )
            }
            _ => {
                format!("# {} {}\n\n[Add content here]\n", crate_name, filename.replace(".md", ""))
            }
        }
    }

    /// Generates a simple diff between two strings.
    fn generate_simple_diff(&self, original: &str, suggested: &str) -> String {
        format!("-{}\n+{}", original, suggested)
    }

    /// Extracts crate name from API path.
    fn extract_crate_name_from_api(&self, api_path: &str) -> String {
        // Try to match against known crates first
        for crate_name in self.crate_registry.keys() {
            let normalized_crate = crate_name.replace("-", "_");
            if api_path.starts_with(&normalized_crate) {
                return crate_name.clone();
            }
        }

        // Fall back to extracting from path
        if let Some(first_part) = api_path.split("::").next() {
            first_part.replace("_", "-")
        } else {
            "unknown".to_string()
        }
    }
}

impl Default for SuggestionConfig {
    fn default() -> Self {
        Self {
            min_confidence: 0.7,
            max_suggestions_per_issue: 5,
            generate_diffs: true,
            include_context: true,
            enable_caching: true,
        }
    }
}

impl SuggestionEngine {
    /// Generate suggestions for a specific category of issues.
    pub async fn generate_suggestions_for_category(
        &self,
        category: crate::reporter::IssueCategory,
        issues: &[&crate::reporter::AuditIssue],
        _crate_registry: &crate::analyzer::CrateRegistry,
    ) -> Result<Vec<crate::reporter::Recommendation>> {
        use crate::reporter::{IssueCategory, Recommendation, RecommendationType};

        let mut recommendations = Vec::new();
        let _config = SuggestionConfig::default();

        match category {
            IssueCategory::ApiMismatch => {
                for issue in issues {
                    let recommendation = Recommendation {
                        id: format!("api-fix-{}", issue.file_path.display()),
                        recommendation_type: RecommendationType::FixIssue,
                        priority: 1, // High priority
                        title: "Fix API Reference".to_string(),
                        description: format!(
                            "Update API reference in {}",
                            issue.file_path.display()
                        ),
                        affected_files: vec![issue.file_path.clone()],
                        estimated_effort_hours: Some(0.5),
                        resolves_issues: vec![format!(
                            "api-mismatch-{}",
                            issue.line_number.unwrap_or(0)
                        )],
                    };
                    recommendations.push(recommendation);
                }
            }
            IssueCategory::VersionInconsistency => {
                let recommendation = Recommendation {
                    id: format!("version-update-{}", issues.len()),
                    recommendation_type: RecommendationType::UpdateContent,
                    priority: 2, // Medium priority
                    title: "Update Version References".to_string(),
                    description: format!(
                        "Update {} version references to current workspace version",
                        issues.len()
                    ),
                    affected_files: issues.iter().map(|i| i.file_path.clone()).collect(),
                    estimated_effort_hours: Some(0.25 * issues.len() as f32),
                    resolves_issues: issues
                        .iter()
                        .enumerate()
                        .map(|(i, _)| format!("version-{}", i))
                        .collect(),
                };
                recommendations.push(recommendation);
            }
            IssueCategory::CompilationError => {
                let recommendation = Recommendation {
                    id: format!("compilation-fix-{}", issues.len()),
                    recommendation_type: RecommendationType::ImproveExamples,
                    priority: 1, // High priority
                    title: "Fix Compilation Errors".to_string(),
                    description: format!(
                        "Fix {} compilation errors in code examples",
                        issues.len()
                    ),
                    affected_files: issues.iter().map(|i| i.file_path.clone()).collect(),
                    estimated_effort_hours: Some(1.0 * issues.len() as f32),
                    resolves_issues: issues
                        .iter()
                        .enumerate()
                        .map(|(i, _)| format!("compile-{}", i))
                        .collect(),
                };
                recommendations.push(recommendation);
            }
            IssueCategory::BrokenLink => {
                let recommendation = Recommendation {
                    id: format!("link-fix-{}", issues.len()),
                    recommendation_type: RecommendationType::FixIssue,
                    priority: 2, // Medium priority
                    title: "Fix Broken Links".to_string(),
                    description: format!("Fix {} broken internal links", issues.len()),
                    affected_files: issues.iter().map(|i| i.file_path.clone()).collect(),
                    estimated_effort_hours: Some(0.1 * issues.len() as f32),
                    resolves_issues: issues
                        .iter()
                        .enumerate()
                        .map(|(i, _)| format!("link-{}", i))
                        .collect(),
                };
                recommendations.push(recommendation);
            }
            IssueCategory::MissingDocumentation => {
                let recommendation = Recommendation {
                    id: format!("doc-addition-{}", issues.len()),
                    recommendation_type: RecommendationType::AddDocumentation,
                    priority: 3, // Low priority
                    title: "Add Missing Documentation".to_string(),
                    description: format!("Document {} undocumented features", issues.len()),
                    affected_files: issues.iter().map(|i| i.file_path.clone()).collect(),
                    estimated_effort_hours: Some(2.0 * issues.len() as f32),
                    resolves_issues: issues
                        .iter()
                        .enumerate()
                        .map(|(i, _)| format!("missing-doc-{}", i))
                        .collect(),
                };
                recommendations.push(recommendation);
            }
            IssueCategory::DeprecatedApi => {
                let recommendation = Recommendation {
                    id: format!("deprecated-fix-{}", issues.len()),
                    recommendation_type: RecommendationType::UpdateContent,
                    priority: 2, // Medium priority
                    title: "Update Deprecated API References".to_string(),
                    description: format!("Update {} deprecated API references", issues.len()),
                    affected_files: issues.iter().map(|i| i.file_path.clone()).collect(),
                    estimated_effort_hours: Some(0.5 * issues.len() as f32),
                    resolves_issues: issues
                        .iter()
                        .enumerate()
                        .map(|(i, _)| format!("deprecated-{}", i))
                        .collect(),
                };
                recommendations.push(recommendation);
            }
            IssueCategory::ProcessingError => {
                let recommendation = Recommendation {
                    id: format!("processing-fix-{}", issues.len()),
                    recommendation_type: RecommendationType::FixIssue,
                    priority: 1, // High priority
                    title: "Fix Processing Errors".to_string(),
                    description: format!("Resolve {} file processing errors", issues.len()),
                    affected_files: issues.iter().map(|i| i.file_path.clone()).collect(),
                    estimated_effort_hours: Some(1.0 * issues.len() as f32),
                    resolves_issues: issues
                        .iter()
                        .enumerate()
                        .map(|(i, _)| format!("processing-{}", i))
                        .collect(),
                };
                recommendations.push(recommendation);
            }
            _ => {
                // Generic recommendation for other categories
                let recommendation = Recommendation {
                    id: format!("generic-fix-{}", issues.len()),
                    recommendation_type: RecommendationType::FixIssue,
                    priority: 2, // Medium priority
                    title: format!("Address {} Issues", category.description()),
                    description: format!(
                        "Review and fix {} issues of type: {}",
                        issues.len(),
                        category.description()
                    ),
                    affected_files: issues.iter().map(|i| i.file_path.clone()).collect(),
                    estimated_effort_hours: Some(1.0 * issues.len() as f32),
                    resolves_issues: issues
                        .iter()
                        .enumerate()
                        .map(|(i, _)| format!("generic-{}", i))
                        .collect(),
                };
                recommendations.push(recommendation);
            }
        }

        Ok(recommendations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dependency, PublicApi};

    fn create_test_crate_info() -> CrateInfo {
        CrateInfo {
            name: "adk-core".to_string(),
            version: "0.1.0".to_string(),
            path: PathBuf::from("/tmp/adk-core"),
            public_apis: vec![
                PublicApi {
                    path: "Agent".to_string(),
                    signature: "pub trait Agent".to_string(),
                    item_type: ApiItemType::Trait,
                    documentation: Some("Core agent trait".to_string()),
                    deprecated: false,
                    source_file: PathBuf::from("src/lib.rs"),
                    line_number: 10,
                },
                PublicApi {
                    path: "LlmAgent".to_string(),
                    signature: "pub struct LlmAgent".to_string(),
                    item_type: ApiItemType::Struct,
                    documentation: Some("LLM-based agent".to_string()),
                    deprecated: false,
                    source_file: PathBuf::from("src/lib.rs"),
                    line_number: 20,
                },
                PublicApi {
                    path: "OldAgent".to_string(),
                    signature: "pub struct OldAgent".to_string(),
                    item_type: ApiItemType::Struct,
                    documentation: Some("Deprecated agent".to_string()),
                    deprecated: true,
                    source_file: PathBuf::from("src/lib.rs"),
                    line_number: 30,
                },
            ],
            feature_flags: vec!["default".to_string()],
            dependencies: vec![Dependency {
                name: "tokio".to_string(),
                version: "1.0".to_string(),
                features: vec!["full".to_string()],
                optional: false,
            }],
            rust_version: Some("1.85.0".to_string()),
        }
    }

    fn create_test_engine() -> SuggestionEngine {
        let mut registry = HashMap::new();
        registry.insert("adk-core".to_string(), create_test_crate_info());

        SuggestionEngine::new(registry, "0.1.0".to_string())
    }

    #[test]
    fn test_suggestion_engine_creation() {
        let engine = create_test_engine();
        assert_eq!(engine.workspace_version, "0.1.0");
        assert!(engine.crate_registry.contains_key("adk-core"));
    }

    #[test]
    fn test_api_signature_correction() {
        let engine = create_test_engine();
        let config = SuggestionConfig::default();

        let api_ref = ApiReference {
            crate_name: "adk-core".to_string(),
            item_path: "Agent".to_string(),
            item_type: ApiItemType::Trait,
            line_number: 10,
            context: "use adk_core::Agent;".to_string(),
        };

        let suggestions = engine
            .suggest_api_signature_corrections(&api_ref, Path::new("test.md"), &config)
            .unwrap();

        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0].suggestion_type, SuggestionType::ApiSignatureCorrection);
        assert_eq!(suggestions[0].confidence, 1.0); // Exact match
    }

    #[test]
    fn test_version_correction() {
        let engine = create_test_engine();
        let config = SuggestionConfig::default();

        let version_ref = VersionReference {
            version: "0.0.1".to_string(), // Outdated version
            version_type: VersionType::CrateVersion,
            line_number: 5,
            context: "adk-core = \"0.0.1\"".to_string(),
        };

        let suggestions = engine
            .suggest_version_corrections(
                &version_ref,
                "adk-core", // Pass crate name separately
                Path::new("test.md"),
                &config,
            )
            .unwrap();

        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0].suggestion_type, SuggestionType::VersionUpdate);
        assert_eq!(suggestions[0].suggested_text, "0.1.0");
    }

    #[test]
    fn test_compilation_fix_suggestions() {
        let engine = create_test_engine();
        let config = SuggestionConfig::default();

        let errors = vec![CompilationError {
            message: "cannot find adk_core in scope".to_string(),
            line: Some(1),
            column: Some(5),
            error_type: ErrorType::UnresolvedImport,
            suggestion: None,
            code_snippet: None,
        }];

        let suggestions =
            engine.suggest_compilation_fixes(&errors, Path::new("test.rs"), &config).unwrap();

        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0].suggestion_type, SuggestionType::ImportFix);
        assert!(suggestions[0].suggested_text.contains("use adk_core"));
    }

    #[test]
    fn test_similarity_calculation() {
        let engine = create_test_engine();

        assert_eq!(engine.calculate_similarity("Agent", "Agent"), 1.0);
        assert!(engine.calculate_similarity("Agent", "Agnt") > 0.6);
        assert!(engine.calculate_similarity("Agent", "LlmAgent") > 0.4);
        assert!(engine.calculate_similarity("Agent", "CompletelyDifferent") < 0.3);
    }

    #[test]
    fn test_levenshtein_distance() {
        let engine = create_test_engine();

        assert_eq!(engine.levenshtein_distance("", ""), 0);
        assert_eq!(engine.levenshtein_distance("abc", "abc"), 0);
        assert_eq!(engine.levenshtein_distance("abc", "ab"), 1);
        assert_eq!(engine.levenshtein_distance("abc", "def"), 3);
    }

    #[test]
    fn test_import_fix_suggestions() {
        let engine = create_test_engine();

        assert_eq!(
            engine.suggest_import_fix("cannot find adk_core"),
            Some("use adk_core::*;".to_string())
        );
        assert_eq!(engine.suggest_import_fix("cannot find tokio"), Some("use tokio;".to_string()));
        assert_eq!(engine.suggest_import_fix("cannot find unknown_crate"), None);
    }

    #[test]
    fn test_dependency_addition_suggestions() {
        let engine = create_test_engine();

        assert!(
            engine.suggest_dependency_addition("missing adk_core").unwrap().contains("adk-core")
        );
        assert!(engine.suggest_dependency_addition("missing tokio").unwrap().contains("tokio"));
        assert_eq!(engine.suggest_dependency_addition("missing unknown"), None);
    }

    #[test]
    fn test_async_pattern_fix_suggestions() {
        let engine = create_test_engine();

        let suggestions = engine.suggest_async_pattern_fixes("async fn main not supported");
        assert!(suggestions.iter().any(|s| s.contains("tokio::main")));

        let suggestions = engine.suggest_async_pattern_fixes("missing await");
        assert!(suggestions.iter().any(|s| s.contains("await")));
    }

    #[test]
    fn test_diff_generation() {
        let engine = create_test_engine();

        let diff = engine.generate_simple_diff("old_text", "new_text");
        assert!(diff.contains("-old_text"));
        assert!(diff.contains("+new_text"));
    }

    #[test]
    fn test_suggestion_config_defaults() {
        let config = SuggestionConfig::default();

        assert_eq!(config.min_confidence, 0.7);
        assert_eq!(config.max_suggestions_per_issue, 5);
        assert!(config.generate_diffs);
        assert!(config.include_context);
        assert!(config.enable_caching);
    }

    #[test]
    fn test_deprecated_api_detection() {
        let engine = create_test_engine();
        let config = SuggestionConfig::default();

        let api_ref = ApiReference {
            crate_name: "adk-core".to_string(),
            item_path: "OldAgent".to_string(),
            item_type: ApiItemType::Struct,
            line_number: 15,
            context: "use adk_core::OldAgent;".to_string(),
        };

        let suggestions = engine
            .suggest_api_signature_corrections(&api_ref, Path::new("test.md"), &config)
            .unwrap();

        // Should find exact match first (even if deprecated), then suggest replacement
        assert!(!suggestions.is_empty());
        // The deprecated API should be found as an exact match, and then a replacement should be suggested
        let has_deprecated_replacement = suggestions
            .iter()
            .any(|s| s.suggestion_type == SuggestionType::DeprecatedApiReplacement);
        let has_exact_match =
            suggestions.iter().any(|s| s.suggestion_type == SuggestionType::ApiSignatureCorrection);

        // Should have either an exact match or a deprecated replacement
        assert!(has_deprecated_replacement || has_exact_match);
    }

    #[test]
    fn test_fuzzy_matching() {
        let engine = create_test_engine();
        let config = SuggestionConfig::default();

        let api_ref = ApiReference {
            crate_name: "adk-core".to_string(),
            item_path: "Agnt".to_string(), // Typo in "Agent"
            item_type: ApiItemType::Trait,
            line_number: 20,
            context: "use adk_core::Agnt;".to_string(),
        };

        let suggestions = engine
            .suggest_api_signature_corrections(&api_ref, Path::new("test.md"), &config)
            .unwrap();

        assert!(!suggestions.is_empty());
        assert!(suggestions[0].suggested_text.contains("Agent"));
        assert!(suggestions[0].confidence > 0.7);
    }

    #[test]
    fn test_documentation_placement_suggestions() {
        let engine = create_test_engine();
        let config = SuggestionConfig::default();

        let undocumented_apis = vec![PublicApi {
            path: "NewAgent".to_string(),
            signature: "pub struct NewAgent".to_string(),
            item_type: ApiItemType::Struct,
            documentation: None,
            deprecated: false,
            source_file: PathBuf::from("src/lib.rs"),
            line_number: 40,
        }];

        let workspace_path = Path::new("/tmp/workspace");
        let docs_path = Path::new("/tmp/docs");

        let suggestions = engine
            .suggest_documentation_placement(&undocumented_apis, workspace_path, docs_path, &config)
            .unwrap();

        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0].suggestion_type, SuggestionType::StructureImprovement);
        assert!(suggestions[0].description.contains("NewAgent"));
    }

    #[test]
    fn test_structure_improvement_suggestions() {
        let engine = create_test_engine();
        let config = SuggestionConfig::default();

        let docs_path = Path::new("/tmp/nonexistent_docs");

        let suggestions = engine.suggest_structure_improvements(docs_path, &config).unwrap();

        assert!(!suggestions.is_empty());
        // Should suggest creating essential files
        assert!(suggestions.iter().any(|s| s.description.contains("Getting Started")));
        assert!(suggestions.iter().any(|s| s.description.contains("API Reference")));
    }

    #[test]
    fn test_crate_name_extraction() {
        let engine = create_test_engine();

        assert_eq!(engine.extract_crate_name_from_api("adk_core::Agent"), "adk-core");
        assert_eq!(engine.extract_crate_name_from_api("Agent"), "Agent"); // No conversion for single names
        assert_eq!(engine.extract_crate_name_from_api("some_module::SomeStruct"), "some-module");
    }

    #[test]
    fn test_documentation_section_determination() {
        let engine = create_test_engine();

        let trait_api = PublicApi {
            path: "TestTrait".to_string(),
            signature: "pub trait TestTrait".to_string(),
            item_type: ApiItemType::Trait,
            documentation: None,
            deprecated: false,
            source_file: PathBuf::from("src/lib.rs"),
            line_number: 50,
        };

        assert_eq!(engine.determine_documentation_section(&trait_api), "Traits");

        let struct_api = PublicApi {
            path: "TestStruct".to_string(),
            signature: "pub struct TestStruct".to_string(),
            item_type: ApiItemType::Struct,
            documentation: None,
            deprecated: false,
            source_file: PathBuf::from("src/lib.rs"),
            line_number: 60,
        };

        assert_eq!(engine.determine_documentation_section(&struct_api), "Structs");
    }

    #[test]
    fn test_documentation_template_generation() {
        let engine = create_test_engine();

        let api = PublicApi {
            path: "TestStruct".to_string(),
            signature: "pub struct TestStruct { field: String }".to_string(),
            item_type: ApiItemType::Struct,
            documentation: Some("A test structure".to_string()),
            deprecated: false,
            source_file: PathBuf::from("src/lib.rs"),
            line_number: 70,
        };

        let template = engine.generate_documentation_template(&api);

        assert!(template.contains("## Structs"));
        assert!(template.contains("### `TestStruct`"));
        assert!(template.contains("A test structure"));
        assert!(template.contains("pub struct TestStruct"));
    }

    #[test]
    fn test_file_template_generation() {
        let engine = create_test_engine();

        let getting_started = engine.generate_file_template("getting-started.md");
        assert!(getting_started.contains("# Getting Started"));
        assert!(getting_started.contains("## Installation"));

        let api_ref = engine.generate_file_template("api-reference.md");
        assert!(api_ref.contains("# API Reference"));
        assert!(api_ref.contains("## Traits"));

        let examples = engine.generate_file_template("examples.md");
        assert!(examples.contains("# Examples and Tutorials"));
        assert!(examples.contains("## Basic Examples"));
    }

    #[test]
    fn test_crate_file_template_generation() {
        let engine = create_test_engine();
        let crate_info = create_test_crate_info();

        let readme = engine.generate_crate_file_template("adk-core", "README.md", &crate_info);
        assert!(readme.contains("# adk-core"));
        assert!(readme.contains("## Installation"));
        assert!(readme.contains("adk-core = \"0.1.0\""));

        let api_doc = engine.generate_crate_file_template("adk-core", "api.md", &crate_info);
        assert!(api_doc.contains("# adk-core API Reference"));
        assert!(api_doc.contains("## Traits"));
        assert!(api_doc.contains("### `Agent`"));
    }

    #[test]
    fn test_index_template_generation() {
        let engine = create_test_engine();
        let docs_path = Path::new("/tmp/docs");

        let index = engine.generate_index_template(docs_path).unwrap();

        assert!(index.contains("# ADK-Rust Documentation"));
        assert!(index.contains("## Getting Started"));
        assert!(index.contains("## Crates"));
        // The crate names should be listed even if directories don't exist
        assert!(index.contains("adk-core") || index.contains("- [adk-core]"));
    }
}
