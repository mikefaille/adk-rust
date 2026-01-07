use crate::config::{AuditConfig, IssueSeverity, OutputFormat};
use crate::error::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use tracing::{debug, info};

/// Documentation audit system for ADK-Rust.
#[derive(Parser)]
#[command(name = "adk-doc-audit")]
#[command(about = "Validates documentation against actual crate implementations")]
#[command(version)]
pub struct AuditCli {
    #[command(subcommand)]
    pub command: AuditCommand,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Enable quiet mode (minimal output)
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Configuration file path
    #[arg(short, long, global = true)]
    pub config: Option<PathBuf>,
}

/// Available audit commands.
#[derive(Subcommand)]
pub enum AuditCommand {
    /// Run a full documentation audit
    Audit {
        /// Path to workspace root
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Path to documentation directory
        #[arg(short, long, default_value = "docs")]
        docs: PathBuf,

        /// Audit only a specific crate (by name)
        #[arg(long)]
        crate_name: Option<String>,

        /// Audit only a specific crate (by path)
        #[arg(long, conflicts_with = "crate_name")]
        crate_path: Option<PathBuf>,

        /// Output format
        #[arg(short, long, default_value = "console")]
        format: CliOutputFormat,

        /// Minimum severity to report
        #[arg(short, long, default_value = "warning")]
        severity: CliSeverity,

        /// Fail build on critical issues
        #[arg(long, default_value = "true")]
        fail_on_critical: bool,

        /// Files to exclude (glob patterns)
        #[arg(long, action = clap::ArgAction::Append)]
        exclude_files: Vec<String>,

        /// Crates to exclude
        #[arg(long, action = clap::ArgAction::Append)]
        exclude_crates: Vec<String>,

        /// Output file path (for JSON/Markdown formats)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Exit with code 0 even if issues are found (for CI/CD flexibility)
        #[arg(long)]
        no_fail: bool,

        /// Maximum number of issues to report (0 = unlimited)
        #[arg(long, default_value = "0")]
        max_issues: usize,

        /// CI/CD mode: optimized output for build systems
        #[arg(long)]
        ci_mode: bool,
    },

    /// Audit a single crate's documentation
    Crate {
        /// Name of the crate to audit (e.g., "core" for adk-core)
        crate_name: String,

        /// Path to workspace root
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Output format
        #[arg(short, long, default_value = "console")]
        format: CliOutputFormat,

        /// Minimum severity to report
        #[arg(short, long, default_value = "warning")]
        severity: CliSeverity,

        /// Fail build on critical issues
        #[arg(long, default_value = "true")]
        fail_on_critical: bool,

        /// Output file path (for JSON/Markdown formats)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Run incremental audit on changed files
    Incremental {
        /// Path to workspace root
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Path to documentation directory
        #[arg(short, long, default_value = "docs")]
        docs: PathBuf,

        /// Changed files to audit
        #[arg(required = true)]
        changed_files: Vec<PathBuf>,

        /// Output format
        #[arg(short, long, default_value = "console")]
        format: CliOutputFormat,
    },

    /// Validate a single documentation file
    Validate {
        /// Path to the file to validate
        file_path: PathBuf,

        /// Path to workspace root (for context)
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Output format
        #[arg(short, long, default_value = "console")]
        format: CliOutputFormat,
    },

    /// Initialize audit configuration
    Init {
        /// Path to create configuration file
        #[arg(long, default_value = "adk-doc-audit.toml")]
        config_path: PathBuf,

        /// Path to workspace root
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Path to documentation directory
        #[arg(short, long, default_value = "docs")]
        docs: PathBuf,
    },

    /// Show audit statistics and history
    Stats {
        /// Path to workspace root
        #[arg(short, long, default_value = ".")]
        workspace: PathBuf,

        /// Number of recent audit runs to show
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
}

/// CLI-compatible output format enum.
#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum CliOutputFormat {
    Console,
    Json,
    Markdown,
}

impl From<CliOutputFormat> for OutputFormat {
    fn from(cli_format: CliOutputFormat) -> Self {
        match cli_format {
            CliOutputFormat::Console => OutputFormat::Console,
            CliOutputFormat::Json => OutputFormat::Json,
            CliOutputFormat::Markdown => OutputFormat::Markdown,
        }
    }
}

/// CLI-compatible severity enum.
#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum CliSeverity {
    Info,
    Warning,
    Critical,
}

impl From<CliSeverity> for IssueSeverity {
    fn from(cli_severity: CliSeverity) -> Self {
        match cli_severity {
            CliSeverity::Info => IssueSeverity::Info,
            CliSeverity::Warning => IssueSeverity::Warning,
            CliSeverity::Critical => IssueSeverity::Critical,
        }
    }
}

impl AuditCli {
    /// Parse command line arguments and create configuration.
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Convert CLI arguments to AuditConfig.
    pub fn to_config(&self) -> Result<AuditConfig> {
        // Load base config from file if specified, or try default locations
        let mut config = if let Some(config_path) = &self.config {
            info!("Loading configuration from: {}", config_path.display());
            AuditConfig::from_file(config_path)?
        } else {
            // Try to load from default locations
            let default_paths = [
                PathBuf::from("adk-doc-audit.toml"),
                PathBuf::from(".adk-doc-audit.toml"),
                PathBuf::from("config/adk-doc-audit.toml"),
            ];

            let mut loaded_config = None;
            for path in &default_paths {
                if path.exists() {
                    info!("Found configuration file at: {}", path.display());
                    loaded_config = Some(AuditConfig::from_file(path)?);
                    break;
                }
            }

            loaded_config.unwrap_or_else(|| {
                debug!("No configuration file found, using defaults");
                AuditConfig::default()
            })
        };

        // Override with CLI arguments
        config.verbose = self.verbose;
        config.quiet = self.quiet;

        match &self.command {
            AuditCommand::Audit {
                workspace,
                docs,
                format,
                severity,
                fail_on_critical,
                exclude_files,
                exclude_crates,
                no_fail,
                max_issues: _,
                ci_mode,
                crate_name,
                crate_path,
                ..
            } => {
                config.workspace_path = workspace.clone();

                // Handle single crate auditing in CLI configuration
                if let Some(name) = crate_name {
                    // Find the crate path by name
                    let crate_dir = config.workspace_path.join(name);
                    if !crate_dir.exists() {
                        // Try with adk- prefix if not found
                        let prefixed_name = format!("adk-{}", name);
                        let prefixed_dir = config.workspace_path.join(&prefixed_name);
                        if prefixed_dir.exists() {
                            config.workspace_path = prefixed_dir.clone();
                            config.docs_path = prefixed_dir.join("docs");
                        } else {
                            return Err(crate::AuditError::ConfigurationError {
                                message: format!(
                                    "Crate '{}' not found in workspace. Tried '{}' and '{}'",
                                    name,
                                    crate_dir.display(),
                                    prefixed_dir.display()
                                ),
                            });
                        }
                    } else {
                        config.workspace_path = crate_dir.clone();
                        config.docs_path = crate_dir.join("docs");
                    }

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
                            // Set docs_path to workspace_path to avoid validation error
                            // The orchestrator will handle the case where no docs exist
                            config.docs_path = config.workspace_path.clone();
                        }
                    }
                } else if let Some(path) = crate_path {
                    if !path.exists() {
                        return Err(crate::AuditError::ConfigurationError {
                            message: format!("Crate path does not exist: {}", path.display()),
                        });
                    }
                    config.workspace_path = path.clone();
                    config.docs_path = path.join("docs");

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
                            // Set docs_path to workspace_path to avoid validation error
                            config.docs_path = config.workspace_path.clone();
                        }
                    }
                } else {
                    // Regular workspace audit
                    config.docs_path = docs.clone();
                }

                config.output_format = (*format).into();
                config.severity_threshold = (*severity).into();
                config.fail_on_critical = *fail_on_critical && !*no_fail; // no_fail overrides fail_on_critical
                config.excluded_files.extend(exclude_files.clone());
                config.excluded_crates.extend(exclude_crates.clone());

                // CI/CD specific settings
                if *ci_mode {
                    config.quiet = true; // CI mode implies quiet output
                }

                // Store CI/CD specific options in config (we'll need to extend AuditConfig for this)
                // For now, we'll handle these in the command execution
            }
            AuditCommand::Crate { workspace, format, severity, fail_on_critical, .. } => {
                config.workspace_path = workspace.clone();
                config.output_format = (*format).into();
                config.severity_threshold = (*severity).into();
                config.fail_on_critical = *fail_on_critical;
                // docs_path will be set based on crate_name in main.rs
            }
            AuditCommand::Incremental { workspace, docs, format, .. } => {
                config.workspace_path = workspace.clone();
                config.docs_path = docs.clone();
                config.output_format = (*format).into();
            }
            AuditCommand::Validate { workspace, format, .. } => {
                config.workspace_path = workspace.clone();
                config.output_format = (*format).into();
            }
            AuditCommand::Init { workspace, docs, .. } => {
                config.workspace_path = workspace.clone();
                config.docs_path = docs.clone();
            }
            AuditCommand::Stats { workspace, .. } => {
                config.workspace_path = workspace.clone();
            }
        }

        // Validate the final configuration
        AuditConfig::builder()
            .workspace_path(&config.workspace_path)
            .docs_path(&config.docs_path)
            .exclude_files(config.excluded_files.clone())
            .exclude_crates(config.excluded_crates.clone())
            .severity_threshold(config.severity_threshold)
            .fail_on_critical(config.fail_on_critical)
            .example_timeout(config.example_timeout)
            .output_format(config.output_format)
            .database_path(config.database_path.clone())
            .verbose(config.verbose)
            .quiet(config.quiet)
            .build()
    }

    /// Get the output file path for commands that support it.
    pub fn get_output_path(&self) -> Option<&PathBuf> {
        match &self.command {
            AuditCommand::Audit { output, .. } => output.as_ref(),
            AuditCommand::Crate { output, .. } => output.as_ref(),
            _ => None,
        }
    }

    /// Get the output format for the current command.
    pub fn get_output_format(&self) -> OutputFormat {
        match &self.command {
            AuditCommand::Audit { format, .. } => (*format).into(),
            AuditCommand::Crate { format, .. } => (*format).into(),
            AuditCommand::Incremental { format, .. } => (*format).into(),
            AuditCommand::Validate { format, .. } => (*format).into(),
            _ => OutputFormat::Console,
        }
    }

    /// Get the output file path with default filename if format requires file output.
    pub fn get_output_path_with_default(&self) -> Option<PathBuf> {
        // First check if user provided explicit output path
        if let Some(path) = self.get_output_path() {
            return Some(path.clone());
        }

        // Generate default filename based on format and command
        let format = self.get_output_format();
        match format {
            crate::config::OutputFormat::Console => None, // Console output doesn't need a file
            crate::config::OutputFormat::Json => {
                let filename = match &self.command {
                    AuditCommand::Audit { .. } => "audit-report.json",
                    AuditCommand::Crate { crate_name, .. } => {
                        return Some(PathBuf::from(format!("audit-{}.json", crate_name)));
                    }
                    _ => "audit-report.json",
                };
                Some(PathBuf::from(filename))
            }
            crate::config::OutputFormat::Markdown => {
                let filename = match &self.command {
                    AuditCommand::Audit { .. } => "audit-report.md",
                    AuditCommand::Crate { crate_name, .. } => {
                        return Some(PathBuf::from(format!("audit-{}.md", crate_name)));
                    }
                    _ => "audit-report.md",
                };
                Some(PathBuf::from(filename))
            }
        }
    }

    /// Get the crate name for single crate audit.
    pub fn get_crate_name(&self) -> Option<&String> {
        match &self.command {
            AuditCommand::Crate { crate_name, .. } => Some(crate_name),
            _ => None,
        }
    }

    /// Get the single crate options for audit command.
    pub fn get_single_crate_options(&self) -> Option<(Option<&String>, Option<&PathBuf>)> {
        match &self.command {
            AuditCommand::Audit { crate_name, crate_path, .. } => {
                Some((crate_name.as_ref(), crate_path.as_ref()))
            }
            _ => None,
        }
    }

    /// Get the changed files for incremental audit.
    pub fn get_changed_files(&self) -> Option<&[PathBuf]> {
        match &self.command {
            AuditCommand::Incremental { changed_files, .. } => Some(changed_files),
            _ => None,
        }
    }

    /// Get the file path for single file validation.
    pub fn get_validate_file(&self) -> Option<&PathBuf> {
        match &self.command {
            AuditCommand::Validate { file_path, .. } => Some(file_path),
            _ => None,
        }
    }

    /// Get the configuration path for init command.
    pub fn get_init_config_path(&self) -> Option<&PathBuf> {
        match &self.command {
            AuditCommand::Init { config_path, .. } => Some(config_path),
            _ => None,
        }
    }

    /// Get the limit for stats command.
    pub fn get_stats_limit(&self) -> Option<usize> {
        match &self.command {
            AuditCommand::Stats { limit, .. } => Some(*limit),
            _ => None,
        }
    }

    /// Get CI/CD specific options for audit command.
    pub fn get_ci_options(&self) -> Option<(bool, usize, bool)> {
        match &self.command {
            AuditCommand::Audit { no_fail, max_issues, ci_mode, .. } => {
                Some((*no_fail, *max_issues, *ci_mode))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn test_cli_verify() {
        // Verify that the CLI definition is valid
        AuditCli::command().debug_assert();
    }

    #[test]
    fn test_cli_parsing() {
        // Test basic audit command
        let cli = AuditCli::try_parse_from([
            "adk-doc-audit",
            "audit",
            "--workspace",
            "/tmp/workspace",
            "--docs",
            "/tmp/docs",
            "--format",
            "json",
            "--severity",
            "critical",
        ]);

        assert!(cli.is_ok());
        let cli = cli.unwrap();

        match cli.command {
            AuditCommand::Audit { workspace, docs, format, severity, .. } => {
                assert_eq!(workspace, PathBuf::from("/tmp/workspace"));
                assert_eq!(docs, PathBuf::from("/tmp/docs"));
                assert!(matches!(format, CliOutputFormat::Json));
                assert!(matches!(severity, CliSeverity::Critical));
            }
            _ => panic!("Expected Audit command"),
        }
    }

    #[test]
    fn test_incremental_command() {
        let cli = AuditCli::try_parse_from([
            "adk-doc-audit",
            "incremental",
            "--workspace",
            "/tmp/workspace",
            "file1.md",
            "file2.md",
        ]);

        assert!(cli.is_ok());
        let cli = cli.unwrap();

        match cli.command {
            AuditCommand::Incremental { changed_files, .. } => {
                assert_eq!(changed_files.len(), 2);
                assert_eq!(changed_files[0], PathBuf::from("file1.md"));
                assert_eq!(changed_files[1], PathBuf::from("file2.md"));
            }
            _ => panic!("Expected Incremental command"),
        }
    }

    #[test]
    fn test_validate_command() {
        let cli =
            AuditCli::try_parse_from(["adk-doc-audit", "validate", "docs/getting-started.md"]);

        assert!(cli.is_ok());
        let cli = cli.unwrap();

        match cli.command {
            AuditCommand::Validate { file_path, .. } => {
                assert_eq!(file_path, PathBuf::from("docs/getting-started.md"));
            }
            _ => panic!("Expected Validate command"),
        }
    }

    #[test]
    fn test_global_flags() {
        let cli = AuditCli::try_parse_from(["adk-doc-audit", "--verbose", "audit"]);

        assert!(cli.is_ok());
        let cli = cli.unwrap();
        assert!(cli.verbose);
        assert!(!cli.quiet);
    }

    #[test]
    fn test_conflicting_flags() {
        let cli = AuditCli::try_parse_from(["adk-doc-audit", "--verbose", "--quiet", "audit"]);

        // Should fail due to conflicting flags
        assert!(cli.is_err());
    }
}
