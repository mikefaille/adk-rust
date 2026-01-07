use std::path::PathBuf;
use thiserror::Error;

/// Result type for the documentation audit system.
pub type Result<T> = std::result::Result<T, AuditError>;

/// Comprehensive error types for the documentation audit system.
#[derive(Debug, Error)]
pub enum AuditError {
    #[error("Failed to parse documentation file: {path}")]
    DocumentationParseError { path: PathBuf },

    #[error("Failed to analyze crate: {crate_name} - {details}")]
    CrateAnalysisError { crate_name: String, details: String },

    #[error("Code example compilation failed: {details}")]
    CompilationError { details: String },

    #[error("Workspace not found at path: {path}")]
    WorkspaceNotFound { path: PathBuf },

    #[error("Invalid configuration: {message}")]
    ConfigurationError { message: String },

    #[error("Database error: {details}")]
    DatabaseError { details: String },

    #[error("IO error accessing {path}: {details}")]
    IoError { path: PathBuf, details: String },

    #[error("API reference validation failed: {api_path} in {crate_name}")]
    ApiValidationError { api_path: String, crate_name: String },

    #[error("Version inconsistency: expected {expected}, found {found} in {file_path}")]
    VersionInconsistency { expected: String, found: String, file_path: PathBuf },

    #[error("Internal link broken: {link} in {file_path}")]
    BrokenLinkError { link: String, file_path: PathBuf },

    #[error("Feature flag '{flag}' not found in crate '{crate_name}'")]
    FeatureFlagNotFound { flag: String, crate_name: String },

    #[error("Temporary directory creation failed: {details}")]
    TempDirError { details: String },

    #[error("Cargo command failed: {command} - {output}")]
    CargoError { command: String, output: String },

    #[error("Regex compilation failed: {pattern} - {details}")]
    RegexError { pattern: String, details: String },

    #[error("JSON serialization/deserialization error: {details}")]
    JsonError { details: String },

    #[error("TOML parsing error in {file_path}: {details}")]
    TomlError { file_path: PathBuf, details: String },

    #[error("Markdown parsing error in {file_path}: {details}")]
    MarkdownError { file_path: PathBuf, details: String },

    #[error("Rust syntax parsing error: {details}")]
    SyntaxError { details: String },

    #[error("Report generation failed: {details}")]
    ReportGeneration { details: String },

    #[error("File not found: {path}")]
    FileNotFound { path: PathBuf },

    #[error("Invalid file type: {path}, expected {expected}")]
    InvalidFileType { path: PathBuf, expected: String },

    #[error("Processing error: {details}")]
    ProcessingError { details: String },
}

impl From<std::io::Error> for AuditError {
    fn from(err: std::io::Error) -> Self {
        AuditError::IoError { path: PathBuf::from("<unknown>"), details: err.to_string() }
    }
}

impl From<serde_json::Error> for AuditError {
    fn from(err: serde_json::Error) -> Self {
        AuditError::JsonError { details: err.to_string() }
    }
}

impl From<regex::Error> for AuditError {
    fn from(err: regex::Error) -> Self {
        AuditError::RegexError { pattern: "<unknown>".to_string(), details: err.to_string() }
    }
}

impl From<rusqlite::Error> for AuditError {
    fn from(err: rusqlite::Error) -> Self {
        AuditError::DatabaseError { details: err.to_string() }
    }
}

impl From<syn::Error> for AuditError {
    fn from(err: syn::Error) -> Self {
        AuditError::SyntaxError { details: err.to_string() }
    }
}

impl From<toml::de::Error> for AuditError {
    fn from(err: toml::de::Error) -> Self {
        AuditError::TomlError { file_path: PathBuf::from("<unknown>"), details: err.to_string() }
    }
}
