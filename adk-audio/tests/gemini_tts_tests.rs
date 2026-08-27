#![cfg(feature = "tts")]

//! Tests for Gemini TTS Interactions streaming in adk-audio.

use adk_audio::pipeline::types::PipelineOutput;
use adk_audio::providers::tts::{CloudTtsConfig, GeminiTts};
use adk_audio::traits::{TtsProvider, TtsRequest};
use futures::StreamExt;
use std::sync::Arc;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn test_tts_with_mock_url(mock_url: String) -> GeminiTts {
    let mut config = CloudTtsConfig::new("test-api-key");
    config.base_url = Some(mock_url);
    GeminiTts::new(config).expect("GeminiTts::new failed")
}

#[tokio::test]
async fn test_gemini_tts_streaming_delayed_stream_and_first_audio() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (frame1_tx, frame1_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        use tokio::io::AsyncWriteExt;

        let b64_chunk1 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            [0x01, 0x02, 0x03, 0x04],
        );
        let b64_chunk2 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            [0x05, 0x06, 0x07, 0x08],
        );

        // Write HTTP headers
        let http_resp = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
        socket.write_all(http_resp.as_bytes()).await.unwrap();

        // Send created event and delta 1
        let chunk1_sse = format!(
            "data: {{\"event_type\":\"interaction.created\",\"interaction\":{{\"id\":\"int_123\",\"status\":\"in_progress\"}}}}\n\n\
             data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n",
            b64_chunk1
        );
        socket.write_all(chunk1_sse.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();

        // Wait until consumer confirms receipt of frame 1 before releasing remaining SSE stream
        let _ = frame1_rx.await;

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send delta 2 and completion event
        let chunk2_sse = format!(
            "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
             data: {{\"event_type\":\"interaction.completed\",\"interaction\":{{\"id\":\"int_123\",\"status\":\"completed\"}}}}\n\n",
            b64_chunk2
        );
        socket.write_all(chunk2_sse.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
    });

    let mock_url = format!("http://{}/v1beta/interactions", addr);
    let tts = test_tts_with_mock_url(mock_url);
    let start_time = std::time::Instant::now();

    let request = TtsRequest { text: "Hello streaming world!".to_string(), ..Default::default() };
    let mut stream = tts.synthesize_stream(&request).await.expect("synthesize_stream failed");

    // Recv frame 1
    let frame1 = stream.next().await.expect("frame 1 missing").expect("frame 1 error");
    let first_audio_ts = start_time.elapsed();

    // Signal provider server that consumer received frame 1
    let _ = frame1_tx.send(());

    // Recv frame 2
    let frame2 = stream.next().await.expect("frame 2 missing").expect("frame 2 error");
    let end_res = stream.next().await;
    assert!(end_res.is_none(), "Stream should terminate after completion");
    let completion_ts = start_time.elapsed();

    assert!(
        first_audio_ts < completion_ts,
        "first consumer audio timestamp ({:?}) < provider completion timestamp ({:?})",
        first_audio_ts,
        completion_ts
    );

    assert_eq!(frame1.sample_rate, 24000);
    assert_eq!(frame1.channels, 1);
    assert_eq!(frame1.data.as_ref(), &[0x02, 0x01, 0x04, 0x03]); // L16 big-endian swapped
    assert_eq!(frame2.data.as_ref(), &[0x06, 0x05, 0x08, 0x07]);
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

    let tts = test_tts_with_mock_url(mock_server.uri());
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
        "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
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

    let tts = test_tts_with_mock_url(mock_server.uri());
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
        "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=48000\",\"sample_rate\":48000,\"channels\":1}}}}\n\n",
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

    let tts = test_tts_with_mock_url(mock_server.uri());
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
async fn test_gemini_tts_l16_sample_split_across_deltas() {
    let mock_server = MockServer::start().await;

    // A 16-bit big-endian PCM sample [0x01, 0x02] -> swap to LE [0x02, 0x01]
    // A second sample [0x03, 0x04] -> swap to LE [0x04, 0x03]
    // Split 3 bytes into delta 1 ([0x01, 0x02, 0x03]) and 1 byte into delta 2 ([0x04])
    let b64_delta1 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0x01, 0x02, 0x03]);
    let b64_delta2 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0x04]);

    let sse_body = format!(
        "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"interaction.completed\",\"interaction\":{{\"id\":\"int_123\",\"status\":\"completed\"}}}}\n\n",
        b64_delta1, b64_delta2
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

    let tts = test_tts_with_mock_url(mock_server.uri());
    let request = TtsRequest { text: "Split L16 test".to_string(), ..Default::default() };

    let mut stream = tts.synthesize_stream(&request).await.expect("stream creation failed");

    let f1 = stream.next().await.unwrap().unwrap();
    // Delta 1 had 3 bytes: 1 sample emitted (bytes [0x02, 0x01]), 1 byte held in remainder
    assert_eq!(f1.data.as_ref(), &[0x02, 0x01]);

    let f2 = stream.next().await.unwrap().unwrap();
    // Delta 2 has 1 byte [0x04]: combined with remainder [0x03, 0x04] -> swapped to [0x04, 0x03]
    assert_eq!(f2.data.as_ref(), &[0x04, 0x03]);
}

#[tokio::test]
async fn test_gemini_tts_dangling_l16_byte_error() {
    let mock_server = MockServer::start().await;

    // Odd number of bytes (3 bytes = 1 sample + 1 dangling byte)
    let b64_delta =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0x01, 0x02, 0x03]);

    let sse_body = format!(
        "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"interaction.completed\",\"interaction\":{{\"id\":\"int_123\",\"status\":\"completed\"}}}}\n\n",
        b64_delta
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

    let tts = test_tts_with_mock_url(mock_server.uri());
    let request = TtsRequest { text: "Dangling L16 byte test".to_string(), ..Default::default() };

    let mut stream = tts.synthesize_stream(&request).await.expect("stream creation failed");

    let f1 = stream.next().await.unwrap().unwrap();
    assert_eq!(f1.data.as_ref(), &[0x02, 0x01]);

    let err_item = stream.next().await.unwrap();
    assert!(err_item.is_err(), "Dangling L16 byte at completion must fail");
    let err_msg = err_item.err().unwrap().to_string();
    assert!(err_msg.contains("dangling L16 sample byte"), "Error message was: {}", err_msg);
}

#[tokio::test]
async fn test_gemini_tts_custom_base_url_preserves_path() {
    let mock_server = MockServer::start().await;

    let b64_chunk =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [1, 2, 3, 4]);

    let sse_body = format!(
        "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"interaction.completed\",\"interaction\":{{\"id\":\"int_123\",\"status\":\"completed\"}}}}\n\n",
        b64_chunk
    );

    Mock::given(method("POST"))
        .and(path("/v1beta/custom/interactions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .append_header("content-type", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    // Custom base URL with path segment ending with trailing slash
    let base_url = format!("{}/v1beta/custom/", mock_server.uri());
    let mut config = CloudTtsConfig::new("test-api-key");
    config.base_url = Some(base_url);
    let tts = GeminiTts::new(config).expect("GeminiTts::new failed");

    let request = TtsRequest { text: "Base URL path test".to_string(), ..Default::default() };
    let mut stream = tts.synthesize_stream(&request).await.expect("synthesize_stream failed");
    let frame = stream.next().await.unwrap().unwrap();
    assert_eq!(frame.data.as_ref(), &[0x02, 0x01, 0x04, 0x03]);
}

#[tokio::test]
async fn test_gemini_tts_custom_base_url_without_trailing_slash_normalizes() {
    let mock_server = MockServer::start().await;

    let b64_chunk =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [1, 2, 3, 4]);

    let sse_body = format!(
        "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"interaction.completed\",\"interaction\":{{\"id\":\"int_123\",\"status\":\"completed\"}}}}\n\n",
        b64_chunk
    );

    Mock::given(method("POST"))
        .and(path("/v1beta/custom/interactions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .append_header("content-type", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    // Base URL lacking trailing slash: should normalize to .../v1beta/custom/ so url.join("interactions") preserves custom segment
    let base_url_no_slash = format!("{}/v1beta/custom", mock_server.uri());
    let mut config = CloudTtsConfig::new("test-api-key");
    config.base_url = Some(base_url_no_slash);
    let tts = GeminiTts::new(config).expect("GeminiTts::new failed");

    let request =
        TtsRequest { text: "Base URL no trailing slash test".to_string(), ..Default::default() };
    let mut stream = tts.synthesize_stream(&request).await.expect("synthesize_stream failed");
    let frame = stream.next().await.unwrap().unwrap();
    assert_eq!(frame.data.as_ref(), &[0x02, 0x01, 0x04, 0x03]);
}

#[tokio::test]
async fn test_gemini_tts_omitted_mime_inherits_l16_and_swaps_endianness() {
    let mock_server = MockServer::start().await;

    let b64_chunk = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        [0x01, 0x02, 0x03, 0x04],
    );

    // mime_type is omitted (None)
    let sse_body = format!(
        "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"interaction.completed\",\"interaction\":{{\"id\":\"int_123\",\"status\":\"completed\"}}}}\n\n",
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

    let tts = test_tts_with_mock_url(mock_server.uri());
    let request = TtsRequest { text: "Omitted MIME test".to_string(), ..Default::default() };

    let mut stream = tts.synthesize_stream(&request).await.expect("stream creation failed");
    let frame = stream.next().await.unwrap().unwrap();
    // Must inherit negotiated audio/l16 and swap big-endian bytes [0x01, 0x02, 0x03, 0x04] -> [0x02, 0x01, 0x04, 0x03]
    assert_eq!(frame.data.as_ref(), &[0x02, 0x01, 0x04, 0x03]);
}

#[tokio::test]
async fn test_gemini_tts_rejects_invalid_mime_suffix() {
    let mock_server = MockServer::start().await;

    let b64_chunk = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        [0x01, 0x02, 0x03, 0x04],
    );

    let sse_body = format!(
        "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16-invalid;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n",
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

    let tts = test_tts_with_mock_url(mock_server.uri());
    let request = TtsRequest { text: "MIME suffix test".to_string(), ..Default::default() };

    let mut stream = tts.synthesize_stream(&request).await.expect("stream creation failed");
    let item = stream.next().await.unwrap();
    assert!(item.is_err(), "audio/l16-invalid must be rejected");
    let err_msg = item.err().unwrap().to_string();
    assert!(err_msg.contains("Unsupported audio MIME type"), "Error message was: {}", err_msg);
}

#[tokio::test]
async fn test_gemini_tts_http_boundary_contract() {
    let mock_server = MockServer::start().await;

    let b64_chunk = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        [0x01, 0x02, 0x03, 0x04],
    );

    let sse_body = format!(
        "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"interaction.completed\",\"interaction\":{{\"id\":\"int_123\",\"status\":\"completed\"}}}}\n\n",
        b64_chunk
    );

    // Assert HTTP boundary headers and JSON request body structure
    Mock::given(method("POST"))
        .and(path("/interactions"))
        .and(header("x-goog-api-key", "test-api-key"))
        .and(header("Api-Revision", "2026-05-20"))
        .and(wiremock::matchers::body_json(serde_json::json!({
            "model": "gemini-3.1-flash-tts-preview",
            "input": "HTTP boundary contract test",
            "response_format": {
                "type": "audio"
            },
            "stream": true,
            "store": false,
            "generation_config": {
                "speech_config": [
                    { "voice": "Kore" }
                ]
            }
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .append_header("content-type", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let tts = test_tts_with_mock_url(mock_server.uri());
    let request =
        TtsRequest { text: "HTTP boundary contract test".to_string(), ..Default::default() };

    let mut stream = tts.synthesize_stream(&request).await.expect("synthesize_stream failed");
    let frame = stream.next().await.unwrap().unwrap();
    assert_eq!(frame.data.as_ref(), &[0x02, 0x01, 0x04, 0x03]);
}

#[tokio::test]
async fn test_gemini_tts_rejects_pcm_encoding_switch_and_odd_remainder() {
    let mock_server = MockServer::start().await;

    // Delta 1: L16 with 3 bytes (1 sample + 1 remainder byte)
    let b64_delta1 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0x01, 0x02, 0x03]);
    // Delta 2: audio/pcm (violates audio/l16 contract)
    let b64_delta2 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0x04, 0x05]);

    let sse_body = format!(
        "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
         data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/pcm;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n",
        b64_delta1, b64_delta2
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

    let tts = test_tts_with_mock_url(mock_server.uri());
    let request = TtsRequest { text: "PCM switch test".to_string(), ..Default::default() };

    let mut stream = tts.synthesize_stream(&request).await.expect("stream creation failed");

    // First frame succeeds (L16 chunk)
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.data.as_ref(), &[0x02, 0x01]);

    // Second delta with audio/pcm must fail before frame emission
    let second = stream.next().await.unwrap();
    assert!(second.is_err(), "audio/pcm delta must be rejected");
    let err_msg = second.err().unwrap().to_string();
    assert!(err_msg.contains("Unsupported audio MIME type"), "Error message was: {}", err_msg);
}

#[tokio::test]
async fn test_gemini_tts_rejects_container_audio_formats() {
    let mock_server = MockServer::start().await;

    let sse_body = "data: {\"event_type\":\"step.delta\",\"index\":0,\"delta\":{\"type\":\"audio\",\"data\":\"RIFF1234WAVEfmt \",\"mime_type\":\"audio/wav;rate=24000\",\"sample_rate\":24000,\"channels\":1}}\n\n";

    Mock::given(method("POST"))
        .and(path("/interactions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .append_header("content-type", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let tts = test_tts_with_mock_url(mock_server.uri());
    let request = TtsRequest { text: "WAV rejection test".to_string(), ..Default::default() };

    let mut stream = tts.synthesize_stream(&request).await.expect("stream creation failed");
    let item = stream.next().await.unwrap();
    assert!(item.is_err(), "audio/wav format must be rejected");
    let err_msg = item.err().unwrap().to_string();
    assert!(
        err_msg.contains("Container/encoded audio format not supported"),
        "Error message was: {}",
        err_msg
    );
}

#[tokio::test]
async fn test_gemini_tts_cancellation_dropping_stream_stops_consumption() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (drop_tx, drop_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        use tokio::io::AsyncWriteExt;

        let b64_chunk = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            [0x01, 0x02, 0x03, 0x04],
        );

        let http_resp = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
        if socket.write_all(http_resp.as_bytes()).await.is_err() {
            return;
        }

        let chunk_sse = format!(
            "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n",
            b64_chunk
        );
        if socket.write_all(chunk_sse.as_bytes()).await.is_err() {
            return;
        }
        let _ = socket.flush().await;

        // Try writing continuously until client drops connection
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            if socket.write_all(chunk_sse.as_bytes()).await.is_err() {
                let _ = drop_tx.send(());
                break;
            }
        }
    });

    let mock_url = format!("http://{}/interactions", addr);
    let tts = test_tts_with_mock_url(mock_url);

    let request = TtsRequest { text: "Cancellation test".to_string(), ..Default::default() };
    let stream = tts.synthesize_stream(&request).await.expect("synthesize_stream failed");

    // Explicitly drop stream consumer to cancel HTTP connection
    drop(stream);

    // Verify server detected dropped socket connection
    let drop_detected = tokio::time::timeout(std::time::Duration::from_millis(500), drop_rx).await;
    assert!(drop_detected.is_ok(), "Dropping TTS stream must terminate server connection");
}

#[tokio::test]
async fn test_gemini_tts_malformed_base64_error() {
    let mock_server = MockServer::start().await;

    let sse_body = "data: {\"event_type\":\"step.delta\",\"index\":0,\"delta\":{\"type\":\"audio\",\"data\":\"!!!NOT_BASE64!!!\",\"mime_type\":\"audio/l16\",\"sample_rate\":24000,\"channels\":1}}\n\n";

    Mock::given(method("POST"))
        .and(path("/interactions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .append_header("content-type", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let tts = test_tts_with_mock_url(mock_server.uri());
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (frame1_tx, frame1_rx) = tokio::sync::oneshot::channel::<()>();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        use tokio::io::AsyncWriteExt;

        let b64_chunk1 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            [0x01, 0x02, 0x03, 0x04],
        );
        let b64_chunk2 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            [0x05, 0x06, 0x07, 0x08],
        );

        let http_resp = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
        socket.write_all(http_resp.as_bytes()).await.unwrap();

        let chunk1_sse = format!(
            "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n",
            b64_chunk1
        );
        socket.write_all(chunk1_sse.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();

        // Wait until pipeline outputs frame #1
        let _ = frame1_rx.await;

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let chunk2_sse = format!(
            "data: {{\"event_type\":\"step.delta\",\"index\":0,\"delta\":{{\"type\":\"audio\",\"data\":\"{}\",\"mime_type\":\"audio/l16;rate=24000\",\"sample_rate\":24000,\"channels\":1}}}}\n\n\
             data: {{\"event_type\":\"interaction.completed\",\"interaction\":{{\"id\":\"int_123\",\"status\":\"completed\"}}}}\n\n",
            b64_chunk2
        );
        socket.write_all(chunk2_sse.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
    });

    let mock_url = format!("http://{}/v1beta/interactions", addr);
    let tts = Arc::new(test_tts_with_mock_url(mock_url));
    let pipeline_handle = adk_audio::pipeline::builder::AudioPipelineBuilder::new()
        .tts(tts)
        .build_tts()
        .expect("build_tts should succeed");

    let mut handle = pipeline_handle;
    let start_time = std::time::Instant::now();
    handle
        .input_tx
        .send(adk_audio::pipeline::types::PipelineInput::Text("Hello world.".to_string()))
        .await
        .expect("send text should succeed");

    let first_out = handle.output_rx.recv().await;
    let first_audio_ts = start_time.elapsed();

    assert!(first_out.is_some());
    if let Some(PipelineOutput::Audio(frame)) = first_out {
        assert_eq!(frame.data.as_ref(), &[0x02, 0x01, 0x04, 0x03]);
    } else {
        panic!("Expected Audio output frame #1");
    }

    // Signal server that pipeline received frame 1
    let _ = frame1_tx.send(());

    let second_out = handle.output_rx.recv().await;
    let completion_ts = start_time.elapsed();

    assert!(second_out.is_some());
    if let Some(PipelineOutput::Audio(frame)) = second_out {
        assert_eq!(frame.data.as_ref(), &[0x06, 0x05, 0x08, 0x07]);
    } else {
        panic!("Expected Audio output frame #2");
    }

    assert!(
        first_audio_ts < completion_ts,
        "first audio timestamp ({:?}) < completion timestamp ({:?})",
        first_audio_ts,
        completion_ts
    );

    let m = handle.metrics.read().await;
    assert!(m.tts_first_audio_latency_ms >= 0.0);
}
