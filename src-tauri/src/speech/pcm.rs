//! Continuous mono conversion. Fractional sampling position survives device callbacks.
pub struct PcmCapture {
    step: f64,
    next: f64,
    position: u64,
    channels: usize,
}

impl PcmCapture {
    pub fn new(input_rate: u32, output_rate: u32, channels: u16) -> Self {
        Self { step: input_rate.max(1) as f64 / output_rate.max(1) as f64,
            next: 0.0, position: 0, channels: channels.max(1) as usize }
    }

    pub fn convert<T: Copy>(&mut self, input: &[T], sample: impl Fn(T) -> i16) -> Vec<i16> {
        let mut output = Vec::new();
        for frame in input.chunks_exact(self.channels) {
            let mono = (frame.iter().map(|v| sample(*v) as i64).sum::<i64>() / self.channels as i64) as i16;
            while self.next <= self.position as f64 {
                output.push(mono);
                self.next += self.step;
            }
            self.position += 1;
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_boundaries_do_not_change_audio_or_duration() {
        let input: Vec<i16> = (0..44100).map(|n| (n % 30000) as i16).collect();
        let mut whole = PcmCapture::new(44100, 16000, 1);
        let expected = whole.convert(&input, |s| s);
        let mut split = PcmCapture::new(44100, 16000, 1);
        let actual: Vec<_> = input.chunks(137).flat_map(|chunk| split.convert(chunk, |s| s)).collect();
        assert_eq!(actual, expected);
        assert_eq!(actual.len(), 16000);
    }
    #[test]
    fn stereo_downmix_and_ten_ms_callbacks_preserve_samples() {
        let mut capture = PcmCapture::new(16000, 16000, 2);
        let mut output = capture.convert(&[100i16, 300].repeat(160), |s| s);
        output.extend(capture.convert(&[300i16, 500].repeat(160), |s| s));
        assert_eq!(output.len(), 320);
        assert!(output[..160].iter().all(|s| *s == 200));
        assert!(output[160..].iter().all(|s| *s == 400));
    }
}
