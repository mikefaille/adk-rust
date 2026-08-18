//! Tests for Gemini TTS Interactions streaming in adk-audio.

use adk_audio::pipeline::types::PipelineOutput;
use adk_audio::providers::tts::{CloudTtsConfig, GeminiTts};
use adk_audio::traits::{TtsProvider, TtsRequest};
use futures::StreamExt;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn test_tts_with_mock_url(mock_url: String) -> GeminiTts {
    let mut config = CloudTtsConfig::new("test-api-key");
    config.base_url = Some(mock_url);
    GeminiTts::new(config)
}

#[tokio::test]
async fn test_gemini_tts_streaming_delayed_stream_and_first_audio() {
    let mock_server = MockServer::start().await;

    // Base64 for 4 bytes of PCM audio (2 samples of 16-bit PCM: [0x01, 0x02, 0x03, 0x04])
    let b64_chunk1 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [1, 2, 3, 4]);
    let b64_chunk2 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [5, 6, 7, 8]);

    let sse_body = format!(
        "data: {{\"event_type\":\"interaction.created\",\"interaction\":{{\"id\":\"int_123\",\"status\":\"in_progress\"}}}}\n\n\
         data: {{\"event_type\":\"step.start\",\"index\":0,\"step\":{{\"type\":\"model_output\"}}}}\n\n\
         data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/pcm;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/pcm;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"step.stop\",\"index\":0}}\n\n\
         data: {{\"event_type\":\"interaction.completed\",\"interaction\":{{\"id\":\"int_123\",\"status\":\"completed\"}}}}\n\n",
        b64_chunk1, b64_chunk2
    );

    Mock::given(method("POST"))
        .and(path("/interactions"))
        .and(header("x-goog-api-key", "test-api-key"))
        .and(header("Api-Revision", "2026-05-20"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .set_delay(std::time::Duration::from_millis(50))
                .append_header("content-type", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let tts = test_tts_with_mock_url(format!("{}/interactions", mock_server.uri()));
    let start_time = std::time::Instant::now();

    let request = TtsRequest { text: "Hello streaming world!".to_string(), ..Default::default() };

    let mut stream = tts.synthesize_stream(&request).await.expect("synthesize_stream failed");

    let mut first_audio_ts = None;
    let mut frames = Vec::new();

    while let Some(res) = stream.next().await {
        let frame = res.expect("frame error");
        if first_audio_ts.is_none() {
            first_audio_ts = Some(start_time.elapsed());
        }
        frames.push(frame);
    }

    let completion_ts = start_time.elapsed();

    assert_eq!(frames.len(), 2, "Expected 2 audio frames");
    assert!(first_audio_ts.is_some(), "First audio timestamp must be recorded");
    let first_ts = first_audio_ts.unwrap();
    assert!(
        first_ts < completion_ts,
        "first consumer audio timestamp ({:?}) < provider completion timestamp ({:?})",
        first_ts,
        completion_ts
    );

    assert_eq!(frames[0].sample_rate, 24000);
    assert_eq!(frames[0].channels, 1);
    assert_eq!(frames[0].data.as_ref(), &[1, 2, 3, 4]);
    assert_eq!(frames[1].data.as_ref(), &[5, 6, 7, 8]);
}

#[tokio::test]
async fn test_gemini_tts_zero_audio_completion_is_error() {
    let mock_server = MockServer::start().await;

    let sse_body = "data: {\"event_type\":\"interaction.created\",\"interaction\":{\"id\":\"int_123\",\"status\":\"in_progress\"}}\n\n\
         data: {\"event_type\":\"interaction.completed\",\"interaction\":{\"id\":\"int_123\",\"status\":\"completed\"}}\n\n";

    Mock::given(method("POST"))
        .and(path("/interactions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .append_header("content-type", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let tts = test_tts_with_mock_url(format!("{}/interactions", mock_server.uri()));
    let request = TtsRequest { text: "Zero audio test".to_string(), ..Default::default() };

    let mut stream = tts.synthesize_stream(&request).await.expect("stream creation failed");
    let res = stream.next().await;
    assert!(res.is_some(), "Stream should yield an error item");
    let err = res.unwrap();
    assert!(err.is_err(), "Stream completion without audio must return an error");
    let err_msg = err.err().unwrap().to_string();
    assert!(err_msg.contains("without emitting audio frames"), "Error message was: {}", err_msg);
}

#[tokio::test]
async fn test_gemini_tts_mid_stream_provider_error() {
    let mock_server = MockServer::start().await;

    let b64_chunk =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [1, 2, 3, 4]);

    let sse_body = format!(
        "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/pcm;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"error\",\"error\":{{\"code\":\"RESOURCE_EXHAUSTED\",\"message\":\"Rate limit exceeded\"}}}}\n\n",
        b64_chunk
    );

    Mock::given(method("POST"))
        .and(path("/interactions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .append_header("content-type", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let tts = test_tts_with_mock_url(format!("{}/interactions", mock_server.uri()));
    let request = TtsRequest { text: "Mid-stream failure test".to_string(), ..Default::default() };

    let mut stream = tts.synthesize_stream(&request).await.expect("stream creation failed");

    // First frame succeeds
    let first = stream.next().await;
    assert!(first.is_some() && first.as_ref().unwrap().is_ok(), "First chunk should be valid");

    // Second event fails
    let second = stream.next().await;
    assert!(second.is_some(), "Should yield error item");
    let err = second.unwrap();
    assert!(err.is_err(), "Mid-stream error must propagate as failure");
    let err_msg = err.err().unwrap().to_string();
    assert!(err_msg.contains("Rate limit exceeded"), "Error message was: {}", err_msg);
}

#[tokio::test]
async fn test_gemini_tts_metadata_mismatch_error() {
    let mock_server = MockServer::start().await;

    let b64_chunk1 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [1, 2, 3, 4]);
    let b64_chunk2 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [5, 6, 7, 8]);

    // First chunk claims 24000Hz, second claims 48000Hz (contradictory metadata)
    let sse_body = format!(
        "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/pcm;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/pcm;rate=48000\",\"sample_rate\":48000,\"channels\":1}}}}\n\n",
        b64_chunk1, b64_chunk2
    );

    Mock::given(method("POST"))
        .and(path("/interactions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .append_header("content-type", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let tts = test_tts_with_mock_url(format!("{}/interactions", mock_server.uri()));
    let request = TtsRequest { text: "Mismatch test".to_string(), ..Default::default() };

    let mut stream = tts.synthesize_stream(&request).await.expect("stream creation failed");
    let first = stream.next().await;
    assert!(first.is_some() && first.as_ref().unwrap().is_ok());

    let second = stream.next().await;
    assert!(second.is_some());
    let err = second.unwrap();
    assert!(err.is_err(), "Sample rate mismatch must fail stream");
    let err_msg = err.err().unwrap().to_string();
    assert!(err_msg.contains("mismatch"), "Error message was: {}", err_msg);
}

#[tokio::test]
async fn test_gemini_tts_malformed_base64_error() {
    let mock_server = MockServer::start().await;

    let sse_body = "data: {\"event_type\":\"step.delta\",\"index\":0,\"delta\":{\"type\":\"audio\",\"data\":\"!!!NOT_BASE64!!!\",\"mime_type\":\"audio/pcm\",\"sample_rate\":24000,\"channels\":1}}\n\n";

    Mock::given(method("POST"))
        .and(path("/interactions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .append_header("content-type", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let tts = test_tts_with_mock_url(format!("{}/interactions", mock_server.uri()));
    let request = TtsRequest { text: "Malformed base64 test".to_string(), ..Default::default() };

    let mut stream = tts.synthesize_stream(&request).await.expect("stream creation failed");
    let item = stream.next().await;
    assert!(item.is_some());
    let err = item.unwrap();
    assert!(err.is_err(), "Malformed base64 must yield an error");
    let err_msg = err.err().unwrap().to_string();
    assert!(err_msg.contains("base64"), "Error message was: {}", err_msg);
}

#[tokio::test]
async fn test_pipeline_emits_first_audio_before_provider_completion() {
    let mock_server = MockServer::start().await;

    let b64_chunk1 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [1, 2, 3, 4]);
    let b64_chunk2 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [5, 6, 7, 8]);

    let sse_body = format!(
        "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/pcm;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/pcm;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"interaction.completed\",\"interaction\":{{\"id\":\"int_123\",\"status\":\"completed\"}}}}\n\n",
        b64_chunk1, b64_chunk2
    );

    Mock::given(method("POST"))
        .and(path("/interactions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .append_header("content-type", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let tts = Arc::new(test_tts_with_mock_url(format!("{}/interactions", mock_server.uri())));
    let (output_tx, mut output_rx) = mpsc::channel(10);
    let metrics = Arc::new(RwLock::new(adk_audio::pipeline::types::PipelineMetrics::default()));

    let tts_clone = Arc::clone(&tts);
    let output_tx_clone = output_tx.clone();
    let metrics_clone = Arc::clone(&metrics);

    tokio::spawn(async move {
        // Feed text directly to process_text_to_speech
        let text = "Hello world.";
        let mut chunker = adk_audio::pipeline::chunker::SentenceChunker::new();
        let sentences = chunker.push(text);
        let remaining = chunker.flush();
        let all_sentences = sentences.into_iter().chain(remaining).collect::<Vec<_>>();

        for sentence in all_sentences {
            let request = TtsRequest { text: sentence, ..Default::default() };
            if let Ok(mut stream) = tts_clone.synthesize_stream(&request).await {
                let tts_start = std::time::Instant::now();
                let mut first_frame = true;
                while let Some(res) = stream.next().await {
                    if let Ok(frame) = res {
                        let elapsed = tts_start.elapsed().as_millis() as f64;
                        {
                            let mut m = metrics_clone.write().await;
                            if first_frame {
                                m.tts_first_audio_latency_ms = elapsed;
                                first_frame = false;
                            }
                            m.tts_latency_ms = elapsed;
                        }
                        let _ = output_tx_clone.send(PipelineOutput::Audio(frame)).await;
                    }
                }
            }
        }
    });

    // Receive first output
    let first_out = output_rx.recv().await;
    assert!(first_out.is_some());
    if let Some(PipelineOutput::Audio(frame)) = first_out {
        assert_eq!(frame.data.as_ref(), &[1, 2, 3, 4]);
    } else {
        panic!("Expected Audio output frame");
    }

    let m = metrics.read().await;
    assert!(m.tts_first_audio_latency_ms >= 0.0);
}
