use std::time::{Duration, Instant};
use std::hint::black_box;
use std::borrow::Cow;

fn main() {
    let bytes: Vec<u8> = (0..10_000_000).map(|i| (i % 256) as u8).collect();
    let num_iterations = 50;
    let num_warmup = 10;

    let mut manual_durations = Vec::new();
    let mut iter_durations = Vec::new();
    let mut bytemuck_durations = Vec::new();

    // Warm-up and benchmark iterations
    for iteration in 0..(num_warmup + num_iterations) {
        let is_warmup = iteration < num_warmup;

        // Method 1: Manual loop
        let start = Instant::now();
        let mut samples1 = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            samples1.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }
        black_box(samples1);
        let dur1 = start.elapsed();
        if !is_warmup {
            manual_durations.push(dur1);
        }

        // Method 2: Iterator
        let start = Instant::now();
        let samples2: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        black_box(samples2);
        let dur2 = start.elapsed();
        if !is_warmup {
            iter_durations.push(dur2);
        }

        // Method 3: Bytemuck (if little endian)
        let start = Instant::now();
        let samples3: Cow<[i16]> = unsafe {
            let ptr = bytes.as_ptr() as *const i16;
            let len = bytes.len() / 2;
            Cow::Borrowed(std::slice::from_raw_parts(ptr, len))
        };
        black_box(samples3);
        let dur3 = start.elapsed();
        if !is_warmup {
            bytemuck_durations.push(dur3);
        }
    }

    // Calculate statistics
    let calc_stats = |durations: &Vec<Duration>| {
        let mut sum = 0.0;
        let mut sum_sq = 0.0;
        let n = durations.len() as f64;

        for d in durations {
            let val = d.as_secs_f64() * 1000.0; // in milliseconds
            sum += val;
            sum_sq += val * val;
        }

        let mean = sum / n;
        let variance = (sum_sq / n) - (mean * mean);
        let stddev = variance.sqrt();
        (mean, stddev)
    };

    let (manual_mean, manual_stddev) = calc_stats(&manual_durations);
    let (iter_mean, iter_stddev) = calc_stats(&iter_durations);
    let (bytemuck_mean, bytemuck_stddev) = calc_stats(&bytemuck_durations);

    println!("Results over {} iterations (after {} warm-up):", num_iterations, num_warmup);
    println!("Manual loop:      {:.3} ms +/- {:.3} ms", manual_mean, manual_stddev);
    println!("Iterator:         {:.3} ms +/- {:.3} ms", iter_mean, iter_stddev);
    println!("Zero-copy cast:   {:.6} ms +/- {:.6} ms", bytemuck_mean, bytemuck_stddev);
}
