use super::asr::{AsrEvent, VadConfig};

/// Per-capture detector. Once emitted, an endpoint cannot be emitted again.
pub struct Endpoint {
    detector: vivian_speech_runtime::vad::Vad,
    events: Option<tokio::sync::broadcast::Sender<AsrEvent>>,
    emitted: bool,
    rate: u32,
    seen: usize,
    speaking: bool,
    preroll: std::collections::VecDeque<i16>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preroll_preserves_the_first_syllable_and_silence_does_not_upload() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        let mut endpoint = Endpoint::new(16000, &VadConfig::default(), 1500, Some(tx));
        assert!(endpoint.filter(&vec![0; 16000]).is_empty());
        assert!(endpoint.filter(&vec![3000; 1600]).is_empty());
        let first = endpoint.filter(&vec![3000; 3200]);
        assert!(first.iter().filter(|sample| **sample == 3000).count() >= 4800);
        endpoint.filter(&vec![0; 24000]);
        assert!(matches!(rx.try_recv().unwrap(), AsrEvent::EndpointDetected));
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn no_speech_timeout_has_no_audio_to_transcribe() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        let mut endpoint = Endpoint::new(16000, &VadConfig::default(), 1500, Some(tx));
        for _ in 0..8 {
            assert!(endpoint.filter(&vec![0; 16000]).is_empty());
        }
        assert!(matches!(rx.try_recv().unwrap(), AsrEvent::NoSpeech));
    }
}

impl Endpoint {
    pub fn new(
        rate: u32,
        config: &VadConfig,
        silence_ms: u64,
        events: Option<tokio::sync::broadcast::Sender<AsrEvent>>,
    ) -> Self {
        Self {
            detector: vivian_speech_runtime::vad::Vad::new(
                rate,
                vivian_speech_runtime::vad::VadConfig {
                    threshold: config.energy_threshold,
                    min_speech_ms: config.min_speech_duration_ms,
                    silence_ms: silence_ms.max(200),
                },
            ),
            events,
            emitted: false,
            rate,
            seen: 0,
            speaking: false,
            preroll: Default::default(),
        }
    }
    pub fn filter(&mut self, samples: &[i16]) -> Vec<i16> {
        let mut output = Vec::new();
        for frame in samples.chunks((self.rate / 100).max(1) as usize) {
            output.extend(self.filter_frame(frame));
        }
        output
    }
    fn filter_frame(&mut self, samples: &[i16]) -> Vec<i16> {
        use vivian_speech_runtime::vad::Activity;
        self.seen += samples.len();
        let activity = self.detector.process(samples);
        let output = if self.speaking {
            samples.to_vec()
        } else {
            self.preroll.extend(samples.iter().copied());
            let remove = self
                .preroll
                .len()
                .saturating_sub(self.rate as usize * 450 / 1000);
            self.preroll.drain(..remove);
            if activity != Activity::Silence {
                self.speaking = true;
                self.preroll.drain(..).collect()
            } else {
                Vec::new()
            }
        };
        if !self.emitted
            && (activity == Activity::End
                || (!self.speaking && self.seen >= self.rate as usize * 8))
        {
            self.emitted = true;
            if let Some(events) = &self.events {
                let event = if self.speaking {
                    AsrEvent::EndpointDetected
                } else {
                    AsrEvent::NoSpeech
                };
                let _ = events.send(event);
            }
        }
        output
    }
}
