//! Canonical audio buffer type used throughout the crate.

use std::borrow::Cow;

/// The canonical audio buffer — raw PCM-16 LE samples with metadata.
///
/// All `adk-audio` components produce and consume `AudioFrame` values,
/// eliminating format negotiation between pipeline stages.
///
/// # Example
///
/// ```
/// use adk_audio::AudioFrame;
///
/// let silence = AudioFrame::silence(16000, 1, 100);
/// assert_eq!(silence.sample_rate, 16000);
/// assert_eq!(silence.channels, 1);
/// assert_eq!(silence.duration_ms, 100);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct AudioFrame<'a> {
    /// Raw PCM-16 LE sample data.
    pub data: Cow<'a, [i16]>,
    /// Sample rate in Hz (e.g. 16000, 24000, 44100, 48000).
    pub sample_rate: u32,
    /// Number of channels (1 = mono, 2 = stereo).
    pub channels: u8,
    /// Duration in milliseconds, computed from data length.
    pub duration_ms: u32,
    /// Samples per channel.
    pub samples_per_channel: u32,
}

impl<'a> AudioFrame<'a> {
    /// Create a new `AudioFrame` from raw PCM-16 LE data.
    ///
    /// Duration is computed automatically from the data length, sample rate,
    /// and channel count.
    pub fn new(data: impl Into<Cow<'a, [i16]>>, sample_rate: u32, channels: u8) -> Self {
        let data = data.into();
        let samples_per_channel =
            if channels > 0 && sample_rate > 0 { data.len() as u32 / channels as u32 } else { 0 };
        let duration_ms = if sample_rate > 0 {
            (samples_per_channel as u64 * 1000 / sample_rate as u64) as u32
        } else {
            0
        };
        Self { data, sample_rate, channels, duration_ms, samples_per_channel }
    }

    /// View the raw data as a slice of i16 samples.
    pub fn samples(&self) -> &[i16] {
        &self.data
    }

    /// Create a silent `AudioFrame` of the given duration.
    pub fn silence(sample_rate: u32, channels: u8, duration_ms: u32) -> Self {
        let n_samples = (sample_rate as usize * channels as usize * duration_ms as usize) / 1000;
        Self {
            data: Cow::Owned(vec![0i16; n_samples]),
            sample_rate,
            channels,
            duration_ms,
            samples_per_channel: if channels > 0 { n_samples as u32 / channels as u32 } else { 0 },
        }
    }
}

/// Merge multiple `AudioFrame` values into a single contiguous frame.
///
/// All frames must share the same sample rate and channel count.
/// Returns an empty frame if the input is empty.
pub fn merge_frames<'a>(frames: &[AudioFrame<'a>]) -> AudioFrame<'static> {
    if frames.is_empty() {
        return AudioFrame::new(Cow::Owned(vec![]), 16000, 1);
    }
    let sample_rate = frames[0].sample_rate;
    let channels = frames[0].channels;
    let total_len: usize = frames.iter().map(|f| f.data.len()).sum();
    let mut buf = Vec::with_capacity(total_len);
    for f in frames {
        buf.extend_from_slice(&f.data);
    }
    AudioFrame::new(Cow::Owned(buf), sample_rate, channels)
}
