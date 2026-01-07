//! Demonstration of multiple output formats for documentation audit reports.
//!
//! This example shows how to generate audit reports in JSON, Markdown, and Console formats.

use adk_doc_audit::reporter::OutputFormat;
use adk_doc_audit::{
    AuditIssue, AuditReport, AuditReportConfig, IssueCategory, IssueSeverity, Recommendation,
    RecommendationType, ReportGenerator,
};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a sample audit report with some issues
    let report = create_sample_report();

    println!("=== Documentation Audit Output Formats Demo ===\n");

    // Demonstrate JSON output
    println!("1. JSON Format (for programmatic consumption):");
    println!("{}", "─".repeat(60));
    let json_generator = ReportGenerator::new(OutputFormat::Json);
    let json_output = json_generator.generate_report_string(&report)?;
    println!("{}\n", json_output);

    // Demonstrate Markdown output
    println!("2. Markdown Format (for human-readable reports):");
    println!("{}", "─".repeat(60));
    let markdown_generator = ReportGenerator::new(OutputFormat::Markdown);
    let markdown_output = markdown_generator.generate_report_string(&report)?;
    println!("{}\n", markdown_output);

    // Demonstrate Console output
    println!("3. Console Format (for interactive use):");
    println!("{}", "─".repeat(60));
    let console_generator = ReportGenerator::new(OutputFormat::Console);
    let console_output = console_generator.generate_report_string(&report)?;
    println!("{}", console_output);

    Ok(())
}

fn create_sample_report() -> AuditReport {
    let mut report = AuditReport::new(AuditReportConfig::default());

    // Add some sample issues
    let api_issue = AuditIssue::new(
        PathBuf::from("docs/getting-started.md"),
        IssueCategory::ApiMismatch,
        "Function signature for `create_agent()` doesn't match current implementation".to_string(),
    )
    .with_line_number(42)
    .with_severity(IssueSeverity::Critical)
    .with_suggestion(
        "Update signature to `create_agent(config: AgentConfig) -> Result<Agent>`".to_string(),
    )
    .with_code_snippet("let agent = create_agent();".to_string());

    let version_issue = AuditIssue::new(
        PathBuf::from("docs/installation.md"),
        IssueCategory::VersionInconsistency,
        "Referenced version 0.1.5 but current version is 0.2.0".to_string(),
    )
    .with_line_number(15)
    .with_severity(IssueSeverity::Warning)
    .with_suggestion("Update to version 0.2.0".to_string());

    let compilation_issue = AuditIssue::new(
        PathBuf::from("docs/examples/basic-usage.md"),
        IssueCategory::CompilationError,
        "Code example fails to compile: missing import for `tokio`".to_string(),
    )
    .with_line_number(28)
    .with_severity(IssueSeverity::Critical)
    .with_suggestion("Add `use tokio;` at the top of the example".to_string());

    let link_issue = AuditIssue::new(
        PathBuf::from("docs/api-reference.md"),
        IssueCategory::BrokenLink,
        "Link to 'advanced-features.md' is broken".to_string(),
    )
    .with_line_number(67)
    .with_severity(IssueSeverity::Warning)
    .with_suggestion("Update link to 'features/advanced.md'".to_string());

    report.add_issue(api_issue);
    report.add_issue(version_issue);
    report.add_issue(compilation_issue);
    report.add_issue(link_issue);

    // Add some recommendations
    let fix_api_rec = Recommendation::new(
        RecommendationType::FixIssue,
        "Update API References".to_string(),
        "Review and update all API references to match current implementation signatures"
            .to_string(),
    )
    .with_priority(1)
    .with_estimated_effort(3.0)
    .with_affected_file(PathBuf::from("docs/getting-started.md"))
    .resolves_issue("api-issue-1".to_string());

    let version_rec = Recommendation::new(
        RecommendationType::UpdateContent,
        "Version Consistency Check".to_string(),
        "Implement automated version checking to prevent version inconsistencies".to_string(),
    )
    .with_priority(2)
    .with_estimated_effort(1.5);

    report.add_recommendation(fix_api_rec);
    report.add_recommendation(version_rec);

    // Calculate summary statistics
    report.calculate_summary();

    report
}
