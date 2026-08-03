//! Pipeline input, output, and control types.

use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

use crate::error::AudioResult;
use crate::frame::AudioFrame;
use crate::traits::Transcript;

/// Messages that can be sent into a pipeline.
pub enum PipelineInput {
    /// Raw audio data.
    Audio(AudioFrame),
    /// Text input (bypasses STT).
    Text(String),
    /// Control message.
    Control(PipelineControl),
}

/// Pipeline control commands.
pub enum PipelineControl {
    /// Shut down the pipeline gracefully.
    Stop,
    /// Pause processing.
    Pause,
    /// Resume processing.
    Resume,
}

/// Messages produced by a pipeline.
pub enum PipelineOutput {
    /// Synthesized or processed audio.
    Audio(AudioFrame),
    /// Transcription result.
    Transcript(Transcript),
    /// Agent text response (before TTS).
    AgentText(String),
    /// Pipeline performance metrics.
    Metrics(PipelineMetrics),
}

/// Real-time latency and quality metrics from pipeline stages.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct PipelineMetrics {
    /// TTS synthesis latency in milliseconds.
    pub tts_latency_ms: f64,
    /// First audio latency in milliseconds.
    pub tts_first_audio_latency_ms: f64,
    /// STT transcription latency in milliseconds.
    pub stt_latency_ms: f64,
    /// LLM agent reasoning latency in milliseconds.
    pub llm_latency_ms: f64,
    /// Total audio processed in milliseconds.
    pub total_audio_ms: u64,
    /// Ratio of speech frames to total frames (0.0–1.0).
    pub vad_speech_ratio: f32,
}

/// Helper to consume a TTS stream, forwarding frames to the output channel
/// and updating performance metrics.
pub async fn consume_tts_stream(
    mut stream: Pin<Box<dyn Stream<Item = AudioResult<AudioFrame>> + Send>>,
    output_tx: &mpsc::Sender<PipelineOutput>,
    metrics: &Arc<RwLock<PipelineMetrics>>,
    tts_start: std::time::Instant,
    first_audio_sent: &mut bool,
) {
    use futures::StreamExt;
    while let Some(frame_res) = stream.next().await {
        match frame_res {
            Ok(frame) => {
                let duration_ms = frame.duration_ms;
                if output_tx.send(PipelineOutput::Audio(frame)).await.is_ok() {
                    let mut m = metrics.write().await;
                    m.total_audio_ms += duration_ms as u64;
                    if !*first_audio_sent {
                        *first_audio_sent = true;
                        m.tts_first_audio_latency_ms = tts_start.elapsed().as_millis() as f64;
                    }
                }
            }
            Err(e) => {
                tracing::error!(
                    error = ?e,
                    "pipeline.tts.stream_error",
                );
            }
        }
    }
}
