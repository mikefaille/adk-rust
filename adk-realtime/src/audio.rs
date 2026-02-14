//! Audio format definitions and utilities.

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

/// Complete audio format specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFormat {
    /// Sample rate in Hz (e.g., 24000, 16000, 8000).
    pub sample_rate: u32,
    /// Number of audio channels (1 = mono, 2 = stereo).
    pub channels: u8,
    /// Bits per sample.
    pub bits_per_sample: u8,
    /// Audio encoding format.
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

    /// Standard PCM16 format at 24kHz (OpenAI default).
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

    /// G.711 μ-law format at 8kHz (telephony standard).
    pub fn g711_ulaw() -> Self {
        Self {
            sample_rate: 8000,
            channels: 1,
            bits_per_sample: 8,
            encoding: AudioEncoding::G711Ulaw,
        }
    }

    /// G.711 A-law format at 8kHz (telephony standard).
    pub fn g711_alaw() -> Self {
        Self {
            sample_rate: 8000,
            channels: 1,
            bits_per_sample: 8,
            encoding: AudioEncoding::G711Alaw,
        }
    }

    /// Calculate bytes per second for this format.
    pub fn bytes_per_second(&self) -> u32 {
        self.sample_rate * self.channels as u32 * (self.bits_per_sample / 8) as u32
    }

    /// Calculate duration in milliseconds for a given number of bytes.
    pub fn duration_ms(&self, bytes: usize) -> f64 {
        let bytes_per_ms = self.bytes_per_second() as f64 / 1000.0;
        bytes as f64 / bytes_per_ms
    }
}

/// Audio chunk with format information.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// Raw audio data.
    pub data: Vec<u8>,
    /// Audio format of this chunk.
    pub format: AudioFormat,
}

impl AudioChunk {
    /// Create a new audio chunk.
    pub fn new(data: Vec<u8>, format: AudioFormat) -> Self {
        Self { data, format }
    }

    /// Create a PCM16 24kHz audio chunk (OpenAI format).
    pub fn pcm16_24khz(data: Vec<u8>) -> Self {
        Self::new(data, AudioFormat::pcm16_24khz())
    }

    /// Create a PCM16 16kHz audio chunk (Gemini input format).
    pub fn pcm16_16khz(data: Vec<u8>) -> Self {
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

    /// Create an AudioChunk from i16 samples (converts to PCM16 LE bytes).
    pub fn from_i16_samples(samples: &[i16], format: AudioFormat) -> Self {
        let mut data = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            data.extend_from_slice(&sample.to_le_bytes());
        }
        Self::new(data, format)
    }

    /// Convert the audio data to a vector of i16 samples (assuming PCM16 LE).
    /// Returns an error if the data length is not even.
    pub fn to_i16_samples(&self) -> Result<Vec<i16>, String> {
        if self.data.len() % 2 != 0 {
            return Err(format!("Invalid data length for PCM16: {}", self.data.len()));
        }
        let mut samples = Vec::with_capacity(self.data.len() / 2);
        for chunk in self.data.chunks_exact(2) {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            samples.push(sample);
        }
        Ok(samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_i16_samples_conversion() {
        let samples: Vec<i16> = vec![0, 1, -1, 32767, -32768];
        let chunk = AudioChunk::from_i16_samples(&samples, AudioFormat::pcm16_24khz());

        let recovered = chunk.to_i16_samples().unwrap();
        assert_eq!(samples, recovered);

        // Test invalid data length
        let mut bad_chunk = chunk.clone();
        bad_chunk.data.push(0);
        assert!(bad_chunk.to_i16_samples().is_err());
    }
}
