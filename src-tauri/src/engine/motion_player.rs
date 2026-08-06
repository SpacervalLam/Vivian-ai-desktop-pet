//! 动作曲线与动作播放器
//!
//! 负责从预解析元数据加载动作信息，按时间插值出参数值。
//! 启用 encryptResources 后，运行时不再直接读取 .motion3.json 文件。

use serde::Deserialize;
use std::path::Path;
use std::time::Instant;
use tracing::{debug, error};

use super::pre_parsed::get_pre_parsed_motion_meta;

/// motion3.json 中单条曲线的原始数据
#[derive(Debug, Deserialize)]
pub struct CurveData {
    #[serde(rename = "Target")]
    pub target: String,
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Segments")]
    pub segments: Vec<f64>,
}

/// 动作元数据（从 build.rs 的预解析数据获取）
#[derive(Debug, Clone)]
pub struct MotionMeta {
    pub duration: f64,
    pub fps: u32,
    pub is_loop: bool,
    pub total_frames: u64,
    pub curve_count: usize,
}

/// motion3.json 文件结构（仅在 dev 模式下回退使用）
#[derive(Debug, Deserialize)]
pub struct MotionFile {
    #[serde(default)]
    pub meta: MotionMetaFile,
    #[serde(default)]
    pub curves: Vec<CurveData>,
}

#[derive(Debug, Default, Deserialize)]
pub struct MotionMetaFile {
    #[serde(default, rename = "Duration")]
    duration: f64,
    #[serde(default = "default_fps", rename = "Fps")]
    fps: u32,
    #[serde(default = "default_loop", rename = "Loop")]
    is_loop: bool,
    #[serde(default, rename = "TotalFrameCount")]
    total_frames: u64,
}

fn default_fps() -> u32 { 30 }
fn default_loop() -> bool { true }

/// 关键帧（解析 Segments 后的内部表示）
#[derive(Debug, Clone)]
struct Keyframe {
    start_time: f64,
    start_value: f64,
    end_time: f64,
    end_value: f64,
}

/// 动作曲线 - 解析 motion3.json 的 `Curve.Segments`，按时间插值取值
#[derive(Debug, Clone)]
pub struct MotionCurve {
    pub target: String,
    pub parameter_id: String,
    keyframes: Vec<Keyframe>,
}

impl MotionCurve {
    pub fn new(curve_data: &CurveData) -> Self {
        let keyframes = Self::parse_segments(&curve_data.segments);
        Self {
            target: curve_data.target.clone(),
            parameter_id: curve_data.id.clone(),
            keyframes,
        }
    }

    fn parse_segments(data: &[f64]) -> Vec<Keyframe> {
        let mut keyframes = Vec::new();
        let mut i = 0;
        while i < data.len() {
            if i + 1 < data.len() {
                let start_time = data[i];
                let start_value = data[i + 1];

                let mut j = i + 2;
                while j < data.len() && data[j] <= start_time {
                    j += 1;
                }

                if j + 1 < data.len() {
                    let end_time = data[j];
                    let end_value = data[j + 1];
                    keyframes.push(Keyframe {
                        start_time,
                        start_value,
                        end_time,
                        end_value,
                    });
                    i = j + 2;
                } else {
                    keyframes.push(Keyframe {
                        start_time,
                        start_value,
                        end_time: start_time + 1.0,
                        end_value: start_value,
                    });
                    break;
                }
            } else {
                break;
            }
        }
        keyframes
    }

    pub fn get_value(&self, time: f64) -> f64 {
        if self.keyframes.is_empty() {
            return 0.0;
        }

        for keyframe in &self.keyframes {
            if keyframe.start_time <= time && time <= keyframe.end_time {
                let span = keyframe.end_time - keyframe.start_time;
                let t = if span.abs() < f64::EPSILON {
                    0.0
                } else {
                    (time - keyframe.start_time) / span
                };
                let value =
                    keyframe.start_value + t * (keyframe.end_value - keyframe.start_value);
                return (value / 10.0).clamp(0.0, 1.0);
            }
        }

        self.keyframes
            .last()
            .map(|k| (k.end_value / 10.0).clamp(0.0, 1.0))
            .unwrap_or(0.0)
    }
}

/// 动作播放器内部状态
struct MotionPlayerInner {
    duration: f64,
    start_time: Option<Instant>,
    is_playing: bool,
    is_looping: bool,
    on_end_callback: Option<Box<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for MotionPlayerInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MotionPlayerInner")
            .field("duration", &self.duration)
            .field("is_playing", &self.is_playing)
            .field("is_looping", &self.is_looping)
            .finish()
    }
}

/// 动作播放器 - 从预解析元数据加载动作信息，按时间输出参数值
pub struct MotionPlayer {
    inner: parking_lot::RwLock<MotionPlayerInner>,
}

impl Default for MotionPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl MotionPlayer {
    pub fn new() -> Self {
        Self {
            inner: parking_lot::RwLock::new(MotionPlayerInner {
                duration: 0.0,
                start_time: None,
                is_playing: false,
                is_looping: false,
                on_end_callback: None,
            }),
        }
    }

    /// 从预解析元数据加载动作（推荐使用，不读取文件）
    pub fn load_from_meta(&self, meta: &MotionMeta) -> bool {
        let mut inner = self.inner.write();
        inner.duration = meta.duration;
        debug!(
            "[MotionPlayer] 从预解析元数据加载: duration={}, fps={}, loop={}",
            inner.duration, meta.fps, meta.is_loop
        );
        true
    }

    /// 从动作文件路径加载（优先使用嵌入元数据，回退到文件读取用于开发模式）
    pub fn load_motion(&self, motion_path: &str) -> bool {
        let path = Path::new(motion_path);

        // 1. 尝试从路径提取 char_id 和 motion_name，查找嵌入元数据
        if let (Some(char_id), Some(motion_name)) = extract_path_info(path) {
            if let Some(meta) = get_pre_parsed_motion_meta(&char_id, &motion_name) {
                return self.load_from_meta(&MotionMeta {
                    duration: meta.duration,
                    fps: meta.fps,
                    is_loop: meta.is_loop,
                    total_frames: meta.total_frames,
                    curve_count: meta.curve_count,
                });
            }
        }

        // 2. 回退：直接读取文件（开发模式或加密被禁用时使用）
        match std::fs::read_to_string(path) {
            Ok(content) => {
                match serde_json::from_str::<MotionFile>(&content) {
                    Ok(file) => {
                        let meta = MotionMeta {
                            duration: file.meta.duration,
                            fps: file.meta.fps,
                            is_loop: file.meta.is_loop,
                            total_frames: file.meta.total_frames,
                            curve_count: file.curves.len(),
                        };
                        self.load_from_meta(&meta)
                    }
                    Err(e) => {
                        error!("[MotionPlayer] 解析动作文件失败: {} - {}", motion_path, e);
                        false
                    }
                }
            }
            Err(e) => {
                error!("[MotionPlayer] 读取动作文件失败: {} - {}", motion_path, e);
                false
            }
        }
    }

    /// 开始播放动作
    pub fn play(&self, r#loop: bool, on_end_callback: Option<Box<dyn Fn() + Send + Sync>>) {
        let mut inner = self.inner.write();
        inner.start_time = Some(Instant::now());
        inner.is_playing = true;
        inner.is_looping = r#loop;
        inner.on_end_callback = on_end_callback;
    }

    /// 停止播放
    pub fn stop(&self) {
        let mut inner = self.inner.write();
        inner.is_playing = false;
        inner.on_end_callback = None;
    }

    /// 是否正在播放
    pub fn is_playing(&self) -> bool {
        self.inner.read().is_playing
    }

    /// 获取动作时长
    pub fn get_duration(&self) -> f64 {
        self.inner.read().duration
    }
}

/// 从文件路径中提取 char_id（父目录名）和 motion_name（文件名去扩展名）
fn extract_path_info(path: &Path) -> (Option<String>, Option<String>) {
    let char_id = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());

    let motion_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_string())
        .and_then(|n| {
            // 去除 .motion3.json 和 .mtn 后缀
            let n = n.strip_suffix(".motion3.json").unwrap_or(&n);
            let n = n.strip_suffix(".mtn").unwrap_or(n);
            // 也处理中间带 .motion3 的情况
            let n = n.strip_suffix(".motion3").unwrap_or(n);
            if n.is_empty() { None } else { Some(n.to_string()) }
        });

    (char_id, motion_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_path_info() {
        let path = Path::new("/Vivian/idel.motion3.json");
        let (char_id, motion_name) = extract_path_info(path);
        assert_eq!(char_id.as_deref(), Some("Vivian"));
        assert_eq!(motion_name.as_deref(), Some("idle"));
    }

    #[test]
    fn test_extract_path_info_windows() {
        let path = Path::new("C:\\projects\\public\\Nana\\blush.motion3.json");
        let (char_id, motion_name) = extract_path_info(path);
        assert_eq!(char_id.as_deref(), Some("Nana"));
        assert_eq!(motion_name.as_deref(), Some("blush"));
    }

    #[test]
    fn test_load_from_meta() {
        let player = MotionPlayer::new();
        let meta = MotionMeta {
            duration: 2.0,
            fps: 30,
            is_loop: true,
            total_frames: 60,
            curve_count: 10,
        };
        assert!(player.load_from_meta(&meta));
        assert_eq!(player.get_duration(), 2.0);
    }

    #[test]
    fn test_motion_curve_parse() {
        let curve_data = CurveData {
            target: "Parameter".to_string(),
            id: "ParamAngleX".to_string(),
            segments: vec![0.0, 0.0, 1.0, 10.0, 2.0, 0.0],
        };
        let curve = MotionCurve::new(&curve_data);
        assert!(!curve.keyframes.is_empty());
        let v = curve.get_value(0.5);
        assert!((v - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_motion_player_load_invalid() {
        let player = MotionPlayer::new();
        assert!(!player.load_motion("nonexistent.motion3.json"));
        assert!(!player.is_playing());
    }
}
