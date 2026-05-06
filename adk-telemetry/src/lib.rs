//! # ADK Telemetry
//!
//! Production-grade observability for ADK using structured logging and distributed tracing.
//!
//! ## Features
//! - Structured logging with `tracing`
//! - OpenTelemetry integration for distributed tracing
//! - OTLP export for observability backends (Jaeger, Datadog, etc.)
//! - Automatic context propagation
//!
//! ## Usage
//!
//! ```rust
//! use adk_telemetry::{init_telemetry, info, instrument};
//!
//! fn main() -> Result<(), adk_telemetry::TelemetryError> {
//!     // Initialize telemetry in your main
//!     init_telemetry("my-service")?;
//!
//!     // Use logging macros
//!     #[instrument]
//!     async fn my_function() {
//!         info!("Function called");
//!     }
//!     Ok(())
//! }
//! ```

pub mod init;
pub mod span_exporter;
pub mod spans;

// Re-export tracing macros for convenience
pub use tracing::{Span, debug, error, info, instrument, trace, warn};

// Re-export span helpers
pub use spans::*;

// Re-export span exporter (ADK-Go style)
pub use span_exporter::*;

// Re-export init functions and error type
pub use init::{TelemetryError, init_telemetry, init_with_adk_exporter, shutdown_telemetry};
#[cfg(feature = "otlp")]
pub use init::{build_otlp_layer, init_with_otlp};

// Re-export metrics
#[cfg(feature = "otlp")]
pub use opentelemetry::global;
#[cfg(feature = "otlp")]
pub use opentelemetry::metrics::{Meter, MeterProvider};
