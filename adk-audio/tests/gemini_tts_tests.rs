//! Tests for Gemini 3.1 TTS streaming and validation.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use adk_audio::{
    AudioFrame, AudioPipelineBuilder, AudioResult, GeminiTts, PipelineInput, PipelineOutput,
    TtsProvider, TtsRequest, Voice,
};
use bytes::Bytes;
use futures::Stream;
use serde_json::json;

// ══════════════════════════════════════════════════════════════════════
// Mock Server Helpers
// ══════════════════════════════════════════════════════════════════════

async fn spawn_mock_sse_server(events: Vec<serde_json::Value>) -> String {
    use tokio::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0; 1024];
            let _ = socket.read(&mut buf).await;

            let mut response = String::from(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\n",
            );
            for event in events {
                response.push_str(&format!("data: {}\n\n", event.to_string()));
            }
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.flush().await;
        }
    });

    format!("http://127.0.0.1:{port}/")
}

// ══════════════════════════════════════════════════════════════════════
// Tests for Validation Rules
// ══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_unsupported_mime_type_returns_error() {
    let base_url = spawn_mock_sse_server(vec![json!({
        "event_type": "step.delta",
        "index": 1,
        "delta": {
            "type": "audio",
            "data": "UklGRgAAAABXQVZFZg==",
            "mime_type": "audio/mp3",
            "sample_rate": 24000,
            "channels": 1
        }
    })])
    .await;

    let config = adk_audio::providers::tts::CloudTtsConfig::new("test_key").with_base_url(base_url);
    let tts = GeminiTts::new(config);

    let req = TtsRequest { text: "hello".to_string(), ..Default::default() };
    let mut stream = tts.synthesize_stream(&req).await.unwrap();

    use futures::StreamExt;
    let res = stream.next().await.unwrap();
    assert!(res.is_err());
    let err_str = res.err().unwrap().to_string();
    assert!(err_str.contains("unsupported audio format"));
}

#[tokio::test]
async fn test_invalid_base64_returns_error() {
    let base_url = spawn_mock_sse_server(vec![json!({
        "event_type": "step.delta",
        "index": 1,
        "delta": {
            "type": "audio",
            "data": "invalid-base64-!!!",
            "mime_type": "audio/pcm",
            "sample_rate": 24000,
            "channels": 1
        }
    })])
    .await;

    let config = adk_audio::providers::tts::CloudTtsConfig::new("test_key").with_base_url(base_url);
    let tts = GeminiTts::new(config);

    let req = TtsRequest { text: "hello".to_string(), ..Default::default() };
    let mut stream = tts.synthesize_stream(&req).await.unwrap();

    use futures::StreamExt;
    let res = stream.next().await.unwrap();
    assert!(res.is_err());
    let err_str = res.err().unwrap().to_string();
    assert!(err_str.contains("invalid base64"));
}

#[tokio::test]
async fn test_missing_initial_metadata_returns_error() {
    let base_url = spawn_mock_sse_server(vec![json!({
        "event_type": "step.delta",
        "index": 1,
        "delta": {
            "type": "audio",
            "data": "UklGRgAAAABXQVZFZg==",
            "sample_rate": 24000,
            "channels": 1
        }
    })])
    .await;

    let config = adk_audio::providers::tts::CloudTtsConfig::new("test_key").with_base_url(base_url);
    let tts = GeminiTts::new(config);

    let req = TtsRequest { text: "hello".to_string(), ..Default::default() };
    let mut stream = tts.synthesize_stream(&req).await.unwrap();

    use futures::StreamExt;
    let res = stream.next().await.unwrap();
    assert!(res.is_err());
    let err_str = res.err().unwrap().to_string();
    assert!(err_str.contains("missing initial metadata: mime_type"));
}

#[tokio::test]
async fn test_contradictory_metadata_returns_error() {
    let base_url = spawn_mock_sse_server(vec![
        json!({
            "event_type": "step.delta",
            "index": 1,
            "delta": {
                "type": "audio",
                "data": "UklGRgAAAABXQVZFZg==",
                "mime_type": "audio/pcm",
                "sample_rate": 24000,
                "channels": 1
            }
        }),
        json!({
            "event_type": "step.delta",
            "index": 1,
            "delta": {
                "type": "audio",
                "data": "UklGRgAAAABXQVZFZg==",
                "sample_rate": 16000
            }
        }),
    ])
    .await;

    let config = adk_audio::providers::tts::CloudTtsConfig::new("test_key").with_base_url(base_url);
    let tts = GeminiTts::new(config);

    let req = TtsRequest { text: "hello".to_string(), ..Default::default() };
    let mut stream = tts.synthesize_stream(&req).await.unwrap();

    use futures::StreamExt;
    let res1 = stream.next().await.unwrap();
    assert!(res1.is_ok());

    let res2 = stream.next().await.unwrap();
    assert!(res2.is_err());
    let err_str = res2.err().unwrap().to_string();
    assert!(err_str.contains("contradictory metadata"));
}

#[tokio::test]
async fn test_uri_only_audio_returns_error() {
    let base_url = spawn_mock_sse_server(vec![json!({
        "event_type": "step.delta",
        "index": 1,
        "delta": {
            "type": "audio",
            "uri": "http://example.com/audio.wav"
        }
    })])
    .await;

    let config = adk_audio::providers::tts::CloudTtsConfig::new("test_key").with_base_url(base_url);
    let tts = GeminiTts::new(config);

    let req = TtsRequest { text: "hello".to_string(), ..Default::default() };
    let mut stream = tts.synthesize_stream(&req).await.unwrap();

    use futures::StreamExt;
    let res = stream.next().await.unwrap();
    assert!(res.is_err());
    let err_str = res.err().unwrap().to_string();
    assert!(err_str.contains("URI-only audio is not supported"));
}

// ══════════════════════════════════════════════════════════════════════
// Pipeline Streaming and Telemetry Tests
// ══════════════════════════════════════════════════════════════════════

struct DelayedMockTts;

#[async_trait::async_trait]
impl TtsProvider for DelayedMockTts {
    async fn synthesize(&self, _req: &TtsRequest) -> AudioResult<AudioFrame> {
        Ok(AudioFrame::new(Bytes::from(vec![0; 400]), 16000, 1))
    }

    async fn synthesize_stream(
        &self,
        _req: &TtsRequest,
    ) -> AudioResult<Pin<Box<dyn Stream<Item = AudioResult<AudioFrame>> + Send>>> {
        let stream = async_stream::stream! {
            // First chunk at 100ms
            tokio::time::sleep(Duration::from_millis(100)).await;
            yield Ok(AudioFrame::new(Bytes::from(vec![0; 480]), 24000, 1));

            // Second chunk at 200ms
            tokio::time::sleep(Duration::from_millis(100)).await;
            yield Ok(AudioFrame::new(Bytes::from(vec![0; 480]), 24000, 1));
        };
        Ok(Box::pin(stream))
    }

    fn voice_catalog(&self) -> &[Voice] {
        &[]
    }
}

#[tokio::test]
async fn test_pipeline_streaming_latency_and_telemetry() {
    let handle = AudioPipelineBuilder::new()
        .tts(Arc::new(DelayedMockTts))
        .build_tts()
        .unwrap();

    let start_time = std::time::Instant::now();
    handle.input_tx.send(PipelineInput::Text("hello world".to_string())).await.unwrap();

    let mut output_rx = handle.output_rx;

    // 1. Wait for first audio frame
    let first_out = output_rx.recv().await.unwrap();
    let first_arrival = start_time.elapsed();
    assert!(first_arrival >= Duration::from_millis(100));

    match first_out {
        PipelineOutput::Audio(frame) => {
            assert_eq!(frame.sample_rate, 24000);
            assert_eq!(frame.channels, 1);
            // 480 bytes at PCM16 (2 bytes/sample) mono is 240 samples.
            // 240 samples / 24000 Hz = 10 ms.
            assert_eq!(frame.duration_ms, 10);
        }
        _ => panic!("Expected PipelineOutput::Audio"),
    }

    // 2. Wait for second audio frame
    let second_out = output_rx.recv().await.unwrap();
    let second_arrival = start_time.elapsed();
    assert!(second_arrival >= Duration::from_millis(200));

    match second_out {
        PipelineOutput::Audio(frame) => {
            assert_eq!(frame.duration_ms, 10);
        }
        _ => panic!("Expected PipelineOutput::Audio"),
    }

    // Proves first PipelineOutput::Audio arrives *before* the stream finishes completing
    assert!(first_arrival < second_arrival);

    // Let the loop run fully
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify metrics inside handle
    let metrics = handle.metrics.read().await;
    assert_eq!(metrics.total_audio_ms, 20); // 10ms + 10ms
    assert!(metrics.tts_first_audio_latency_ms >= 100.0);
    assert!(metrics.tts_latency_ms >= 200.0);
    assert!(metrics.tts_latency_ms > metrics.tts_first_audio_latency_ms);
}

// ══════════════════════════════════════════════════════════════════════
// Explicitly Gated Live Integration Test (requires GEMINI_API_KEY)
// ══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_live_gemini_tts_stream() {
    let Ok(api_key) = std::env::var("GEMINI_API_KEY") else {
        println!("test_live_gemini_tts_stream: GEMINI_API_KEY is not set. Skipping live proof.");
        return;
    };

    let config = adk_audio::providers::tts::CloudTtsConfig::new(api_key);
    let tts = GeminiTts::new(config).with_model("gemini-3.1-flash-tts-preview");

    let req = TtsRequest {
        text: "This is a live test of Gemini 3.1 TTS streaming.".to_string(),
        ..Default::default()
    };

    let start_time = std::time::Instant::now();
    let mut stream = match tts.synthesize_stream(&req).await {
        Ok(s) => s,
        Err(e) => {
            panic!("Failed to start live stream: {:?}", e);
        }
    };

    use futures::StreamExt;
    let mut total_chunks = 0;
    let mut total_bytes = 0;
    let mut first_arrival: Option<Duration> = None;

    while let Some(chunk_res) = stream.next().await {
        match chunk_res {
            Ok(frame) => {
                total_chunks += 1;
                total_bytes += frame.data.len();
                if first_arrival.is_none() {
                    first_arrival = Some(start_time.elapsed());
                }
            }
            Err(e) => {
                panic!("Error in live TTS stream: {:?}", e);
            }
        }
    }

    let end_arrival = start_time.elapsed();

    assert!(total_chunks > 0, "No chunks received in live test");
    assert!(total_bytes > 0, "Empty audio data received in live test");

    println!(
        "Live proof success! Received {} chunks ({} bytes). First arrival: {:?}, stream complete: {:?}",
        total_chunks, total_bytes, first_arrival.unwrap(), end_arrival
    );
    assert!(first_arrival.unwrap() < end_arrival);
}
