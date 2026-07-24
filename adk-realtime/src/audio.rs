//! Audio models and format conversions.

pub mod g711;

use bytes::BytesMut;
use serde::{Deserialize, Serialize};

/// Audio encoding formats supported by realtime APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AudioEncoding {
    /// 16-bit PCM audio (most common).
    #[serde(rename = "pcm16")]
    #[default]
    Pcm16,
    /// G.711 μ-law encoding.
    #[serde(rename = "g711_ulaw")]
    G711Ulaw,
    /// G.711 A-law encoding.
    #[serde(rename = "g711_alaw")]
    G711Alaw,
}

impl std::fmt::Display for AudioEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pcm16 => write!(f, "pcm16"),
            Self::G711Ulaw => write!(f, "g711_ulaw"),
            Self::G711Alaw => write!(f, "g711_alaw"),
        }
    }
}

/// Description of the audio format used by the models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFormat {
    /// The sample rate of the audio in Hz (e.g., 24000).
    pub sample_rate: u32,
    /// Number of channels (usually 1 for mono).
    pub channels: u8,
    /// Bits per sample (usually 16).
    pub bits_per_sample: u8,
    /// The encoding format (e.g., PCM16, Opus).
    pub encoding: AudioEncoding,
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self::pcm16_24khz()
    }
}

impl AudioFormat {
    /// Create a new audio format specification.
    pub fn new(
        sample_rate: u32,
        channels: u8,
        bits_per_sample: u8,
        encoding: AudioEncoding,
    ) -> Self {
        Self { sample_rate, channels, bits_per_sample, encoding }
    }

    /// Default PCM16 format at 24kHz (OpenAI standard).
    pub fn pcm16_24khz() -> Self {
        Self {
            sample_rate: 24000,
            channels: 1,
            bits_per_sample: 16,
            encoding: AudioEncoding::Pcm16,
        }
    }

    /// PCM16 format at 16kHz (Gemini input default).
    pub fn pcm16_16khz() -> Self {
        Self {
            sample_rate: 16000,
            channels: 1,
            bits_per_sample: 16,
            encoding: AudioEncoding::Pcm16,
        }
    }

    /// PCM16 format at 8kHz (often used in telephony).
    pub fn pcm16_8khz() -> Self {
        Self { sample_rate: 8000, channels: 1, bits_per_sample: 16, encoding: AudioEncoding::Pcm16 }
    }

    /// G.711 μ-law format at 8kHz (Telephony standard).
    pub fn g711_ulaw() -> Self {
        Self {
            sample_rate: 8000,
            channels: 1,
            bits_per_sample: 8,
            encoding: AudioEncoding::G711Ulaw,
        }
    }

    /// G.711 A-law format at 8kHz (Telephony standard).
    pub fn g711_alaw() -> Self {
        Self {
            sample_rate: 8000,
            channels: 1,
            bits_per_sample: 8,
            encoding: AudioEncoding::G711Alaw,
        }
    }

    /// Returns the number of bytes per second for this format.
    pub fn bytes_per_second(&self) -> usize {
        (self.sample_rate * self.channels as u32 * (self.bits_per_sample as u32 / 8)) as usize
    }

    /// Calculates the duration of an audio buffer in milliseconds based on this format.
    pub fn duration_ms(&self, byte_count: usize) -> f64 {
        (byte_count as f64 / self.bytes_per_second() as f64) * 1000.0
    }
}

/// A block of audio data bundled with its format specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioChunk {
    /// The raw audio data.
    pub data: bytes::Bytes,
    /// The format specification of the audio.
    pub format: AudioFormat,
}

impl AudioChunk {
    /// Create a new audio chunk.
    pub fn new(data: impl Into<bytes::Bytes>, format: AudioFormat) -> Self {
        Self { data: data.into(), format }
    }

    /// Create a PCM16 24kHz audio chunk (OpenAI format).
    pub fn pcm16_24khz(data: impl Into<bytes::Bytes>) -> Self {
        Self::new(data, AudioFormat::pcm16_24khz())
    }

    /// Create a PCM16 16kHz audio chunk (Gemini input format).
    pub fn pcm16_16khz(data: impl Into<bytes::Bytes>) -> Self {
        Self::new(data, AudioFormat::pcm16_16khz())
    }

    /// Get duration of this audio chunk in milliseconds.
    pub fn duration_ms(&self) -> f64 {
        self.format.duration_ms(self.data.len())
    }

    /// Encode audio data as base64.
    pub fn to_base64(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&self.data)
    }

    /// Decode audio data from base64.
    pub fn from_base64(encoded: &str, format: AudioFormat) -> Result<Self, base64::DecodeError> {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD.decode(encoded)?;
        Ok(Self::new(data, format))
    }

    /// Create an AudioChunk from i16 samples (converts to PCM16 little-endian bytes).
    ///
    /// This is useful when working with audio APIs (like LiveKit) that provide
    /// samples as `i16` slices rather than raw byte buffers.
    #[cfg(target_endian = "little")]
    pub fn from_i16_samples(samples: &[i16], format: AudioFormat) -> Self {
        let bytes: &[u8] = bytemuck::cast_slice(samples);
        let data = bytes::Bytes::copy_from_slice(bytes);
        Self::new(data, format)
    }

    #[cfg(not(target_endian = "little"))]
    pub fn from_i16_samples(samples: &[i16], format: AudioFormat) -> Self {
        let mut data = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        Self::new(data, format)
    }

    /// Convert the audio data to a slice of i16 samples (assuming PCM16 little-endian).
    ///
    /// Returns an error string if the data length is not even (not valid PCM16).
    #[cfg(target_endian = "little")]
    pub fn to_i16_samples(&self) -> Result<std::borrow::Cow<'_, [i16]>, String> {
        if !self.data.len().is_multiple_of(2) {
            return Err(format!(
                "Invalid data length for PCM16: {} (must be even)",
                self.data.len()
            ));
        }

        // bytemuck::cast_slice requires the slice to be aligned
        #[allow(clippy::manual_is_multiple_of)]
        if (self.data.as_ptr() as usize) % std::mem::align_of::<i16>() == 0 {
            let samples: &[i16] = bytemuck::cast_slice(self.data.as_ref());
            Ok(std::borrow::Cow::Borrowed(samples))
        } else {
            let mut samples = Vec::with_capacity(self.data.len() / 2);
            for chunk in self.data.chunks_exact(2) {
                samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
            }
            Ok(std::borrow::Cow::Owned(samples))
        }
    }

    #[cfg(not(target_endian = "little"))]
    pub fn to_i16_samples(&self) -> Result<std::borrow::Cow<'_, [i16]>, String> {
        if !self.data.len().is_multiple_of(2) {
            return Err(format!(
                "Invalid data length for PCM16: {} (must be even)",
                self.data.len()
            ));
        }
        let mut samples = Vec::with_capacity(self.data.len() / 2);
        for chunk in self.data.chunks_exact(2) {
            samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }
        Ok(std::borrow::Cow::Owned(samples))
    }
}

/// Buffers audio samples until a target duration is reached.
///
/// Smart buffering (e.g., 40-80ms) is essential for AI voice services to:
/// 1. **Reduce Network Overhead**: Aggregating small frames into larger chunks
///    drastically reduces packet rate, lowering CPU usage and bandwidth overhead.
/// 2. **Improve Model Performance**: Provides sufficient context for Voice Activity
///    Detection (VAD) to distinguish speech from noise.
/// 3. **Resist Jitter**: Smooths out mobile networks.
/// 4. **Latency Trade-off**: Maintains a real-time feel while gaining stability.
#[derive(Debug, Clone)]
pub struct SmartAudioBuffer {
    buffer: BytesMut,
    sample_rate: u32,
    target_duration_ms: u32,
}

impl SmartAudioBuffer {
    /// Create a new smart audio buffer.
    pub fn new(sample_rate: u32, target_duration_ms: u32) -> Self {
        let target_bytes = Self::calculate_target_bytes(sample_rate, target_duration_ms);
        Self { buffer: BytesMut::with_capacity(target_bytes), sample_rate, target_duration_ms }
    }

    /// Push new samples into the buffer.
    #[cfg(target_endian = "little")]
    pub fn push(&mut self, samples: &[i16]) {
        let bytes: &[u8] = bytemuck::cast_slice(samples);
        self.buffer.extend_from_slice(bytes);
    }

    #[cfg(not(target_endian = "little"))]
    pub fn push(&mut self, samples: &[i16]) {
        for sample in samples {
            self.buffer.extend_from_slice(&sample.to_le_bytes());
        }
    }

    fn should_flush(&self) -> bool {
        self.buffer.len() >= self.target_bytes_len()
            && self.target_duration_ms > 0
            && self.sample_rate > 0
    }

    /// Flush the buffer if the target duration has been reached.
    ///
    /// Note: This copies the bytes to a new `Vec<i16>` to maintain backward compatibility.
    /// For zero-copy, use `pop_chunk()`.
    pub fn flush(&mut self) -> Option<Vec<i16>> {
        if self.should_flush() {
            let bytes = self.buffer.split().freeze();
            let samples = bytemuck::cast_slice(&bytes).to_vec();
            Some(samples)
        } else {
            None
        }
    }

    /// Flush any remaining samples in the buffer.
    pub fn flush_remaining(&mut self) -> Option<Vec<i16>> {
        if self.buffer.is_empty() {
            None
        } else {
            let bytes = self.buffer.split().freeze();
            let samples = bytemuck::cast_slice(&bytes).to_vec();
            Some(samples)
        }
    }

    /// Returns the current capacity of the underlying buffer in bytes.
    pub fn capacity(&self) -> usize {
        self.buffer.capacity()
    }

    /// Process the buffered samples with a closure and then clear the buffer while retaining capacity.
    ///
    /// This is a more efficient alternative to `flush()` when the samples don't need
    /// to be owned by the caller after the closure returns (e.g., they are immediately
    /// encoded to base64 or copied).
    pub fn process_and_clear<F, R>(&mut self, f: F) -> Option<R>
    where
        F: FnOnce(&[i16]) -> R,
    {
        if self.should_flush() {
            let samples: &[i16] = bytemuck::cast_slice(&self.buffer);
            let result = f(samples);
            self.buffer.clear();
            Some(result)
        } else {
            None
        }
    }

    fn calculate_target_bytes(sample_rate: u32, target_duration_ms: u32) -> usize {
        (sample_rate as u64 * target_duration_ms as u64).div_ceil(1000) as usize * 2
    }

    fn target_bytes_len(&self) -> usize {
        Self::calculate_target_bytes(self.sample_rate, self.target_duration_ms)
    }

    /// Pops an AudioChunk with zero heap allocations on steady-state hot paths.
    /// This method extracts exactly `target_duration_ms` of audio and returns it,
    /// leaving any excess in the buffer.
    pub fn pop_chunk(&mut self, format: AudioFormat) -> Option<AudioChunk> {
        if self.should_flush() {
            let target_bytes = self.target_bytes_len();
            let chunk_data = self.buffer.split_to(target_bytes).freeze();
            Some(AudioChunk::new(chunk_data, format))
        } else {
            None
        }
    }

    /// Pops any remaining samples in the buffer as an AudioChunk.
    pub fn pop_remaining_chunk(&mut self, format: AudioFormat) -> Option<AudioChunk> {
        if self.buffer.is_empty() {
            None
        } else {
            let chunk_data = self.buffer.split().freeze();
            Some(AudioChunk::new(chunk_data, format))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smart_audio_buffer_flush_threshold() {
        let sample_rate = 1000;
        let target_ms = 100;
        // 1000 samples/sec -> 1 sample = 1ms.
        // target 100ms -> 100 samples.

        let mut buffer = SmartAudioBuffer::new(sample_rate, target_ms);

        // Push 50 samples (50ms)
        buffer.push(&[0; 50]);
        assert!(buffer.flush().is_none());

        // Push 49 samples (total 99ms)
        buffer.push(&[0; 49]);
        assert!(buffer.flush().is_none());

        // Push 1 sample (total 100ms)
        buffer.push(&[0; 1]);
        let flushed = buffer.flush();
        assert!(flushed.is_some());
        assert_eq!(flushed.unwrap().len(), 100);
        assert!(buffer.buffer.is_empty());
    }

    #[test]
    fn test_smart_audio_buffer_flush_remaining() {
        let sample_rate = 1000;
        let target_ms = 100;
        let mut buffer = SmartAudioBuffer::new(sample_rate, target_ms);

        buffer.push(&[0; 50]);
        assert!(buffer.flush().is_none());

        let remaining = buffer.flush_remaining();
        assert!(remaining.is_some());
        assert_eq!(remaining.unwrap().len(), 50);
        assert!(buffer.buffer.is_empty());
    }

    #[test]
    fn test_smart_audio_buffer_empty_flush() {
        let mut buffer = SmartAudioBuffer::new(1000, 100);
        assert!(buffer.flush().is_none());
        assert!(buffer.flush_remaining().is_none());
    }

    #[test]
    fn test_smart_audio_buffer_pop_threshold() {
        let sample_rate = 1000;
        let target_ms = 100;
        // 1000 samples/sec -> 1 sample = 1ms.
        // target 100ms -> 100 samples (200 bytes).

        let mut buffer = SmartAudioBuffer::new(sample_rate, target_ms);

        // Push 50 samples (50ms)
        buffer.push(&[0; 50]);
        assert!(buffer.pop_chunk(AudioFormat::pcm16_24khz()).is_none());

        // Push 49 samples (total 99ms)
        buffer.push(&[0; 49]);
        assert!(buffer.pop_chunk(AudioFormat::pcm16_24khz()).is_none());

        // Push 1 sample (total 100ms)
        buffer.push(&[0; 1]);
        let popped = buffer.pop_chunk(AudioFormat::pcm16_24khz());
        assert!(popped.is_some());
        assert_eq!(popped.unwrap().data.len(), 200); // 100 samples * 2 bytes
        assert!(buffer.buffer.is_empty());
    }

    #[test]
    fn test_smart_audio_buffer_pop_remaining() {
        let sample_rate = 1000;
        let target_ms = 100;
        let mut buffer = SmartAudioBuffer::new(sample_rate, target_ms);

        buffer.push(&[0; 50]);
        assert!(buffer.pop_chunk(AudioFormat::pcm16_24khz()).is_none());

        let remaining = buffer.pop_remaining_chunk(AudioFormat::pcm16_24khz());
        assert!(remaining.is_some());
        assert_eq!(remaining.unwrap().data.len(), 100);
        assert!(buffer.buffer.is_empty());
    }

    #[test]
    fn test_smart_audio_buffer_empty_pop() {
        let mut buffer = SmartAudioBuffer::new(1000, 100);
        assert!(buffer.pop_chunk(AudioFormat::pcm16_24khz()).is_none());
        assert!(buffer.pop_remaining_chunk(AudioFormat::pcm16_24khz()).is_none());
    }

    #[test]
    fn test_smart_audio_buffer_zero_duration_guard() {
        let mut buffer = SmartAudioBuffer::new(1000, 0);
        buffer.push(&[0; 50]);
        assert!(buffer.pop_chunk(AudioFormat::pcm16_24khz()).is_none());
    }

    #[test]
    fn test_audio_format_bytes_per_second() {
        let pcm16_24k = AudioFormat::pcm16_24khz();
        assert_eq!(pcm16_24k.bytes_per_second(), 48000); // 24000 * 1 * 2

        let pcm16_16k = AudioFormat::pcm16_16khz();
        assert_eq!(pcm16_16k.bytes_per_second(), 32000); // 16000 * 1 * 2
    }

    #[test]
    fn test_audio_format_duration() {
        let format = AudioFormat::pcm16_24khz();
        // 48000 bytes = 1 second
        let duration = format.duration_ms(48000);
        assert!((duration - 1000.0).abs() < 0.001);
    }

    #[test]
    fn test_audio_chunk_base64() {
        let original = AudioChunk::pcm16_24khz(vec![0, 1, 2, 3, 4, 5]);
        let encoded = original.to_base64();
        let decoded = AudioChunk::from_base64(&encoded, AudioFormat::pcm16_24khz()).unwrap();
        assert_eq!(original.data, decoded.data);
    }

    #[test]
    fn test_i16_samples_roundtrip() {
        let samples: Vec<i16> = vec![0, 1, -1, 32767, -32768, 1000, -1000];
        let chunk = AudioChunk::from_i16_samples(&samples, AudioFormat::pcm16_24khz());
        let recovered = chunk.to_i16_samples().unwrap();
        assert_eq!(samples.as_slice(), recovered.as_ref());
    }

    #[test]
    fn test_i16_samples_empty() {
        let chunk = AudioChunk::from_i16_samples(&[], AudioFormat::pcm16_24khz());
        assert!(chunk.data.is_empty());
        assert_eq!(chunk.to_i16_samples().unwrap().as_ref(), Vec::<i16>::new().as_slice());
    }

    #[test]
    fn test_i16_samples_odd_bytes_error() {
        let chunk = AudioChunk::pcm16_24khz(vec![0, 1, 2]); // 3 bytes = invalid PCM16
        assert!(chunk.to_i16_samples().is_err());
    }

    #[test]
    fn test_smart_audio_buffer_capacity_retention() {
        let mut buffer = SmartAudioBuffer::new(1000, 10); // 10ms target
        buffer.push(&[0; 100]); // 100ms
        let initial_cap = buffer.capacity();
        assert!(initial_cap >= 200);

        let processed = buffer.pop_chunk(AudioFormat::pcm16_24khz());
        assert!(processed.is_some());
    }
}
