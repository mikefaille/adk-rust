use std::borrow::Cow;

/// A stub representation of LiveKit's NativeAudioSource to satisfy downstream unit tests.
pub struct NativeAudioSource;

/// A stub representation of LiveKit's AudioFrame to satisfy downstream unit tests.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioFrame<'a> {
    pub data: Cow<'a, [i16]>,
    pub sample_rate: u32,
    pub num_channels: u32,
    pub samples_per_channel: u32,
}
