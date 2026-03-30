//! Multi-track audio mixer with per-track volume control.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::error::{AudioError, AudioResult};
use crate::frame::AudioFrame;
use crate::traits::AudioProcessor;

/// A track in the mixer with volume and buffered audio.
struct MixerTrack {
    volume: f32,
    buffer: Option<AudioFrame<'static>>,
}

/// Multi-track audio mixer.
///
/// Combines multiple named audio tracks into a single output with
/// per-track volume control. Missing tracks are treated as silence.
///
/// # Example
///
/// ```ignore
/// let mut mixer = Mixer::new(24000);
/// mixer.add_track("narration", 1.0);
/// mixer.add_track("music", 0.3);
/// mixer.push_frame("narration", narration_frame);
/// mixer.push_frame("music", music_frame);
/// let mixed = mixer.mix()?;
/// ```
pub struct Mixer {
    tracks: HashMap<String, MixerTrack>,
    output_sample_rate: u32,
}

impl Mixer {
    /// Create a new mixer with the given output sample rate.
    pub fn new(output_sample_rate: u32) -> Self {
        Self { tracks: HashMap::new(), output_sample_rate }
    }

    /// Add a named track with the given volume (0.0–1.0).
    pub fn add_track(&mut self, name: impl Into<String>, volume: f32) {
        self.tracks
            .insert(name.into(), MixerTrack { volume: volume.clamp(0.0, 1.0), buffer: None });
    }

    /// Set the volume for a named track.
    pub fn set_volume(&mut self, name: &str, volume: f32) {
        if let Some(track) = self.tracks.get_mut(name) {
            track.volume = volume.clamp(0.0, 1.0);
        }
    }

    /// Push an audio frame to a named track.
    pub fn push_frame(&mut self, track: &str, frame: AudioFrame<'static>) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.buffer = Some(frame);
        }
    }

    /// Mix all tracks into a single output frame.
    ///
    /// Tracks without buffered audio are treated as silence.
    /// All tracks are mixed at the output sample rate.
    pub fn mix(&mut self) -> AudioResult<AudioFrame<'static>> {
        if self.tracks.is_empty() {
            return Err(AudioError::Fx("mixer has no tracks".into()));
        }

        // Validate formats
        for (name, track) in &self.tracks {
            if let Some(ref frame) = track.buffer {
                if frame.sample_rate != self.output_sample_rate {
                    return Err(AudioError::Fx(format!(
                        "Track '{}' sample rate {} does not match mixer output rate {}",
                        name, frame.sample_rate, self.output_sample_rate
                    )));
                }
                if frame.channels != 1 {
                    return Err(AudioError::Fx(format!(
                        "Track '{}' is not mono. Mixer currently only supports mono tracks.",
                        name
                    )));
                }
            }
        }

        // Find the maximum sample count across all buffered tracks
        let max_samples = self
            .tracks
            .values()
            .filter_map(|t| t.buffer.as_ref())
            .map(|f| f.data.len())
            .max()
            .unwrap_or(0);

        if max_samples == 0 {
            return Ok(AudioFrame::silence(self.output_sample_rate, 1, 0));
        }

        let mut mixed = vec![0i32; max_samples];

        for track in self.tracks.values() {
            let volume = track.volume;
            if let Some(ref frame) = track.buffer {
                let samples = frame.samples();
                for (i, &s) in samples.iter().enumerate() {
                    if i < max_samples {
                        mixed[i] += (s as f32 * volume) as i32;
                    }
                }
            }
        }

        // Clamp to i16 range
        let pcm: Vec<i16> = mixed.iter().map(|&s| s.clamp(-32768, 32767) as i16).collect();

        // Clear buffers
        for track in self.tracks.values_mut() {
            track.buffer = None;
        }

        Ok(AudioFrame::new(std::borrow::Cow::Owned(pcm), self.output_sample_rate, 1))
    }
}

#[async_trait]
impl AudioProcessor for Mixer {
    async fn process<'a>(&'a self, frame: &AudioFrame<'a>) -> AudioResult<AudioFrame<'static>> {
        // Single-track passthrough: apply first track's volume
        let volume = self.tracks.values().next().map(|t| t.volume).unwrap_or(1.0);

        let samples = frame.samples();
        let pcm: Vec<i16> = samples
            .iter()
            .map(|&s| {
                let scaled = (s as f32 * volume) as i32;
                scaled.clamp(-32768, 32767) as i16
            })
            .collect();

        Ok(AudioFrame::new(std::borrow::Cow::Owned(pcm), frame.sample_rate, frame.channels))
    }
}
