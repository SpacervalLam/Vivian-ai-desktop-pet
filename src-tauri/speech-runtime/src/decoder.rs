use crate::streaming::AudioReader;
use std::io::Read;

pub(crate) struct Wave {
    reader: AudioReader,
    rate: u32,
    channels: u16,
    bits: u16,
    float: bool,
    remaining: Option<u64>,
}

impl Wave {
    pub fn new(mut reader: AudioReader) -> Result<Self, String> {
        let mut riff = [0; 12];
        reader.read_exact(&mut riff).map_err(|e| e.to_string())?;
        if &riff[..4] != b"RIFF" || &riff[8..] != b"WAVE" {
            return Err("Expected streaming WAV".into());
        }
        let mut format = None;
        let mut header_bytes = 12usize;
        loop {
            let mut chunk = [0; 8];
            reader.read_exact(&mut chunk).map_err(|e| e.to_string())?;
            let size = u32::from_le_bytes(chunk[4..].try_into().unwrap());
            if &chunk[..4] == b"data" {
                let (code, rate, channels, bits) = format.ok_or("WAV data precedes format")?;
                return Ok(Self {
                    reader,
                    rate,
                    channels,
                    bits,
                    float: code == 3,
                    remaining: if size == 0 || size == u32::MAX {
                        None
                    } else {
                        Some(size as u64)
                    },
                });
            }
            header_bytes = header_bytes.saturating_add(size as usize + 8);
            if size > 65536 || header_bytes > 1_048_576 {
                return Err("WAV header exceeds limit".into());
            }
            let mut bytes = vec![0; size as usize + size as usize % 2];
            reader.read_exact(&mut bytes).map_err(|e| e.to_string())?;
            if &chunk[..4] == b"fmt " {
                if bytes.len() < 16 {
                    return Err("Incomplete WAV format".into());
                }
                let mut code = u16::from_le_bytes(bytes[..2].try_into().unwrap());
                if code == 65534 && bytes.len() >= 40 {
                    code = u16::from_le_bytes(bytes[24..26].try_into().unwrap());
                }
                let channels = u16::from_le_bytes(bytes[2..4].try_into().unwrap());
                let rate = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
                let bits = u16::from_le_bytes(bytes[14..16].try_into().unwrap());
                if !(1..=8).contains(&channels)
                    || !(8000..=192000).contains(&rate)
                    || !((code == 1 && [8, 16, 24, 32].contains(&bits))
                        || (code == 3 && bits == 32))
                {
                    return Err("Unsupported streaming WAV format".into());
                }
                format = Some((code, rate, channels, bits));
            }
        }
    }
}

impl Iterator for Wave {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let count = (self.bits / 8) as usize;
        if self.remaining.is_some_and(|left| left < count as u64) {
            return None;
        }
        let mut bytes = [0; 4];
        self.reader.read_exact(&mut bytes[..count]).ok()?;
        if let Some(left) = &mut self.remaining {
            *left -= count as u64;
        }
        let sample = match self.bits {
            8 => (bytes[0] as f32 - 128.0) / 128.0,
            16 => i16::from_le_bytes(bytes[..2].try_into().unwrap()) as f32 / 32768.0,
            24 => ((i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8) as f32) / 8388608.0,
            _ if self.float => f32::from_le_bytes(bytes),
            _ => i32::from_le_bytes(bytes) as f32 / 2147483648.0,
        };
        Some(if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        })
    }
}
impl Source for Wave {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}
use rodio::Source;
use std::time::Duration;
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{Decoder, DecoderOptions},
    formats::{FormatOptions, FormatReader},
    io::{MediaSourceStream, ReadOnlySource},
    meta::MetadataOptions,
    probe::Hint,
};

/// A network MP3 is non-seekable. Pretending it is seekable makes the demuxer
/// scan the entire stream to estimate its duration before returning a sample.
pub(crate) struct Mpeg {
    demuxer: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    frame: std::vec::IntoIter<f32>,
    rate: u32,
    channels: u16,
}

impl Mpeg {
    pub fn new(reader: AudioReader) -> Result<Self, String> {
        let source =
            MediaSourceStream::new(Box::new(ReadOnlySource::new(reader)), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("mp3");
        let probe = symphonia::default::get_probe()
            .format(
                &hint,
                source,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(|e| e.to_string())?;
        let track = probe
            .format
            .default_track()
            .ok_or("MP3 contains no track")?;
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| e.to_string())?;
        let mut this = Self {
            demuxer: probe.format,
            decoder,
            frame: Vec::new().into_iter(),
            rate: 0,
            channels: 0,
        };
        this.load()?;
        Ok(this)
    }
    fn load(&mut self) -> Result<(), String> {
        for _ in 0..8 {
            let packet = self.demuxer.next_packet().map_err(|e| e.to_string())?;
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    self.rate = spec.rate;
                    self.channels = spec.channels.count() as u16;
                    let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                    buffer.copy_interleaved_ref(decoded);
                    self.frame = buffer.samples().to_vec().into_iter();
                    return Ok(());
                }
                Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
        Err("MP3 decoder could not recover a frame".into())
    }
}
impl Iterator for Mpeg {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if let Some(sample) = self.frame.next() {
            return Some(sample);
        }
        self.load().ok()?;
        self.frame.next()
    }
}
impl Source for Mpeg {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.frame.len())
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}
