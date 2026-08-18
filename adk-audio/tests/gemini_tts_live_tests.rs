//! Gated live test against real Gemini 3.1 TTS endpoint.

use adk_audio::providers::tts::GeminiTts;
use adk_audio::traits::{TtsProvider, TtsRequest};
use futures::StreamExt;

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

    println!("Running live Gemini 3.1 TTS streaming test...");

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

    let total_time = start_time.elapsed();

    assert!(chunks_count > 0, "Non-zero audio frames must be received");
    assert!(total_pcm_bytes > 0, "Non-zero total PCM bytes must be received");
    assert!(first_audio_received_time.is_some(), "First audio timestamp must be recorded");

    let first_audio_time = first_audio_received_time.unwrap();
    println!(
        "Live test completed in {:?}: first audio at {:?}, received {} chunks ({} bytes)",
        total_time, first_audio_time, chunks_count, total_pcm_bytes
    );

    assert!(first_audio_time <= total_time, "First audio must arrive before total completion");
}
