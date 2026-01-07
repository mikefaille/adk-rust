//! Report generation functionality for documentation audits.
//!
//! This module provides comprehensive reporting capabilities including:
//! - Structured audit reports with issue categorization
//! - Severity assignment and statistics calculation
//! - Multiple output formats (JSON, Markdown, Console)
//! - Actionable recommendations for fixing issues

use crate::{AuditError, IssueSeverity, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write;
use std::io::Write as IoWrite;
use std::path::PathBuf;

/// Comprehensive audit report containing all findings and statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditReport {
    /// High-level summary of the audit results
    pub summary: AuditSummary,
    /// Detailed results for each audited file
    pub file_results: Vec<FileAuditResult>,
    /// All issues found during the audit
    pub issues: Vec<AuditIssue>,
    /// Actionable recommendations for improvements
    pub recommendations: Vec<Recommendation>,
    /// When the audit was performed
    pub timestamp: DateTime<Utc>,
    /// Configuration used for the audit
    pub audit_config: AuditReportConfig,
}

/// High-level statistics and summary of audit results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditSummary {
    /// Total number of files audited
    pub total_files: usize,
    /// Number of files that had at least one issue
    pub files_with_issues: usize,
    /// Total number of issues found across all files
    pub total_issues: usize,
    /// Number of critical severity issues
    pub critical_issues: usize,
    /// Number of warning severity issues
    pub warning_issues: usize,
    /// Number of info severity issues
    pub info_issues: usize,
    /// Percentage of documentation that is accurate (0.0 to 100.0)
    pub coverage_percentage: f64,
    /// Average issues per file
    pub average_issues_per_file: f64,
    /// Most common issue category
    pub most_common_issue: Option<IssueCategory>,
    /// Files with the most issues (top 5)
    pub problematic_files: Vec<ProblematicFile>,
}

/// Information about a file with many issues.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblematicFile {
    /// Path to the file
    pub path: PathBuf,
    /// Number of issues in this file
    pub issue_count: usize,
    /// Most severe issue in this file
    pub max_severity: IssueSeverity,
}

/// Audit results for a specific file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAuditResult {
    /// Path to the audited file
    pub file_path: PathBuf,
    /// Hash of the file content when audited
    pub file_hash: String,
    /// When the file was last modified
    pub last_modified: DateTime<Utc>,
    /// Number of issues found in this file
    pub issues_count: usize,
    /// Issues found in this file
    pub issues: Vec<AuditIssue>,
    /// Whether the file passed the audit (no critical issues)
    pub passed: bool,
    /// Time taken to audit this file
    pub audit_duration_ms: u64,
}

/// A specific issue found during documentation audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditIssue {
    /// Unique identifier for this issue
    pub id: String,
    /// Path to the file containing the issue
    pub file_path: PathBuf,
    /// Line number where the issue occurs (if applicable)
    pub line_number: Option<usize>,
    /// Column number where the issue occurs (if applicable)
    pub column_number: Option<usize>,
    /// Severity level of the issue
    pub severity: IssueSeverity,
    /// Category of the issue
    pub category: IssueCategory,
    /// Human-readable description of the issue
    pub message: String,
    /// Suggested fix for the issue (if available)
    pub suggestion: Option<String>,
    /// Additional context or details
    pub context: Option<String>,
    /// Code snippet showing the problematic area
    pub code_snippet: Option<String>,
    /// Related issues (by ID)
    pub related_issues: Vec<String>,
}

/// Categories of issues that can be found during audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IssueCategory {
    /// API reference doesn't match actual implementation
    ApiMismatch,
    /// Version numbers are inconsistent or outdated
    VersionInconsistency,
    /// Code example fails to compile
    CompilationError,
    /// Internal link is broken or incorrect
    BrokenLink,
    /// Feature exists but is not documented
    MissingDocumentation,
    /// Documentation references deprecated API
    DeprecatedApi,
    /// Import statement is invalid or incorrect
    InvalidImport,
    /// Configuration parameter is wrong or missing
    ConfigurationError,
    /// Async/await pattern is incorrect
    AsyncPatternError,
    /// Feature flag reference is invalid
    InvalidFeatureFlag,
    /// Crate name reference is invalid
    InvalidCrateName,
    /// General documentation quality issue
    QualityIssue,
    /// Error occurred while processing the file
    ProcessingError,
    /// Error occurred during validation
    ValidationError,
}

impl IssueCategory {
    /// Get a human-readable description of the issue category.
    pub fn description(&self) -> &'static str {
        match self {
            IssueCategory::ApiMismatch => "API reference doesn't match implementation",
            IssueCategory::VersionInconsistency => "Version numbers are inconsistent",
            IssueCategory::CompilationError => "Code example fails to compile",
            IssueCategory::BrokenLink => "Internal link is broken",
            IssueCategory::MissingDocumentation => "Missing documentation for feature",
            IssueCategory::DeprecatedApi => "References deprecated API",
            IssueCategory::InvalidImport => "Import statement is invalid",
            IssueCategory::ConfigurationError => "Configuration parameter error",
            IssueCategory::AsyncPatternError => "Async/await pattern is incorrect",
            IssueCategory::InvalidFeatureFlag => "Feature flag reference is invalid",
            IssueCategory::InvalidCrateName => "Crate name reference is invalid",
            IssueCategory::QualityIssue => "General documentation quality issue",
            IssueCategory::ProcessingError => "Error occurred while processing file",
            IssueCategory::ValidationError => "Error occurred during validation",
        }
    }

    /// Get the default severity for this category.
    pub fn default_severity(&self) -> IssueSeverity {
        match self {
            IssueCategory::ApiMismatch => IssueSeverity::Critical,
            IssueCategory::CompilationError => IssueSeverity::Critical,
            IssueCategory::VersionInconsistency => IssueSeverity::Warning,
            IssueCategory::BrokenLink => IssueSeverity::Warning,
            IssueCategory::DeprecatedApi => IssueSeverity::Warning,
            IssueCategory::InvalidImport => IssueSeverity::Critical,
            IssueCategory::ConfigurationError => IssueSeverity::Warning,
            IssueCategory::AsyncPatternError => IssueSeverity::Warning,
            IssueCategory::InvalidFeatureFlag => IssueSeverity::Warning,
            IssueCategory::InvalidCrateName => IssueSeverity::Warning,
            IssueCategory::MissingDocumentation => IssueSeverity::Info,
            IssueCategory::QualityIssue => IssueSeverity::Info,
            IssueCategory::ProcessingError => IssueSeverity::Critical,
            IssueCategory::ValidationError => IssueSeverity::Warning,
        }
    }
}

/// Actionable recommendation for improving documentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    /// Unique identifier for this recommendation
    pub id: String,
    /// Type of recommendation
    pub recommendation_type: RecommendationType,
    /// Priority level (1 = highest, 5 = lowest)
    pub priority: u8,
    /// Title of the recommendation
    pub title: String,
    /// Detailed description
    pub description: String,
    /// Files that would be affected by this recommendation
    pub affected_files: Vec<PathBuf>,
    /// Estimated effort to implement (in hours)
    pub estimated_effort_hours: Option<f32>,
    /// Issues that would be resolved by this recommendation
    pub resolves_issues: Vec<String>,
}

/// Types of recommendations that can be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecommendationType {
    /// Fix a specific issue
    FixIssue,
    /// Improve documentation structure
    StructuralImprovement,
    /// Add missing documentation
    AddDocumentation,
    /// Update outdated content
    UpdateContent,
    /// Improve code examples
    ImproveExamples,
    /// Enhance cross-references
    EnhanceCrossReferences,
    /// Process improvement
    ProcessImprovement,
}

/// Configuration for audit reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditReportConfig {
    /// Minimum severity level to include in reports
    pub min_severity: IssueSeverity,
    /// Whether to include suggestions in the report
    pub include_suggestions: bool,
    /// Whether to include code snippets in issues
    pub include_code_snippets: bool,
    /// Maximum number of issues to include per file
    pub max_issues_per_file: Option<usize>,
    /// Whether to include statistics in the report
    pub include_statistics: bool,
    /// Whether to include recommendations
    pub include_recommendations: bool,
}

impl Default for AuditReportConfig {
    fn default() -> Self {
        Self {
            min_severity: IssueSeverity::Info,
            include_suggestions: true,
            include_code_snippets: true,
            max_issues_per_file: None,
            include_statistics: true,
            include_recommendations: true,
        }
    }
}

impl AuditReport {
    /// Create a new audit report with the given configuration.
    pub fn new(config: AuditReportConfig) -> Self {
        Self {
            summary: AuditSummary::default(),
            file_results: Vec::new(),
            issues: Vec::new(),
            recommendations: Vec::new(),
            timestamp: Utc::now(),
            audit_config: config,
        }
    }

    /// Add a file result to the report.
    pub fn add_file_result(&mut self, file_result: FileAuditResult) {
        // Add issues from this file to the main issues list
        self.issues.extend(file_result.issues.clone());
        self.file_results.push(file_result);
    }

    /// Add an issue to the report.
    pub fn add_issue(&mut self, issue: AuditIssue) {
        // Check if this issue meets the minimum severity threshold
        if issue.severity >= self.audit_config.min_severity {
            self.issues.push(issue);
        }
    }

    /// Add a recommendation to the report.
    pub fn add_recommendation(&mut self, recommendation: Recommendation) {
        if self.audit_config.include_recommendations {
            self.recommendations.push(recommendation);
        }
    }

    /// Calculate and update the summary statistics.
    pub fn calculate_summary(&mut self) {
        let total_files = self.file_results.len();
        let files_with_issues = self.file_results.iter().filter(|f| f.issues_count > 0).count();

        let total_issues = self.issues.len();
        let critical_issues =
            self.issues.iter().filter(|i| i.severity == IssueSeverity::Critical).count();
        let warning_issues =
            self.issues.iter().filter(|i| i.severity == IssueSeverity::Warning).count();
        let info_issues = self.issues.iter().filter(|i| i.severity == IssueSeverity::Info).count();

        // Calculate coverage percentage (files without critical issues / total files)
        let files_without_critical = self
            .file_results
            .iter()
            .filter(|f| !f.issues.iter().any(|i| i.severity == IssueSeverity::Critical))
            .count();
        let coverage_percentage = if total_files > 0 {
            (files_without_critical as f64 / total_files as f64) * 100.0
        } else {
            100.0
        };

        let average_issues_per_file =
            if total_files > 0 { total_issues as f64 / total_files as f64 } else { 0.0 };

        // Find most common issue category
        let mut category_counts: HashMap<IssueCategory, usize> = HashMap::new();
        for issue in &self.issues {
            *category_counts.entry(issue.category).or_insert(0) += 1;
        }
        let most_common_issue = category_counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(category, _)| category);

        // Find most problematic files (top 5)
        let mut file_issue_counts: Vec<_> = self
            .file_results
            .iter()
            .map(|f| ProblematicFile {
                path: f.file_path.clone(),
                issue_count: f.issues_count,
                max_severity: f
                    .issues
                    .iter()
                    .map(|i| i.severity)
                    .max()
                    .unwrap_or(IssueSeverity::Info),
            })
            .collect();
        file_issue_counts.sort_by(|a, b| b.issue_count.cmp(&a.issue_count));
        file_issue_counts.truncate(5);

        self.summary = AuditSummary {
            total_files,
            files_with_issues,
            total_issues,
            critical_issues,
            warning_issues,
            info_issues,
            coverage_percentage,
            average_issues_per_file,
            most_common_issue,
            problematic_files: file_issue_counts,
        };
    }

    /// Check if the audit passed (no critical issues).
    pub fn passed(&self) -> bool {
        self.summary.critical_issues == 0
    }

    /// Get issues by category.
    pub fn issues_by_category(&self) -> HashMap<IssueCategory, Vec<&AuditIssue>> {
        let mut categorized = HashMap::new();
        for issue in &self.issues {
            categorized.entry(issue.category).or_insert_with(Vec::new).push(issue);
        }
        categorized
    }

    /// Get issues by severity.
    pub fn issues_by_severity(&self) -> HashMap<IssueSeverity, Vec<&AuditIssue>> {
        let mut by_severity = HashMap::new();
        for issue in &self.issues {
            by_severity.entry(issue.severity).or_insert_with(Vec::new).push(issue);
        }
        by_severity
    }

    /// Get issues for a specific file.
    pub fn issues_for_file(&self, file_path: &PathBuf) -> Vec<&AuditIssue> {
        self.issues.iter().filter(|issue| &issue.file_path == file_path).collect()
    }
}

impl Default for AuditSummary {
    fn default() -> Self {
        Self {
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
        }
    }
}

impl AuditIssue {
    /// Create a new audit issue with the given parameters.
    pub fn new(file_path: PathBuf, category: IssueCategory, message: String) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let severity = category.default_severity();

        Self {
            id,
            file_path,
            line_number: None,
            column_number: None,
            severity,
            category,
            message,
            suggestion: None,
            context: None,
            code_snippet: None,
            related_issues: Vec::new(),
        }
    }

    /// Set the line number for this issue.
    pub fn with_line_number(mut self, line_number: usize) -> Self {
        self.line_number = Some(line_number);
        self
    }

    /// Set the column number for this issue.
    pub fn with_column_number(mut self, column_number: usize) -> Self {
        self.column_number = Some(column_number);
        self
    }

    /// Set the severity for this issue.
    pub fn with_severity(mut self, severity: IssueSeverity) -> Self {
        self.severity = severity;
        self
    }

    /// Set a suggestion for fixing this issue.
    pub fn with_suggestion(mut self, suggestion: String) -> Self {
        self.suggestion = Some(suggestion);
        self
    }

    /// Set additional context for this issue.
    pub fn with_context(mut self, context: String) -> Self {
        self.context = Some(context);
        self
    }

    /// Set a code snippet showing the problematic area.
    pub fn with_code_snippet(mut self, code_snippet: String) -> Self {
        self.code_snippet = Some(code_snippet);
        self
    }

    /// Add a related issue ID.
    pub fn with_related_issue(mut self, issue_id: String) -> Self {
        self.related_issues.push(issue_id);
        self
    }
}

impl Recommendation {
    /// Create a new recommendation.
    pub fn new(
        recommendation_type: RecommendationType,
        title: String,
        description: String,
    ) -> Self {
        let id = uuid::Uuid::new_v4().to_string();

        Self {
            id,
            recommendation_type,
            priority: 3, // Default to medium priority
            title,
            description,
            affected_files: Vec::new(),
            estimated_effort_hours: None,
            resolves_issues: Vec::new(),
        }
    }

    /// Set the priority for this recommendation (1 = highest, 5 = lowest).
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority.clamp(1, 5);
        self
    }

    /// Add an affected file.
    pub fn with_affected_file(mut self, file_path: PathBuf) -> Self {
        self.affected_files.push(file_path);
        self
    }

    /// Set the estimated effort in hours.
    pub fn with_estimated_effort(mut self, hours: f32) -> Self {
        self.estimated_effort_hours = Some(hours);
        self
    }

    /// Add an issue that this recommendation would resolve.
    pub fn resolves_issue(mut self, issue_id: String) -> Self {
        self.resolves_issues.push(issue_id);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_report_creation() {
        let config = AuditReportConfig::default();
        let report = AuditReport::new(config);

        assert_eq!(report.summary.total_files, 0);
        assert_eq!(report.issues.len(), 0);
        assert!(report.passed());
    }

    #[test]
    fn test_issue_creation() {
        let issue = AuditIssue::new(
            PathBuf::from("test.md"),
            IssueCategory::ApiMismatch,
            "Test issue".to_string(),
        )
        .with_line_number(42)
        .with_suggestion("Fix this".to_string());

        assert_eq!(issue.file_path, PathBuf::from("test.md"));
        assert_eq!(issue.category, IssueCategory::ApiMismatch);
        assert_eq!(issue.severity, IssueSeverity::Critical);
        assert_eq!(issue.line_number, Some(42));
        assert_eq!(issue.suggestion, Some("Fix this".to_string()));
    }

    #[test]
    fn test_issue_category_descriptions() {
        assert_eq!(
            IssueCategory::ApiMismatch.description(),
            "API reference doesn't match implementation"
        );
        assert_eq!(IssueCategory::CompilationError.description(), "Code example fails to compile");
    }

    #[test]
    fn test_issue_category_default_severity() {
        assert_eq!(IssueCategory::ApiMismatch.default_severity(), IssueSeverity::Critical);
        assert_eq!(IssueCategory::VersionInconsistency.default_severity(), IssueSeverity::Warning);
        assert_eq!(IssueCategory::MissingDocumentation.default_severity(), IssueSeverity::Info);
    }

    #[test]
    fn test_recommendation_creation() {
        let rec = Recommendation::new(
            RecommendationType::FixIssue,
            "Fix API references".to_string(),
            "Update all API references to match current implementation".to_string(),
        )
        .with_priority(1)
        .with_estimated_effort(2.5);

        assert_eq!(rec.recommendation_type, RecommendationType::FixIssue);
        assert_eq!(rec.priority, 1);
        assert_eq!(rec.estimated_effort_hours, Some(2.5));
    }

    #[test]
    fn test_audit_summary_calculation() {
        let mut report = AuditReport::new(AuditReportConfig::default());

        // Add some test file results
        let file1 = FileAuditResult {
            file_path: PathBuf::from("file1.md"),
            file_hash: "hash1".to_string(),
            last_modified: Utc::now(),
            issues_count: 2,
            issues: vec![
                AuditIssue::new(
                    PathBuf::from("file1.md"),
                    IssueCategory::ApiMismatch,
                    "Issue 1".to_string(),
                ),
                AuditIssue::new(
                    PathBuf::from("file1.md"),
                    IssueCategory::VersionInconsistency,
                    "Issue 2".to_string(),
                ),
            ],
            passed: false,
            audit_duration_ms: 100,
        };

        let file2 = FileAuditResult {
            file_path: PathBuf::from("file2.md"),
            file_hash: "hash2".to_string(),
            last_modified: Utc::now(),
            issues_count: 0,
            issues: vec![],
            passed: true,
            audit_duration_ms: 50,
        };

        report.add_file_result(file1);
        report.add_file_result(file2);
        report.calculate_summary();

        assert_eq!(report.summary.total_files, 2);
        assert_eq!(report.summary.files_with_issues, 1);
        assert_eq!(report.summary.total_issues, 2);
        assert_eq!(report.summary.critical_issues, 1);
        assert_eq!(report.summary.warning_issues, 1);
        assert_eq!(report.summary.coverage_percentage, 50.0); // 1 file without critical issues out of 2
    }

    #[test]
    fn test_issues_by_category() {
        let mut report = AuditReport::new(AuditReportConfig::default());

        report.add_issue(AuditIssue::new(
            PathBuf::from("test.md"),
            IssueCategory::ApiMismatch,
            "API issue".to_string(),
        ));

        report.add_issue(AuditIssue::new(
            PathBuf::from("test.md"),
            IssueCategory::ApiMismatch,
            "Another API issue".to_string(),
        ));

        report.add_issue(AuditIssue::new(
            PathBuf::from("test.md"),
            IssueCategory::CompilationError,
            "Compilation issue".to_string(),
        ));

        let by_category = report.issues_by_category();
        assert_eq!(by_category.get(&IssueCategory::ApiMismatch).unwrap().len(), 2);
        assert_eq!(by_category.get(&IssueCategory::CompilationError).unwrap().len(), 1);
    }

    #[test]
    fn test_report_generator_json() {
        let mut report = AuditReport::new(AuditReportConfig::default());

        report.add_issue(AuditIssue::new(
            PathBuf::from("test.md"),
            IssueCategory::ApiMismatch,
            "Test issue".to_string(),
        ));

        report.calculate_summary();

        let generator = ReportGenerator::new(OutputFormat::Json);
        let json_output = generator.generate_report_string(&report).unwrap();

        // Verify it's valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&json_output).unwrap();
        assert!(parsed.get("summary").is_some());
        assert!(parsed.get("issues").is_some());
        assert!(parsed.get("timestamp").is_some());
    }

    #[test]
    fn test_report_generator_markdown() {
        let mut report = AuditReport::new(AuditReportConfig::default());

        report.add_issue(
            AuditIssue::new(
                PathBuf::from("test.md"),
                IssueCategory::CompilationError,
                "Compilation failed".to_string(),
            )
            .with_line_number(42),
        );

        report.calculate_summary();

        let generator = ReportGenerator::new(OutputFormat::Markdown);
        let markdown_output = generator.generate_report_string(&report).unwrap();

        // Verify markdown structure
        assert!(markdown_output.contains("# Documentation Audit Report"));
        assert!(markdown_output.contains("## Executive Summary"));
        assert!(markdown_output.contains("test.md"));
        assert!(markdown_output.contains("line 42"));
    }

    #[test]
    fn test_report_generator_console() {
        let mut report = AuditReport::new(AuditReportConfig::default());

        report.add_issue(AuditIssue::new(
            PathBuf::from("test.md"),
            IssueCategory::VersionInconsistency,
            "Version mismatch".to_string(),
        ));

        report.calculate_summary();

        let generator = ReportGenerator::new(OutputFormat::Console);
        let console_output = generator.generate_report_string(&report).unwrap();

        // Verify console structure
        assert!(console_output.contains("DOCUMENTATION AUDIT REPORT"));
        assert!(console_output.contains("SUMMARY"));
        assert!(console_output.contains("Total Files:"));
        assert!(console_output.contains("🟡 WARNING"));
    }

    #[test]
    fn test_wrap_text() {
        use super::wrap_text;

        let text = "This is a very long line that should be wrapped at the specified width";
        let wrapped = wrap_text(text, 20);

        for line in wrapped.lines() {
            assert!(line.len() <= 20);
        }

        // Should preserve all words
        let original_words: Vec<&str> = text.split_whitespace().collect();
        let wrapped_words: Vec<&str> = wrapped.split_whitespace().collect();
        assert_eq!(original_words, wrapped_words);
    }
}
/// Output formats supported by the report generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputFormat {
    /// JSON format for programmatic consumption
    Json,
    /// Markdown format for human-readable reports
    Markdown,
    /// Console format for interactive use
    Console,
}

impl From<crate::config::OutputFormat> for OutputFormat {
    fn from(config_format: crate::config::OutputFormat) -> Self {
        match config_format {
            crate::config::OutputFormat::Console => OutputFormat::Console,
            crate::config::OutputFormat::Json => OutputFormat::Json,
            crate::config::OutputFormat::Markdown => OutputFormat::Markdown,
        }
    }
}

/// Report generator that can output audit reports in multiple formats.
pub struct ReportGenerator {
    output_format: OutputFormat,
    config: AuditReportConfig,
}

impl ReportGenerator {
    /// Create a new report generator with the specified output format.
    pub fn new(output_format: OutputFormat) -> Self {
        Self { output_format, config: AuditReportConfig::default() }
    }

    /// Create a new report generator with custom configuration.
    pub fn with_config(output_format: OutputFormat, config: AuditReportConfig) -> Self {
        Self { output_format, config }
    }

    /// Generate a report and write it to the provided writer.
    pub fn generate_report<W: IoWrite>(&self, report: &AuditReport, writer: &mut W) -> Result<()> {
        match self.output_format {
            OutputFormat::Json => self.generate_json_report(report, writer),
            OutputFormat::Markdown => self.generate_markdown_report(report, writer),
            OutputFormat::Console => self.generate_console_report(report, writer),
        }
    }

    /// Generate a report as a string.
    pub fn generate_report_string(&self, report: &AuditReport) -> Result<String> {
        let mut buffer = Vec::new();
        self.generate_report(report, &mut buffer)?;
        String::from_utf8(buffer).map_err(|e| AuditError::ReportGeneration {
            details: format!("UTF-8 conversion error: {}", e),
        })
    }

    /// Generate JSON format report.
    fn generate_json_report<W: IoWrite>(&self, report: &AuditReport, writer: &mut W) -> Result<()> {
        let json = if self.config.include_statistics {
            serde_json::to_string_pretty(report)
        } else {
            // Create a simplified report without detailed statistics
            let simplified = SimplifiedReport {
                summary: &report.summary,
                issues: &report.issues,
                recommendations: if self.config.include_recommendations {
                    Some(&report.recommendations)
                } else {
                    None
                },
                timestamp: report.timestamp,
            };
            serde_json::to_string_pretty(&simplified)
        };

        let json = json.map_err(|e| AuditError::ReportGeneration {
            details: format!("JSON serialization error: {}", e),
        })?;
        writer
            .write_all(json.as_bytes())
            .map_err(|e| AuditError::ReportGeneration { details: format!("Write error: {}", e) })?;

        Ok(())
    }

    /// Generate Markdown format report.
    fn generate_markdown_report<W: IoWrite>(
        &self,
        report: &AuditReport,
        writer: &mut W,
    ) -> Result<()> {
        let mut output = String::new();

        // Title and summary
        writeln!(output, "# Documentation Audit Report").unwrap();
        writeln!(output).unwrap();
        writeln!(output, "**Generated:** {}", report.timestamp.format("%Y-%m-%d %H:%M:%S UTC"))
            .unwrap();
        writeln!(output, "**Status:** {}", if report.passed() { "✅ PASSED" } else { "❌ FAILED" })
            .unwrap();
        writeln!(output).unwrap();

        // Executive summary
        writeln!(output, "## Executive Summary").unwrap();
        writeln!(output).unwrap();
        writeln!(output, "- **Total Files Audited:** {}", report.summary.total_files).unwrap();
        writeln!(output, "- **Files with Issues:** {}", report.summary.files_with_issues).unwrap();
        writeln!(output, "- **Total Issues:** {}", report.summary.total_issues).unwrap();
        writeln!(output, "- **Critical Issues:** {}", report.summary.critical_issues).unwrap();
        writeln!(output, "- **Warning Issues:** {}", report.summary.warning_issues).unwrap();
        writeln!(output, "- **Info Issues:** {}", report.summary.info_issues).unwrap();
        writeln!(
            output,
            "- **Documentation Coverage:** {:.1}%",
            report.summary.coverage_percentage
        )
        .unwrap();
        writeln!(output).unwrap();

        // Issues by category
        if !report.issues.is_empty() {
            writeln!(output, "## Issues by Category").unwrap();
            writeln!(output).unwrap();

            let issues_by_category = report.issues_by_category();
            for (category, issues) in issues_by_category {
                writeln!(output, "### {} ({} issues)", category.description(), issues.len())
                    .unwrap();
                writeln!(output).unwrap();

                for issue in
                    issues.iter().take(self.config.max_issues_per_file.unwrap_or(usize::MAX))
                {
                    let severity_icon = match issue.severity {
                        IssueSeverity::Critical => "🔴",
                        IssueSeverity::Warning => "🟡",
                        IssueSeverity::Info => "🔵",
                    };

                    write!(
                        output,
                        "- {} **{}**: {}",
                        severity_icon,
                        issue.file_path.display(),
                        issue.message
                    )
                    .unwrap();

                    if let Some(line) = issue.line_number {
                        write!(output, " (line {})", line).unwrap();
                    }
                    writeln!(output).unwrap();

                    if self.config.include_suggestions {
                        if let Some(suggestion) = &issue.suggestion {
                            writeln!(output, "  - *Suggestion:* {}", suggestion).unwrap();
                        }
                    }

                    if self.config.include_code_snippets {
                        if let Some(snippet) = &issue.code_snippet {
                            writeln!(output, "  ```").unwrap();
                            writeln!(output, "  {}", snippet).unwrap();
                            writeln!(output, "  ```").unwrap();
                        }
                    }
                }
                writeln!(output).unwrap();
            }
        }

        // Most problematic files
        if !report.summary.problematic_files.is_empty() {
            writeln!(output, "## Most Problematic Files").unwrap();
            writeln!(output).unwrap();

            for (i, file) in report.summary.problematic_files.iter().enumerate() {
                let severity_icon = match file.max_severity {
                    IssueSeverity::Critical => "🔴",
                    IssueSeverity::Warning => "🟡",
                    IssueSeverity::Info => "🔵",
                };
                writeln!(
                    output,
                    "{}. {} {} ({} issues)",
                    i + 1,
                    severity_icon,
                    file.path.display(),
                    file.issue_count
                )
                .unwrap();
            }
            writeln!(output).unwrap();
        }

        // Recommendations
        if self.config.include_recommendations && !report.recommendations.is_empty() {
            writeln!(output, "## Recommendations").unwrap();
            writeln!(output).unwrap();

            let mut sorted_recommendations = report.recommendations.clone();
            sorted_recommendations.sort_by_key(|r| r.priority);

            for rec in sorted_recommendations {
                let priority_text = match rec.priority {
                    1 => "🔴 High",
                    2 => "🟡 Medium-High",
                    3 => "🟡 Medium",
                    4 => "🔵 Medium-Low",
                    5 => "🔵 Low",
                    _ => "🔵 Low",
                };

                writeln!(output, "### {} - {}", priority_text, rec.title).unwrap();
                writeln!(output).unwrap();
                writeln!(output, "{}", rec.description).unwrap();

                if let Some(effort) = rec.estimated_effort_hours {
                    writeln!(output, "**Estimated Effort:** {:.1} hours", effort).unwrap();
                }

                if !rec.affected_files.is_empty() {
                    writeln!(output, "**Affected Files:**").unwrap();
                    for file in &rec.affected_files {
                        writeln!(output, "- {}", file.display()).unwrap();
                    }
                }
                writeln!(output).unwrap();
            }
        }

        writer
            .write_all(output.as_bytes())
            .map_err(|e| AuditError::ReportGeneration { details: format!("Write error: {}", e) })?;

        Ok(())
    }

    /// Generate console format report.
    fn generate_console_report<W: IoWrite>(
        &self,
        report: &AuditReport,
        writer: &mut W,
    ) -> Result<()> {
        let mut output = String::new();

        // Header
        writeln!(
            output,
            "╔══════════════════════════════════════════════════════════════════════════════╗"
        )
        .unwrap();
        writeln!(
            output,
            "║                          DOCUMENTATION AUDIT REPORT                         ║"
        )
        .unwrap();
        writeln!(
            output,
            "╚══════════════════════════════════════════════════════════════════════════════╝"
        )
        .unwrap();
        writeln!(output).unwrap();

        // Status
        let status = if report.passed() { "✅ PASSED" } else { "❌ FAILED" };
        writeln!(output, "Status: {}", status).unwrap();
        writeln!(output, "Generated: {}", report.timestamp.format("%Y-%m-%d %H:%M:%S UTC"))
            .unwrap();
        writeln!(output).unwrap();

        // Summary box
        writeln!(
            output,
            "┌─ SUMMARY ─────────────────────────────────────────────────────────────────────┐"
        )
        .unwrap();
        writeln!(
            output,
            "│ Total Files:        {:>8}                                                │",
            report.summary.total_files
        )
        .unwrap();
        writeln!(
            output,
            "│ Files with Issues:  {:>8}                                                │",
            report.summary.files_with_issues
        )
        .unwrap();
        writeln!(
            output,
            "│ Total Issues:       {:>8}                                                │",
            report.summary.total_issues
        )
        .unwrap();
        writeln!(
            output,
            "│ Critical Issues:    {:>8}                                                │",
            report.summary.critical_issues
        )
        .unwrap();
        writeln!(
            output,
            "│ Warning Issues:     {:>8}                                                │",
            report.summary.warning_issues
        )
        .unwrap();
        writeln!(
            output,
            "│ Info Issues:        {:>8}                                                │",
            report.summary.info_issues
        )
        .unwrap();
        writeln!(
            output,
            "│ Coverage:           {:>7.1}%                                               │",
            report.summary.coverage_percentage
        )
        .unwrap();
        writeln!(
            output,
            "└───────────────────────────────────────────────────────────────────────────────┘"
        )
        .unwrap();
        writeln!(output).unwrap();

        // Issues by severity
        if report.summary.total_issues > 0 {
            writeln!(output, "ISSUES BY SEVERITY:").unwrap();
            writeln!(
                output,
                "─────────────────────────────────────────────────────────────────────────────"
            )
            .unwrap();

            let issues_by_severity = report.issues_by_severity();

            if let Some(critical_issues) = issues_by_severity.get(&IssueSeverity::Critical) {
                writeln!(output, "🔴 CRITICAL ({}):", critical_issues.len()).unwrap();
                for issue in critical_issues.iter().take(5) {
                    writeln!(output, "   {} - {}", issue.file_path.display(), issue.message)
                        .unwrap();
                }
                if critical_issues.len() > 5 {
                    writeln!(output, "   ... and {} more", critical_issues.len() - 5).unwrap();
                }
                writeln!(output).unwrap();
            }

            if let Some(warning_issues) = issues_by_severity.get(&IssueSeverity::Warning) {
                writeln!(output, "🟡 WARNING ({}):", warning_issues.len()).unwrap();
                for issue in warning_issues.iter().take(3) {
                    writeln!(output, "   {} - {}", issue.file_path.display(), issue.message)
                        .unwrap();
                }
                if warning_issues.len() > 3 {
                    writeln!(output, "   ... and {} more", warning_issues.len() - 3).unwrap();
                }
                writeln!(output).unwrap();
            }

            if let Some(info_issues) = issues_by_severity.get(&IssueSeverity::Info) {
                writeln!(output, "🔵 INFO ({}):", info_issues.len()).unwrap();
                for issue in info_issues.iter().take(2) {
                    writeln!(output, "   {} - {}", issue.file_path.display(), issue.message)
                        .unwrap();
                }
                if info_issues.len() > 2 {
                    writeln!(output, "   ... and {} more", info_issues.len() - 2).unwrap();
                }
                writeln!(output).unwrap();
            }
        }

        // Most problematic files
        if !report.summary.problematic_files.is_empty() {
            writeln!(output, "MOST PROBLEMATIC FILES:").unwrap();
            writeln!(
                output,
                "─────────────────────────────────────────────────────────────────────────────"
            )
            .unwrap();

            for (i, file) in report.summary.problematic_files.iter().enumerate() {
                let severity_icon = match file.max_severity {
                    IssueSeverity::Critical => "🔴",
                    IssueSeverity::Warning => "🟡",
                    IssueSeverity::Info => "🔵",
                };
                writeln!(
                    output,
                    "{}. {} {} ({} issues)",
                    i + 1,
                    severity_icon,
                    file.path.display(),
                    file.issue_count
                )
                .unwrap();
            }
            writeln!(output).unwrap();
        }

        // Top recommendations
        if self.config.include_recommendations && !report.recommendations.is_empty() {
            writeln!(output, "TOP RECOMMENDATIONS:").unwrap();
            writeln!(
                output,
                "─────────────────────────────────────────────────────────────────────────────"
            )
            .unwrap();

            let mut sorted_recommendations = report.recommendations.clone();
            sorted_recommendations.sort_by_key(|r| r.priority);

            for rec in sorted_recommendations.iter().take(3) {
                let priority_text = match rec.priority {
                    1 => "🔴 HIGH",
                    2 => "🟡 MED-HIGH",
                    3 => "🟡 MEDIUM",
                    4 => "🔵 MED-LOW",
                    5 => "🔵 LOW",
                    _ => "🔵 LOW",
                };

                writeln!(output, "{}: {}", priority_text, rec.title).unwrap();

                // Wrap description to fit console width
                let wrapped_desc = wrap_text(&rec.description, 75);
                for line in wrapped_desc.lines() {
                    writeln!(output, "   {}", line).unwrap();
                }
                writeln!(output).unwrap();
            }

            if report.recommendations.len() > 3 {
                writeln!(
                    output,
                    "... and {} more recommendations",
                    report.recommendations.len() - 3
                )
                .unwrap();
                writeln!(output).unwrap();
            }
        }

        // Footer
        writeln!(
            output,
            "─────────────────────────────────────────────────────────────────────────────"
        )
        .unwrap();
        if report.passed() {
            writeln!(output, "✅ Audit completed successfully! No critical issues found.").unwrap();
        } else {
            writeln!(output, "❌ Audit failed. Please address critical issues before proceeding.")
                .unwrap();
        }

        writer
            .write_all(output.as_bytes())
            .map_err(|e| AuditError::ReportGeneration { details: format!("Write error: {}", e) })?;

        Ok(())
    }

    /// Save a report to a file.
    pub fn save_to_file(&self, report: &AuditReport, file_path: &std::path::Path) -> Result<()> {
        use std::fs::File;
        use std::io::BufWriter;

        let file = File::create(file_path).map_err(|e| AuditError::IoError {
            path: file_path.to_path_buf(),
            details: format!("Failed to create report file: {}", e),
        })?;

        let mut writer = BufWriter::new(file);
        self.generate_report(report, &mut writer)?;

        Ok(())
    }
}

/// Simplified report structure for JSON output when statistics are disabled.
#[derive(Serialize)]
struct SimplifiedReport<'a> {
    summary: &'a AuditSummary,
    issues: &'a [AuditIssue],
    #[serde(skip_serializing_if = "Option::is_none")]
    recommendations: Option<&'a [Recommendation]>,
    timestamp: DateTime<Utc>,
}

/// Wrap text to fit within the specified width.
fn wrap_text(text: &str, width: usize) -> String {
    let mut result = String::new();
    let mut current_line = String::new();

    for word in text.split_whitespace() {
        if current_line.len() + word.len() + 1 > width && !current_line.is_empty() {
            result.push_str(&current_line);
            result.push('\n');
            current_line.clear();
        }

        if !current_line.is_empty() {
            current_line.push(' ');
        }
        current_line.push_str(word);
    }

    if !current_line.is_empty() {
        result.push_str(&current_line);
    }

    result
}
