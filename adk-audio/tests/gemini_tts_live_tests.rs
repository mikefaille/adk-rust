#![cfg(feature = "tts")]

//! Gated live test against real Gemini 3.1 TTS endpoint.

use adk_audio::providers::tts::GeminiTts;
use adk_audio::traits::{TtsProvider, TtsRequest};
use adk_gemini::{GeminiBuilder, Model, Part};
use base64::Engine;
use futures::StreamExt;
use futures::TryStreamExt;

#[ignore = "Gated live test requiring GEMINI_API_KEY environment variable"]
#[tokio::test]
async fn test_live_gemini_3_1_tts_event_level() {
    let api_key = match std::env::var("GEMINI_API_KEY").or_else(|_| std::env::var("GOOGLE_API_KEY"))
    {
        Ok(key) if !key.is_empty() => key,
        _ => {
            println!("Skipping live test: GEMINI_API_KEY / GOOGLE_API_KEY absent");
            return;
        }
    };

    println!("Running live event-level Gemini 3.1 TTS stream test...");

    let gemini = GeminiBuilder::new(&api_key)
        .with_model(Model::from("gemini-3.1-flash-tts-preview".to_string()))
        .build()
        .expect("Failed to build Gemini client");

    let start_time = std::time::Instant::now();
    let mut response_stream = gemini
        .generate_content()
        .with_user_message(
            "Hello, this is a live event-level verification for Gemini 3.1 text to speech synthesis.",
        )
        .with_voice("Kore")
        .execute_stream()
        .await
        .expect("execute_stream failed");

    let mut first_audio_event_time = None;
    let mut audio_event_count = 0;
    let mut total_audio_bytes = 0;

    while let Some(response) =
        response_stream.try_next().await.expect("Error receiving generation response")
    {
        for candidate in response.candidates {
            if let Some(parts) = candidate.content.parts {
                for part in parts {
                    if let Part::InlineData { inline_data } = part {
                        if first_audio_event_time.is_none() {
                            first_audio_event_time = Some(start_time.elapsed());
                            println!(
                                "[RECEIPT] First audio event received at offset: {:?}",
                                first_audio_event_time.unwrap()
                            );
                        }
                        audio_event_count += 1;
                        assert!(
                            inline_data.mime_type.contains("audio/l16")
                                || inline_data.mime_type.contains("audio/pcm"),
                            "Unexpected audio MIME type: {}",
                            inline_data.mime_type
                        );
                        if let Ok(decoded) =
                            base64::engine::general_purpose::STANDARD.decode(&inline_data.data)
                        {
                            total_audio_bytes += decoded.len();
                        }
                    }
                }
            }
        }
    }
    let stream_completed_time = start_time.elapsed();

    assert!(first_audio_event_time.is_some(), "Must receive at least one audio event");

    let first_audio_ts = first_audio_event_time.unwrap();
    let completed_ts = stream_completed_time;

    println!(
        "[RECEIPT] Event Timestamps: First Audio Event = {:?}, InteractionCompleted = {:?}",
        first_audio_ts, completed_ts
    );
    println!(
        "[RECEIPT] Summary: {} audio SSE events, {} total decoded audio bytes",
        audio_event_count, total_audio_bytes
    );

    assert!(
        first_audio_ts < completed_ts,
        "first audio event timestamp ({:?}) must be strictly less than stream completion timestamp ({:?})",
        first_audio_ts,
        completed_ts
    );
    assert!(audio_event_count > 1, "Expected multiple audio events, got {}", audio_event_count);
    assert!(total_audio_bytes > 0, "Audio bytes must be non-zero");
}

#[ignore = "Gated live test requiring GEMINI_API_KEY environment variable"]
#[tokio::test]
async fn test_live_gemini_3_1_tts_streaming() {
    let _api_key =
        match std::env::var("GEMINI_API_KEY").or_else(|_| std::env::var("GOOGLE_API_KEY")) {
            Ok(key) if !key.is_empty() => key,
            _ => {
                println!("Skipping live test: GEMINI_API_KEY / GOOGLE_API_KEY absent");
                return;
            }
        };

    println!("Running live Gemini 3.1 TTS streaming test via GeminiTts...");

    let tts = GeminiTts::from_env().expect("Failed to initialize GeminiTts from env");
    let request = TtsRequest {
        text: "Hello, this is a live streaming test for Gemini 3.1 text to speech synthesis."
            .to_string(),
        ..Default::default()
    };

    let start_time = std::time::Instant::now();
    let mut stream = match tts.synthesize_stream(&request).await {
        Ok(s) => s,
        Err(e) => {
            panic!("Live synthesize_stream request failed: {e}");
        }
    };

    let mut first_audio_received_time = None;
    let mut chunks_count = 0;
    let mut total_pcm_bytes = 0;

    while let Some(res) = stream.next().await {
        let frame = res.expect("Live stream yielded frame error");
        if first_audio_received_time.is_none() {
            first_audio_received_time = Some(start_time.elapsed());
        }
        chunks_count += 1;
        total_pcm_bytes += frame.data.len();
        assert_eq!(frame.sample_rate, 24000);
        assert_eq!(frame.channels, 1);
        assert!(!frame.data.is_empty());
    }

    let stream_completion_time = start_time.elapsed();

    assert!(chunks_count > 1, "Expected multiple streaming audio chunks, got {}", chunks_count);
    assert!(total_pcm_bytes > 0, "Non-zero total PCM bytes must be received");
    assert!(first_audio_received_time.is_some(), "First audio timestamp must be recorded");

    let first_audio_time = first_audio_received_time.unwrap();
    println!(
        "[RECEIPT] GeminiTts streaming completed in {:?}: first audio frame at {:?}, received {} chunks ({} bytes)",
        stream_completion_time, first_audio_time, chunks_count, total_pcm_bytes
    );

    assert!(
        first_audio_time < stream_completion_time,
        "first valid audio ({:?}) must arrive before stream completion ({:?})",
        first_audio_time,
        stream_completion_time
    );
}
