//! Audit orchestrator that coordinates all validation components.
//!
//! This module provides the main orchestration logic for running documentation audits.
//! It coordinates between the parser, analyzer, validator, and reporter components to
//! provide comprehensive documentation validation.

use crate::{
    AuditConfig, AuditError, AuditIssue, AuditReport, AuditSummary, CodeAnalyzer,
    DocumentationParser, ExampleValidator, FileAuditResult, IssueCategory, IssueSeverity,
    ReportGenerator, Result, SuggestionEngine, VersionValidator, reporter::AuditReportConfig,
};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{debug, error, info, instrument, warn};
use walkdir::WalkDir;

/// Main orchestrator that coordinates the audit process.
pub struct AuditOrchestrator {
    /// Configuration for the audit.
    config: AuditConfig,
    /// Documentation parser for extracting information from markdown files.
    parser: DocumentationParser,
    /// Code analyzer for validating API references.
    analyzer: CodeAnalyzer,
    /// Example validator for testing code compilation.
    validator: ExampleValidator,
    /// Version validator for checking version consistency.
    version_validator: VersionValidator,
    /// Suggestion engine for generating fix recommendations.
    _suggestion_engine: SuggestionEngine,
    /// Report generator for creating audit reports.
    _report_generator: ReportGenerator,
}

impl AuditOrchestrator {
    /// Create a new audit orchestrator with the given configuration.
    #[instrument(skip(config))]
    pub async fn new(config: AuditConfig) -> Result<Self> {
        info!("Initializing audit orchestrator");
        debug!("Configuration: {:?}", config);

        // Basic validation - check paths exist
        if !config.workspace_path.exists() {
            return Err(AuditError::WorkspaceNotFound { path: config.workspace_path.clone() });
        }

        // Initialize all components
        info!("Initializing documentation parser");
        let parser = DocumentationParser::new("0.1.0".to_string(), "1.85.0".to_string())?;

        info!("Initializing code analyzer");
        let analyzer = CodeAnalyzer::new(config.workspace_path.clone());

        info!("Initializing example validator");
        let validator =
            ExampleValidator::new("0.1.0".to_string(), config.workspace_path.clone()).await?;

        info!("Initializing version validator");
        let version_validator = VersionValidator::new(&config.workspace_path).await?;

        info!("Initializing suggestion engine");
        let suggestion_engine = SuggestionEngine::new_empty();

        info!("Initializing report generator");
        let report_generator = ReportGenerator::new(crate::reporter::OutputFormat::Console);

        info!("Audit orchestrator initialized successfully");

        Ok(Self {
            config,
            parser,
            analyzer,
            validator,
            version_validator,
            _suggestion_engine: suggestion_engine,
            _report_generator: report_generator,
        })
    }

    /// Run a full audit of all documentation files.
    #[instrument(skip(self))]
    pub async fn run_full_audit(&mut self) -> Result<AuditReport> {
        info!("Starting full documentation audit");
        let start_time = Instant::now();

        // Discover all documentation files
        let doc_files = self.discover_documentation_files().await?;
        info!("Found {} documentation files to audit", doc_files.len());

        // Process each documentation file
        let mut file_results = Vec::new();
        let mut all_issues = Vec::new();
        let all_recommendations = Vec::new();

        for doc_file in &doc_files {
            if self.should_skip_file(doc_file) {
                debug!("Skipping excluded file: {}", doc_file.display());
                continue;
            }

            info!("Processing file: {}", doc_file.display());

            match self.process_documentation_file(doc_file).await {
                Ok((file_result, mut issues, _recommendations)) => {
                    file_results.push(file_result);
                    all_issues.append(&mut issues);
                }
                Err(e) => {
                    error!("Failed to process file {}: {}", doc_file.display(), e);

                    // Create a failed file result
                    let file_result = FileAuditResult {
                        file_path: doc_file.clone(),
                        file_hash: self
                            .calculate_file_hash(doc_file)
                            .unwrap_or_else(|_| "error".to_string()),
                        last_modified: Utc::now(),
                        issues_count: 1,
                        issues: vec![self.create_processing_error_issue(doc_file, &e)],
                        passed: false,
                        audit_duration_ms: 0,
                    };

                    file_results.push(file_result);
                    all_issues.push(self.create_processing_error_issue(doc_file, &e));
                }
            }
        }

        // Create audit summary
        let summary = self.create_audit_summary(&file_results, &all_issues);

        let total_time = start_time.elapsed();
        info!("Full audit completed in {:?}", total_time);
        info!("Found {} total issues across {} files", all_issues.len(), file_results.len());

        // Generate the final report
        let report = AuditReport {
            summary,
            file_results,
            issues: all_issues,
            recommendations: all_recommendations,
            timestamp: Utc::now(),
            audit_config: AuditReportConfig::default(),
        };

        Ok(report)
    }

    /// Run an incremental audit on only the specified changed files.
    #[instrument(skip(self, changed_files))]
    pub async fn run_incremental_audit(
        &mut self,
        changed_files: &[PathBuf],
    ) -> Result<AuditReport> {
        info!("Starting incremental audit on {} files", changed_files.len());
        let start_time = Instant::now();

        // Filter to only documentation files that exist
        let mut doc_files = Vec::new();
        for file in changed_files {
            if self.is_documentation_file(file) && file.exists() {
                doc_files.push(file.clone());
            } else {
                debug!("Skipping non-documentation file: {}", file.display());
            }
        }

        if doc_files.is_empty() {
            info!("No documentation files to audit in changed files");
            return Ok(AuditReport {
                summary: AuditSummary {
                    total_files: 0,
                    files_with_issues: 0,
                    total_issues: 0,
                    critical_issues: 0,
                    warning_issues: 0,
                    info_issues: 0,
                    coverage_percentage: 100.0,
                    average_issues_per_file: 0.0,
                    most_common_issue: None,
                    problematic_files: Vec::new(),
                },
                file_results: Vec::new(),
                issues: Vec::new(),
                recommendations: Vec::new(),
                timestamp: Utc::now(),
                audit_config: AuditReportConfig::default(),
            });
        }

        info!("Processing {} documentation files", doc_files.len());

        // Process each changed documentation file
        let mut file_results = Vec::new();
        let mut all_issues = Vec::new();
        let all_recommendations = Vec::new();

        for doc_file in &doc_files {
            if self.should_skip_file(doc_file) {
                debug!("Skipping excluded file: {}", doc_file.display());
                continue;
            }

            info!("Processing changed file: {}", doc_file.display());

            match self.process_documentation_file(doc_file).await {
                Ok((file_result, mut issues, _recommendations)) => {
                    file_results.push(file_result);
                    all_issues.append(&mut issues);
                }
                Err(e) => {
                    error!("Failed to process file {}: {}", doc_file.display(), e);

                    // Create a failed file result
                    let file_result = FileAuditResult {
                        file_path: doc_file.clone(),
                        file_hash: self
                            .calculate_file_hash(doc_file)
                            .unwrap_or_else(|_| "error".to_string()),
                        last_modified: Utc::now(),
                        issues_count: 1,
                        issues: vec![self.create_processing_error_issue(doc_file, &e)],
                        passed: false,
                        audit_duration_ms: 0,
                    };

                    file_results.push(file_result);
                    all_issues.push(self.create_processing_error_issue(doc_file, &e));
                }
            }
        }

        // Create summary
        let summary = self.create_audit_summary(&file_results, &all_issues);

        let total_time = start_time.elapsed();
        info!("Incremental audit completed in {:?}", total_time);

        Ok(AuditReport {
            summary,
            file_results,
            issues: all_issues,
            recommendations: all_recommendations,
            timestamp: Utc::now(),
            audit_config: AuditReportConfig::default(),
        })
    }

    /// Validate a single documentation file.
    #[instrument(skip(self))]
    pub async fn validate_file(&mut self, file_path: &Path) -> Result<FileAuditResult> {
        info!("Validating single file: {}", file_path.display());

        if !file_path.exists() {
            return Err(AuditError::FileNotFound { path: file_path.to_path_buf() });
        }

        if !self.is_documentation_file(file_path) {
            return Err(AuditError::InvalidFileType {
                path: file_path.to_path_buf(),
                expected: "markdown documentation file".to_string(),
            });
        }

        // Process the single file
        match self.process_documentation_file(file_path).await {
            Ok((file_result, _issues, _recommendations)) => Ok(file_result),
            Err(e) => {
                error!("Failed to validate file {}: {}", file_path.display(), e);

                // Return a failed file result
                Ok(FileAuditResult {
                    file_path: file_path.to_path_buf(),
                    file_hash: self
                        .calculate_file_hash(file_path)
                        .unwrap_or_else(|_| "error".to_string()),
                    last_modified: Utc::now(),
                    issues_count: 1,
                    issues: vec![self.create_processing_error_issue(file_path, &e)],
                    passed: false,
                    audit_duration_ms: 0,
                })
            }
        }
    }

    /// Process a single documentation file through all validation stages.
    #[instrument(skip(self))]
    async fn process_documentation_file(
        &mut self,
        file_path: &Path,
    ) -> Result<(FileAuditResult, Vec<AuditIssue>, Vec<crate::Recommendation>)> {
        let file_start_time = Instant::now();
        debug!("Processing documentation file: {}", file_path.display());

        // Calculate file hash and metadata
        let file_hash = self.calculate_file_hash(file_path)?;
        let last_modified = self.get_file_modified_time(file_path)?;

        // Parse the documentation file
        debug!("Parsing documentation file");
        let parsed_doc = self.parser.parse_file(file_path).await?;

        let mut all_issues = Vec::new();
        let mut all_recommendations = Vec::new();

        // Stage 1: API Reference Validation
        debug!("Validating API references");
        for api_ref in &parsed_doc.api_references {
            match self.analyzer.validate_api_reference(api_ref).await {
                Ok(result) => {
                    if !result.success {
                        all_issues.push(AuditIssue {
                            id: format!("api-ref-{}", api_ref.item_path),
                            file_path: file_path.to_path_buf(),
                            line_number: Some(api_ref.line_number),
                            column_number: None,
                            severity: IssueSeverity::Warning,
                            category: IssueCategory::ApiMismatch,
                            message: format!(
                                "API reference '{}' not found in crate",
                                api_ref.item_path
                            ),
                            suggestion: Some(format!(
                                "Check if '{}' is correctly spelled and exported",
                                api_ref.item_path
                            )),
                            context: Some(api_ref.context.clone()),
                            code_snippet: None,
                            related_issues: Vec::new(),
                        });
                    }
                }
                Err(e) => {
                    debug!("Error validating API reference '{}': {}", api_ref.item_path, e);
                }
            }
        }

        // Stage 2: Code Example Validation
        debug!("Validating code examples");
        for example in &parsed_doc.code_examples {
            if example.is_runnable {
                match self.validator.validate_example(example).await {
                    Ok(result) => {
                        if !result.success {
                            all_issues.push(AuditIssue {
                                id: format!("example-{}", example.line_number),
                                file_path: file_path.to_path_buf(),
                                line_number: Some(example.line_number),
                                column_number: None,
                                severity: IssueSeverity::Critical,
                                category: IssueCategory::CompilationError,
                                message: "Code example does not compile".to_string(),
                                suggestion: result.suggestions.first().cloned(),
                                context: Some(example.content.clone()),
                                code_snippet: Some(example.content.clone()),
                                related_issues: Vec::new(),
                            });
                        }

                        // Check for warnings (potential async pattern issues)
                        for warning in &result.warnings {
                            all_issues.push(AuditIssue {
                                id: format!("async-{}", example.line_number),
                                file_path: file_path.to_path_buf(),
                                line_number: Some(example.line_number),
                                column_number: None,
                                severity: IssueSeverity::Warning,
                                category: IssueCategory::AsyncPatternError,
                                message: warning.clone(),
                                suggestion: Some(
                                    "Consider using proper async patterns".to_string(),
                                ),
                                context: Some(example.content.clone()),
                                code_snippet: Some(example.content.clone()),
                                related_issues: Vec::new(),
                            });
                        }
                    }
                    Err(e) => {
                        debug!("Error validating example at line {}: {}", example.line_number, e);
                    }
                }
            }
        }

        // Stage 3: Version Consistency Validation
        debug!("Validating version references");
        let version_config = crate::version::VersionValidationConfig::default();
        for version_ref in &parsed_doc.version_references {
            match self.version_validator.validate_version_reference(version_ref, &version_config) {
                Ok(result) => {
                    if !result.is_valid {
                        all_issues.push(AuditIssue {
                            id: format!("version-{}", version_ref.line_number),
                            file_path: file_path.to_path_buf(),
                            line_number: Some(version_ref.line_number),
                            column_number: None,
                            severity: IssueSeverity::Warning,
                            category: IssueCategory::VersionInconsistency,
                            message: format!(
                                "Version '{}' does not match workspace version",
                                version_ref.version
                            ),
                            suggestion: Some(
                                "Update version to match workspace Cargo.toml".to_string(),
                            ),
                            context: Some(version_ref.context.clone()),
                            code_snippet: None,
                            related_issues: Vec::new(),
                        });
                    }
                }
                Err(e) => {
                    debug!("Error validating version reference '{}': {}", version_ref.version, e);
                }
            }
        }

        // Stage 4: Internal Link Validation
        debug!("Validating internal links");
        for link in &parsed_doc.internal_links {
            if !self.validate_internal_link(link, file_path) {
                all_issues.push(AuditIssue {
                    id: format!("link-{}", link.line_number),
                    file_path: file_path.to_path_buf(),
                    line_number: Some(link.line_number),
                    column_number: None,
                    severity: IssueSeverity::Info,
                    category: IssueCategory::BrokenLink,
                    message: format!("Internal link '{}' may be broken", link.target),
                    suggestion: Some("Check if the target file or section exists".to_string()),
                    context: Some(link.text.clone()),
                    code_snippet: None,
                    related_issues: Vec::new(),
                });
            }
        }

        // Stage 5: Feature Flag Validation
        debug!("Validating feature flags");
        for feature in &parsed_doc.feature_mentions {
            let result = self.version_validator.validate_feature_flag(
                &feature.feature_name,
                feature.crate_name.as_deref().unwrap_or(""),
            );
            if !result.is_valid {
                all_issues.push(AuditIssue {
                    id: format!("feature-{}", feature.line_number),
                    file_path: file_path.to_path_buf(),
                    line_number: Some(feature.line_number),
                    column_number: None,
                    severity: IssueSeverity::Warning,
                    category: IssueCategory::InvalidFeatureFlag,
                    message: format!(
                        "Feature flag '{}' not found in any crate",
                        feature.feature_name
                    ),
                    suggestion: Some(
                        "Check if feature name is correct or add to Cargo.toml".to_string(),
                    ),
                    context: Some(feature.context.clone()),
                    code_snippet: None,
                    related_issues: Vec::new(),
                });
            }
        }

        // Generate suggestions for found issues (simplified for now)
        if !all_issues.is_empty() {
            debug!("Found {} issues, generating basic recommendations", all_issues.len());
            // For now, just create a simple recommendation
            all_recommendations.push(crate::Recommendation {
                id: "general-fix".to_string(),
                recommendation_type: crate::RecommendationType::FixIssue,
                priority: 3, // Medium priority
                title: "Fix Documentation Issues".to_string(),
                description: format!(
                    "Fix {} documentation issues found in {}",
                    all_issues.len(),
                    file_path.file_name().unwrap_or_default().to_string_lossy()
                ),
                affected_files: vec![file_path.to_path_buf()],
                estimated_effort_hours: Some(1.0),
                resolves_issues: all_issues.iter().map(|i| i.id.clone()).collect(),
            });
        }

        let processing_time = file_start_time.elapsed();

        // Create file audit result
        let file_result = FileAuditResult {
            file_path: file_path.to_path_buf(),
            file_hash,
            last_modified,
            issues_count: all_issues.len(),
            issues: all_issues.clone(),
            passed: all_issues.iter().all(|issue| issue.severity != IssueSeverity::Critical),
            audit_duration_ms: processing_time.as_millis() as u64,
        };

        debug!(
            "Completed processing file {} in {:?} with {} issues",
            file_path.display(),
            processing_time,
            all_issues.len()
        );

        Ok((file_result, all_issues, all_recommendations))
    }

    /// Validate an internal link within the documentation.
    fn validate_internal_link(&self, link: &crate::InternalLink, current_file: &Path) -> bool {
        // Simple validation - check if target file exists
        if link.target.starts_with("http://") || link.target.starts_with("https://") {
            return true; // External links are not validated here
        }

        // Handle relative paths
        let target_path = if link.target.starts_with('/') {
            // Absolute path from docs root
            self.config.docs_path.join(&link.target[1..])
        } else {
            // Relative path from current file
            current_file.parent().unwrap_or(&self.config.docs_path).join(&link.target)
        };

        target_path.exists()
    }

    /// Create a processing error issue for a file.
    fn create_processing_error_issue(&self, file_path: &Path, error: &AuditError) -> AuditIssue {
        AuditIssue {
            id: format!(
                "processing-error-{}",
                file_path.file_name().unwrap_or_default().to_string_lossy()
            ),
            file_path: file_path.to_path_buf(),
            line_number: None,
            column_number: None,
            severity: IssueSeverity::Critical,
            category: IssueCategory::ProcessingError,
            message: format!("Failed to process file: {}", error),
            suggestion: None,
            context: None,
            code_snippet: None,
            related_issues: Vec::new(),
        }
    }

    /// Discover all documentation files in the docs directory.
    async fn discover_documentation_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        if !self.config.docs_path.exists() {
            warn!("Documentation directory does not exist: {}", self.config.docs_path.display());
            return Ok(files);
        }

        for entry in WalkDir::new(&self.config.docs_path)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if self.is_documentation_file(path) {
                files.push(path.to_path_buf());
            }
        }

        debug!("Discovered {} documentation files", files.len());
        Ok(files)
    }

    /// Check if a file is a documentation file (markdown).
    fn is_documentation_file(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
            .unwrap_or(false)
    }

    /// Check if a file should be skipped based on exclusion patterns.
    fn should_skip_file(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        for pattern in &self.config.excluded_files {
            if glob_match::glob_match(pattern, &path_str) {
                return true;
            }
        }

        false
    }

    /// Create audit summary from file results and issues.
    fn create_audit_summary(
        &self,
        file_results: &[FileAuditResult],
        issues: &[AuditIssue],
    ) -> AuditSummary {
        let total_files = file_results.len();
        let files_with_issues = file_results.iter().filter(|r| !r.issues.is_empty()).count();
        let total_issues = issues.len();

        let critical_issues =
            issues.iter().filter(|i| i.severity == IssueSeverity::Critical).count();
        let warning_issues = issues.iter().filter(|i| i.severity == IssueSeverity::Warning).count();
        let info_issues = issues.iter().filter(|i| i.severity == IssueSeverity::Info).count();

        let coverage_percentage = if total_files > 0 {
            ((total_files - files_with_issues) as f64 / total_files as f64) * 100.0
        } else {
            100.0
        };

        let average_issues_per_file =
            if total_files > 0 { total_issues as f64 / total_files as f64 } else { 0.0 };

        AuditSummary {
            total_files,
            files_with_issues,
            total_issues,
            critical_issues,
            warning_issues,
            info_issues,
            coverage_percentage,
            average_issues_per_file,
            most_common_issue: None,
            problematic_files: Vec::new(),
        }
    }

    /// Calculate SHA256 hash of a file for change detection.
    fn calculate_file_hash(&self, file_path: &Path) -> Result<String> {
        let content = fs::read(file_path).map_err(|e| AuditError::IoError {
            path: file_path.to_path_buf(),
            details: e.to_string(),
        })?;

        let mut hasher = Sha256::new();
        hasher.update(&content);
        let hash = hasher.finalize();
        Ok(format!("{:x}", hash))
    }

    /// Get the last modified time of a file.
    fn get_file_modified_time(&self, file_path: &Path) -> Result<chrono::DateTime<Utc>> {
        let metadata = fs::metadata(file_path).map_err(|e| AuditError::IoError {
            path: file_path.to_path_buf(),
            details: e.to_string(),
        })?;

        let modified = metadata.modified().map_err(|e| AuditError::IoError {
            path: file_path.to_path_buf(),
            details: e.to_string(),
        })?;

        Ok(chrono::DateTime::from(modified))
    }
}

// Simple glob matching implementation
mod glob_match {
    pub fn glob_match(pattern: &str, text: &str) -> bool {
        // Simple implementation - in a real system you'd use a proper glob library
        if pattern.contains('*') {
            // Handle ** patterns (recursive directory matching)
            if pattern.contains("**") {
                let pattern = pattern.replace("**", "*");
                return glob_match_simple(&pattern, text);
            } else {
                return glob_match_simple(pattern, text);
            }
        }

        pattern == text
    }

    fn glob_match_simple(pattern: &str, text: &str) -> bool {
        let parts: Vec<&str> = pattern.split('*').collect();

        if parts.len() == 1 {
            // No wildcards
            return pattern == text;
        }

        if parts.len() == 2 {
            // Single wildcard
            let prefix = parts[0];
            let suffix = parts[1];
            return text.starts_with(prefix)
                && text.ends_with(suffix)
                && text.len() >= prefix.len() + suffix.len();
        }

        // Multiple wildcards - more complex matching
        let mut text_pos = 0;

        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                continue;
            }

            if i == 0 {
                // First part must match at the beginning
                if !text[text_pos..].starts_with(part) {
                    return false;
                }
                text_pos += part.len();
            } else if i == parts.len() - 1 {
                // Last part must match at the end
                return text[text_pos..].ends_with(part);
            } else {
                // Middle parts can match anywhere after current position
                if let Some(pos) = text[text_pos..].find(part) {
                    text_pos += pos + part.len();
                } else {
                    return false;
                }
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    async fn create_test_orchestrator() -> (AuditOrchestrator, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let workspace_path = temp_dir.path().to_path_buf();
        let docs_path = workspace_path.join("docs");

        // Create basic directory structure
        fs::create_dir_all(&docs_path).unwrap();

        // Create a simple Cargo.toml
        let cargo_toml = r#"
[package]
name = "test-crate"
version = "0.1.0"
edition = "2021"
"#;
        fs::write(workspace_path.join("Cargo.toml"), cargo_toml).unwrap();

        let config = AuditConfig::builder()
            .workspace_path(&workspace_path)
            .docs_path(&docs_path)
            .build()
            .unwrap();

        let orchestrator = AuditOrchestrator::new(config).await.unwrap();
        (orchestrator, temp_dir)
    }

    #[tokio::test]
    async fn test_orchestrator_creation() {
        let (_orchestrator, _temp_dir) = create_test_orchestrator().await;
        // If we get here, orchestrator was created successfully
    }

    #[tokio::test]
    async fn test_discover_documentation_files() {
        let (orchestrator, temp_dir) = create_test_orchestrator().await;

        // Create some test files
        let docs_path = temp_dir.path().join("docs");
        fs::write(docs_path.join("test1.md"), "# Test 1").unwrap();
        fs::write(docs_path.join("test2.markdown"), "# Test 2").unwrap();
        fs::write(docs_path.join("not_docs.txt"), "Not docs").unwrap();

        let files = orchestrator.discover_documentation_files().await.unwrap();

        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|f| f.file_name().unwrap() == "test1.md"));
        assert!(files.iter().any(|f| f.file_name().unwrap() == "test2.markdown"));
    }

    #[tokio::test]
    async fn test_is_documentation_file() {
        let (orchestrator, _temp_dir) = create_test_orchestrator().await;

        assert!(orchestrator.is_documentation_file(Path::new("test.md")));
        assert!(orchestrator.is_documentation_file(Path::new("test.markdown")));
        assert!(orchestrator.is_documentation_file(Path::new("test.MD")));
        assert!(!orchestrator.is_documentation_file(Path::new("test.txt")));
        assert!(!orchestrator.is_documentation_file(Path::new("test.rs")));
    }

    #[tokio::test]
    async fn test_should_skip_file() {
        let temp_dir = TempDir::new().unwrap();
        let workspace_path = temp_dir.path().to_path_buf();
        let docs_path = workspace_path.join("docs");
        fs::create_dir_all(&docs_path).unwrap();

        // Create a simple Cargo.toml
        let cargo_toml = r#"
[package]
name = "test-crate"
version = "0.1.0"
edition = "2021"
"#;
        fs::write(workspace_path.join("Cargo.toml"), cargo_toml).unwrap();

        let config = AuditConfig::builder()
            .workspace_path(&workspace_path)
            .docs_path(&docs_path)
            .exclude_files(vec!["**/internal/**".to_string(), "draft_*.md".to_string()])
            .build()
            .unwrap();

        let orchestrator = AuditOrchestrator::new(config).await.unwrap();

        assert!(orchestrator.should_skip_file(Path::new("docs/internal/secret.md")));
        assert!(orchestrator.should_skip_file(Path::new("draft_feature.md")));
        assert!(!orchestrator.should_skip_file(Path::new("docs/public.md")));
    }

    #[tokio::test]
    async fn test_empty_incremental_audit() {
        let (mut orchestrator, _temp_dir) = create_test_orchestrator().await;

        let result = orchestrator.run_incremental_audit(&[]).await.unwrap();

        assert_eq!(result.summary.total_files, 0);
        assert_eq!(result.summary.total_issues, 0);
        assert_eq!(result.file_results.len(), 0);
    }
}
