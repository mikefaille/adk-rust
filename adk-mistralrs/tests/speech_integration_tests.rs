//! Integration tests for speech model support.
//!
//! These tests validate the MistralRsSpeechModel implementation with the Dia model.
//! Speech models generate audio from text (text-to-speech).
//!
//! **Validates: Requirements 6.1, 6.2, 6.3**
//!
//! Run with: cargo test -p adk-mistralrs --features metal --test speech_integration_tests -- --ignored --nocapture

use adk_mistralrs::MistralRsSpeechModel;
use std::path::PathBuf;

// Test model - Dia 1.6B
const SPEECH_MODEL: &str = "nari-labs/Dia-1.6B";

/// Get the output path for generated audio files
fn get_output_path(filename: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models").join(filename)
}

// =============================================================================
// Integration Tests
// =============================================================================

#[tokio::test]
#[ignore = "Requires HuggingFace auth and model download (~3GB) - run manually"]
async fn test_speech_model_load() {
    println!("Testing Dia speech model loading: {}", SPEECH_MODEL);

    let model =
        MistralRsSpeechModel::from_hf(SPEECH_MODEL).await.expect("Failed to load speech model");

    assert_eq!(model.name(), SPEECH_MODEL);
    println!("✓ Speech model loaded successfully: {}", model.name());
}

/// Test basic text-to-speech generation.
#[tokio::test]
#[ignore = "Requires HuggingFace auth and model download (~3GB) - run manually"]
async fn test_speech_generate_simple() {
    println!("Testing simple speech generation...");
    println!("Model: {}", SPEECH_MODEL);

    let model =
        MistralRsSpeechModel::from_hf(SPEECH_MODEL).await.expect("Failed to load speech model");

    // Simple text without speaker tags
    let text = "Hello! This is a test of the Dia speech model.";
    println!("Generating speech for: \"{}\"", text);

    let audio = model.generate_speech(text).await.expect("Failed to generate speech");

    println!("Audio generated:");
    println!("  Sample rate: {} Hz", audio.sample_rate);
    println!("  Channels: {}", audio.channels);
    println!("  Duration: {:.2} seconds", audio.duration_secs());
    println!("  PCM samples: {}", audio.pcm_data.len());

    assert!(!audio.pcm_data.is_empty(), "Should have generated audio data");
    assert!(audio.sample_rate > 0, "Should have valid sample rate");
    assert!(audio.channels > 0, "Should have valid channel count");

    // Save to WAV file
    let output_path = get_output_path("speech_simple.wav");
    let wav_bytes = audio.to_wav_bytes().expect("Failed to encode WAV");
    std::fs::write(&output_path, wav_bytes).expect("Failed to write WAV file");
    println!("✓ Audio saved to: {:?}", output_path);
}

/// Test multi-speaker dialogue generation with [S1] and [S2] tags.
#[tokio::test]
#[ignore = "Requires HuggingFace auth and model download (~3GB) - run manually"]
async fn test_speech_generate_dialogue() {
    println!("Testing dialogue generation with speaker tags...");
    println!("Model: {}", SPEECH_MODEL);

    let model =
        MistralRsSpeechModel::from_hf(SPEECH_MODEL).await.expect("Failed to load speech model");

    // Multi-speaker dialogue with [S1] and [S2] tags
    let dialogue = "[S1] Hello! How are you today? [S2] I'm doing great, thanks for asking! [S1] That's wonderful to hear.";
    println!("Generating dialogue: \"{}\"", dialogue);

    let audio = model.generate_dialogue(dialogue).await.expect("Failed to generate dialogue");

    println!("Audio generated:");
    println!("  Sample rate: {} Hz", audio.sample_rate);
    println!("  Channels: {}", audio.channels);
    println!("  Duration: {:.2} seconds", audio.duration_secs());
    println!("  PCM samples: {}", audio.pcm_data.len());

    assert!(!audio.pcm_data.is_empty(), "Should have generated audio data");
    assert!(audio.duration_secs() > 0.5, "Dialogue should be at least 0.5 seconds");

    // Save to WAV file
    let output_path = get_output_path("speech_dialogue.wav");
    let wav_bytes = audio.to_wav_bytes().expect("Failed to encode WAV");
    std::fs::write(&output_path, wav_bytes).expect("Failed to write WAV file");
    println!("✓ Audio saved to: {:?}", output_path);
}

/// Test speech generation with non-verbal sounds.
#[tokio::test]
#[ignore = "Requires HuggingFace auth and model download (~3GB) - run manually"]
async fn test_speech_with_nonverbal() {
    println!("Testing speech with non-verbal sounds...");
    println!("Model: {}", SPEECH_MODEL);

    let model =
        MistralRsSpeechModel::from_hf(SPEECH_MODEL).await.expect("Failed to load speech model");

    // Text with non-verbal sounds
    let text = "[S1] That's hilarious! (laughs) [S2] I know, right? (sighs) It's been a long day.";
    println!("Generating: \"{}\"", text);

    let audio = model.generate_speech(text).await.expect("Failed to generate speech");

    println!("Audio generated:");
    println!("  Duration: {:.2} seconds", audio.duration_secs());

    assert!(!audio.pcm_data.is_empty(), "Should have generated audio data");

    // Save to WAV file
    let output_path = get_output_path("speech_nonverbal.wav");
    let wav_bytes = audio.to_wav_bytes().expect("Failed to encode WAV");
    std::fs::write(&output_path, wav_bytes).expect("Failed to write WAV file");
    println!("✓ Audio saved to: {:?}", output_path);
}

/// Test the example from mistral.rs documentation.
#[tokio::test]
#[ignore = "Requires HuggingFace auth and model download (~3GB) - run manually"]
async fn test_speech_mistralrs_example() {
    println!("Testing mistral.rs example dialogue...");
    println!("Model: {}", SPEECH_MODEL);

    let model =
        MistralRsSpeechModel::from_hf(SPEECH_MODEL).await.expect("Failed to load speech model");

    // Example from mistral.rs docs
    let text = "[S1] mistral r s is a local LLM inference engine. [S2] You can run text and vision models, and also image generation and speech generation. [S1] There is agentic web search, tool calling, and a convenient Python API. [S2] Check it out on github.";
    println!("Generating: \"{}\"", text);

    let start = std::time::Instant::now();
    let audio = model.generate_speech(text).await.expect("Failed to generate speech");
    let elapsed = start.elapsed();

    println!("Audio generated in {:.2}s:", elapsed.as_secs_f32());
    println!("  Sample rate: {} Hz", audio.sample_rate);
    println!("  Channels: {}", audio.channels);
    println!("  Duration: {:.2} seconds", audio.duration_secs());

    // Save to WAV file
    let output_path = get_output_path("speech_mistralrs_example.wav");
    let wav_bytes = audio.to_wav_bytes().expect("Failed to encode WAV");
    std::fs::write(&output_path, wav_bytes).expect("Failed to write WAV file");
    println!("✓ Audio saved to: {:?}", output_path);
}
