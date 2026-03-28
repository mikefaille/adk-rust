use std::time::Instant;
use std::hint::black_box;
use std::borrow::Cow;

fn main() {
    let bytes: Vec<u8> = (0..10_000_000).map(|i| (i % 256) as u8).collect();

    // Method 1: Manual loop
    let start = Instant::now();
    let mut samples1 = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        samples1.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }
    black_box(samples1);
    let dur1 = start.elapsed();
    println!("Manual loop: {:?}", dur1);

    // Method 2: Iterator
    let start = Instant::now();
    let samples2: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    black_box(samples2);
    let dur2 = start.elapsed();
    println!("Iterator: {:?}", dur2);

    // Method 3: Bytemuck (if little endian)
    let start = Instant::now();
    // Simulate bytemuck if aligned, bytemuck just does pointer cast
    // For safety in this script we just use slice::from_raw_parts
    let samples3: Cow<[i16]> = unsafe {
        let ptr = bytes.as_ptr() as *const i16;
        let len = bytes.len() / 2;
        Cow::Borrowed(std::slice::from_raw_parts(ptr, len))
    };
    black_box(samples3);
    let dur3 = start.elapsed();
    println!("Zero-copy (simulated bytemuck): {:?}", dur3);
}
