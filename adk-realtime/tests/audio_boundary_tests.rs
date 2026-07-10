use adk_realtime::audio::{AudioChunk, AudioFormat, SmartAudioBuffer};

#[test]
fn test_pcm_byte_equivalence() {
    let samples = vec![i16::MIN, -1, 0, 1, i16::MAX];
    let format = AudioFormat::pcm16_24khz();
    let chunk = AudioChunk::from_i16_samples(&samples, format.clone());

    // Manual verification of little-endian bytes
    let expected_bytes = vec![
        0x00, 0x80, // i16::MIN (-32768)
        0xFF, 0xFF, // -1
        0x00, 0x00, // 0
        0x01, 0x00, // 1
        0xFF, 0x7F, // i16::MAX (32767)
    ];
    assert_eq!(chunk.data.as_ref(), &expected_bytes);

    let recovered = chunk.to_i16_samples().unwrap();
    assert_eq!(samples, recovered);
}

#[test]
fn test_misaligned_and_empty_bytes() {
    let format = AudioFormat::pcm16_24khz();

    // Empty
    let empty_chunk = AudioChunk::new(vec![], format.clone());
    assert!(empty_chunk.to_i16_samples().unwrap().is_empty());

    // Misaligned (odd length)
    let misaligned_chunk = AudioChunk::new(vec![0x01, 0x02, 0x03], format.clone());
    assert!(misaligned_chunk.to_i16_samples().is_err());
}

#[test]
fn test_smart_audio_buffer_retained_capacity() {
    let mut buffer = SmartAudioBuffer::new(16000, 40);
    let large_push = vec![0i16; 1600]; // 100ms
    buffer.push(&large_push);

    let initial_cap = buffer.capacity();
    assert!(initial_cap >= 1600);

    // Flush via process_and_clear
    buffer.process_and_clear(|_| {});
    assert_eq!(buffer.capacity(), initial_cap);
    assert_eq!(buffer.flush_remaining(), None);

    // Refill and check capacity again
    buffer.push(&large_push);
    assert_eq!(buffer.capacity(), initial_cap);
}

#[test]
fn test_interruption_clear_behavior() {
    let mut buffer = SmartAudioBuffer::new(16000, 40);
    buffer.push(&[1, 2, 3]);

    // Simulation of interruption/clear
    let remaining = buffer.flush_remaining();
    assert!(remaining.is_some());
    assert_eq!(remaining.unwrap(), vec![1, 2, 3]);
    assert_eq!(buffer.capacity(), 0); // mem::take loses capacity, which is expected for flush_remaining
}
