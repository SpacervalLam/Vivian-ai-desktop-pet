//! SpeexDSP 1.2.1 MDF acoustic echo cancellation; input and reference are mono PCM16.
use std::{
    ffi::{c_int, c_void},
    ptr::NonNull,
};

unsafe extern "C" {
    fn speex_echo_state_init(frame: c_int, filter: c_int) -> *mut c_void;
    fn speex_echo_state_destroy(state: *mut c_void);
    fn speex_echo_ctl(state: *mut c_void, request: c_int, value: *mut c_void) -> c_int;
    fn speex_echo_cancellation(
        state: *mut c_void,
        input: *const i16,
        reference: *const i16,
        output: *mut i16,
    );
}

pub struct EchoCanceller {
    state: NonNull<c_void>,
    frame: usize,
}

impl EchoCanceller {
    pub fn new(sample_rate: u32, frame: usize, tail_ms: u32) -> Result<Self, String> {
        if ![16000, 24000, 32000, 48000].contains(&sample_rate) || frame == 0 || frame > 2048 {
            return Err("Invalid AEC frame specification".into());
        }
        let filter = (sample_rate * tail_ms.clamp(40, 500) / 1000) as c_int;
        // SAFETY: Speex owns this state until Drop; processing requires &mut self.
        let state = NonNull::new(unsafe { speex_echo_state_init(frame as c_int, filter) })
            .ok_or("AEC allocation failed")?;
        let mut rate = sample_rate as c_int;
        unsafe {
            speex_echo_ctl(state.as_ptr(), 24, (&mut rate as *mut c_int).cast());
        }
        Ok(Self { state, frame })
    }

    pub fn process(
        &mut self,
        input: &[i16],
        reference: &[i16],
        output: &mut [i16],
    ) -> Result<(), String> {
        if input.len() != self.frame || reference.len() != self.frame || output.len() != self.frame
        {
            return Err("AEC expects exactly one complete frame".into());
        }
        unsafe {
            speex_echo_cancellation(
                self.state.as_ptr(),
                input.as_ptr(),
                reference.as_ptr(),
                output.as_mut_ptr(),
            );
        }
        Ok(())
    }
}

impl Drop for EchoCanceller {
    fn drop(&mut self) {
        unsafe {
            speex_echo_state_destroy(self.state.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancels_delayed_echo_and_preserves_near_end_speech() {
        let mut aec = EchoCanceller::new(16000, 320, 200).unwrap();
        let mut seed = 7u32;
        let signal: Vec<i16> = (0..16000 * 8)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                ((seed >> 17) as i32 - 16384) as i16
            })
            .collect();
        let mut before = 0f64;
        let mut after = 0f64;
        for (index, reference) in signal.chunks_exact(320).enumerate() {
            let input: Vec<i16> = (0..320)
                .map(|n| {
                    let position = index * 320 + n;
                    if position >= 97 {
                        (signal[position - 97] as f32 * 0.55) as i16
                    } else {
                        0
                    }
                })
                .collect();
            let mut output = [0; 320];
            aec.process(&input, reference, &mut output).unwrap();
            if index > 300 {
                before += input.iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
                after += output.iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
            }
        }
        let erle = 10.0 * (before / after.max(1.0)).log10();
        eprintln!("synthetic_echo_erle_db={erle:.1}");
        assert!(erle > 15.0, "echo reduction {erle}dB");
        let mut correlation = 0f64;
        let mut near_energy = 0f64;
        let mut output_energy = 0f64;
        for index in 0..50 {
            let position = 80000 + index * 320;
            let near: Vec<i16> = (0..320)
                .map(|n| {
                    ((position + n) as f32 * std::f32::consts::TAU * 300.0 / 16000.0)
                        .sin()
                        .mul_add(4500.0, 0.0) as i16
                })
                .collect();
            let input: Vec<i16> = (0..320)
                .map(|n| (near[n] as f32 + signal[position + n - 97] as f32 * 0.55) as i16)
                .collect();
            let mut output = [0; 320];
            aec.process(&input, &signal[position..position + 320], &mut output)
                .unwrap();
            if index > 10 {
                for n in 0..320 {
                    correlation += near[n] as f64 * output[n] as f64;
                    near_energy += (near[n] as f64).powi(2);
                    output_energy += (output[n] as f64).powi(2);
                }
            }
        }
        let preserved = correlation / (near_energy * output_energy).sqrt();
        eprintln!("double_talk_near_end_correlation={preserved:.3}");
        assert!(preserved > 0.7, "double talk must preserve near-end speech");
        // No reference must not turn the microphone into a mute gate.
        let mut fresh = EchoCanceller::new(16000, 320, 200).unwrap();
        let mut energy = 0f64;
        for reference in signal.chunks_exact(320).take(30) {
            let mut output = [0; 320];
            fresh.process(reference, &[0; 320], &mut output).unwrap();
            energy += output.iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
        }
        assert!(energy > 1e9, "near end audio must pass through");
    }
}
