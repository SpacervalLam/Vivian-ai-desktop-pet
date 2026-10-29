//! Exercise the production capture adapter without loading Tauri or a microphone.
mod asr {
    #[derive(Clone, Debug)]
    pub enum AsrEvent {
        EndpointDetected,
        NoSpeech,
    }
    pub struct VadConfig {
        pub energy_threshold: f32,
        pub min_speech_duration_ms: u64,
    }
    impl Default for VadConfig {
        fn default() -> Self {
            Self {
                energy_threshold: 500.0,
                min_speech_duration_ms: 250,
            }
        }
    }
}
#[path = "../../src/speech/endpoint.rs"]
mod endpoint;

#[test]
fn a_large_device_callback_cannot_discard_the_beginning_of_speech() {
    let mut endpoint = endpoint::Endpoint::new(16000, &asr::VadConfig::default(), 1500, None);
    assert!(endpoint.filter(&[0; 16000]).is_empty());
    let output = endpoint.filter(&[3000; 16000]);
    assert_eq!(
        output.iter().filter(|sample| **sample == 3000).count(),
        16000
    );
}
