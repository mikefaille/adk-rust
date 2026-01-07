use adk_doc_audit::{
    AuditCli, AuditCommand, AuditConfig, AuditError, AuditOrchestrator, IssueSeverity, Result,
    reporter::ReportGenerator,
};
use std::path::{Path, PathBuf};
use std::process;
use std::time::Duration;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Initialize tracing
    let result = run().await;

    match result {
        Ok(exit_code) => process::exit(exit_code),
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    }
}

async fn run() -> Result<i32> {
    let cli = AuditCli::parse_args();

    // Initialize logging based on verbosity
    init_logging(cli.verbose, cli.quiet);

    info!("Starting adk-doc-audit v{}", adk_doc_audit::VERSION);

    match &cli.command {
        AuditCommand::Audit { .. } => {
            let config = cli.to_config()?;
            let (no_fail, max_issues, ci_mode) = cli.get_ci_options().unwrap_or((false, 0, false));
            let single_crate_options = cli.get_single_crate_options();
            run_audit_command(config, &cli, no_fail, max_issues, ci_mode, single_crate_options)
                .await
        }
        AuditCommand::Crate { .. } => {
            let config = cli.to_config()?;
            let crate_name = cli.get_crate_name().unwrap();
            run_crate_audit_command(config, &cli, crate_name).await
        }
        AuditCommand::Incremental { .. } => {
            let config = cli.to_config()?;
            let changed_files = cli.get_changed_files().unwrap_or(&[]);
            run_incremental_command(config, changed_files).await
        }
        AuditCommand::Validate { .. } => {
            let config = cli.to_config()?;
            let file_path = cli.get_validate_file().unwrap();
            run_validate_command(config, file_path).await
        }
        AuditCommand::Init { .. } => {
            let config = cli.to_config()?;
            let config_path = cli.get_init_config_path().unwrap();
            run_init_command(config, config_path).await
        }
        AuditCommand::Stats { .. } => {
            let config = cli.to_config()?;
            let limit = cli.get_stats_limit().unwrap_or(10);
            run_stats_command(config, limit).await
        }
    }
}

fn init_logging(verbose: bool, quiet: bool) {
    let level = if quiet {
        tracing::Level::ERROR
    } else if verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_level(true)
                .with_filter(tracing_subscriber::filter::LevelFilter::from_level(level)),
        )
        .init();
}

async fn run_audit_command(
    config: AuditConfig,
    cli: &AuditCli,
    no_fail: bool,
    max_issues: usize,
    ci_mode: bool,
    single_crate_options: Option<(Option<&String>, Option<&PathBuf>)>,
) -> Result<i32> {
    // Single crate configuration is now handled in CLI, just log what we're doing
    if single_crate_options.is_some() {
        info!("Running audit for single crate");
    } else {
        info!("Running full audit on workspace: {}", config.workspace_path.display());
    }

    info!("Documentation path: {}", config.docs_path.display());
    debug!("Configuration: {:?}", config);
    debug!("CI/CD options: no_fail={}, max_issues={}, ci_mode={}", no_fail, max_issues, ci_mode);

    // Create orchestrator and run audit
    let mut orchestrator = AuditOrchestrator::new(config.clone()).await?;
    let report = orchestrator.run_full_audit().await?;

    // Apply max_issues limit if specified
    let total_issues = report.summary.total_issues;
    let reported_issues =
        if max_issues > 0 && total_issues > max_issues { max_issues } else { total_issues };

    // Output results based on format and CI mode
    if ci_mode {
        // GitHub Actions compatible output
        println!("::group::Documentation Audit Configuration");
        println!("workspace={}", config.workspace_path.display());
        println!("docs={}", config.docs_path.display());
        println!("severity_threshold={:?}", config.severity_threshold);
        println!("fail_on_critical={}", config.fail_on_critical);
        println!("::endgroup::");

        println!("::group::Audit Results");
        println!("critical_issues={}", report.summary.critical_issues);
        println!("warning_issues={}", report.summary.warning_issues);
        println!("info_issues={}", report.summary.info_issues);
        println!("total_issues={}", total_issues);
        println!("reported_issues={}", reported_issues);
        println!("files_processed={}", report.summary.total_files);
        println!("files_with_issues={}", report.summary.files_with_issues);
        println!("coverage_percentage={:.1}", report.summary.coverage_percentage);

        if report.summary.critical_issues > 0 {
            println!(
                "::error::Found {} critical documentation issues",
                report.summary.critical_issues
            );
        }
        if report.summary.warning_issues > 0 {
            println!(
                "::warning::Found {} warning documentation issues",
                report.summary.warning_issues
            );
        }

        println!("::endgroup::");

        // Set output variables for GitHub Actions
        println!("::set-output name=critical_issues::{}", report.summary.critical_issues);
        println!("::set-output name=warning_issues::{}", report.summary.warning_issues);
        println!("::set-output name=info_issues::{}", report.summary.info_issues);
        println!("::set-output name=total_issues::{}", total_issues);
    } else if !config.quiet {
        println!();
        println!("Documentation Audit Results:");
        println!("============================");
        println!("Files processed: {}", report.summary.total_files);
        println!("Files with issues: {}", report.summary.files_with_issues);
        println!("Coverage: {:.1}%", report.summary.coverage_percentage);
        println!();
        println!("Issues found:");
        println!("  Critical: {}", report.summary.critical_issues);
        println!("  Warning:  {}", report.summary.warning_issues);
        println!("  Info:     {}", report.summary.info_issues);
        println!("  Total:    {}", total_issues);

        if max_issues > 0 && total_issues > max_issues {
            println!(
                "  (Showing {} of {} total issues due to --max-issues limit)",
                reported_issues, total_issues
            );
        }

        // Show some sample issues if any exist
        if !report.issues.is_empty() {
            println!();
            println!("Sample issues:");
            let sample_count = std::cmp::min(5, report.issues.len());
            for issue in report.issues.iter().take(sample_count) {
                let severity_icon = match issue.severity {
                    IssueSeverity::Critical => "❌",
                    IssueSeverity::Warning => "⚠️",
                    IssueSeverity::Info => "ℹ️",
                };

                println!(
                    "  {} {} ({}:{})",
                    severity_icon,
                    issue.message,
                    issue.file_path.display(),
                    issue.line_number.map(|n| n.to_string()).unwrap_or_else(|| "?".to_string())
                );
            }

            if report.issues.len() > sample_count {
                println!("  ... and {} more issues", report.issues.len() - sample_count);
            }
        }

        // Show recommendations if any
        if !report.recommendations.is_empty() {
            println!();
            println!("Recommendations:");
            for (i, rec) in report.recommendations.iter().take(3).enumerate() {
                println!("  {}. {}", i + 1, rec.description);
            }
            if report.recommendations.len() > 3 {
                println!("  ... and {} more recommendations", report.recommendations.len() - 3);
            }
        }
    }

    // Save report to file if requested or if format requires it
    let output_path = cli.get_output_path_with_default();
    if let Some(output_file) = output_path {
        let format = cli.get_output_format().into();
        let generator = ReportGenerator::new(format);

        match generator.save_to_file(&report, &output_file) {
            Ok(()) => {
                info!("Report saved to: {}", output_file.display());
                if !config.quiet && !ci_mode {
                    println!();
                    println!("📄 Report saved to: {}", output_file.display());
                }
            }
            Err(e) => {
                warn!("Failed to save report to file: {}", e);
                if !config.quiet && !ci_mode {
                    println!();
                    println!("⚠️  Failed to save report: {}", e);
                }
            }
        }
    }

    // CI/CD integration: Return appropriate exit codes
    if no_fail {
        info!("No-fail mode enabled, returning success regardless of issues");
        if !config.quiet && !ci_mode {
            println!();
            println!("ℹ️  No-fail mode: Build will succeed despite {} issues", total_issues);
        }
        return Ok(0);
    }

    if report.summary.critical_issues > 0 && config.fail_on_critical {
        error!("Critical issues found and fail_on_critical is enabled");
        if ci_mode {
            println!(
                "::error::Audit failed due to {} critical issues",
                report.summary.critical_issues
            );
        } else if !config.quiet {
            println!();
            println!("❌ Audit failed: {} critical issues found", report.summary.critical_issues);
            println!("Build should fail due to critical documentation issues.");
        }
        return Ok(1); // Exit code 1 for CI/CD failure
    }

    // Check severity threshold
    let total_issues_above_threshold = match config.severity_threshold {
        IssueSeverity::Critical => report.summary.critical_issues,
        IssueSeverity::Warning => report.summary.critical_issues + report.summary.warning_issues,
        IssueSeverity::Info => {
            report.summary.critical_issues
                + report.summary.warning_issues
                + report.summary.info_issues
        }
    };

    if total_issues_above_threshold > 0 {
        warn!(
            "Found {} issues at or above {:?} severity",
            total_issues_above_threshold, config.severity_threshold
        );
        if ci_mode {
            println!(
                "::notice::Audit completed with {} issues at or above {:?} severity",
                total_issues_above_threshold, config.severity_threshold
            );
        } else if !config.quiet {
            println!();
            println!(
                "⚠️  Audit completed with {} issues at or above {:?} severity",
                total_issues_above_threshold, config.severity_threshold
            );
        }
    } else {
        info!("No issues found at or above {:?} severity", config.severity_threshold);
        if ci_mode {
            println!(
                "::notice::Audit passed - no issues found at or above {:?} severity",
                config.severity_threshold
            );
        } else if !config.quiet {
            println!();
            println!(
                "✅ Audit passed: No issues found at or above {:?} severity",
                config.severity_threshold
            );
        }
    }

    Ok(0)
}

async fn run_incremental_command(
    config: AuditConfig,
    changed_files: &[std::path::PathBuf],
) -> Result<i32> {
    info!("Running incremental audit on {} files", changed_files.len());
    debug!("Configuration: {:?}", config);

    // Create orchestrator and run incremental audit
    let mut orchestrator = AuditOrchestrator::new(config.clone()).await?;
    let report = orchestrator.run_incremental_audit(changed_files).await?;

    if !config.quiet {
        println!();
        println!("Incremental Documentation Audit Results:");
        println!("=======================================");
        println!("Files processed: {}", report.summary.total_files);
        println!("Files with issues: {}", report.summary.files_with_issues);
        println!();
        println!("Issues found:");
        println!("  Critical: {}", report.summary.critical_issues);
        println!("  Warning:  {}", report.summary.warning_issues);
        println!("  Info:     {}", report.summary.info_issues);
        println!("  Total:    {}", report.summary.total_issues);

        // Show changed files that were processed
        println!();
        println!("Changed files processed:");
        for file in changed_files {
            let status = if file.exists() {
                if report.file_results.iter().any(|r| r.file_path == *file) {
                    "✓ Processed"
                } else {
                    "- Skipped (not documentation)"
                }
            } else {
                "⚠ File not found"
            };
            println!("  {} - {}", file.display(), status);
        }

        // Show sample issues if any exist
        if !report.issues.is_empty() {
            println!();
            println!("Issues found:");
            for issue in &report.issues {
                let severity_icon = match issue.severity {
                    IssueSeverity::Critical => "❌",
                    IssueSeverity::Warning => "⚠️",
                    IssueSeverity::Info => "ℹ️",
                };

                println!(
                    "  {} {} ({}:{})",
                    severity_icon,
                    issue.message,
                    issue.file_path.display(),
                    issue.line_number.map(|n| n.to_string()).unwrap_or_else(|| "?".to_string())
                );
            }
        }
    }

    // Return appropriate exit code based on issues found
    if report.summary.critical_issues > 0 && config.fail_on_critical {
        error!("Critical issues found in incremental audit");
        if !config.quiet {
            println!();
            println!(
                "❌ Incremental audit failed: {} critical issues found",
                report.summary.critical_issues
            );
        }
        return Ok(1);
    }

    if !config.quiet {
        if report.summary.total_issues == 0 {
            println!();
            println!("✅ Incremental audit passed: No issues found");
        } else {
            println!();
            println!("⚠️  Incremental audit completed with {} issues", report.summary.total_issues);
        }
    }

    Ok(0)
}

async fn run_validate_command(config: AuditConfig, file_path: &Path) -> Result<i32> {
    info!("Validating file: {}", file_path.display());
    debug!("Configuration: {:?}", config);

    // Validate that the file exists
    if !file_path.exists() {
        error!("File does not exist: {}", file_path.display());
        if !config.quiet {
            println!("❌ File not found: {}", file_path.display());
        }
        return Ok(1);
    }

    // Create orchestrator and validate single file
    let mut orchestrator = AuditOrchestrator::new(config.clone()).await?;

    match orchestrator.validate_file(file_path).await {
        Ok(result) => {
            if !config.quiet {
                println!();
                println!("Single File Validation Results:");
                println!("==============================");
                println!("File: {}", file_path.display());
                println!("Status: {}", if result.passed { "✅ Passed" } else { "❌ Failed" });
                println!("Processing time: {:?}", Duration::from_millis(result.audit_duration_ms));
                println!("Issues found: {}", result.issues.len());

                if !result.issues.is_empty() {
                    println!();
                    println!("Issues:");
                    for issue in &result.issues {
                        let severity_icon = match issue.severity {
                            IssueSeverity::Critical => "❌",
                            IssueSeverity::Warning => "⚠️",
                            IssueSeverity::Info => "ℹ️",
                        };

                        println!(
                            "  {} {} (line {})",
                            severity_icon,
                            issue.message,
                            issue
                                .line_number
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "?".to_string())
                        );

                        if let Some(suggestion) = &issue.suggestion {
                            println!("    💡 Suggestion: {}", suggestion);
                        }
                    }
                }
            }

            // Return appropriate exit code
            if !result.passed && config.fail_on_critical {
                let has_critical =
                    result.issues.iter().any(|i| i.severity == IssueSeverity::Critical);
                if has_critical {
                    return Ok(1);
                }
            }

            Ok(0)
        }
        Err(e) => {
            error!("Failed to validate file: {}", e);
            if !config.quiet {
                println!("❌ Validation failed: {}", e);
            }
            Ok(1)
        }
    }
}

async fn run_init_command(config: AuditConfig, config_path: &std::path::PathBuf) -> Result<i32> {
    info!("Initializing configuration at: {}", config_path.display());

    // Check if config file already exists
    if config_path.exists() {
        warn!("Configuration file already exists: {}", config_path.display());
        if !config.quiet {
            println!("Configuration file already exists at: {}", config_path.display());
            println!("Use --force to overwrite (not implemented yet)");
        }
        return Ok(1);
    }

    // Create parent directory if it doesn't exist
    if let Some(parent) = config_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| AuditError::IoError {
                path: parent.to_path_buf(),
                details: e.to_string(),
            })?;
            debug!("Created parent directory: {}", parent.display());
        }
    }

    // Save the current configuration to the specified path
    config.save_to_file(config_path)?;

    if !config.quiet {
        println!("Configuration file created at: {}", config_path.display());
        println!("You can edit this file to customize audit settings.");
        println!();
        println!("Example usage:");
        println!("  adk-doc-audit --config {} audit", config_path.display());
    }

    info!("Configuration initialization completed successfully");
    Ok(0)
}

async fn run_stats_command(config: AuditConfig, limit: usize) -> Result<i32> {
    info!("Showing audit statistics (limit: {})", limit);
    debug!("Configuration: {:?}", config);

    // TODO: Implement stats functionality when database is available

    warn!("Stats functionality not yet implemented - database component pending");

    if !config.quiet {
        println!("Audit statistics configuration:");
        println!("  Workspace: {}", config.workspace_path.display());
        println!("  Database: {}", config.get_database_path().display());
        println!("  Limit: {} recent runs", limit);
        println!();
        println!("Would show:");
        println!("  - Recent audit run timestamps");
        println!("  - Issue counts by severity");
        println!("  - Trend analysis");
        println!("  - Most problematic files");
    }

    // Check if database file exists
    let db_path = config.get_database_path();
    if db_path.exists() {
        debug!("Database file exists: {}", db_path.display());
    } else {
        debug!("Database file does not exist yet: {}", db_path.display());
    }

    Ok(0)
}

async fn run_crate_audit_command(
    mut config: AuditConfig,
    cli: &AuditCli,
    crate_name: &str,
) -> Result<i32> {
    info!("Running audit for single crate: {}", crate_name);

    // Find the crate path by name
    let crate_dir = config.workspace_path.join(crate_name);
    if !crate_dir.exists() {
        // Try with adk- prefix if not found
        let prefixed_name = format!("adk-{}", crate_name);
        let prefixed_dir = config.workspace_path.join(&prefixed_name);
        if prefixed_dir.exists() {
            config.workspace_path = prefixed_dir;
        } else {
            return Err(AuditError::ConfigurationError {
                message: format!(
                    "Crate '{}' not found in workspace. Tried '{}' and '{}'",
                    crate_name,
                    crate_dir.display(),
                    prefixed_dir.display()
                ),
            });
        }
    } else {
        config.workspace_path = crate_dir;
    }

    // Set documentation path for the crate
    config.docs_path = config.workspace_path.join("docs");

    // Check if the single crate has documentation
    if !config.docs_path.exists() {
        // Try alternative documentation locations
        let alt_docs = [
            config.workspace_path.join("README.md"),
            config.workspace_path.join("doc"),
            config.workspace_path.join("documentation"),
        ];

        let mut found_docs = false;
        for alt_path in &alt_docs {
            if alt_path.exists() {
                if alt_path.is_file() {
                    // Single README file - audit the parent directory
                    config.docs_path = config.workspace_path.clone();
                } else {
                    // Alternative docs directory
                    config.docs_path = alt_path.clone();
                }
                found_docs = true;
                break;
            }
        }

        if !found_docs {
            warn!(
                "No documentation found for crate '{}' at: {}",
                crate_name,
                config.workspace_path.display()
            );
            warn!("Tried: docs/, README.md, doc/, documentation/");
            return Ok(0); // Exit gracefully if no docs found
        }
    }

    info!("Crate path: {}", config.workspace_path.display());
    info!("Documentation path: {}", config.docs_path.display());

    // Create orchestrator and run audit
    let mut orchestrator = AuditOrchestrator::new(config.clone()).await?;
    let report = orchestrator.run_full_audit().await?;

    // Output results
    if !config.quiet {
        println!();
        println!("Single Crate Audit Results for '{}':", crate_name);
        println!("=====================================");
        println!("Crate path: {}", config.workspace_path.display());
        println!("Files processed: {}", report.summary.total_files);
        println!("Files with issues: {}", report.summary.files_with_issues);
        println!("Coverage: {:.1}%", report.summary.coverage_percentage);
        println!();
        println!("Issues found:");
        println!("  Critical: {}", report.summary.critical_issues);
        println!("  Warning:  {}", report.summary.warning_issues);
        println!("  Info:     {}", report.summary.info_issues);
        println!("  Total:    {}", report.summary.total_issues);

        // Show sample issues if any exist
        if !report.issues.is_empty() {
            println!();
            println!("Issues found:");
            for issue in &report.issues {
                let severity_icon = match issue.severity {
                    IssueSeverity::Critical => "❌",
                    IssueSeverity::Warning => "⚠️",
                    IssueSeverity::Info => "ℹ️",
                };

                println!(
                    "  {} {} ({}:{})",
                    severity_icon,
                    issue.message,
                    issue.file_path.display(),
                    issue.line_number.map(|n| n.to_string()).unwrap_or_else(|| "?".to_string())
                );
            }
        }

        // Show recommendations if any
        if !report.recommendations.is_empty() {
            println!();
            println!("Recommendations:");
            for (i, rec) in report.recommendations.iter().take(3).enumerate() {
                println!("  {}. {}", i + 1, rec.description);
            }
            if report.recommendations.len() > 3 {
                println!("  ... and {} more recommendations", report.recommendations.len() - 3);
            }
        }
    }

    // Save report to file if requested or if format requires it
    let output_path = cli.get_output_path_with_default();
    if let Some(output_file) = output_path {
        let format = cli.get_output_format().into();
        let generator = ReportGenerator::new(format);

        match generator.save_to_file(&report, &output_file) {
            Ok(()) => {
                info!("Report saved to: {}", output_file.display());
                if !config.quiet {
                    println!();
                    println!("📄 Report saved to: {}", output_file.display());
                }
            }
            Err(e) => {
                warn!("Failed to save report to file: {}", e);
                if !config.quiet {
                    println!();
                    println!("⚠️  Failed to save report: {}", e);
                }
            }
        }
    }

    // Return appropriate exit code
    if report.summary.critical_issues > 0 && config.fail_on_critical {
        error!("Critical issues found in crate '{}'", crate_name);
        if !config.quiet {
            println!();
            println!(
                "❌ Audit failed: {} critical issues found in crate '{}'",
                report.summary.critical_issues, crate_name
            );
        }
        return Ok(1);
    }

    if !config.quiet {
        if report.summary.total_issues == 0 {
            println!();
            println!("✅ Audit passed: No issues found in crate '{}'", crate_name);
        } else {
            println!();
            println!(
                "⚠️  Audit completed with {} issues in crate '{}'",
                report.summary.total_issues, crate_name
            );
        }
    }

    Ok(0)
}
