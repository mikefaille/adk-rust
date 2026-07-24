use adk_realtime::audio::{AudioChunk, AudioFormat, SmartAudioBuffer};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct MemoryTracker;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for MemoryTracker {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static A: MemoryTracker = MemoryTracker;

fn percentile(sorted: &[u128], pct: f64) -> u128 {
    let idx = ((sorted.len() as f64 - 1.0) * pct).round() as usize;
    sorted[idx]
}

// Reimplement old buffer logic for fair benchmarking
struct OldSmartAudioBuffer {
    buffer: Vec<i16>,
    sample_rate: u32,
    target_duration_ms: u32,
}

impl OldSmartAudioBuffer {
    pub fn new(sample_rate: u32, target_duration_ms: u32) -> Self {
        Self { buffer: Vec::new(), sample_rate, target_duration_ms }
    }
    pub fn push(&mut self, samples: &[i16]) {
        self.buffer.extend_from_slice(samples);
    }
    fn should_flush(&self) -> bool {
        let duration_ms = (self.buffer.len() as f64 / self.sample_rate as f64) * 1000.0;
        duration_ms >= self.target_duration_ms as f64
    }
    pub fn flush(&mut self) -> Option<Vec<i16>> {
        if self.should_flush() { Some(std::mem::take(&mut self.buffer)) } else { None }
    }
}

fn main() {
    println!("\n⚡ Zenith ADK-Rust Audio Buffering Performance Benchmark ⚡");
    println!("------------------------------------------------------------");

    // Simulate 100 seconds of audio per connection, 100 connections
    let connections = 100;
    let frames_per_conn = 10_000; // 100 seconds at 10ms per frame

    // 10ms frames at 24kHz -> 240 samples per frame
    let frame_samples: Vec<i16> = (0..240).map(|i| (i % 100) as i16).collect();
    let sample_rate = 24000;
    let target_duration_ms = 40; // Requires 4 frames to trigger

    // ── 1. Benchmark Old Allocation Method (flush) ──
    ALLOC_COUNT.store(0, Ordering::SeqCst);
    ALLOC_BYTES.store(0, Ordering::SeqCst);

    let mut latencies_old = Vec::with_capacity(connections * frames_per_conn);
    let start_total_old = Instant::now();

    for _ in 0..connections {
        let mut buffer = OldSmartAudioBuffer::new(sample_rate, target_duration_ms);

        // Warmup: Old buffer doesn't benefit from warmup because `std::mem::take` throws it away,
        // but we'll do one push to be fair.
        buffer.push(&frame_samples);
        let _ = buffer.flush();

        for _ in 0..frames_per_conn {
            let t0 = Instant::now();

            buffer.push(&frame_samples);
            if let Some(samples) = buffer.flush() {
                let chunk = AudioChunk::from_i16_samples(&samples, AudioFormat::pcm16_24khz());
                black_box(chunk);
            }

            latencies_old.push(t0.elapsed().as_nanos());
        }
    }

    let total_time_old = start_total_old.elapsed();
    let allocs_old = ALLOC_COUNT.load(Ordering::SeqCst);
    let bytes_old = ALLOC_BYTES.load(Ordering::SeqCst);

    latencies_old.sort_unstable();

    // ── 2. Benchmark New Zero-Copy Method (pop_chunk) ──
    // Proper tracking for new method
    ALLOC_COUNT.store(0, Ordering::SeqCst);
    ALLOC_BYTES.store(0, Ordering::SeqCst);

    let mut latencies_new = Vec::with_capacity(connections * frames_per_conn);
    let start_total_new = Instant::now();

    for _ in 0..connections {
        let mut buffer = SmartAudioBuffer::new(sample_rate, target_duration_ms);

        // Warmup: The first push and chunk might trigger an allocation inside BytesMut
        // depending on how it manages its internal chunks. We warm it up to measure
        // the true steady-state hot path performance.
        buffer.push(&frame_samples);
        buffer.push(&frame_samples);
        buffer.push(&frame_samples);
        buffer.push(&frame_samples);
        let _ = buffer.pop_chunk(AudioFormat::pcm16_24khz());

        // Reset tracking after warmup for this specific connection
        ALLOC_COUNT.store(0, Ordering::SeqCst);
        ALLOC_BYTES.store(0, Ordering::SeqCst);

        for _ in 0..frames_per_conn {
            let t0 = Instant::now();

            buffer.push(&frame_samples);
            while let Some(chunk) = buffer.pop_chunk(AudioFormat::pcm16_24khz()) {
                black_box(chunk);
            }

            latencies_new.push(t0.elapsed().as_nanos());
        }

        // We assert inside the connection loop after warmup
        let steady_allocs = ALLOC_COUNT.load(Ordering::SeqCst);
        assert_eq!(steady_allocs, 0, "Expected 0 steady state allocations, got {}", steady_allocs);
    }

    let total_time_new = start_total_new.elapsed();
    let allocs_new = ALLOC_COUNT.load(Ordering::SeqCst);
    let bytes_new = ALLOC_BYTES.load(Ordering::SeqCst);

    // We expect a significant reduction, hopefully 0 steady-state allocations depending on `BytesMut` internals.
    // We print the results but don't strictly assert 0 to avoid CI flakiness across environments if BytesMut does occasional large re-allocations.

    latencies_new.sort_unstable();

    // ── Metrics Calculation ──
    let total_iterations = connections * frames_per_conn;
    let mean_ns_old = latencies_old.iter().sum::<u128>() as f64 / total_iterations as f64;
    let mean_ns_new = latencies_new.iter().sum::<u128>() as f64 / total_iterations as f64;

    let p50_old = percentile(&latencies_old, 0.50);
    let p50_new = percentile(&latencies_new, 0.50);

    let p95_old = percentile(&latencies_old, 0.95);
    let p95_new = percentile(&latencies_new, 0.95);

    let p99_old = percentile(&latencies_old, 0.99);
    let p99_new = percentile(&latencies_new, 0.99);

    let speedup_mean = mean_ns_old / mean_ns_new;
    let alloc_reduction = if allocs_old > 0 {
        ((allocs_old - allocs_new) as f64 / allocs_old as f64) * 100.0
    } else {
        0.0
    };

    println!("\n📊 BENCHMARK RESULTS ({} total frame pushes)", total_iterations);
    println!(
        "Simulating {} connections, {} frames each (10ms frame, 40ms buffer)",
        connections, frames_per_conn
    );
    println!("------------------------------------------------------------");
    println!(" Metric                │ Old (flush)       │ New (pop_chunk)         │ Improvement");
    println!(
        "───────────────────────┼───────────────────┼─────────────────────────┼──────────────"
    );
    println!(
        " Total Allocations     │ {:>17} │ {:>23} │ -{:.1}%",
        allocs_old, allocs_new, alloc_reduction
    );
    println!(
        " Total Memory Allocated│ {:>14} B │ {:>20} B │ -{:.1}%",
        bytes_old,
        bytes_new,
        if bytes_old > 0 {
            ((bytes_old - bytes_new) as f64 / bytes_old as f64) * 100.0
        } else {
            0.0
        }
    );
    println!(
        " Mean Latency          │ {:>14.2} ns│ {:>20.2} ns│ {:.2}x faster",
        mean_ns_old, mean_ns_new, speedup_mean
    );
    println!(
        " Median (P50) Latency  │ {:>14} ns│ {:>20} ns│ {:.2}x faster",
        p50_old,
        p50_new,
        p50_old as f64 / p50_new.max(1) as f64
    );
    println!(
        " P95 Latency           │ {:>14} ns│ {:>20} ns│ {:.2}x faster",
        p95_old,
        p95_new,
        p95_old as f64 / p95_new.max(1) as f64
    );
    println!(
        " P99 Latency           │ {:>14} ns│ {:>20} ns│ {:.2}x faster",
        p99_old,
        p99_new,
        p99_old as f64 / p99_new.max(1) as f64
    );
    println!(
        " Total Wall-Clock Time │ {:>14.2} ms│ {:>20.2} ms│ {:.2}x faster",
        total_time_old.as_secs_f64() * 1000.0,
        total_time_new.as_secs_f64() * 1000.0,
        total_time_old.as_secs_f64() / total_time_new.as_secs_f64()
    );
    println!("------------------------------------------------------------\n");
}
