//! Pipeline handle for interacting with a running pipeline.

use std::sync::Arc;

use tokio::sync::{RwLock, mpsc, oneshot};

use crate::pipeline::types::{PipelineInput, PipelineMetrics, PipelineOutput};

/// Handle to a running audio pipeline.
///
/// Provides channels for sending input, receiving output, reading metrics,
/// and shutting down the pipeline.
pub struct PipelineHandle<'a> {
    /// Send audio, text, or control messages into the pipeline.
    pub input_tx: mpsc::Sender<PipelineInput<'a>>,
    /// Receive audio, transcript, or metrics output from the pipeline.
    pub output_rx: mpsc::Receiver<PipelineOutput<'static>>,
    /// Real-time pipeline metrics (updated after each stage).
    pub metrics: Arc<RwLock<PipelineMetrics>>,
    /// One-shot channel to signal graceful shutdown.
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl<'a> PipelineHandle<'a> {
    /// Create a new `PipelineHandle`.
    pub(crate) fn new(
        input_tx: mpsc::Sender<PipelineInput<'a>>,
        output_rx: mpsc::Receiver<PipelineOutput<'static>>,
        metrics: Arc<RwLock<PipelineMetrics>>,
        shutdown_tx: oneshot::Sender<()>,
    ) -> Self {
        Self { input_tx, output_rx, metrics, shutdown_tx: Some(shutdown_tx) }
    }

    /// Signal the pipeline to shut down gracefully.
    pub fn shutdown(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}
