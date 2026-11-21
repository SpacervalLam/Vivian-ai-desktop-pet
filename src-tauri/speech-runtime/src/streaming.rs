//! Bounded encoded input -> one continuous decoder -> nonblocking PCM source.
//! The output device is owned by one long-lived thread, independent of network I/O.
use rodio::{OutputStream, Sink, Source};
use std::{
    collections::VecDeque,
    io::{self, Read, Seek, SeekFrom},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::Duration,
};

pub type Cancel = Arc<AtomicBool>;

#[derive(Clone)]
pub struct AudioWriter {
    tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    capture: Option<Arc<Mutex<Option<Vec<u8>>>>>,
    boundary: Option<Arc<dyn Fn(String, u64, u64) + Send + Sync>>,
}
pub struct AudioReader {
    rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    bytes: VecDeque<u8>,
    base: u64,
    position: u64,
    cancel: Cancel,
}

pub fn pipe(cancel: Cancel) -> (AudioWriter, AudioReader) {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    (
        AudioWriter {
            tx,
            capture: None,
            boundary: None,
        },
        AudioReader {
            rx,
            bytes: VecDeque::new(),
            base: 0,
            position: 0,
            cancel,
        },
    )
}

impl AudioWriter {
    pub fn capture(&mut self) -> Arc<Mutex<Option<Vec<u8>>>> {
        let capture = Arc::new(Mutex::new(Some(Vec::new())));
        self.capture = Some(capture.clone());
        capture
    }
    pub fn on_boundary(&mut self, callback: Arc<dyn Fn(String, u64, u64) + Send + Sync>) {
        self.boundary = Some(callback);
    }
    pub fn word(&self, text: String, offset_ms: u64, duration_ms: u64) {
        if let Some(callback) = &self.boundary {
            callback(text, offset_ms, duration_ms);
        }
    }
    pub async fn write(&self, bytes: &[u8]) -> Result<(), String> {
        for chunk in bytes.chunks(16 * 1024) {
            self.tx
                .send(chunk.to_vec())
                .await
                .map_err(|_| "音频流已关闭".to_string())?;
            if let Some(capture) = &self.capture {
                let mut capture = capture.lock().unwrap_or_else(|e| e.into_inner());
                if capture
                    .as_ref()
                    .is_some_and(|bytes| bytes.len() + chunk.len() > 8 * 1024 * 1024)
                {
                    *capture = None;
                }
                if let Some(bytes) = capture.as_mut() {
                    bytes.extend_from_slice(chunk);
                }
            }
        }
        Ok(())
    }
}

impl Read for AudioReader {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        if target.is_empty() {
            return Ok(0);
        }
        loop {
            if self.cancel.load(Ordering::SeqCst) {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
            let offset = (self.position - self.base) as usize;
            if offset < self.bytes.len() {
                let count = target.len().min(self.bytes.len() - offset);
                for (out, value) in target
                    .iter_mut()
                    .zip(self.bytes.iter().skip(offset))
                    .take(count)
                {
                    *out = *value;
                }
                self.position += count as u64;
                // Enough lookbehind for codec probing without retaining the response.
                let remove = (self.position - self.base).saturating_sub(64 * 1024) as usize;
                self.bytes.drain(..remove);
                self.base += remove as u64;
                return Ok(count);
            }
            match self.rx.try_recv() {
                Ok(bytes) => self.bytes.extend(bytes),
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return Ok(0),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(2))
                }
            }
        }
    }
}

impl Seek for AudioReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(position) => position as i128,
            SeekFrom::Current(offset) => self.position as i128 + offset as i128,
            SeekFrom::End(_) => return Err(io::ErrorKind::Unsupported.into()),
        };
        if position < self.base as i128 || position > (self.base + self.bytes.len() as u64) as i128
        {
            return Err(io::ErrorKind::Unsupported.into());
        }
        self.position = position as u64;
        Ok(self.position)
    }
}

struct PcmSource {
    rx: mpsc::Receiver<Vec<f32>>,
    current: std::vec::IntoIter<f32>,
    rate: u32,
    channels: u16,
    cancel: Cancel,
    started: Arc<AtomicBool>,
    underruns: Arc<AtomicU64>,
}

impl Iterator for PcmSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.cancel.load(Ordering::SeqCst) {
            return None;
        }
        if let Some(sample) = self.current.next() {
            self.started.store(true, Ordering::Relaxed);
            return Some(sample);
        }
        match self.rx.try_recv() {
            Ok(samples) => {
                self.current = samples.into_iter();
                self.next()
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.underruns.fetch_add(1, Ordering::Relaxed);
                Some(0.0)
            }
            Err(mpsc::TryRecvError::Disconnected) => None,
        }
    }
}

impl Source for PcmSource {
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

struct PlayJob {
    source: PcmSource,
    gain: Box<dyn Fn() -> f32 + Send>,
    started: Box<dyn Fn() + Send>,
    done: mpsc::Sender<Result<(), String>>,
}

fn device() -> &'static mpsc::Sender<PlayJob> {
    static DEVICE: OnceLock<mpsc::Sender<PlayJob>> = OnceLock::new();
    DEVICE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<PlayJob>();
        std::thread::Builder::new()
            .name("speech-output".into())
            .spawn(move || {
                let mut output = None;
                let mut active: Vec<(Sink, PlayJobState)> = Vec::new();
                loop {
                    match rx.recv_timeout(Duration::from_millis(10)) {
                        Ok(job) => {
                            if output.is_none() {
                                output = OutputStream::try_default().ok();
                            }
                            let sink = output
                                .as_ref()
                                .and_then(|(_, handle)| Sink::try_new(handle).ok());
                            if let Some(sink) = sink {
                                sink.set_volume((job.gain)().clamp(0.0, 1.0));
                                let state = PlayJobState {
                                    cancel: job.source.cancel.clone(),
                                    first: job.source.started.clone(),
                                    callback: Some(job.started),
                                    done: job.done,
                                    gain: job.gain,
                                };
                                sink.append(job.source);
                                active.push((sink, state));
                            } else {
                                let _ = job.done.send(Err("无法打开音频输出设备".into()));
                                output = None;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    let mut index = 0;
                    while index < active.len() {
                        let (sink, state) = &mut active[index];
                        sink.set_volume((state.gain)().clamp(0.0, 1.0));
                        if state.first.load(Ordering::Relaxed) {
                            if let Some(callback) = state.callback.take() {
                                callback();
                            }
                        }
                        if state.cancel.load(Ordering::SeqCst) {
                            sink.stop();
                        }
                        if sink.empty() {
                            let (_, state) = active.swap_remove(index);
                            let _ = state.done.send(Ok(()));
                        } else {
                            index += 1;
                        }
                    }
                }
            })
            .expect("speech output thread");
        tx
    })
}

struct PlayJobState {
    cancel: Cancel,
    first: Arc<AtomicBool>,
    callback: Option<Box<dyn Fn() + Send>>,
    done: mpsc::Sender<Result<(), String>>,
    gain: Box<dyn Fn() -> f32 + Send>,
}

/// Runs on a blocking worker. Decode never runs on the device callback thread.
pub fn play(
    reader: AudioReader,
    mp3: bool,
    cancel: Cancel,
    gain: Box<dyn Fn() -> f32 + Send>,
    on_started: Box<dyn Fn() + Send>,
) -> Result<u64, String> {
    struct CancelOnDrop(Cancel);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let _cancel_on_exit = CancelOnDrop(cancel.clone());
    // Decoder probing can rewind within the bounded input lookbehind.
    let decoder: Box<dyn Source<Item = f32> + Send> = if mp3 {
        Box::new(crate::decoder::Mpeg::new(reader)?)
    } else {
        Box::new(crate::decoder::Wave::new(reader)?)
    };
    let channels = decoder.channels();
    let rate = decoder.sample_rate();
    let (tx, rx) = mpsc::sync_channel(8);
    let source = PcmSource {
        rx,
        current: Vec::new().into_iter(),
        channels,
        rate,
        cancel: cancel.clone(),
        started: Arc::new(AtomicBool::new(false)),
        underruns: Arc::new(AtomicU64::new(0)),
    };
    let underruns = source.underruns.clone();
    let (done, completion) = mpsc::channel();
    // Prebuffer 40ms before submitting to the persistent device.
    let mut samples = decoder;
    let size = ((rate as usize * channels as usize) / 25).max(1);
    let initial: Vec<_> = samples.by_ref().take(size).collect();
    if initial.is_empty() {
        return Err("音频流不含样本".into());
    }
    tx.send(initial).map_err(|_| "音频流已关闭")?;
    device()
        .send(PlayJob {
            source,
            gain,
            started: on_started,
            done,
        })
        .map_err(|_| "音频设备线程已退出")?;
    for chunk in &mut samples.collect_chunks(size) {
        let mut chunk = chunk;
        loop {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            match tx.try_send(chunk) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Full(value)) => {
                    chunk = value;
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    cancel.store(true, Ordering::SeqCst);
                    break;
                }
            }
        }
        if cancel.load(Ordering::SeqCst) {
            break;
        }
    }
    drop(tx);
    completion
        .recv_timeout(Duration::from_secs(120))
        .map_err(|_| "等待流式播放结束超时")??;
    Ok(underruns.load(Ordering::Relaxed))
}

trait Chunks: Iterator + Sized {
    fn collect_chunks(self, size: usize) -> ChunkIterator<Self> {
        ChunkIterator { source: self, size }
    }
}
impl<T: Iterator> Chunks for T {}
struct ChunkIterator<T> {
    source: T,
    size: usize,
}
impl<T: Iterator> Iterator for ChunkIterator<T> {
    type Item = Vec<T::Item>;
    fn next(&mut self) -> Option<Self::Item> {
        let values: Vec<_> = self.source.by_ref().take(self.size).collect();
        if values.is_empty() {
            None
        } else {
            Some(values)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wav() -> Vec<u8> {
        let mut bytes = Vec::new();
        let length = 32000u32;
        bytes.extend(b"RIFF");
        bytes.extend((length + 36).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(16000u32.to_le_bytes());
        bytes.extend(32000u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(length.to_le_bytes());
        bytes.extend((0..16000).flat_map(|n| (n as i16).to_le_bytes()));
        bytes
    }
    #[tokio::test]
    async fn decoder_yields_before_response_ends_and_handles_fragmented_headers() {
        let cancel = Arc::new(AtomicBool::new(false));
        let (writer, reader) = pipe(cancel.clone());
        let (first, got_first) = tokio::sync::oneshot::channel();
        let decoder = std::thread::spawn(move || {
            let mut decoder = crate::decoder::Wave::new(reader).unwrap();
            assert_eq!(decoder.sample_rate(), 16000);
            assert_eq!(decoder.next(), Some(0.0));
            let _ = first.send(());
            let rest: Vec<_> = decoder.collect();
            assert_eq!(rest.len(), 15999);
        });
        let bytes = wav();
        for chunk in bytes[..4096].chunks(7) {
            writer.write(chunk).await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(2), got_first)
            .await
            .unwrap()
            .unwrap();
        writer.write(&bytes[4096..]).await.unwrap();
        drop(writer);
        decoder.join().unwrap();
    }
    #[tokio::test]
    async fn mp3_frames_decode_before_eof_without_restarting_the_decoder() {
        // Generated silent MPEG1 Layer III frames: no downloaded media fixture.
        let mut bytes = Vec::new();
        for _ in 0..200 {
            bytes.extend([0xff, 0xfb, 0x90, 0xc4]);
            bytes.extend([0; 413]);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let (writer, reader) = pipe(cancel);
        let (first, got_first) = tokio::sync::oneshot::channel();
        let decoder = std::thread::spawn(move || {
            let mut decoder = crate::decoder::Mpeg::new(reader).unwrap();
            assert_eq!(decoder.sample_rate(), 44100);
            assert_eq!(decoder.next(), Some(0.0));
            let _ = first.send(());
            assert!(decoder.count() > 10000);
        });
        for chunk in bytes[..8192].chunks(97) {
            writer.write(chunk).await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(2), got_first)
            .await
            .unwrap()
            .unwrap();
        writer.write(&bytes[8192..]).await.unwrap();
        drop(writer);
        decoder.join().unwrap();
    }
    #[tokio::test]
    async fn streaming_wav_with_unknown_data_length_is_not_treated_as_empty() {
        let mut bytes = wav();
        bytes[40..44].copy_from_slice(&0u32.to_le_bytes());
        let (writer, reader) = pipe(Arc::new(AtomicBool::new(false)));
        let (first, received) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            let mut decoder = crate::decoder::Wave::new(reader).unwrap();
            assert_eq!(decoder.next(), Some(0.0));
            first.send(()).unwrap();
            assert_eq!(decoder.count(), 15999);
        });
        writer.write(&bytes[..512]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), received)
            .await
            .unwrap()
            .unwrap();
        writer.write(&bytes[512..]).await.unwrap();
        drop(writer);
        thread.join().unwrap();
    }
    #[test]
    fn cancellation_interrupts_a_reader_even_when_the_sender_stays_open() {
        let cancel = Arc::new(AtomicBool::new(false));
        let (_writer, mut reader) = pipe(cancel.clone());
        let thread = std::thread::spawn(move || reader.read(&mut [0; 4]));
        cancel.store(true, Ordering::SeqCst);
        assert_eq!(
            thread.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
    }
    #[test]
    fn pcm_underrun_is_silence_and_does_not_wait_for_network() {
        let (tx, rx) = mpsc::sync_channel(2);
        let mut source = PcmSource {
            rx,
            current: vec![0.2].into_iter(),
            rate: 16000,
            channels: 1,
            cancel: Arc::new(AtomicBool::new(false)),
            started: Arc::new(AtomicBool::new(false)),
            underruns: Arc::new(AtomicU64::new(0)),
        };
        assert_eq!(source.next(), Some(0.2));
        assert_eq!(source.next(), Some(0.0));
        tx.send(vec![0.4]).unwrap();
        assert_eq!(source.next(), Some(0.4));
        drop(tx);
        assert_eq!(source.next(), None);
    }
}
