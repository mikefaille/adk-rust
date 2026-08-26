use std::borrow::Cow;
use std::hint::black_box;
use std::time::{Duration, Instant};

fn main() {
    // 192,000 bytes = 1 second of 48kHz stereo 16-bit PCM (96,000 samples, or 50 standard 20ms WebRTC frames)
    let buffer_size = 192_000;
    let bytes: Vec<u8> = vec![0; buffer_size];
    let iterations = 100;
    let warmup = 10;

    let mut manual_durations = Vec::with_capacity(iterations);
    let mut iter_durations = Vec::new();
    let mut bytemuck_durations = Vec::new();

    let mut samples1 = Vec::new();
    let mut samples2 = Vec::new();

    // Warm-up
    for _ in 0..warmup {
        let mut s = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            s.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }
        black_box(s);

        let s: Vec<i16> = bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        black_box(s);

        let s: Cow<[i16]> = unsafe {
            Cow::Borrowed(std::slice::from_raw_parts(bytes.as_ptr() as *const i16, bytes.len() / 2))
        };
        black_box(s);
    }

    // Benchmark
    for _ in 0..iterations {
        let start = Instant::now();
        let mut s = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            s.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }
        manual_durations.push(start.elapsed());
        samples1 = s;

        let start = Instant::now();
        let s: Vec<i16> = bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        iter_durations.push(start.elapsed());
        samples2 = s;

        let start = Instant::now();
        let s: Cow<[i16]> = unsafe {
            Cow::Borrowed(std::slice::from_raw_parts(bytes.as_ptr() as *const i16, bytes.len() / 2))
        };
        black_box(s);
        bytemuck_durations.push(start.elapsed());
    }

    assert_eq!(samples1, samples2, "Fatal: pcm16 outputs differ!");

    print_stats("Manual Loop", &mut manual_durations, buffer_size);
    print_stats("Iterator / Collect", &mut iter_durations, buffer_size);
    print_stats("Absolute Zero-Copy (bytemuck simulated)", &mut bytemuck_durations, buffer_size);
}

fn print_stats(name: &str, durations: &mut [Duration], buffer_bytes: usize) {
    durations.sort_unstable();
    let count = durations.len();
    let sum: Duration = durations.iter().sum();
    let mean = sum / count as u32;

    let median = if count.is_multiple_of(2) {
        (durations[count / 2 - 1] + durations[count / 2]) / 2
    } else {
        durations[count / 2]
    };

    let mean_f64 = mean.as_secs_f64();
    let variance = durations
        .iter()
        .map(|d| {
            let diff = d.as_secs_f64() - mean_f64;
            diff * diff
        })
        .sum::<f64>()
        / count as f64;
    let stddev = Duration::from_secs_f64(variance.sqrt());

    let throughput_mb_s = if mean_f64 > 0.0 {
        (buffer_bytes as f64 / (1024.0 * 1024.0)) / mean_f64
    } else {
        f64::INFINITY
    };

    println!("=== {} ===", name);
    println!("Throughput: {:.2} MB/s", throughput_mb_s);
    println!("Mean:       {:?}", mean);
    println!("Median:     {:?}", median);
    println!("StdDev:     {:?}", stddev);
    println!("Min:        {:?}", durations[0]);
    println!("Max:        {:?}", durations[count - 1]);
    println!();
}
