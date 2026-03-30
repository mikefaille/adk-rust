//! Audio processor trait and FxChain composition.

use async_trait::async_trait;

use crate::error::AudioResult;
use crate::frame::AudioFrame;

/// Trait for stateless or stateful DSP transforms on audio frames.
///
/// Implementors include normalizers, resamplers, noise suppressors,
/// compressors, and the `FxChain` itself (enabling nested chains).
///
/// # Example
///
/// ```rust,ignore
/// use adk_audio::traits::AudioProcessor;
/// use adk_audio::frame::AudioFrame;
/// use adk_audio::error::AudioResult;
/// use async_trait::async_trait;
/// use std::borrow::Cow;
///
/// struct MyProcessor;
/// #[async_trait]
/// impl AudioProcessor for MyProcessor {
///     async fn process<'a>(&'a self, frame: &AudioFrame<'a>) -> AudioResult<AudioFrame<'static>> {
///         // Operate on borrowed data from the input `frame`
///         let pcm = frame.samples().to_vec(); // own the data
///         Ok(AudioFrame::new(Cow::Owned(pcm), frame.sample_rate, frame.channels))
///     }
/// }
/// ```
#[async_trait]
pub trait AudioProcessor: Send + Sync {
    /// Process a single audio frame, returning the transformed result.
    async fn process<'a>(&'a self, frame: &AudioFrame<'a>) -> AudioResult<AudioFrame<'static>>;
}

/// An ordered chain of `AudioProcessor` stages applied in series.
///
/// The output of stage N becomes the input to stage N+1.
/// An empty chain returns the input frame unchanged.
///
/// # Example
///
/// ```ignore
/// let chain = FxChain::new()
///     .push(normalizer)
///     .push(resampler);
/// let output = chain.process(&input).await?;
/// ```
pub struct FxChain {
    stages: Vec<Box<dyn AudioProcessor>>,
}

impl FxChain {
    /// Create an empty FxChain.
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    /// Append a processing stage to the chain.
    pub fn push(mut self, processor: impl AudioProcessor + 'static) -> Self {
        self.stages.push(Box::new(processor));
        self
    }

    /// Returns the number of stages in the chain.
    pub fn len(&self) -> usize {
        self.stages.len()
    }

    /// Returns true if the chain has no stages.
    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }
}

impl Default for FxChain {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AudioProcessor for FxChain {
    async fn process<'a>(&'a self, frame: &AudioFrame<'a>) -> AudioResult<AudioFrame<'static>> {
        let mut produced: Option<AudioFrame<'static>> = None;
        for stage in &self.stages {
            let output =
                stage.process(if let Some(ref owned) = produced { owned } else { frame }).await?;
            produced = Some(output);
        }
        Ok(produced.unwrap_or_else(|| AudioFrame {
            data: std::borrow::Cow::Owned(frame.data.to_vec()),
            sample_rate: frame.sample_rate,
            channels: frame.channels,
            duration_ms: frame.duration_ms,
            samples_per_channel: frame.samples_per_channel,
        }))
    }
}
