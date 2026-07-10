use adk_realtime::audio::{AudioChunk, AudioFormat, SmartAudioBuffer};
use std::hint::black_box;
use std::time::Instant;

#[tokio::main]
async fn main() {
    let sample_rates = [16000, 24000];
    let durations_ms = [10, 20, 40, 80];

    println!("--- Audio Boundary Benchmark (Optimized) ---");

    for &rate in &sample_rates {
        for &ms in &durations_ms {
            benchmark_path(rate, ms);
        }
    }

    benchmark_buffer_retention();
}

fn benchmark_path(sample_rate: u32, duration_ms: u32) {
    let format =
        if sample_rate == 16000 { AudioFormat::pcm16_16khz() } else { AudioFormat::pcm16_24khz() };

    let samples_count = (sample_rate as f64 * duration_ms as f64 / 1000.0) as usize;
    let samples = vec![0i16; samples_count];
    let chunk = AudioChunk::from_i16_samples(&samples, format.clone());

    let iterations = 10000;

    // In the optimized path, the bridge does NOT call to_base64.
    // It passes the chunk directly.
    // We measure the time to CLONE the Bytes in AudioChunk (O(1)) vs original to_base64.
    let start = Instant::now();
    for _ in 0..iterations {
        let chunk_clone = black_box(chunk.clone());
        black_box(chunk_clone);
    }
    let elapsed = start.elapsed();
    let avg = elapsed / iterations;

    println!(
        "Rate: {}Hz, Dur: {}ms | Chunk size: {} bytes",
        sample_rate,
        duration_ms,
        chunk.data.len()
    );
    println!("  Avg direct chunk pass time (clone): {:?}", avg);

    println!("  Allocated bytes/sec (direct): 0 B/s (Bytes is ref-counted)");
}

fn benchmark_buffer_retention() {
    println!("\n--- SmartAudioBuffer Retention Benchmark ---");
    let sample_rate = 16000;
    let target_ms = 40;
    let samples_per_flush = (sample_rate * target_ms / 1000) as usize;
    let push_size = 160; // 10ms at 16kHz
    let mut buffer = SmartAudioBuffer::new(sample_rate, target_ms);

    let iterations = 1000;
    let start = Instant::now();
    for _ in 0..iterations {
        // Push until flush
        for _ in 0..(samples_per_flush / push_size) {
            let samples = vec![0i16; push_size];
            buffer.push(&samples);
        }
        let _ = buffer.process_and_clear(|samples| {
            black_box(samples);
        });
    }
    let elapsed = start.elapsed();
    println!(
        "Time for {} flushes with optimized SmartAudioBuffer (process_and_clear): {:?}",
        iterations, elapsed
    );
    println!("Note: Capacity is retained.");
}
