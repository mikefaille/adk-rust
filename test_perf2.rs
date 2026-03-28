use std::time::{Duration, Instant};
use std::hint::black_box;
use std::convert::TryInto;

fn main() {
    let bytes: Vec<u8> = (0..10_000_000).map(|i| (i % 256) as u8).collect();
    let num_iterations = 50;
    let num_warmup = 10;

    let mut iter1_durations = Vec::new();
    let mut iter2_durations = Vec::new();

    for iteration in 0..(num_warmup + num_iterations) {
        let is_warmup = iteration < num_warmup;

        let start = Instant::now();
        let samples1: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        black_box(samples1);
        let dur1 = start.elapsed();
        if !is_warmup {
            iter1_durations.push(dur1);
        }

        let start = Instant::now();
        let samples2: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes(c.try_into().unwrap()))
            .collect();
        black_box(samples2);
        let dur2 = start.elapsed();
        if !is_warmup {
            iter2_durations.push(dur2);
        }
    }

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

    let (iter1_mean, iter1_stddev) = calc_stats(&iter1_durations);
    let (iter2_mean, iter2_stddev) = calc_stats(&iter2_durations);

    println!("Current Iterator:   {:.3} ms +/- {:.3} ms", iter1_mean, iter1_stddev);
    println!("Alternate Iter:     {:.3} ms +/- {:.3} ms", iter2_mean, iter2_stddev);
}
