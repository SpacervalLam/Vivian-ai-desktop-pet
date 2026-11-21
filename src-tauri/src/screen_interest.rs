//! 本地屏幕兴趣点标注。
//!
//! 与智能避让共用同一路 1/5 分辨率本机截图（`commands::window::capture_desktop`）：
//! 不调用任何模型、不上传任何画面，纯 Rust 统计像素。标注算法：
//!
//! 1. 提取亮度 + Sobel 边缘图（与避让完全相同的一份统计）；
//! 2. 把画面按 160px 见方切格，逐格计算三项特征：
//!    - 结构覆盖率（Sobel 强度超过阈值的像素占比）—— 文字、控件、图标的主要特征；
//!    - 亮度方差 —— 纹理、图片细节；
//!    - 彩度均值 —— 照片、彩色图标；
//! 3. 丢掉几乎没有结构的格子（纯色背景、渐变壁纸）；
//! 4. 按分数从高到低做非极大值抑制，只留下彼此隔开的若干兴趣点；
//! 5. 跳过桌宠当前占据的位置——那里没有可围观的内容。
//!
//! 输出物理屏幕坐标，前端据此决定「去哪儿围观」。

use serde::Serialize;
use std::sync::Arc;

use crate::commands::window::{
    capture_desktop, erase_pet_region, luminance_and_edge, CaptureSkip, DesktopCapture,
    DESKTOP_DOWNSCALE,
};

#[derive(Clone, Serialize)]
pub struct InterestPoint {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub score: f64,
    /// `image`（彩色内容，如照片/彩色图标）或 `ui`（文字/控件）。
    pub kind: &'static str,
}

/// 格子边长（降采样像素）：32 × 5 = 物理屏幕 160px。
const CELL: i32 = 32;
/// 一次最多标注几个兴趣点。
const MAX_POINTS: usize = 4;
/// 两个兴趣点中心的最小物理间距：避免同一块内容被切成多个点。
const MIN_SEPARATION_PX: i32 = 220;
/// 单像素 Sobel 强度达到此值才算「有结构」。
const EDGE_THRESHOLD: u16 = 20;
/// 有结构的像素占比下限：只画了一条边框的空窗口不会成为兴趣点。
const MIN_EDGE_COVERAGE: f64 = 0.08;
/// 综合分数下限：低对比文字块也能过线，空窗口/纯色区域过不去。
const MIN_SCORE: f64 = 10.0;
/// 彩度达到此值视为「彩色内容」（图片），否则视为 UI/文字。
const IMAGE_CHROMA: f64 = 55.0;

#[tauri::command]
pub async fn find_screen_interest_points(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<crate::state::AppState>>,
) -> Result<serde_json::Value, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || analyze_screen(&window, &state))
        .await
        .map_err(|e| e.to_string())?
}

fn analyze_screen(
    window: &tauri::WebviewWindow,
    state: &crate::state::AppState,
) -> Result<serde_json::Value, String> {
    match capture_desktop(window, state) {
        Ok(capture) => Ok(serde_json::json!({ "points": annotate(capture) })),
        Err(CaptureSkip::Busy) => Ok(serde_json::json!({ "busy": true, "points": [] })),
        Err(CaptureSkip::Failed) => Ok(serde_json::json!({ "points": [] })),
    }
}

fn annotate(capture: DesktopCapture) -> Vec<InterestPoint> {
    let DesktopCapture { pixels, cw, ch, vx, vy, pet_windows, need_erase, .. } = capture;
    let mut pixels = pixels;

    // WDA 失败时截图中会带桌宠贴图：按避让的同一口径（中央 1/3 足迹）擦掉。
    if need_erase {
        for &(px, py, pw, ph) in &pet_windows {
            erase_pet_region(
                &mut pixels,
                cw,
                ch,
                px + pw / 3,
                py,
                (pw / 3).max(1),
                ph,
                vx,
                vy,
                DESKTOP_DOWNSCALE,
            );
        }
    }
    if cw < CELL || ch < CELL {
        return Vec::new();
    }

    let (lum, edge) = luminance_and_edge(&pixels, cw, ch);
    let cw_us = cw as usize;
    let cell_px = CELL * DESKTOP_DOWNSCALE;

    let mut candidates: Vec<InterestPoint> = Vec::new();
    let mut y = 0;
    while y + CELL <= ch {
        let mut x = 0;
        while x + CELL <= cw {
            let count = (CELL * CELL) as f64;
            let mut structured = 0u32;
            let mut sum_l = 0f64;
            let mut sum_l2 = 0f64;
            let mut sum_chroma = 0u32;
            for yy in y..y + CELL {
                for xx in x..x + CELL {
                    let idx = (yy as usize) * cw_us + (xx as usize);
                    if edge[idx] >= EDGE_THRESHOLD {
                        structured += 1;
                    }
                    let l = lum[idx] as f64;
                    sum_l += l;
                    sum_l2 += l * l;
                    let p = idx * 4;
                    let r = pixels[p] as u32;
                    let g = pixels[p + 1] as u32;
                    let b = pixels[p + 2] as u32;
                    sum_chroma += r.max(g).max(b) - r.min(g).min(b);
                }
            }
            let coverage = structured as f64 / count;
            let mean_l = sum_l / count;
            let var_l = (sum_l2 / count - mean_l * mean_l).max(0.0);
            let chroma_mean = sum_chroma as f64 / count;
            if coverage >= MIN_EDGE_COVERAGE {
                // 方差按对数压缩，避免高对比大片色块独占分数。
                let var_norm = if var_l <= 1.0 { 0.0 } else { (var_l.ln() * 2.0).min(20.0) };
                let score = coverage * 60.0 + var_norm * 0.2 + chroma_mean * 0.12;
                if score >= MIN_SCORE {
                    candidates.push(InterestPoint {
                        x: vx + x * DESKTOP_DOWNSCALE,
                        y: vy + y * DESKTOP_DOWNSCALE,
                        width: cell_px,
                        height: cell_px,
                        score: (score * 10.0).round() / 10.0,
                        kind: if chroma_mean >= IMAGE_CHROMA { "image" } else { "ui" },
                    });
                }
            }
            x += CELL;
        }
        y += CELL;
    }

    // 桌宠此刻占据的位置不算兴趣点：走过去也只是站在原地。
    let pet_bodies: Vec<(i32, i32, i32, i32)> = pet_windows
        .iter()
        .map(|&(px, py, pw, ph)| (px + pw / 3, py, (pw / 3).max(1), ph))
        .collect();
    candidates.retain(|point| {
        !pet_bodies.iter().any(|&(px, py, pw, ph)| {
            point.x < px + pw && point.x + point.width > px
                && point.y < py + ph && point.y + point.height > py
        })
    });

    // 非极大值抑制：高分优先，保留彼此隔开的点。
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let mut points: Vec<InterestPoint> = Vec::new();
    for candidate in candidates {
        let cx = candidate.x + candidate.width / 2;
        let cy = candidate.y + candidate.height / 2;
        let too_close = points.iter().any(|kept| {
            let dx = kept.x + kept.width / 2 - cx;
            let dy = kept.y + kept.height / 2 - cy;
            dx * dx + dy * dy < MIN_SEPARATION_PX * MIN_SEPARATION_PX
        });
        if too_close {
            continue;
        }
        points.push(candidate);
        if points.len() >= MAX_POINTS {
            break;
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(cw: i32, ch: i32, pixels: Vec<u8>) -> DesktopCapture {
        DesktopCapture {
            pixels,
            cw,
            ch,
            vx: 0,
            vy: 0,
            other_pet_windows: Vec::new(),
            pet_windows: Vec::new(),
            need_erase: false,
        }
    }

    fn blank(cw: i32, ch: i32, value: u8) -> DesktopCapture {
        let mut pixels = vec![0u8; (cw * ch * 4) as usize];
        for pixel in pixels.chunks_mut(4) {
            pixel[0] = value;
            pixel[1] = value;
            pixel[2] = value;
            pixel[3] = 255;
        }
        capture(cw, ch, pixels)
    }

    #[test]
    fn blank_or_flat_desktop_has_no_interest_points() {
        assert!(annotate(blank(64, 64, 128)).is_empty());
    }

    #[test]
    fn structured_content_is_annotated_and_separated() {
        // 左半屏画 1px 棋盘（等效于密集文字/控件），右半屏纯色。
        let cw = 64;
        let ch = 64;
        let mut pixels = vec![0u8; (cw * ch * 4) as usize];
        for y in 0..ch {
            for x in 0..cw {
                let value = if x < 32 && (x + y) % 2 == 0 { 255 } else { 16 };
                let idx = ((y * cw + x) * 4) as usize;
                pixels[idx] = value;
                pixels[idx + 1] = value;
                pixels[idx + 2] = value;
                pixels[idx + 3] = 255;
            }
        }
        let points = annotate(capture(cw, ch, pixels));
        assert!(!points.is_empty(), "left half must produce interest");
        assert!(points.iter().all(|point| point.x < 32 * DESKTOP_DOWNSCALE),
            "the flat right half must not be annotated");
        assert!(points.iter().all(|point| point.kind == "ui"));
        // 同一侧两块相邻内容的中心距离小于抑制阈值，只能留下一个点。
        assert_eq!(points.len(), 1);
    }
}
