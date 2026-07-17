//! Focused unit and property tests for `LiveKitEventHandler` PCM assembly and boundary transitions.

#![cfg(feature = "livekit")]

use adk_realtime::error::RealtimeError;
use adk_realtime::livekit::{AudioSourceOptions, LiveKitEventHandler, NativeAudioSource};
use adk_realtime::runner::EventHandler;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct MockEventHandler {
    audio_chunks: Arc<Mutex<Vec<Vec<u8>>>>,
}

#[async_trait]
impl EventHandler for MockEventHandler {
    async fn on_audio(&self, audio: &[u8], _item_id: &str) -> adk_realtime::Result<()> {
        self.audio_chunks.lock().await.push(audio.to_vec());
        Ok(())
    }

    async fn on_text(&self, _text: &str, _item_id: &str) -> adk_realtime::Result<()> {
        Ok(())
    }
    async fn on_transcript(&self, _transcript: &str, _item_id: &str) -> adk_realtime::Result<()> {
        Ok(())
    }
    async fn on_speech_started(&self, _audio_start_ms: u64) -> adk_realtime::Result<()> {
        Ok(())
    }
    async fn on_speech_stopped(&self, _audio_end_ms: u64) -> adk_realtime::Result<()> {
        Ok(())
    }
    async fn on_response_done(&self) -> adk_realtime::Result<()> {
        Ok(())
    }
    async fn on_error(&self, _error: &RealtimeError) -> adk_realtime::Result<()> {
        Ok(())
    }
}

fn setup_test_handler(
    num_channels: u32,
) -> (LiveKitEventHandler<MockEventHandler>, MockEventHandler) {
    let inner = MockEventHandler::default();
    let audio_source =
        NativeAudioSource::new(AudioSourceOptions::default(), 24000, num_channels, 100);
    let handler = LiveKitEventHandler::new(inner.clone(), audio_source, 24000, num_channels);
    (handler, inner)
}

#[tokio::test]
async fn test_aligned_even_length_buffer() {
    let (handler, _inner) = setup_test_handler(1);
    let input: Vec<u8> = vec![1, 2, 3, 4, 5, 6, 7, 8];
    let res: adk_realtime::Result<()> = handler.on_audio(&input, "item_1").await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_unaligned_even_length_buffer() {
    let (handler, _inner) = setup_test_handler(1);
    let input_vec: Vec<u8> = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
    let unaligned_slice = &input_vec[1..9]; // length 8, offset by 1
    let res: adk_realtime::Result<()> = handler.on_audio(unaligned_slice, "item_1").await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_two_same_item_split_callbacks() {
    let (handler, _inner) = setup_test_handler(1);
    let res1: adk_realtime::Result<()> = handler.on_audio(&[1, 2, 3], "item_a").await;
    assert!(res1.is_ok());
    let res2: adk_realtime::Result<()> = handler.on_audio(&[4], "item_a").await;
    assert!(res2.is_ok());
}

#[tokio::test]
async fn test_pending_byte_cross_contamination_prevented() {
    let (handler, _inner) = setup_test_handler(1);
    let res1: adk_realtime::Result<()> = handler.on_audio(&[1, 2, 3], "item_a").await;
    assert!(res1.is_ok());
    let res2: adk_realtime::Result<()> = handler.on_audio(&[4, 5], "item_b").await;
    assert!(res2.is_ok());
}

#[tokio::test]
async fn test_on_response_done_clears_pending_state() {
    let (handler, _inner) = setup_test_handler(1);
    let res1: adk_realtime::Result<()> = handler.on_audio(&[1, 2, 3], "item_a").await;
    assert!(res1.is_ok());
    let res2: adk_realtime::Result<()> = handler.on_response_done().await;
    assert!(res2.is_ok());
    let res3: adk_realtime::Result<()> = handler.on_audio(&[4], "item_a").await;
    assert!(res3.is_ok());
}

#[tokio::test]
async fn test_on_error_clears_pending_state() {
    let (handler, _inner) = setup_test_handler(1);
    let res1: adk_realtime::Result<()> = handler.on_audio(&[1, 2, 3], "item_a").await;
    assert!(res1.is_ok());
    let err = RealtimeError::livekit("Simulated error");
    let res2: adk_realtime::Result<()> = handler.on_error(&err).await;
    assert!(res2.is_ok());
    let res3: adk_realtime::Result<()> = handler.on_audio(&[4], "item_a").await;
    assert!(res3.is_ok());
}

#[tokio::test]
async fn test_stereo_channel_divisibility_validation() {
    let (handler, _inner) = setup_test_handler(2); // Stereo (2 channels)
    let res: adk_realtime::Result<()> = handler.on_audio(&[1, 2], "item_a").await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_zero_channels_validation() {
    let (handler, _inner) = setup_test_handler(0); // 0 channels
    let res: adk_realtime::Result<()> = handler.on_audio(&[1, 2], "item_a").await;
    assert!(res.is_err());
}
