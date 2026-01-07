//! Version consistency validation for documentation audit.
//!
//! This module provides functionality to validate version references in documentation
//! against actual workspace versions, ensuring consistency across all documentation files.

use crate::{AuditError, FeatureMention, Result, VersionReference, VersionType};
use regex::Regex;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use toml::Value;

/// Version validator that checks consistency between documentation and workspace.
#[derive(Debug)]
pub struct VersionValidator {
    /// Current workspace version information
    workspace_info: WorkspaceVersionInfo,
    /// Compiled regex patterns for version parsing
    patterns: VersionPatterns,
}

/// Comprehensive workspace version information.
#[derive(Debug, Clone)]
pub struct WorkspaceVersionInfo {
    /// Main workspace version
    pub workspace_version: String,
    /// Required Rust version
    pub rust_version: String,
    /// Individual crate versions in the workspace
    pub crate_versions: HashMap<String, String>,
    /// Dependency versions used across the workspace
    pub dependency_versions: HashMap<String, String>,
    /// Feature flags defined in workspace crates
    pub workspace_features: HashMap<String, Vec<String>>,
}

/// Compiled regex patterns for version validation.
#[derive(Debug)]
struct VersionPatterns {
    /// Pattern for semantic version strings
    semver: Regex,
    /// Pattern for version requirements (e.g., "^1.0", ">=0.5")
    #[allow(dead_code)]
    version_req: Regex,
    /// Pattern for git version references
    #[allow(dead_code)]
    git_version: Regex,
    /// Pattern for path dependencies
    #[allow(dead_code)]
    path_dependency: Regex,
}

/// Result of version validation for a single reference.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionValidationResult {
    /// Whether the version reference is valid
    pub is_valid: bool,
    /// Expected version if different from found version
    pub expected_version: Option<String>,
    /// Detailed validation message
    pub message: String,
    /// Severity of the validation issue
    pub severity: ValidationSeverity,
    /// Suggested fix for the issue
    pub suggestion: Option<String>,
}

/// Severity levels for version validation issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationSeverity {
    /// Critical issue that must be fixed
    Critical,
    /// Warning that should be addressed
    Warning,
    /// Informational notice
    Info,
}

/// Configuration for version validation behavior.
#[derive(Debug, Clone)]
pub struct VersionValidationConfig {
    /// Whether to allow pre-release versions
    pub allow_prerelease: bool,
    /// Whether to enforce exact version matches
    pub strict_matching: bool,
    /// Whether to validate git dependencies
    pub validate_git_deps: bool,
    /// Tolerance for version differences (e.g., patch versions)
    pub version_tolerance: VersionTolerance,
}

/// Tolerance levels for version differences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionTolerance {
    /// Must match exactly
    Exact,
    /// Allow patch version differences
    Patch,
    /// Allow minor version differences
    Minor,
    /// Allow major version differences (not recommended)
    Major,
}

/// Cargo.toml dependency specification.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum DependencySpec {
    /// Simple version string
    Simple(String),
    /// Detailed dependency specification
    Detailed {
        version: Option<String>,
        git: Option<String>,
        branch: Option<String>,
        tag: Option<String>,
        rev: Option<String>,
        path: Option<String>,
        features: Option<Vec<String>>,
        #[serde(rename = "default-features")]
        default_features: Option<bool>,
        optional: Option<bool>,
    },
}

impl VersionValidator {
    /// Creates a new version validator for the given workspace.
    ///
    /// # Arguments
    ///
    /// * `workspace_path` - Path to the workspace root directory
    ///
    /// # Returns
    ///
    /// A new `VersionValidator` instance or an error if workspace analysis fails.
    pub async fn new(workspace_path: &Path) -> Result<Self> {
        let workspace_info = Self::analyze_workspace(workspace_path).await?;
        let patterns = VersionPatterns::new()?;

        Ok(Self { workspace_info, patterns })
    }

    /// Creates a version validator with custom workspace information.
    ///
    /// This is useful for testing or when workspace information is already available.
    pub fn with_workspace_info(workspace_info: WorkspaceVersionInfo) -> Result<Self> {
        let patterns = VersionPatterns::new()?;

        Ok(Self { workspace_info, patterns })
    }

    /// Validates a version reference against workspace information.
    ///
    /// # Arguments
    ///
    /// * `version_ref` - The version reference to validate
    /// * `config` - Validation configuration options
    ///
    /// # Returns
    ///
    /// A `VersionValidationResult` indicating whether the reference is valid.
    pub fn validate_version_reference(
        &self,
        version_ref: &VersionReference,
        config: &VersionValidationConfig,
    ) -> Result<VersionValidationResult> {
        match version_ref.version_type {
            VersionType::RustVersion => self.validate_rust_version(version_ref, config),
            VersionType::WorkspaceVersion => self.validate_workspace_version(version_ref, config),
            VersionType::CrateVersion => self.validate_crate_version(version_ref, config),
            VersionType::Generic => self.validate_generic_version(version_ref, config),
        }
    }

    /// Validates multiple version references in batch.
    ///
    /// # Arguments
    ///
    /// * `version_refs` - Collection of version references to validate
    /// * `config` - Validation configuration options
    ///
    /// # Returns
    ///
    /// A vector of validation results corresponding to each input reference.
    pub fn validate_version_references(
        &self,
        version_refs: &[VersionReference],
        config: &VersionValidationConfig,
    ) -> Result<Vec<VersionValidationResult>> {
        version_refs
            .iter()
            .map(|version_ref| self.validate_version_reference(version_ref, config))
            .collect()
    }

    /// Validates dependency version compatibility across the workspace.
    ///
    /// This method checks that all dependency versions used in documentation
    /// are compatible with the versions actually used in the workspace.
    pub fn validate_dependency_compatibility(
        &self,
        dependency_name: &str,
        documented_version: &str,
        config: &VersionValidationConfig,
    ) -> Result<VersionValidationResult> {
        if let Some(workspace_version) =
            self.workspace_info.dependency_versions.get(dependency_name)
        {
            self.compare_versions(
                documented_version,
                workspace_version,
                &format!("dependency '{}'", dependency_name),
                config,
            )
        } else {
            Ok(VersionValidationResult {
                is_valid: false,
                expected_version: None,
                message: format!("Dependency '{}' not found in workspace", dependency_name),
                severity: ValidationSeverity::Warning,
                suggestion: Some(format!(
                    "Remove reference to '{}' or add it to workspace dependencies",
                    dependency_name
                )),
            })
        }
    }

    /// Checks if a version string represents a compatible version.
    ///
    /// This method uses semantic versioning rules to determine compatibility.
    pub fn is_version_compatible(
        &self,
        version1: &str,
        version2: &str,
        tolerance: &VersionTolerance,
    ) -> Result<bool> {
        let v1 = Version::parse(version1).map_err(|e| AuditError::ConfigurationError {
            message: format!("Invalid version '{}': {}", version1, e),
        })?;

        let v2 = Version::parse(version2).map_err(|e| AuditError::ConfigurationError {
            message: format!("Invalid version '{}': {}", version2, e),
        })?;

        let compatible = match tolerance {
            VersionTolerance::Exact => v1 == v2,
            VersionTolerance::Patch => v1.major == v2.major && v1.minor == v2.minor,
            VersionTolerance::Minor => v1.major == v2.major,
            VersionTolerance::Major => true, // Always compatible with major tolerance
        };

        Ok(compatible)
    }

    /// Validates that mentioned crate names exist in the workspace.
    ///
    /// # Arguments
    ///
    /// * `crate_name` - Name of the crate to validate
    ///
    /// # Returns
    ///
    /// A `VersionValidationResult` indicating whether the crate exists.
    pub fn validate_crate_exists(&self, crate_name: &str) -> VersionValidationResult {
        if self.workspace_info.crate_versions.contains_key(crate_name) {
            VersionValidationResult {
                is_valid: true,
                expected_version: None,
                message: format!("Crate '{}' exists in workspace", crate_name),
                severity: ValidationSeverity::Info,
                suggestion: None,
            }
        } else {
            let suggestion = self.suggest_similar_crate_name(crate_name);
            VersionValidationResult {
                is_valid: false,
                expected_version: None,
                message: format!("Crate '{}' not found in workspace", crate_name),
                severity: ValidationSeverity::Warning,
                suggestion,
            }
        }
    }

    /// Validates that feature flags exist in the specified crate.
    ///
    /// # Arguments
    ///
    /// * `crate_name` - Name of the crate that should define the feature
    /// * `feature_name` - Name of the feature flag to validate
    ///
    /// # Returns
    ///
    /// A `VersionValidationResult` indicating whether the feature exists.
    pub fn validate_feature_flag(
        &self,
        crate_name: &str,
        feature_name: &str,
    ) -> VersionValidationResult {
        // First check if the crate exists
        if !self.workspace_info.crate_versions.contains_key(crate_name) {
            return VersionValidationResult {
                is_valid: false,
                expected_version: None,
                message: format!(
                    "Cannot validate feature '{}': crate '{}' not found in workspace",
                    feature_name, crate_name
                ),
                severity: ValidationSeverity::Warning,
                suggestion: self.suggest_similar_crate_name(crate_name),
            };
        }

        // Check if the feature exists in the crate
        if let Some(features) = self.workspace_info.workspace_features.get(crate_name) {
            if features.contains(&feature_name.to_string()) {
                VersionValidationResult {
                    is_valid: true,
                    expected_version: None,
                    message: format!("Feature '{}' exists in crate '{}'", feature_name, crate_name),
                    severity: ValidationSeverity::Info,
                    suggestion: None,
                }
            } else {
                let suggestion = self.suggest_similar_feature_name(crate_name, feature_name);
                VersionValidationResult {
                    is_valid: false,
                    expected_version: None,
                    message: format!(
                        "Feature '{}' not found in crate '{}'",
                        feature_name, crate_name
                    ),
                    severity: ValidationSeverity::Warning,
                    suggestion,
                }
            }
        } else {
            VersionValidationResult {
                is_valid: false,
                expected_version: None,
                message: format!("Crate '{}' has no features defined", crate_name),
                severity: ValidationSeverity::Info,
                suggestion: Some(format!("Check if '{}' is the correct crate name", crate_name)),
            }
        }
    }

    /// Validates multiple crate names in batch.
    ///
    /// # Arguments
    ///
    /// * `crate_names` - Collection of crate names to validate
    ///
    /// # Returns
    ///
    /// A vector of validation results corresponding to each input crate name.
    pub fn validate_crate_names(&self, crate_names: &[String]) -> Vec<VersionValidationResult> {
        crate_names.iter().map(|crate_name| self.validate_crate_exists(crate_name)).collect()
    }

    /// Validates feature flag references from documentation.
    ///
    /// This method processes feature mentions extracted from documentation
    /// and validates them against the workspace feature definitions.
    pub fn validate_feature_mentions(
        &self,
        feature_mentions: &[FeatureMention],
    ) -> Vec<VersionValidationResult> {
        feature_mentions
            .iter()
            .map(|mention| {
                if let Some(crate_name) = &mention.crate_name {
                    self.validate_feature_flag(crate_name, &mention.feature_name)
                } else {
                    // Try to infer crate name from context or check all crates
                    self.validate_feature_in_any_crate(&mention.feature_name)
                }
            })
            .collect()
    }

    /// Validates that a feature exists in any workspace crate.
    ///
    /// This is used when the crate name cannot be determined from context.
    pub fn validate_feature_in_any_crate(&self, feature_name: &str) -> VersionValidationResult {
        for (crate_name, features) in &self.workspace_info.workspace_features {
            if features.contains(&feature_name.to_string()) {
                return VersionValidationResult {
                    is_valid: true,
                    expected_version: None,
                    message: format!("Feature '{}' found in crate '{}'", feature_name, crate_name),
                    severity: ValidationSeverity::Info,
                    suggestion: None,
                };
            }
        }

        VersionValidationResult {
            is_valid: false,
            expected_version: None,
            message: format!("Feature '{}' not found in any workspace crate", feature_name),
            severity: ValidationSeverity::Warning,
            suggestion: self.suggest_similar_feature_in_workspace(feature_name),
        }
    }

    /// Gets all crate names in the workspace.
    pub fn get_workspace_crates(&self) -> Vec<String> {
        self.workspace_info.crate_versions.keys().cloned().collect()
    }

    /// Gets all features defined in a specific crate.
    pub fn get_crate_features(&self, crate_name: &str) -> Option<Vec<String>> {
        self.workspace_info.workspace_features.get(crate_name).cloned()
    }

    /// Gets all features defined across the workspace.
    pub fn get_all_workspace_features(&self) -> HashMap<String, Vec<String>> {
        self.workspace_info.workspace_features.clone()
    }

    /// Suggests the correct version for an invalid reference.
    ///
    /// This method provides intelligent suggestions based on the type of version
    /// reference and available workspace information.
    pub fn suggest_correct_version(&self, version_ref: &VersionReference) -> Option<String> {
        match version_ref.version_type {
            VersionType::RustVersion => Some(self.workspace_info.rust_version.clone()),
            VersionType::WorkspaceVersion => Some(self.workspace_info.workspace_version.clone()),
            VersionType::CrateVersion => {
                // Try to extract crate name from context
                if let Some(crate_name) = self.extract_crate_name_from_context(&version_ref.context)
                {
                    self.workspace_info.crate_versions.get(&crate_name).cloned()
                } else {
                    None
                }
            }
            VersionType::Generic => {
                // Try to match against known dependencies
                self.find_best_version_match(&version_ref.version)
            }
        }
    }

    /// Suggests a similar crate name when a crate is not found.
    fn suggest_similar_crate_name(&self, target: &str) -> Option<String> {
        let crate_names: Vec<&String> = self.workspace_info.crate_versions.keys().collect();

        // Look for exact substring matches first
        for crate_name in &crate_names {
            if crate_name.contains(target) || target.contains(crate_name.as_str()) {
                return Some(format!("Did you mean '{}'?", crate_name));
            }
        }

        // Look for similar names using edit distance
        let mut best_match = None;
        let mut best_distance = usize::MAX;

        for crate_name in &crate_names {
            let distance = self.edit_distance(target, crate_name);
            if distance < best_distance && distance <= 3 {
                best_distance = distance;
                best_match = Some(crate_name);
            }
        }

        if let Some(match_name) = best_match {
            Some(format!("Did you mean '{}'?", match_name))
        } else if !crate_names.is_empty() {
            Some(format!(
                "Available crates: {}",
                crate_names.iter().take(5).map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ))
        } else {
            None
        }
    }

    /// Suggests a similar feature name when a feature is not found.
    fn suggest_similar_feature_name(&self, crate_name: &str, target: &str) -> Option<String> {
        if let Some(features) = self.workspace_info.workspace_features.get(crate_name) {
            // Look for exact substring matches first
            for feature in features {
                if feature.contains(target) || target.contains(feature.as_str()) {
                    return Some(format!("Did you mean '{}'?", feature));
                }
            }

            // Look for similar names using edit distance
            let mut best_match = None;
            let mut best_distance = usize::MAX;

            for feature in features {
                let distance = self.edit_distance(target, feature);
                if distance < best_distance && distance <= 2 {
                    best_distance = distance;
                    best_match = Some(feature);
                }
            }

            if let Some(match_name) = best_match {
                Some(format!("Did you mean '{}'?", match_name))
            } else if !features.is_empty() {
                Some(format!(
                    "Available features in '{}': {}",
                    crate_name,
                    features.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
                ))
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Suggests a similar feature name across all workspace crates.
    fn suggest_similar_feature_in_workspace(&self, target: &str) -> Option<String> {
        let mut all_features = Vec::new();

        for (crate_name, features) in &self.workspace_info.workspace_features {
            for feature in features {
                all_features.push((crate_name, feature));
            }
        }

        // Look for exact substring matches first
        for (crate_name, feature) in &all_features {
            if feature.contains(target) || target.contains(feature.as_str()) {
                return Some(format!("Did you mean '{}' in crate '{}'?", feature, crate_name));
            }
        }

        // Look for similar names using edit distance
        let mut best_match = None;
        let mut best_distance = usize::MAX;

        for (crate_name, feature) in &all_features {
            let distance = self.edit_distance(target, feature);
            if distance < best_distance && distance <= 2 {
                best_distance = distance;
                best_match = Some((crate_name, feature));
            }
        }

        if let Some((crate_name, feature)) = best_match {
            Some(format!("Did you mean '{}' in crate '{}'?", feature, crate_name))
        } else if !all_features.is_empty() {
            let sample_features: Vec<String> = all_features
                .iter()
                .take(3)
                .map(|(crate_name, feature)| format!("'{}' in '{}'", feature, crate_name))
                .collect();
            Some(format!("Available features: {}", sample_features.join(", ")))
        } else {
            None
        }
    }

    /// Calculates edit distance between two strings for similarity matching.
    fn edit_distance(&self, s1: &str, s2: &str) -> usize {
        let len1 = s1.len();
        let len2 = s2.len();

        if len1 == 0 {
            return len2;
        }
        if len2 == 0 {
            return len1;
        }

        let mut matrix = vec![vec![0; len2 + 1]; len1 + 1];

        // Initialize first row and column
        for (i, row) in matrix.iter_mut().enumerate().take(len1 + 1) {
            row[0] = i;
        }
        #[allow(clippy::needless_range_loop)]
        for j in 0..=len2 {
            matrix[0][j] = j;
        }

        let s1_chars: Vec<char> = s1.chars().collect();
        let s2_chars: Vec<char> = s2.chars().collect();

        for i in 1..=len1 {
            for j in 1..=len2 {
                let cost = if s1_chars[i - 1] == s2_chars[j - 1] { 0 } else { 1 };

                matrix[i][j] = std::cmp::min(
                    std::cmp::min(
                        matrix[i - 1][j] + 1, // deletion
                        matrix[i][j - 1] + 1, // insertion
                    ),
                    matrix[i - 1][j - 1] + cost, // substitution
                );
            }
        }

        matrix[len1][len2]
    }

    /// Analyzes the workspace to extract version information.
    async fn analyze_workspace(workspace_path: &Path) -> Result<WorkspaceVersionInfo> {
        let workspace_toml_path = workspace_path.join("Cargo.toml");
        let workspace_content =
            tokio::fs::read_to_string(&workspace_toml_path).await.map_err(|e| {
                AuditError::IoError { path: workspace_toml_path.clone(), details: e.to_string() }
            })?;

        let workspace_toml: Value = toml::from_str(&workspace_content).map_err(|e| {
            AuditError::TomlError { file_path: workspace_toml_path, details: e.to_string() }
        })?;

        // Extract workspace version
        let workspace_version = workspace_toml
            .get("workspace")
            .and_then(|w| w.get("package"))
            .and_then(|p| p.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or("0.1.0")
            .to_string();

        // Extract Rust version requirement
        let rust_version = workspace_toml
            .get("workspace")
            .and_then(|w| w.get("package"))
            .and_then(|p| p.get("rust-version"))
            .and_then(|v| v.as_str())
            .unwrap_or("1.85.0")
            .to_string();

        // Analyze individual crates in the workspace
        let mut crate_versions = HashMap::new();
        let mut dependency_versions = HashMap::new();
        let mut workspace_features = HashMap::new();

        if let Some(members) = workspace_toml
            .get("workspace")
            .and_then(|w| w.get("members"))
            .and_then(|m| m.as_array())
        {
            for member in members {
                if let Some(member_path) = member.as_str() {
                    let crate_path = workspace_path.join(member_path);
                    if let Ok(crate_info) = Self::analyze_crate(&crate_path).await {
                        crate_versions.insert(crate_info.name.clone(), crate_info.version);

                        // Collect dependencies
                        for dep in crate_info.dependencies {
                            dependency_versions.insert(dep.name, dep.version);
                        }

                        // Collect features
                        if !crate_info.features.is_empty() {
                            workspace_features.insert(crate_info.name, crate_info.features);
                        }
                    }
                }
            }
        }

        Ok(WorkspaceVersionInfo {
            workspace_version,
            rust_version,
            crate_versions,
            dependency_versions,
            workspace_features,
        })
    }

    /// Analyzes a single crate to extract its version and dependency information.
    async fn analyze_crate(crate_path: &Path) -> Result<CrateAnalysisResult> {
        let cargo_toml_path = crate_path.join("Cargo.toml");
        let content = tokio::fs::read_to_string(&cargo_toml_path).await.map_err(|e| {
            AuditError::IoError { path: cargo_toml_path.clone(), details: e.to_string() }
        })?;

        let cargo_toml: Value = toml::from_str(&content).map_err(|e| AuditError::TomlError {
            file_path: cargo_toml_path,
            details: e.to_string(),
        })?;

        // Extract crate name and version
        let name = cargo_toml
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("unknown")
            .to_string();

        let version = cargo_toml
            .get("package")
            .and_then(|p| p.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or("0.1.0")
            .to_string();

        // Extract dependencies
        let mut dependencies = Vec::new();
        if let Some(deps) = cargo_toml.get("dependencies").and_then(|d| d.as_table()) {
            for (dep_name, dep_spec) in deps {
                if let Some(version) = Self::extract_dependency_version(dep_spec) {
                    dependencies.push(DependencyInfo { name: dep_name.clone(), version });
                }
            }
        }

        // Extract features
        let mut features = Vec::new();
        if let Some(feature_table) = cargo_toml.get("features").and_then(|f| f.as_table()) {
            features.extend(feature_table.keys().cloned());
        }

        Ok(CrateAnalysisResult { name, version, dependencies, features })
    }

    /// Extracts version string from a dependency specification.
    fn extract_dependency_version(dep_spec: &Value) -> Option<String> {
        match dep_spec {
            Value::String(version) => Some(version.clone()),
            Value::Table(table) => {
                table.get("version").and_then(|v| v.as_str()).map(|s| s.to_string())
            }
            _ => None,
        }
    }

    /// Validates a Rust version reference.
    fn validate_rust_version(
        &self,
        version_ref: &VersionReference,
        config: &VersionValidationConfig,
    ) -> Result<VersionValidationResult> {
        self.compare_versions(
            &version_ref.version,
            &self.workspace_info.rust_version,
            "Rust version",
            config,
        )
    }

    /// Validates a workspace version reference.
    fn validate_workspace_version(
        &self,
        version_ref: &VersionReference,
        config: &VersionValidationConfig,
    ) -> Result<VersionValidationResult> {
        self.compare_versions(
            &version_ref.version,
            &self.workspace_info.workspace_version,
            "workspace version",
            config,
        )
    }

    /// Validates a crate version reference.
    fn validate_crate_version(
        &self,
        version_ref: &VersionReference,
        config: &VersionValidationConfig,
    ) -> Result<VersionValidationResult> {
        // Try to extract crate name from context
        if let Some(crate_name) = self.extract_crate_name_from_context(&version_ref.context) {
            if let Some(expected_version) = self.workspace_info.crate_versions.get(&crate_name) {
                return self.compare_versions(
                    &version_ref.version,
                    expected_version,
                    &format!("crate '{}' version", crate_name),
                    config,
                );
            }
        }

        // If we can't determine the specific crate, provide a generic validation
        Ok(VersionValidationResult {
            is_valid: true, // Assume valid if we can't verify
            expected_version: None,
            message: "Unable to verify crate version - crate name not found in context".to_string(),
            severity: ValidationSeverity::Info,
            suggestion: Some(
                "Ensure crate name is clearly specified in the documentation".to_string(),
            ),
        })
    }

    /// Validates a generic version reference.
    fn validate_generic_version(
        &self,
        version_ref: &VersionReference,
        _config: &VersionValidationConfig,
    ) -> Result<VersionValidationResult> {
        // For generic versions, we can only do basic format validation
        if self.patterns.semver.is_match(&version_ref.version) {
            Ok(VersionValidationResult {
                is_valid: true,
                expected_version: None,
                message: "Version format is valid".to_string(),
                severity: ValidationSeverity::Info,
                suggestion: None,
            })
        } else {
            Ok(VersionValidationResult {
                is_valid: false,
                expected_version: None,
                message: format!("Invalid version format: '{}'", version_ref.version),
                severity: ValidationSeverity::Warning,
                suggestion: Some("Use semantic versioning format (e.g., '1.0.0')".to_string()),
            })
        }
    }

    /// Compares two version strings and returns validation result.
    fn compare_versions(
        &self,
        found_version: &str,
        expected_version: &str,
        context: &str,
        config: &VersionValidationConfig,
    ) -> Result<VersionValidationResult> {
        if found_version == expected_version {
            return Ok(VersionValidationResult {
                is_valid: true,
                expected_version: None,
                message: format!("{} is correct", context),
                severity: ValidationSeverity::Info,
                suggestion: None,
            });
        }

        // Check if versions are compatible based on tolerance
        let compatible =
            self.is_version_compatible(found_version, expected_version, &config.version_tolerance)?;

        if compatible {
            Ok(VersionValidationResult {
                is_valid: true,
                expected_version: Some(expected_version.to_string()),
                message: format!(
                    "{} '{}' is compatible with expected '{}'",
                    context, found_version, expected_version
                ),
                severity: ValidationSeverity::Info,
                suggestion: None,
            })
        } else {
            let severity = if config.strict_matching {
                ValidationSeverity::Critical
            } else {
                ValidationSeverity::Warning
            };

            Ok(VersionValidationResult {
                is_valid: false,
                expected_version: Some(expected_version.to_string()),
                message: format!(
                    "{} mismatch: found '{}', expected '{}'",
                    context, found_version, expected_version
                ),
                severity,
                suggestion: Some(format!("Update to version '{}'", expected_version)),
            })
        }
    }

    /// Extracts crate name from the context string.
    fn extract_crate_name_from_context(&self, context: &str) -> Option<String> {
        // Look for patterns like 'adk-core = { version = "..."' or 'adk_core::'
        if let Some(captures) = Regex::new(r#"(adk[-_]\w+)"#).ok()?.captures(context) {
            if let Some(crate_match) = captures.get(1) {
                return Some(crate_match.as_str().replace('_', "-"));
            }
        }
        None
    }

    /// Finds the best version match for a given version string.
    fn find_best_version_match(&self, version: &str) -> Option<String> {
        // Try to find a dependency with a similar version
        for dep_version in self.workspace_info.dependency_versions.values() {
            if let (Ok(v1), Ok(v2)) = (Version::parse(version), Version::parse(dep_version)) {
                if v1.major == v2.major {
                    return Some(dep_version.clone());
                }
            }
        }
        None
    }
}

impl VersionPatterns {
    /// Creates new compiled regex patterns for version validation.
    fn new() -> Result<Self> {
        Ok(Self {
            semver: Regex::new(r"^\d+\.\d+\.\d+(?:-[a-zA-Z0-9.-]+)?(?:\+[a-zA-Z0-9.-]+)?$")
                .map_err(|e| AuditError::RegexError {
                    pattern: "semver".to_string(),
                    details: e.to_string(),
                })?,

            version_req: Regex::new(r"^[~^>=<]*\d+(?:\.\d+)?(?:\.\d+)?").map_err(|e| {
                AuditError::RegexError {
                    pattern: "version_req".to_string(),
                    details: e.to_string(),
                }
            })?,

            git_version: Regex::new(r#"git\s*=\s*"([^"]+)""#).map_err(|e| {
                AuditError::RegexError {
                    pattern: "git_version".to_string(),
                    details: e.to_string(),
                }
            })?,

            path_dependency: Regex::new(r#"path\s*=\s*"([^"]+)""#).map_err(|e| {
                AuditError::RegexError {
                    pattern: "path_dependency".to_string(),
                    details: e.to_string(),
                }
            })?,
        })
    }
}

impl Default for VersionValidationConfig {
    fn default() -> Self {
        Self {
            allow_prerelease: false,
            strict_matching: false,
            validate_git_deps: true,
            version_tolerance: VersionTolerance::Patch,
        }
    }
}

/// Result of analyzing a single crate.
#[derive(Debug, Clone)]
struct CrateAnalysisResult {
    name: String,
    version: String,
    dependencies: Vec<DependencyInfo>,
    features: Vec<String>,
}

/// Information about a dependency.
#[derive(Debug, Clone)]
struct DependencyInfo {
    name: String,
    version: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn create_test_workspace_info() -> WorkspaceVersionInfo {
        let mut crate_versions = HashMap::new();
        crate_versions.insert("adk-core".to_string(), "0.1.0".to_string());
        crate_versions.insert("adk-model".to_string(), "0.1.0".to_string());

        let mut dependency_versions = HashMap::new();
        dependency_versions.insert("serde".to_string(), "1.0.195".to_string());
        dependency_versions.insert("tokio".to_string(), "1.35.0".to_string());

        let mut workspace_features = HashMap::new();
        workspace_features.insert("adk-core".to_string(), vec!["async".to_string()]);

        WorkspaceVersionInfo {
            workspace_version: "0.1.0".to_string(),
            rust_version: "1.85.0".to_string(),
            crate_versions,
            dependency_versions,
            workspace_features,
        }
    }

    fn create_test_validator() -> VersionValidator {
        let workspace_info = create_test_workspace_info();
        VersionValidator::with_workspace_info(workspace_info).unwrap()
    }

    #[test]
    fn test_validator_creation() {
        let validator = create_test_validator();
        assert_eq!(validator.workspace_info.workspace_version, "0.1.0");
        assert_eq!(validator.workspace_info.rust_version, "1.85.0");
    }

    #[test]
    fn test_rust_version_validation() {
        let validator = create_test_validator();
        let config = VersionValidationConfig::default();

        // Valid Rust version
        let valid_ref = VersionReference {
            version: "1.85.0".to_string(),
            version_type: VersionType::RustVersion,
            line_number: 1,
            context: "rust-version = \"1.85.0\"".to_string(),
        };

        let result = validator.validate_version_reference(&valid_ref, &config).unwrap();
        assert!(result.is_valid);
        assert_eq!(result.severity, ValidationSeverity::Info);

        // Invalid Rust version
        let invalid_ref = VersionReference {
            version: "1.80.0".to_string(),
            version_type: VersionType::RustVersion,
            line_number: 1,
            context: "rust-version = \"1.80.0\"".to_string(),
        };

        let result = validator.validate_version_reference(&invalid_ref, &config).unwrap();
        assert!(!result.is_valid);
        assert_eq!(result.expected_version, Some("1.85.0".to_string()));
    }

    #[test]
    fn test_workspace_version_validation() {
        let validator = create_test_validator();
        let config = VersionValidationConfig::default();

        let version_ref = VersionReference {
            version: "0.1.0".to_string(),
            version_type: VersionType::WorkspaceVersion,
            line_number: 1,
            context: "adk-core = { version = \"0.1.0\" }".to_string(),
        };

        let result = validator.validate_version_reference(&version_ref, &config).unwrap();
        assert!(result.is_valid);
    }

    #[test]
    fn test_crate_version_validation() {
        let validator = create_test_validator();
        let config = VersionValidationConfig::default();

        let version_ref = VersionReference {
            version: "0.1.0".to_string(),
            version_type: VersionType::CrateVersion,
            line_number: 1,
            context: "adk-core = { version = \"0.1.0\" }".to_string(),
        };

        let result = validator.validate_version_reference(&version_ref, &config).unwrap();
        assert!(result.is_valid);
    }

    #[test]
    fn test_version_compatibility() {
        let validator = create_test_validator();

        // Test exact compatibility
        assert!(
            validator.is_version_compatible("1.0.0", "1.0.0", &VersionTolerance::Exact).unwrap()
        );
        assert!(
            !validator.is_version_compatible("1.0.0", "1.0.1", &VersionTolerance::Exact).unwrap()
        );

        // Test patch compatibility
        assert!(
            validator.is_version_compatible("1.0.0", "1.0.1", &VersionTolerance::Patch).unwrap()
        );
        assert!(
            !validator.is_version_compatible("1.0.0", "1.1.0", &VersionTolerance::Patch).unwrap()
        );

        // Test minor compatibility
        assert!(
            validator.is_version_compatible("1.0.0", "1.1.0", &VersionTolerance::Minor).unwrap()
        );
        assert!(
            !validator.is_version_compatible("1.0.0", "2.0.0", &VersionTolerance::Minor).unwrap()
        );

        // Test major compatibility
        assert!(
            validator.is_version_compatible("1.0.0", "2.0.0", &VersionTolerance::Major).unwrap()
        );
    }

    #[test]
    fn test_dependency_compatibility() {
        let validator = create_test_validator();
        let config = VersionValidationConfig::default();

        // Valid dependency version
        let result =
            validator.validate_dependency_compatibility("serde", "1.0.195", &config).unwrap();
        assert!(result.is_valid);

        // Invalid dependency version (different minor version)
        let result =
            validator.validate_dependency_compatibility("serde", "1.1.0", &config).unwrap();
        assert!(!result.is_valid);
        assert_eq!(result.expected_version, Some("1.0.195".to_string()));

        // Unknown dependency
        let result =
            validator.validate_dependency_compatibility("unknown", "1.0.0", &config).unwrap();
        assert!(!result.is_valid);
        assert_eq!(result.severity, ValidationSeverity::Warning);
    }

    #[test]
    fn test_version_suggestion() {
        let validator = create_test_validator();

        // Rust version suggestion
        let rust_ref = VersionReference {
            version: "1.80.0".to_string(),
            version_type: VersionType::RustVersion,
            line_number: 1,
            context: "rust-version = \"1.80.0\"".to_string(),
        };
        assert_eq!(validator.suggest_correct_version(&rust_ref), Some("1.85.0".to_string()));

        // Workspace version suggestion
        let workspace_ref = VersionReference {
            version: "0.0.1".to_string(),
            version_type: VersionType::WorkspaceVersion,
            line_number: 1,
            context: "version = \"0.0.1\"".to_string(),
        };
        assert_eq!(validator.suggest_correct_version(&workspace_ref), Some("0.1.0".to_string()));
    }

    #[test]
    fn test_crate_name_extraction() {
        let validator = create_test_validator();

        // Test various context formats
        assert_eq!(
            validator.extract_crate_name_from_context("adk-core = { version = \"0.1.0\" }"),
            Some("adk-core".to_string())
        );

        assert_eq!(
            validator.extract_crate_name_from_context("use adk_core::Agent;"),
            Some("adk-core".to_string())
        );

        assert_eq!(
            validator.extract_crate_name_from_context("The adk_model crate provides..."),
            Some("adk-model".to_string())
        );

        assert_eq!(validator.extract_crate_name_from_context("No crate name here"), None);
    }

    #[test]
    fn test_batch_validation() {
        let validator = create_test_validator();
        let config = VersionValidationConfig::default();

        let version_refs = vec![
            VersionReference {
                version: "1.85.0".to_string(),
                version_type: VersionType::RustVersion,
                line_number: 1,
                context: "rust-version = \"1.85.0\"".to_string(),
            },
            VersionReference {
                version: "0.1.0".to_string(),
                version_type: VersionType::WorkspaceVersion,
                line_number: 2,
                context: "version = \"0.1.0\"".to_string(),
            },
        ];

        let results = validator.validate_version_references(&version_refs, &config).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].is_valid);
        assert!(results[1].is_valid);
    }

    #[test]
    fn test_crate_name_validation() {
        let validator = create_test_validator();

        // Valid crate name
        let result = validator.validate_crate_exists("adk-core");
        assert!(result.is_valid);
        assert_eq!(result.severity, ValidationSeverity::Info);

        // Invalid crate name
        let result = validator.validate_crate_exists("nonexistent-crate");
        assert!(!result.is_valid);
        assert_eq!(result.severity, ValidationSeverity::Warning);
        assert!(result.suggestion.is_some());
    }

    #[test]
    fn test_feature_flag_validation() {
        let validator = create_test_validator();

        // Valid feature in existing crate
        let result = validator.validate_feature_flag("adk-core", "async");
        assert!(result.is_valid);
        assert_eq!(result.severity, ValidationSeverity::Info);

        // Invalid feature in existing crate
        let result = validator.validate_feature_flag("adk-core", "nonexistent-feature");
        assert!(!result.is_valid);
        assert_eq!(result.severity, ValidationSeverity::Warning);

        // Feature in nonexistent crate
        let result = validator.validate_feature_flag("nonexistent-crate", "some-feature");
        assert!(!result.is_valid);
        assert_eq!(result.severity, ValidationSeverity::Warning);
    }

    #[test]
    fn test_batch_crate_validation() {
        let validator = create_test_validator();

        let crate_names =
            vec!["adk-core".to_string(), "adk-model".to_string(), "nonexistent".to_string()];

        let results = validator.validate_crate_names(&crate_names);
        assert_eq!(results.len(), 3);
        assert!(results[0].is_valid); // adk-core exists
        assert!(results[1].is_valid); // adk-model exists
        assert!(!results[2].is_valid); // nonexistent doesn't exist
    }

    #[test]
    fn test_feature_in_any_crate() {
        let validator = create_test_validator();

        // Feature that exists in workspace
        let result = validator.validate_feature_in_any_crate("async");
        assert!(result.is_valid);

        // Feature that doesn't exist anywhere
        let result = validator.validate_feature_in_any_crate("nonexistent-feature");
        assert!(!result.is_valid);
    }

    #[test]
    fn test_workspace_queries() {
        let validator = create_test_validator();

        // Test getting workspace crates
        let crates = validator.get_workspace_crates();
        assert!(crates.contains(&"adk-core".to_string()));
        assert!(crates.contains(&"adk-model".to_string()));

        // Test getting crate features
        let features = validator.get_crate_features("adk-core");
        assert!(features.is_some());
        assert!(features.unwrap().contains(&"async".to_string()));

        // Test getting all features
        let all_features = validator.get_all_workspace_features();
        assert!(all_features.contains_key("adk-core"));
    }

    #[test]
    fn test_similar_name_suggestions() {
        let validator = create_test_validator();

        // Test crate name suggestion
        let result = validator.validate_crate_exists("adk-cor"); // typo in adk-core
        assert!(!result.is_valid);
        assert!(result.suggestion.is_some());
        assert!(result.suggestion.unwrap().contains("adk-core"));

        // Test feature name suggestion
        let result = validator.validate_feature_flag("adk-core", "asyn"); // typo in async
        assert!(!result.is_valid);
        assert!(result.suggestion.is_some());
        assert!(result.suggestion.unwrap().contains("async"));
    }

    #[test]
    fn test_edit_distance() {
        let validator = create_test_validator();

        assert_eq!(validator.edit_distance("", ""), 0);
        assert_eq!(validator.edit_distance("abc", "abc"), 0);
        assert_eq!(validator.edit_distance("abc", "ab"), 1);
        assert_eq!(validator.edit_distance("abc", "def"), 3);
        assert_eq!(validator.edit_distance("kitten", "sitting"), 3);
    }

    #[test]
    fn test_version_tolerance_config() {
        let validator = create_test_validator();

        // Strict matching config
        let strict_config = VersionValidationConfig {
            strict_matching: true,
            version_tolerance: VersionTolerance::Exact,
            ..Default::default()
        };

        let version_ref = VersionReference {
            version: "1.84.0".to_string(),
            version_type: VersionType::RustVersion,
            line_number: 1,
            context: "rust-version = \"1.84.0\"".to_string(),
        };

        let result = validator.validate_version_reference(&version_ref, &strict_config).unwrap();
        assert!(!result.is_valid);
        assert_eq!(result.severity, ValidationSeverity::Critical);

        // Lenient matching config with major version difference
        let lenient_config = VersionValidationConfig {
            strict_matching: false,
            version_tolerance: VersionTolerance::Minor,
            ..Default::default()
        };

        // Use a version with different major version to ensure it fails
        let major_diff_ref = VersionReference {
            version: "2.0.0".to_string(),
            version_type: VersionType::RustVersion,
            line_number: 1,
            context: "rust-version = \"2.0.0\"".to_string(),
        };

        let result =
            validator.validate_version_reference(&major_diff_ref, &lenient_config).unwrap();
        // Invalid because major version difference, but warning instead of critical
        assert!(!result.is_valid);
        assert_eq!(result.severity, ValidationSeverity::Warning);
    }
}
