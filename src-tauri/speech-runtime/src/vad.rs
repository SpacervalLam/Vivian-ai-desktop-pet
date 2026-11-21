//! Frame based adaptive energy VAD. Silence alone never creates an utterance.
#[derive(Clone, Copy, Debug)]
pub struct VadConfig {
    pub threshold: f32,
    pub min_speech_ms: u64,
    pub silence_ms: u64,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            threshold: 500.0,
            min_speech_ms: 250,
            silence_ms: 1500,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Silence,
    Speech,
    End,
}

pub struct Vad {
    config: VadConfig,
    rate: u32,
    noise: f32,
    candidate: u64,
    silence: u64,
    speaking: bool,
    ended: bool,
    frame: Vec<i16>,
}

impl Vad {
    pub fn new(rate: u32, config: VadConfig) -> Self {
        Self {
            config,
            rate,
            noise: 80.0,
            candidate: 0,
            silence: 0,
            speaking: false,
            ended: false,
            frame: Vec::with_capacity((rate / 100) as usize),
        }
    }
    pub fn process(&mut self, samples: &[i16]) -> Activity {
        if self.ended {
            return Activity::End;
        }
        let size = (self.rate / 100).max(1) as usize;
        for &sample in samples {
            self.frame.push(sample);
            if self.frame.len() != size {
                continue;
            }
            let rms =
                (self.frame.iter().map(|s| (*s as f32).powi(2)).sum::<f32>() / size as f32).sqrt();
            self.frame.clear();
            let active = rms >= self.config.threshold.max(self.noise * 3.0);
            if active {
                self.candidate += 10;
                self.silence = 0;
                if self.candidate >= self.config.min_speech_ms {
                    self.speaking = true;
                }
            } else {
                if !self.speaking {
                    self.noise = 0.98 * self.noise + 0.02 * rms;
                    self.candidate = 0;
                }
                self.silence += 10;
                if self.speaking && self.silence >= self.config.silence_ms {
                    self.ended = true;
                    return Activity::End;
                }
            }
        }
        if self.speaking {
            Activity::Speech
        } else {
            Activity::Silence
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn silence_and_impulses_do_not_endpoint_but_real_speech_does() {
        let mut vad = Vad::new(16000, VadConfig::default());
        assert_eq!(vad.process(&vec![0; 16000 * 4]), Activity::Silence);
        assert_eq!(vad.process(&[3000; 160]), Activity::Silence);
        assert_eq!(vad.process(&[0; 160]), Activity::Silence);
        assert_eq!(vad.process(&[2500; 4800]), Activity::Speech);
        assert_eq!(vad.process(&[0; 16000]), Activity::Speech);
        assert_eq!(vad.process(&[0; 8000]), Activity::End);
    }
}
