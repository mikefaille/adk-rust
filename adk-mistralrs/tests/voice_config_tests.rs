//! Property tests for voice configuration completeness.
//!
//! **Property 9: Voice Config Completeness**
//! *For any* speech generation request with voice parameters, the values SHALL be
//! correctly propagated and stored in the configuration.
//!
//! **Validates: Requirements 7.3**

use adk_mistralrs::{Device, ModelSource, SpeechConfig, VoiceConfig};
use mistralrs::SpeechLoaderType;
use proptest::prelude::*;

// Generators for voice config parameters
fn arb_speaker_id() -> impl Strategy<Value = Option<u32>> {
    prop_oneof![Just(None), (0u32..10).prop_map(Some),]
}

fn arb_speed() -> impl Strategy<Value = Option<f32>> {
    prop_oneof![Just(None), (0.5f32..2.0f32).prop_map(Some),]
}

fn arb_pitch() -> impl Strategy<Value = Option<f32>> {
    prop_oneof![Just(None), (-1.0f32..1.0f32).prop_map(Some),]
}

fn arb_energy() -> impl Strategy<Value = Option<f32>> {
    prop_oneof![Just(None), (-1.0f32..1.0f32).prop_map(Some),]
}

fn arb_device() -> impl Strategy<Value = Device> {
    prop_oneof![
        Just(Device::Auto),
        Just(Device::Cpu),
        (0usize..8).prop_map(Device::Cuda),
        Just(Device::Metal),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Feature: mistral-rs-integration, Property 9: Voice Config Completeness**
    /// *For any* speech generation request with voice parameters, the values SHALL be
    /// correctly propagated and stored in the configuration.
    /// **Validates: Requirements 7.3**
    #[test]
    fn prop_voice_config_completeness(
        speaker_id in arb_speaker_id(),
        speed in arb_speed(),
        pitch in arb_pitch(),
        energy in arb_energy(),
    ) {
        // Build voice config with all parameters
        let mut config = VoiceConfig::new();

        if let Some(id) = speaker_id {
            config = config.with_speaker_id(id);
        }
        if let Some(s) = speed {
            config = config.with_speed(s);
        }
        if let Some(p) = pitch {
            config = config.with_pitch(p);
        }
        if let Some(e) = energy {
            config = config.with_energy(e);
        }

        // Verify all values are correctly stored
        prop_assert_eq!(config.speaker_id, speaker_id);
        prop_assert_eq!(config.speed, speed);
        prop_assert_eq!(config.pitch, pitch);
        prop_assert_eq!(config.energy, energy);
    }

    /// Property test for voice config in speech config
    #[test]
    fn prop_voice_config_in_speech_config(
        speaker_id in arb_speaker_id(),
        speed in arb_speed(),
        device in arb_device(),
    ) {
        // Build voice config
        let mut voice = VoiceConfig::new();
        if let Some(id) = speaker_id {
            voice = voice.with_speaker_id(id);
        }
        if let Some(s) = speed {
            voice = voice.with_speed(s);
        }

        // Build speech config with voice
        let config = SpeechConfig::builder()
            .model_source(ModelSource::huggingface("nari-labs/Dia-1.6B"))
            .loader_type(SpeechLoaderType::Dia)
            .device(device)
            .voice(voice.clone())
            .build();

        // Verify voice config is correctly stored
        prop_assert_eq!(config.voice.speaker_id, speaker_id);
        prop_assert_eq!(config.voice.speed, speed);
        prop_assert_eq!(config.device, device);
    }

    /// Property test for voice config builder chaining
    #[test]
    fn prop_voice_config_builder_chaining(
        speaker_id in 0u32..10,
        speed in 0.5f32..2.0f32,
        pitch in -1.0f32..1.0f32,
        energy in -1.0f32..1.0f32,
    ) {
        // Build config using chained builder pattern
        let config = VoiceConfig::new()
            .with_speaker_id(speaker_id)
            .with_speed(speed)
            .with_pitch(pitch)
            .with_energy(energy);

        // Verify all values are correctly stored
        prop_assert_eq!(config.speaker_id, Some(speaker_id));
        prop_assert_eq!(config.speed, Some(speed));
        prop_assert_eq!(config.pitch, Some(pitch));
        prop_assert_eq!(config.energy, Some(energy));
    }

    /// Property test for speech config with all options
    #[test]
    fn prop_speech_config_completeness(
        max_seqs in 1usize..64,
        device in arb_device(),
    ) {
        let config = SpeechConfig::builder()
            .model_source(ModelSource::huggingface("test/model"))
            .loader_type(SpeechLoaderType::Dia)
            .device(device)
            .max_num_seqs(max_seqs)
            .dac_model_id("custom/dac")
            .build();

        prop_assert_eq!(config.max_num_seqs, Some(max_seqs));
        prop_assert_eq!(config.device, device);
        prop_assert_eq!(config.dac_model_id, Some("custom/dac".to_string()));
        prop_assert!(matches!(config.loader_type, SpeechLoaderType::Dia));
    }
}
