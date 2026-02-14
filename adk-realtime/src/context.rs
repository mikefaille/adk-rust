//! Context engineering utilities for Realtime sessions.
//!
//! This module provides tools for managing conversation history and context
//! in long-running voice sessions, specifically addressing the unique challenges
//! of realtime audio:
//!
//! 1. **Interruption Handling**: Pruning model speech that the user interrupted.
//! 2. **Context Caching**: Automatically offloading conversation history to
//!    long-term storage (Gemini Context Caching) to save tokens and reduce latency.
//!
//! # Example
//!
//! ```rust,ignore
//! use adk_realtime::context::{InterruptionCompactor, gemini::AudioContextCacheProcessor};
//!
//! let mut compactor = InterruptionCompactor::new();
//! // ... receive events ...
//! compactor.push(event);
//!
//! // On interruption signal:
//! compactor.handle_interruption(timestamp);
//! ```

use crate::events::ServerEvent;

/// Managing interruptions by pruning invalid history.
///
/// When a user interrupts the model, the model might have generated audio
/// that the user never heard (or heard only partially). This struct
/// helps maintain a "true" history of the conversation by pruning
/// model events that occurred after the interruption.
///
/// This is critical for preventing the model from "hallucinating" a conversation
/// flow where it said things the user never acknowledged.
#[derive(Debug, Default)]
pub struct InterruptionCompactor {
    history: Vec<ServerEvent>,
}

impl InterruptionCompactor {
    /// Create a new compactor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an event to the history.
    pub fn push(&mut self, event: ServerEvent) {
        self.history.push(event);
    }

    /// Handle an interruption.
    ///
    /// This removes all pending model output events (AudioDelta, TextDelta, etc.)
    /// from the tail of the history, assuming they were interrupted.
    ///
    /// # Arguments
    ///
    /// * `timestamp_ms` - The timestamp of the interruption (currently unused,
    ///   tail pruning strategy is used).
    pub fn handle_interruption(&mut self, _timestamp_ms: u64) {
        // Remove trailing audio/text deltas which represent the interrupted response.
        while let Some(last) = self.history.last() {
             match last {
                 ServerEvent::AudioDelta { .. } |
                 ServerEvent::TextDelta { .. } |
                 ServerEvent::TranscriptDelta { .. } => {
                     self.history.pop();
                 }
                 // If we hit a Done event, the response completed, so we stop pruning.
                 // (Unless the interruption timestamp implies we should prune even completed events,
                 // but for now we assume interruption happens during streaming).
                 ServerEvent::ResponseDone { .. } |
                 ServerEvent::TextDone { .. } |
                 ServerEvent::AudioDone { .. } => {
                     break;
                 }
                 // If we hit a user event or other system event, stop.
                 _ => break,
             }
        }
    }

    /// Get the current valid history.
    pub fn history(&self) -> &[ServerEvent] {
        &self.history
    }
}

#[cfg(feature = "gemini")]
pub mod gemini {
    use super::*;
    use crate::gemini::GeminiRealtimeModel;
    use crate::events::ClientEvent;
    use crate::error::Result;
    use adk_gemini::{GeminiClient, CacheBuilder, Content, Role, Part, GeminiBuilder};
    use adk_gemini::GeminiLiveBackend;
    use std::sync::Arc;
    use std::time::Duration;

    /// Approx 1 token per 1700 base64 chars (approx 1 sec audio ~ 25 tokens).
    const BASE64_CHARS_PER_AUDIO_TOKEN: usize = 1700;
    /// Approx 1 token per 4 text chars.
    const CHARS_PER_TEXT_TOKEN: usize = 4;
    /// Default TTL for cached content (10 minutes).
    const DEFAULT_CACHE_TTL_SECS: u64 = 600;

    /// Automatically caches audio context when it exceeds a threshold.
    ///
    /// Realtime audio sessions consume tokens very rapidly (approx 10x text).
    /// This processor monitors the accumulated token count and, when a threshold
    /// is reached, automatically creates a Gemini Context Cache.
    ///
    /// The returned cache name can then be used to update the session configuration,
    /// effectively "resetting" the active window while preserving history.
    pub struct AudioContextCacheProcessor {
        client: Arc<GeminiClient>,
        buffer: Vec<Content>,
        token_count: usize,
        threshold: usize,
    }

    impl AudioContextCacheProcessor {
        /// Create a new processor.
        ///
        /// # Arguments
        ///
        /// * `model` - The source model to derive authentication from.
        /// * `threshold` - Token count threshold to trigger caching (e.g., 100,000).
        pub fn new(model: &GeminiRealtimeModel, threshold: usize) -> Result<Self> {
            let client = match model.backend() {
                GeminiLiveBackend::Studio { api_key } => {
                    GeminiBuilder::new(api_key.clone()).build().map_err(|e| crate::error::RealtimeError::config(format!("Failed to build Gemini client: {}", e)))?
                }
                #[cfg(feature = "vertex")]
                GeminiLiveBackend::VertexADC { .. } => {
                     GeminiBuilder::new_without_api_key()
                        .with_google_cloud_adc()
                        .map_err(|e| crate::error::RealtimeError::config(format!("Failed to configure ADC: {}", e)))?
                        .build()
                        .map_err(|e| crate::error::RealtimeError::config(format!("Failed to build Gemini client: {}", e)))?
                }
                #[cfg(feature = "vertex")]
                GeminiLiveBackend::Vertex(_) => {
                    return Err(crate::error::RealtimeError::config("Vertex(context) backend not supported for auto-caching yet"));
                }
            };

            Ok(Self {
                client: Arc::new(client),
                buffer: Vec::new(),
                token_count: 0,
                threshold,
            })
        }

        /// Process an input event (User).
        ///
        /// Adds user audio/text to the accumulation buffer.
        pub fn process_input(&mut self, event: &ClientEvent) {
             match event {
                 ClientEvent::AppendInputAudio { audio, .. } => {
                     self.token_count += audio.len() / BASE64_CHARS_PER_AUDIO_TOKEN;

                     self.buffer.push(Content {
                         role: Role::User,
                         parts: vec![Part::InlineData {
                             mime_type: "audio/pcm".to_string(),
                             data: audio.clone(),
                         }],
                     });
                 }
                 ClientEvent::TextMessage { text, .. } => {
                     self.token_count += text.len() / CHARS_PER_TEXT_TOKEN;
                     self.buffer.push(Content::user(text));
                 }
                 _ => {}
             }
        }

        /// Process an output event (Model).
        ///
        /// Adds model responses to the accumulation buffer.
        pub fn process_output(&mut self, event: &ServerEvent) {
            match event {
                ServerEvent::TextDone { text, .. } => {
                     self.token_count += text.len() / CHARS_PER_TEXT_TOKEN;
                     self.buffer.push(Content::model(text));
                }
                // Note: We don't easily get the full audio blob from ServerEvent unless we accumulate deltas.
                // For this implementation, we only cache text responses from the model to save bandwidth/complexity,
                // or we rely on the fact that the user heard the audio so context is established.
                // Ideally, we should cache the model's audio too if possible.
                // But ServerEvent::AudioDone doesn't contain data.
                // We'd need to track AudioDeltas.
                _ => {}
            }
        }

        /// Check if threshold is reached and create a cache.
        ///
        /// Returns `Ok(Some(cache_name))` if a new cache was created.
        /// Returns `Ok(None)` if threshold not reached.
        pub async fn check_and_cache(&mut self) -> Result<Option<String>> {
            if self.token_count < self.threshold {
                return Ok(None);
            }

            if self.buffer.is_empty() {
                return Ok(None);
            }

            // Efficiently move contents out of buffer without cloning
            let contents = std::mem::take(&mut self.buffer);

            // Create cache with default TTL
            let builder = CacheBuilder::new(self.client.clone())
                .with_contents(contents)
                .with_ttl(Duration::from_secs(DEFAULT_CACHE_TTL_SECS));

            let handle = builder.execute().await.map_err(|e| {
                crate::error::RealtimeError::provider(format!("Cache creation failed: {}", e))
            })?;

            // Reset token count (buffer is already cleared by take)
            self.token_count = 0;

            Ok(Some(handle.name().to_string()))
        }
    }
}
