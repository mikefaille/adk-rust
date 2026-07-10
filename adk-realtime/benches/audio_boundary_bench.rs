use adk_realtime::audio::{AudioChunk, AudioFormat};
use adk_realtime::error::Result;
use adk_realtime::events::{ClientEvent, ServerEvent, ToolResponse};
use adk_realtime::model::RealtimeModel;
use adk_realtime::runner::RealtimeRunner;
use adk_realtime::session::{ContextMutationOutcome, RealtimeSession};
use async_trait::async_trait;
use futures::Stream;
use std::hint::black_box;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

struct BenchSession;
#[async_trait]
impl RealtimeSession for BenchSession {
    fn session_id(&self) -> &str {
        "bench"
    }
    fn is_connected(&self) -> bool {
        true
    }
    async fn send_audio(&self, audio: &AudioChunk) -> Result<()> {
        black_box(audio);
        Ok(())
    }
    async fn send_audio_base64(&self, audio_base64: &str) -> Result<()> {
        black_box(audio_base64);
        Ok(())
    }
    async fn send_text(&self, _text: &str) -> Result<()> {
        Ok(())
    }
    async fn send_tool_response(&self, _response: ToolResponse) -> Result<()> {
        Ok(())
    }
    async fn commit_audio(&self) -> Result<()> {
        Ok(())
    }
    async fn clear_audio(&self) -> Result<()> {
        Ok(())
    }
    async fn create_response(&self) -> Result<()> {
        Ok(())
    }
    async fn interrupt(&self) -> Result<()> {
        Ok(())
    }
    async fn send_event(&self, _event: ClientEvent) -> Result<()> {
        Ok(())
    }
    async fn next_event(&self) -> Option<Result<ServerEvent>> {
        None
    }
    fn events(&self) -> Pin<Box<dyn Stream<Item = Result<ServerEvent>> + Send + '_>> {
        Box::pin(futures::stream::empty())
    }
    async fn close(&self) -> Result<()> {
        Ok(())
    }
    async fn mutate_context(
        &self,
        _config: adk_realtime::config::RealtimeConfig,
    ) -> Result<ContextMutationOutcome> {
        Ok(ContextMutationOutcome::Applied)
    }
}

struct BenchModel;
#[async_trait]
impl RealtimeModel for BenchModel {
    fn provider(&self) -> &str {
        "bench"
    }
    fn model_id(&self) -> &str {
        "bench"
    }
    fn supported_input_formats(&self) -> Vec<AudioFormat> {
        vec![]
    }
    fn supported_output_formats(&self) -> Vec<AudioFormat> {
        vec![]
    }
    fn available_voices(&self) -> Vec<&str> {
        vec![]
    }
    async fn connect(
        &self,
        _config: adk_realtime::config::RealtimeConfig,
    ) -> Result<Box<dyn RealtimeSession>> {
        Ok(Box::new(BenchSession))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let runner = RealtimeRunner::builder().model(Arc::new(BenchModel)).build()?;
    runner.connect().await?;
    let runner = Arc::new(runner);

    let sample_rates = [16000, 24000];
    let durations_ms = [10, 20, 40, 80];

    println!("--- Audio Boundary Benchmark: Baseline vs Optimized ---");

    for &rate in &sample_rates {
        for &ms in &durations_ms {
            run_comparison(runner.clone(), rate, ms).await?;
        }
    }

    Ok(())
}

async fn run_comparison(
    runner: Arc<RealtimeRunner>,
    sample_rate: u32,
    duration_ms: u32,
) -> Result<()> {
    let format =
        if sample_rate == 16000 { AudioFormat::pcm16_16khz() } else { AudioFormat::pcm16_24khz() };

    let samples_count = (sample_rate as f64 * duration_ms as f64 / 1000.0) as usize;
    let samples = vec![0i16; samples_count];
    let chunk = AudioChunk::from_i16_samples(&samples, format);

    let iterations = 10000;

    // Baseline: Bridge (simulated) -> to_base64 -> runner.send_audio_base64
    let start = Instant::now();
    for _ in 0..iterations {
        let b64 = chunk.to_base64();
        runner.send_audio_base64(&b64).await?;
    }
    let baseline_elapsed = start.elapsed();

    // Optimized: Bridge (simulated) -> runner.send_audio_chunk
    let start = Instant::now();
    for _ in 0..iterations {
        runner.send_audio_chunk(&chunk).await?;
    }
    let optimized_elapsed = start.elapsed();

    println!(
        "Rate: {}Hz, Dur: {}ms | Chunk size: {} bytes",
        sample_rate,
        duration_ms,
        chunk.data.len()
    );
    println!("  Baseline (base64):  {:>10?}", baseline_elapsed / iterations);
    println!("  Optimized (raw):    {:>10?}", optimized_elapsed / iterations);
    let speedup = baseline_elapsed.as_secs_f64() / optimized_elapsed.as_secs_f64();
    println!("  Speedup:            {:.2}x", speedup);

    Ok(())
}
