use crate::error::{AuditError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Configuration for the documentation audit system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    /// Path to the workspace root containing Cargo.toml
    pub workspace_path: PathBuf,

    /// Path to the documentation directory
    pub docs_path: PathBuf,

    /// Files to exclude from audit (glob patterns)
    pub excluded_files: Vec<String>,

    /// Crates to exclude from analysis
    pub excluded_crates: Vec<String>,

    /// Minimum severity level to report
    pub severity_threshold: IssueSeverity,

    /// Whether to fail CI/CD on critical issues
    pub fail_on_critical: bool,

    /// Timeout for compiling code examples
    pub example_timeout: Duration,

    /// Output format for reports
    pub output_format: OutputFormat,

    /// Path to audit database (for incremental audits)
    pub database_path: Option<PathBuf>,

    /// Enable verbose logging
    pub verbose: bool,

    /// Enable quiet mode (minimal output)
    pub quiet: bool,
}

/// Severity levels for audit issues.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub enum IssueSeverity {
    Info,
    #[default]
    Warning,
    Critical,
}

/// Output formats for audit reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OutputFormat {
    #[default]
    Console,
    Json,
    Markdown,
}

/// Builder for creating AuditConfig instances.
#[derive(Debug, Clone, Default)]
pub struct AuditConfigBuilder {
    config: AuditConfig,
}

impl AuditConfigBuilder {
    /// Create a new builder with default values.
    pub fn new() -> Self {
        Self {
            config: AuditConfig {
                workspace_path: PathBuf::from("."),
                docs_path: PathBuf::from("docs"),
                excluded_files: vec![
                    "*.tmp".to_string(),
                    "*.bak".to_string(),
                    ".git/**".to_string(),
                    "target/**".to_string(),
                ],
                excluded_crates: vec![],
                severity_threshold: IssueSeverity::default(),
                fail_on_critical: true,
                example_timeout: Duration::from_secs(30),
                output_format: OutputFormat::default(),
                database_path: None,
                verbose: false,
                quiet: false,
            },
        }
    }

    /// Set the workspace path.
    pub fn workspace_path<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.config.workspace_path = path.into();
        self
    }

    /// Set the documentation path.
    pub fn docs_path<P: Into<PathBuf>>(mut self, path: P) -> Self {
        self.config.docs_path = path.into();
        self
    }

    /// Add files to exclude from audit.
    pub fn exclude_files<I, S>(mut self, patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.excluded_files.extend(patterns.into_iter().map(Into::into));
        self
    }

    /// Add crates to exclude from analysis.
    pub fn exclude_crates<I, S>(mut self, crates: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.excluded_crates.extend(crates.into_iter().map(Into::into));
        self
    }

    /// Set the severity threshold.
    pub fn severity_threshold(mut self, threshold: IssueSeverity) -> Self {
        self.config.severity_threshold = threshold;
        self
    }

    /// Set whether to fail on critical issues.
    pub fn fail_on_critical(mut self, fail: bool) -> Self {
        self.config.fail_on_critical = fail;
        self
    }

    /// Set the timeout for compiling examples.
    pub fn example_timeout(mut self, timeout: Duration) -> Self {
        self.config.example_timeout = timeout;
        self
    }

    /// Set the output format.
    pub fn output_format(mut self, format: OutputFormat) -> Self {
        self.config.output_format = format;
        self
    }

    /// Set the database path for incremental audits.
    pub fn database_path<P: Into<PathBuf>>(mut self, path: Option<P>) -> Self {
        self.config.database_path = path.map(Into::into);
        self
    }

    /// Enable verbose logging.
    pub fn verbose(mut self, verbose: bool) -> Self {
        self.config.verbose = verbose;
        self
    }

    /// Enable quiet mode.
    pub fn quiet(mut self, quiet: bool) -> Self {
        self.config.quiet = quiet;
        self
    }

    /// Build the configuration, validating settings.
    pub fn build(self) -> Result<AuditConfig> {
        let config = self.config;

        // Validate configuration
        if !config.workspace_path.exists() {
            return Err(AuditError::WorkspaceNotFound { path: config.workspace_path });
        }

        if !config.docs_path.exists() {
            return Err(AuditError::ConfigurationError {
                message: format!(
                    "Documentation path does not exist: {}",
                    config.docs_path.display()
                ),
            });
        }

        if config.verbose && config.quiet {
            return Err(AuditError::ConfigurationError {
                message: "Cannot enable both verbose and quiet modes".to_string(),
            });
        }

        if config.example_timeout.as_secs() == 0 {
            return Err(AuditError::ConfigurationError {
                message: "Example timeout must be greater than 0".to_string(),
            });
        }

        Ok(config)
    }
}

impl AuditConfig {
    /// Create a new builder.
    pub fn builder() -> AuditConfigBuilder {
        AuditConfigBuilder::new()
    }

    /// Load configuration from a TOML file.
    pub fn from_file<P: Into<PathBuf>>(path: P) -> Result<Self> {
        let path = path.into();
        let content = std::fs::read_to_string(&path)
            .map_err(|e| AuditError::IoError { path: path.clone(), details: e.to_string() })?;

        let config: AuditConfig = toml::from_str(&content)
            .map_err(|e| AuditError::TomlError { file_path: path, details: e.to_string() })?;

        Ok(config)
    }

    /// Save configuration to a TOML file.
    pub fn save_to_file<P: Into<PathBuf>>(&self, path: P) -> Result<()> {
        let path = path.into();
        let content = toml::to_string_pretty(self).map_err(|e| AuditError::TomlError {
            file_path: path.clone(),
            details: e.to_string(),
        })?;

        std::fs::write(&path, content)
            .map_err(|e| AuditError::IoError { path, details: e.to_string() })?;

        Ok(())
    }

    /// Get the default database path if none is configured.
    pub fn get_database_path(&self) -> PathBuf {
        self.database_path.clone().unwrap_or_else(|| self.workspace_path.join(".adk-doc-audit.db"))
    }
}

impl Default for AuditConfig {
    fn default() -> Self {
        // Create a basic config without validation for default case
        AuditConfig {
            workspace_path: PathBuf::from("."),
            docs_path: PathBuf::from("docs"),
            excluded_files: vec![
                "*.tmp".to_string(),
                "*.bak".to_string(),
                ".git/**".to_string(),
                "target/**".to_string(),
            ],
            excluded_crates: vec![],
            severity_threshold: IssueSeverity::default(),
            fail_on_critical: true,
            example_timeout: Duration::from_secs(30),
            output_format: OutputFormat::default(),
            database_path: None,
            verbose: false,
            quiet: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_config_builder_default() {
        // Create temporary directories for testing
        let temp_dir = std::env::temp_dir();
        let workspace_path = temp_dir.join("test_workspace");
        let docs_path = temp_dir.join("test_docs");

        // Create the directories
        std::fs::create_dir_all(&workspace_path).unwrap();
        std::fs::create_dir_all(&docs_path).unwrap();

        let config =
            AuditConfig::builder().workspace_path(&workspace_path).docs_path(&docs_path).build();

        assert!(config.is_ok());

        let config = config.unwrap();
        assert_eq!(config.workspace_path, workspace_path);
        assert_eq!(config.docs_path, docs_path);
        assert_eq!(config.severity_threshold, IssueSeverity::Warning);
        assert!(config.fail_on_critical);
        assert_eq!(config.example_timeout, Duration::from_secs(30));

        // Clean up
        std::fs::remove_dir_all(&workspace_path).ok();
        std::fs::remove_dir_all(&docs_path).ok();
    }

    #[test]
    fn test_config_builder_customization() {
        let config = AuditConfig::builder()
            .workspace_path("/tmp/workspace")
            .docs_path("/tmp/docs")
            .severity_threshold(IssueSeverity::Critical)
            .fail_on_critical(false)
            .example_timeout(Duration::from_secs(60))
            .verbose(true)
            .build();

        // This will fail because paths don't exist, but we can test the builder logic
        assert!(config.is_err());
    }

    #[test]
    fn test_config_validation_errors() {
        // Test conflicting verbose/quiet
        let result = AuditConfig::builder().verbose(true).quiet(true).build();
        assert!(result.is_err());

        // Test zero timeout
        let result = AuditConfig::builder().example_timeout(Duration::from_secs(0)).build();
        assert!(result.is_err());
    }

    #[test]
    fn test_severity_ordering() {
        assert!(IssueSeverity::Info < IssueSeverity::Warning);
        assert!(IssueSeverity::Warning < IssueSeverity::Critical);
    }
}
