use std::time::Instant;

fn main() {
    let bytes: Vec<u8> = (0..10_000_000).map(|i| (i % 256) as u8).collect();

    // Method 1: Manual loop
    let start = Instant::now();
    let mut samples1 = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        samples1.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }
    println!("Manual loop: {:?}", start.elapsed());

    // Method 2: Iterator
    let start = Instant::now();
    let samples2: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    println!("Iterator: {:?}", start.elapsed());

    // Method 3: bytes.array_chunks
    // (Available in nightly, but we can simulate it with arrayref or chunks_exact)
}
