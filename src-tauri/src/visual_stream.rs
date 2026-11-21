//! Bounded local temporal sampling for one explicitly shared window region.
//!
//! Frames are scored on a low-resolution HSV grid, kept only on scene changes,
//! accumulated motion or a static fallback, then composed into one numbered
//! chronological contact sheet. Nothing here touches the filesystem or survives
//! the sharing session.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

/// A paused/stopped share can never publish results from an earlier generation.
#[derive(Default)]
pub struct Revocation {
    generation: AtomicU64,
    pub cancelled: Arc<AtomicBool>,
}
impl Revocation {
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.invalidate();
    }
    pub fn accepts(&self, generation: u64) -> bool {
        !self.cancelled.load(Ordering::Acquire) && self.generation() == generation
    }
}

pub const MAX_FRAMES: usize = 4;
/// Grid used for change scoring. 64×36 keeps text areas comparable without
/// reading the full-resolution pixels.
const GRID_W: usize = 64;
const GRID_H: usize = 36;
const MAX_EDGE: u32 = 800;
/// "Medium motion" score per frame on the 0–255 scale.
const REF_SCORE: f32 = 4.0;
/// A medium-motion passage must accumulate this many seconds to earn a frame.
const INTEGRAL_SECS: f32 = 3.0;
/// Absolute scene-change floor and how much it must stand out from its neighbours.
const CUT_ABS: f32 = 12.0;
const CUT_SPIKE: f32 = 2.5;
/// Small but isolated changes (popups, subtitle lines) would be diluted by the
/// full-frame average, so they get a lower floor with a stricter spike test.
const CUT_WEAK_ABS: f32 = 6.0;
const CUT_WEAK_SPIKE: f32 = 3.0;
const CUT_MIN_GAP: f32 = 0.4;
const MARK_MIN_GAP: f32 = 0.5;
/// A candidate too similar to the last kept frame is not worth a cell.
const DUP_MIN: f32 = 8.0;
/// A completely static view still gets one confirmation frame this often.
const FLOOR_SECS: f32 = 20.0;
/// Soft upper bound on how long a sheet waits for more frames.
const BATCH_MAX_WAIT: f32 = 15.0;
/// The initial sheet should not wait for a full batch.
const FIRST_SHEET_SECS: f32 = 2.0;
/// A frame waits at most this long for its successor before the cut decision
/// is made without future evidence (capture is around 2 fps).
const DECIDE_DELAY_MS: u64 = 900;
/// Per-cell difference that counts as "this cell changed".
const CHANGE_TAU: f32 = 15.0;

const SHEET_W: usize = 798;
const SHEET_H: usize = 450;
const SHEET_BG: [u8; 3] = [18, 18, 20];

pub type Grid = Vec<[f32; 3]>;

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub at: Duration,
}
impl Frame {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>, at: Duration) -> Result<Self, String> {
        if width < 2
            || height < 2
            || width as u64 * height as u64 > 32_000_000
            || rgba.len() as u64 != width as u64 * height as u64 * 4
        {
            return Err("Invalid visual frame".into());
        }
        let scale = (MAX_EDGE as f64 / width.max(height) as f64).min(1.0);
        let (w, h) = (
            (width as f64 * scale).round().max(2.0) as u32,
            (height as f64 * scale).round().max(2.0) as u32,
        );
        let rgba = resize(&rgba, width, height, w, h);
        Ok(Self {
            width: w,
            height: h,
            rgba,
            at,
        })
    }
    fn grid(&self) -> Grid {
        (0..GRID_W * GRID_H)
            .map(|i| {
                let (gx, gy) = (i % GRID_W, i / GRID_W);
                let mut rgb = [0.0f32; 3];
                for dy in [0.25, 0.75] {
                    for dx in [0.25, 0.75] {
                        let x = ((gx as f32 + dx) * self.width as f32 / GRID_W as f32) as usize;
                        let y = ((gy as f32 + dy) * self.height as f32 / GRID_H as f32) as usize;
                        let p = (y * self.width as usize + x) * 4;
                        for c in 0..3 {
                            rgb[c] += self.rgba[p + c] as f32 / 4.0;
                        }
                    }
                }
                hsv(rgb)
            })
            .collect()
    }
}
impl Clone for Frame {
    fn clone(&self) -> Self {
        Self {
            width: self.width,
            height: self.height,
            rgba: self.rgba.clone(),
            at: self.at,
        }
    }
}

fn hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [
        h * 255.0 / 6.0,
        if max == 0.0 { 0.0 } else { d / max * 255.0 },
        max,
    ]
}

/// Hue differences are circular and low-saturation pixels contribute little hue.
fn cell_diff(a: &[[f32; 3]], b: &[[f32; 3]], i: usize) -> f32 {
    let dh = (a[i][0] - b[i][0]).abs();
    (dh.min(255.0 - dh) * a[i][1].min(b[i][1]) / 255.0
        + (a[i][1] - b[i][1]).abs()
        + (a[i][2] - b[i][2]).abs())
        / 3.0
}
fn difference(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    a.iter()
        .enumerate()
        .map(|(i, _)| cell_diff(a, b, i))
        .sum::<f32>()
        / a.len() as f32
}

/// Bounding box of changed grid cells, normalised to 0..1. `None` means nothing moved.
fn change_box(a: &[[f32; 3]], b: &[[f32; 3]]) -> Option<[f32; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (GRID_W, GRID_H, 0usize, 0usize);
    let mut any = false;
    for i in 0..a.len() {
        if cell_diff(a, b, i) > CHANGE_TAU {
            any = true;
            let (x, y) = (i % GRID_W, i / GRID_W);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    any.then(|| {
        [
            x0 as f32 / (GRID_W - 1) as f32,
            y0 as f32 / (GRID_H - 1) as f32,
            x1 as f32 / (GRID_W - 1) as f32,
            y1 as f32 / (GRID_H - 1) as f32,
        ]
    })
}

/// One cell of the contact sheet as described to the model.
#[derive(Clone)]
pub struct FrameInfo {
    pub at: f64,
    pub via: &'static str,
    pub score: Option<f32>,
    pub changed: Option<[f32; 4]>,
}

struct Pending {
    frame: Frame,
    grid: Grid,
    score: Option<f32>,
    novelty: f32,
    changed: Option<[f32; 4]>,
    dt: f32,
    prev: Option<f32>,
}

#[derive(Default)]
pub struct Sampler {
    previous: Option<Grid>,
    last_kept: Option<Grid>,
    pending: Option<Pending>,
    frames: Vec<Frame>,
    info: Vec<FrameInfo>,
    last_frame: Option<Frame>,
    last_bound: Option<Duration>,
    last_cut: Option<Duration>,
    last_score: Option<f32>,
    acc: f32,
    sheet_start: Option<Duration>,
    kept_serial: u64,
}
impl Sampler {
    /// Feed one captured frame. Returns whether it was kept.
    pub fn push(&mut self, frame: Frame) -> bool {
        let at = frame.at;
        let before = self.kept_serial;
        let grid = frame.grid();
        let score = self.previous.as_ref().map(|p| difference(p, &grid));
        let novelty = self
            .last_kept
            .as_ref()
            .map_or(255.0, |p| difference(p, &grid));
        let changed = self.previous.as_ref().and_then(|p| change_box(p, &grid));
        if let Some(pending) = self.pending.take() {
            // The next score is available now, so the spike test can run.
            self.decide(pending, Some(score.unwrap_or(0.0)));
        }
        let dt = self
            .last_frame
            .as_ref()
            .map_or(0.0, |last| (at.saturating_sub(last.at)).as_secs_f32().min(1.0));
        let first = score.is_none();
        self.pending = Some(Pending {
            frame: frame.clone(),
            grid: grid.clone(),
            score,
            novelty,
            changed,
            dt,
            prev: self.last_score,
        });
        self.previous = Some(grid);
        self.last_score = score;
        self.last_frame = Some(frame);
        if first {
            if let Some(pending) = self.pending.take() {
                self.decide(pending, None);
            }
        }
        self.resolve_old(at);
        self.maybe_floor(at);
        self.kept_serial > before
    }

    /// Advance time without a new capture frame (window is static or WGC coalesced).
    pub fn tick(&mut self, at: Duration) -> bool {
        let before = self.kept_serial;
        self.resolve_old(at);
        self.maybe_floor(at);
        self.kept_serial > before
    }

    /// A sheet is due when it is full, has enough material, or a lone frame waited too long.
    /// `first` gives the initial sheet a shorter deadline.
    pub fn take_if_due(&mut self, at: Duration, first: bool) -> Option<Storyboard> {
        if self.frames.is_empty() {
            return None;
        }
        let start = self.sheet_start.unwrap_or(at);
        let elapsed = at.saturating_sub(start).as_secs_f32();
        let due = self.frames.len() >= MAX_FRAMES
            || (self.frames.len() >= 2 && elapsed >= BATCH_MAX_WAIT)
            || (self.frames.len() == 1 && elapsed >= BATCH_MAX_WAIT * 2.0)
            || (first && elapsed >= FIRST_SHEET_SECS);
        due.then(|| self.take()).flatten()
    }

    pub fn take(&mut self) -> Option<Storyboard> {
        if self.frames.is_empty() {
            return None;
        }
        self.sheet_start = None;
        Some(Storyboard {
            frames: std::mem::take(&mut self.frames),
            info: std::mem::take(&mut self.info),
        })
    }

    fn resolve_old(&mut self, at: Duration) -> bool {
        let Some(pending) = self.pending.as_ref() else {
            return false;
        };
        if at.saturating_sub(pending.frame.at) < Duration::from_millis(DECIDE_DELAY_MS) {
            return false;
        }
        let pending = self.pending.take().unwrap();
        self.decide(pending, None);
        true
    }

    fn maybe_floor(&mut self, at: Duration) -> bool {
        let Some(last) = self.last_bound else {
            return false;
        };
        if at.saturating_sub(last).as_secs_f32() < FLOOR_SECS {
            return false;
        }
        let (Some(frame), Some(grid)) = (self.last_frame.clone(), self.previous.clone()) else {
            return false;
        };
        let frame = Frame { at, ..frame };
        self.keep(frame, grid, None, None, "floor")
    }

    fn decide(&mut self, pending: Pending, next: Option<f32>) {
        let Some(score) = pending.score else {
            self.keep(
                pending.frame,
                pending.grid,
                pending.changed,
                None,
                "start",
            );
            return;
        };
        let gap_bound = self
            .last_bound
            .map_or(f32::INFINITY, |at| {
                pending.frame.at.saturating_sub(at).as_secs_f32()
            });
        let gap_cut = self.last_cut.map_or(f32::INFINITY, |at| {
            pending.frame.at.saturating_sub(at).as_secs_f32()
        });
        let base = 1.0f32
            .max(pending.prev.unwrap_or(0.0))
            .max(next.unwrap_or(0.0));
        let isolated = score < CUT_ABS && score >= CUT_WEAK_ABS && score > CUT_WEAK_SPIKE * base;
        if gap_cut >= CUT_MIN_GAP
            && ((score >= CUT_ABS && score > CUT_SPIKE * base) || isolated)
        {
            self.acc = 0.0;
            self.last_cut = Some(pending.frame.at);
            self.keep(pending.frame, pending.grid, pending.changed, Some(score), "cut");
            return;
        }
        self.acc += score * pending.dt;
        if self.acc >= REF_SCORE * INTEGRAL_SECS && gap_bound >= MARK_MIN_GAP {
            if pending.novelty >= DUP_MIN {
                self.keep(
                    pending.frame,
                    pending.grid,
                    pending.changed,
                    Some(score),
                    "mark",
                );
                self.acc = 0.0;
            }
        }
    }

    fn keep(
        &mut self,
        frame: Frame,
        grid: Grid,
        changed: Option<[f32; 4]>,
        score: Option<f32>,
        via: &'static str,
    ) -> bool {
        let at = frame.at;
        if self.frames.len() == MAX_FRAMES {
            // Keep the opening frame and the most recent changes.
            self.frames.remove(1);
            self.info.remove(1);
        }
        if self.frames.is_empty() {
            self.sheet_start = Some(at);
        }
        self.frames.push(frame);
        self.info.push(FrameInfo {
            at: at.as_secs_f64(),
            via,
            score,
            changed,
        });
        self.last_kept = Some(grid);
        self.last_bound = Some(at);
        self.kept_serial += 1;
        true
    }
}

pub struct Storyboard {
    frames: Vec<Frame>,
    info: Vec<FrameInfo>,
}
impl Storyboard {
    pub fn info(&self) -> &[FrameInfo] {
        &self.info
    }
    pub fn png(&self) -> Result<Vec<u8>, String> {
        let rects = layout(self.frames.len());
        let mut rgb = vec![0u8; SHEET_W * SHEET_H * 3];
        for chunk in rgb.chunks_exact_mut(3) {
            chunk.copy_from_slice(&SHEET_BG);
        }
        for (frame, rect) in self.frames.iter().zip(rects.iter()) {
            draw_fit(&mut rgb, frame, *rect);
        }
        for (i, rect) in rects.iter().enumerate() {
            draw_badge(&mut rgb, rect.0 + 5, rect.1 + 4, i + 1);
        }
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, SHEET_W as u32, SHEET_H as u32);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
            writer.write_image_data(&rgb).map_err(|e| e.to_string())?;
        }
        Ok(bytes)
    }
}

type Rect = (usize, usize, usize, usize);
/// One-pixel seams keep small cells visually separated.
fn layout(n: usize) -> Vec<Rect> {
    match n {
        0 => Vec::new(),
        1 => vec![(0, 0, SHEET_W, SHEET_H)],
        2 => vec![(0, 0, SHEET_W, 224), (0, 226, SHEET_W, 224)],
        3 => vec![(0, 0, 398, SHEET_H), (400, 0, 398, 224), (400, 226, 398, 224)],
        _ => vec![
            (0, 0, 398, 224),
            (400, 0, 398, 224),
            (0, 226, 398, 224),
            (400, 226, 398, 224),
        ],
    }
}

/// Contain-fit: preserve the whole selected region and its aspect, with dark
/// bars instead of cropping unseen content. Regions are arbitrary rectangles,
/// so cropping would silently drop the part the user chose to share.
fn draw_fit(dst: &mut [u8], frame: &Frame, rect: Rect) {
    let (x0, y0, cw, ch) = rect;
    if cw == 0 || ch == 0 {
        return;
    }
    let scale = (cw as f32 / frame.width as f32).min(ch as f32 / frame.height as f32);
    let (dw, dh) = (
        (frame.width as f32 * scale).round().max(1.0) as usize,
        (frame.height as f32 * scale).round().max(1.0) as usize,
    );
    let (ox, oy) = (x0 + (cw - dw) / 2, y0 + (ch - dh) / 2);
    for py in 0..dh {
        let sy = (py * frame.height as usize / dh).min(frame.height as usize - 1);
        for px in 0..dw {
            let sx = (px * frame.width as usize / dw).min(frame.width as usize - 1);
            let src = (sy * frame.width as usize + sx) * 4;
            let at = ((oy + py) * SHEET_W + ox + px) * 3;
            dst[at..at + 3].copy_from_slice(&frame.rgba[src..src + 3]);
        }
    }
}

/// 5×7 bitmap digits; only cell indices are drawn.
const DIGITS: [[&str; 7]; 10] = [
    ["01110", "10001", "10001", "10001", "10001", "10001", "01110"],
    ["00100", "01100", "00100", "00100", "00100", "00100", "01110"],
    ["01110", "10001", "00001", "00010", "00100", "01000", "11111"],
    ["11111", "00010", "00100", "00010", "00001", "10001", "01110"],
    ["00010", "00110", "01010", "10010", "11111", "00010", "00010"],
    ["11111", "10000", "11110", "00001", "00001", "10001", "01110"],
    ["00110", "01000", "10000", "11110", "10001", "10001", "01110"],
    ["11111", "00001", "00010", "00100", "01000", "01000", "01000"],
    ["01110", "10001", "10001", "01110", "10001", "10001", "01110"],
    ["01110", "10001", "10001", "01111", "00001", "00010", "01100"],
];
const BADGE_FG: [u8; 3] = [255, 235, 59];
const BADGE_BG: [u8; 3] = [0, 0, 0];

fn draw_badge(dst: &mut [u8], x: usize, y: usize, number: usize) {
    let scale = 3usize;
    let text = number.to_string();
    for (dx, dy) in [
        (-1i32, 0i32),
        (1, 0),
        (0, -1),
        (0, 1),
        (-1, -1),
        (1, -1),
        (-1, 1),
        (1, 1),
    ] {
        draw_number(dst, x, y, &text, scale, BADGE_BG, dx, dy);
    }
    draw_number(dst, x, y, &text, scale, BADGE_FG, 0, 0);
}

fn draw_number(
    dst: &mut [u8],
    x: usize,
    y: usize,
    text: &str,
    scale: usize,
    color: [u8; 3],
    ox: i32,
    oy: i32,
) {
    let mut cursor = x as i32 + ox;
    let top = y as i32 + oy;
    for ch in text.chars() {
        let Some(digit) = ch.to_digit(10) else {
            continue;
        };
        for (row, bits) in DIGITS[digit as usize].iter().enumerate() {
            for (col, bit) in bits.bytes().enumerate() {
                if bit != b'1' {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = cursor + (col * scale + sx) as i32;
                        let py = top + (row * scale + sy) as i32;
                        if px < 0 || py < 0 || px >= SHEET_W as i32 || py >= SHEET_H as i32 {
                            continue;
                        }
                        let i = (py as usize * SHEET_W + px as usize) * 3;
                        dst[i..i + 3].copy_from_slice(&color);
                    }
                }
            }
        }
        cursor += 6 * scale as i32;
    }
}

// Area averaging preserves small UI text better than point sampling when shrinking.
fn resize(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    if (sw, sh) == (dw, dh) {
        return src.to_vec();
    }
    let mut dst = vec![0; dw as usize * dh as usize * 4];
    for y in 0..dh {
        for x in 0..dw {
            let (x0, x1) = (
                x * sw / dw,
                ((x + 1) * sw / dw).max(x * sw / dw + 1).min(sw),
            );
            let (y0, y1) = (
                y * sh / dh,
                ((y + 1) * sh / dh).max(y * sh / dh + 1).min(sh),
            );
            let mut sum = [0u64; 3];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = (sy as usize * sw as usize + sx as usize) * 4;
                    for c in 0..3 {
                        sum[c] += src[p + c] as u64;
                    }
                }
            }
            let p = (y as usize * dw as usize + x as usize) * 4;
            let n = ((x1 - x0) * (y1 - y0)) as u64;
            for c in 0..3 {
                dst[p + c] = (sum[c] / n) as u8;
            }
            dst[p + 3] = 255;
        }
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revoked_results_cannot_reappear_after_resume_or_stop() {
        let lease = Revocation::default();
        let first_request = lease.generation();
        assert!(lease.accepts(first_request));
        lease.invalidate();
        assert!(!lease.accepts(first_request));
        let resumed_request = lease.generation();
        assert!(lease.accepts(resumed_request));
        assert!(!lease.accepts(first_request));
        lease.cancel();
        assert!(!lease.accepts(resumed_request));
        assert!(!lease.accepts(lease.generation()));
    }
    fn frame(rgb: [u8; 3], t: u64) -> Frame {
        Frame::new(
            64,
            36,
            (0..64 * 36)
                .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
                .collect(),
            Duration::from_secs(t),
        )
        .unwrap()
    }
    #[test]
    fn static_view_only_reports_when_the_fallback_is_due() {
        let mut s = Sampler::default();
        assert!(s.push(frame([70, 70, 70], 0)));
        assert!(s.take().is_some());
        for t in 1..19 {
            assert!(!s.tick(Duration::from_secs(t)));
        }
        assert!(s.tick(Duration::from_secs(20)), "static fallback after 20s");
        assert_eq!(s.info.last().unwrap().via, "floor");
        assert!(s.take_if_due(Duration::from_secs(20), false).is_none());
        s.tick(Duration::from_secs(40));
        assert!(s.take_if_due(Duration::from_secs(40), false).is_some());
    }
    #[test]
    fn changes_are_kept_in_order_and_the_sheet_is_bounded() {
        let mut s = Sampler::default();
        for (t, rgb) in [
            [255, 0, 0],
            [0, 255, 0],
            [0, 0, 255],
            [200, 200, 200],
            [0, 0, 0],
            [255, 255, 255],
        ]
        .into_iter()
        .enumerate()
        {
            s.push(frame(rgb, t as u64));
        }
        let b = s.take().unwrap();
        assert_eq!(b.info.len(), MAX_FRAMES);
        assert!(b.info.first().unwrap().at <= b.info.last().unwrap().at);
        let mut counts = 0;
        let mut s = Sampler::default();
        for t in 0..30 {
            if s.push(frame([t as u8 * 3; 3], t)) {
                counts += 1;
            }
        }
        assert!(counts > 0);
    }
    #[test]
    fn change_box_is_normalised_and_pixels_match_cells() {
        let a = vec![[0.0, 0.0, 0.0]; GRID_W * GRID_H];
        let mut b = a.clone();
        b[10 * GRID_W + 20] = [255.0, 255.0, 255.0];
        let box_ = change_box(&a, &b).unwrap();
        assert!((box_[0] - 20.0 / 63.0).abs() < 1e-4);
        assert!((box_[1] - 10.0 / 35.0).abs() < 1e-4);
        let b = Storyboard {
            frames: vec![
                frame([255, 0, 0], 1),
                frame([0, 255, 0], 3),
                frame([0, 0, 255], 5),
            ],
            info: vec![],
        };
        let mut reader = png::Decoder::new(std::io::Cursor::new(b.png().unwrap()))
            .read_info()
            .unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (SHEET_W as u32, SHEET_H as u32));
        // The tall left cell letterboxes a 16:9 frame vertically.
        let left = (113 * SHEET_W) * 3;
        assert_eq!(&buf[left..left + 3], &[255, 0, 0]);
        assert_eq!(&buf[(400 * 3)..(400 * 3 + 3)], &[0, 255, 0]);
        assert_eq!(&buf[(226 * SHEET_W * 3 + 400 * 3)..(226 * SHEET_W * 3 + 400 * 3 + 3)], &[0, 0, 255]);
    }
    #[test]
    fn malformed_frames_are_rejected_and_resolution_is_bounded() {
        assert!(Frame::new(u32::MAX, 2, vec![], Duration::ZERO).is_err());
        assert!(Frame::new(2, 2, vec![0; 15], Duration::ZERO).is_err());
        let f = Frame::new(1920, 1080, vec![255; 1920 * 1080 * 4], Duration::ZERO).unwrap();
        assert_eq!((f.width, f.height), (800, 450));
    }
}
