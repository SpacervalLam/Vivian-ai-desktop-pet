//! 抠底：把平坦的键控背景转成 alpha 通道。
//!
//! 为什么需要：真透明背景不是所有生图模型都支持。不支持时只能走「生成时铺一块纯色底
//! → 本地把这个颜色变透明」，否则 sprite / 图标这类素材落地后带着一块死白或死绿，
//! 在深色 UI 上直接穿帮。
//!
//! 判定用 Chebyshev 距离（各通道差的最大值）而不是欧氏距离：对纯色背景来说，
//! 各通道差的最大值才是"离这个颜色有多远"的直观度量，且不受通道数平方根影响。
//! 边界用 smoothstep 过渡，避免硬阈值在抗锯齿边缘留下锯齿。

/// 键色的透明度阈值：距离小于它视为纯背景。
const TRANSPARENT_THRESHOLD: f32 = 12.0;
/// 距离大于它视为完全不透明。
const OPAQUE_THRESHOLD: f32 = 60.0;

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 从四角取样推测键色（取中位数，避免某一角被主体占据时判断失准）。
pub fn guess_key_color(rgba: &[u8], width: usize, height: usize) -> Option<(u8, u8, u8)> {
    if width == 0 || height == 0 {
        return None;
    }
    let at = |x: usize, y: usize| -> (u8, u8, u8) {
        let i = (y * width + x) * 4;
        (rgba[i], rgba[i + 1], rgba[i + 2])
    };
    let corners = [
        at(0, 0),
        at(width - 1, 0),
        at(0, height - 1),
        at(width - 1, height - 1),
    ];
    // 四角颜色不一致说明不是纯色底，抠出来的边缘会很脏，直接放弃
    let (mut r, mut g, mut b): (Vec<u8>, Vec<u8>, Vec<u8>) = (vec![], vec![], vec![]);
    for c in corners {
        r.push(c.0);
        g.push(c.1);
        b.push(c.2);
    }
    r.sort_unstable();
    g.sort_unstable();
    b.sort_unstable();
    let med = (r[1], g[1], b[1]);
    let spread = corners
        .iter()
        .map(|c| {
            (c.0 as i32 - med.0 as i32)
                .abs()
                .max((c.1 as i32 - med.1 as i32).abs())
                .max((c.2 as i32 - med.2 as i32).abs())
        })
        .max()
        .unwrap_or(0);
    if spread > 24 {
        return None;
    }
    Some(med)
}

/// 解码 PNG → RGBA8。仅接受 8 位；其它格式返回 Err（调用方据此跳过抠底）。
pub fn decode_rgba_png(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("PNG 解码失败：{e}"))?;
    let mut buf = vec![
        0u8;
        reader
            .output_buffer_size()
            .ok_or_else(|| "PNG 尺寸异常".to_string())?
    ];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("PNG 读取失败：{e}"))?;
    buf.truncate(info.buffer_size());
    if info.bit_depth != png::BitDepth::Eight {
        return Err("只支持 8 位 PNG".to_string());
    }
    // 统一转成 RGBA8：灰度/RGB 都要补 alpha，否则下面按 4 字节步进会错位
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(buf.len() / 3 * 4);
            for px in buf.chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity(buf.len() * 4);
            for v in buf {
                out.extend_from_slice(&[v, v, v, 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity(buf.len() / 2 * 4);
            for px in buf.chunks_exact(2) {
                out.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
            out
        }
        other => return Err(format!("不支持的 PNG 色彩类型：{other:?}")),
    };
    Ok((rgba, info.width, info.height))
}

/// 图片是否已经带真透明（任一像素 alpha < 250）。
pub fn has_alpha(bytes: &[u8]) -> bool {
    match decode_rgba_png(bytes) {
        Ok((rgba, _, _)) => rgba.chunks_exact(4).any(|p| p[3] < 250),
        Err(_) => false,
    }
}

/// 把键控背景转成 alpha，返回新的 PNG 字节。
///
/// `key` 为 None 时从四角推断；推断失败（四角颜色不一致）返回 Err，
/// 由调用方决定是保留原图还是提示用户改用支持真透明的模型。
pub fn cutout_png(bytes: &[u8], key: Option<(u8, u8, u8)>) -> Result<(Vec<u8>, (u8, u8, u8)), String> {
    let (mut rgba, width, height) = decode_rgba_png(bytes)?;
    let key_color = match key {
        Some(k) => k,
        None => guess_key_color(&rgba, width as usize, height as usize)
            .ok_or_else(|| "四角颜色不一致，无法自动判断背景色；请改用支持透明背景的模型".to_string())?,
    };
    let (kr, kg, kb) = (key_color.0 as f32, key_color.1 as f32, key_color.2 as f32);

    for px in rgba.chunks_exact_mut(4) {
        let (r, g, b) = (px[0] as f32, px[1] as f32, px[2] as f32);
        let distance = (r - kr).abs().max((g - kg).abs()).max((b - kb).abs());
        if distance >= OPAQUE_THRESHOLD {
            continue;
        }
        let alpha = if distance <= TRANSPARENT_THRESHOLD {
            0.0
        } else {
            let t = (distance - TRANSPARENT_THRESHOLD) / (OPAQUE_THRESHOLD - TRANSPARENT_THRESHOLD);
            smoothstep(t)
        };
        let original_alpha = px[3] as f32 / 255.0;
        px[3] = (255.0 * alpha.min(original_alpha)).round().clamp(0.0, 255.0) as u8;
        // 去溢色：半透明边缘常带着键色，按「键色通道不许超过其它通道太多」压一下
        if px[3] > 0 {
            let key_max = kf_max(key_color);
            if key_max >= 128.0 {
                let spill: Vec<usize> = [kr, kg, kb]
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| **v >= key_max - 16.0)
                    .map(|(i, _)| i)
                    .collect();
                if !spill.is_empty() {
                    let non_spill_max = (0..3)
                        .filter(|i| !spill.contains(i))
                        .map(|i| px[i])
                        .max()
                        .unwrap_or(0);
                    for i in spill {
                        if px[i] > non_spill_max.saturating_add(8) {
                            px[i] = non_spill_max;
                        }
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| format!("PNG 编码失败：{e}"))?;
        writer
            .write_image_data(&rgba)
            .map_err(|e| format!("PNG 写入失败：{e}"))?;
    }
    Ok((out, key_color))
}

fn kf_max(c: (u8, u8, u8)) -> f32 {
    c.0.max(c.1).max(c.2) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张 8x8 的纯绿底 + 中心红块，抠底后中心不透明、角落全透明。
    #[test]
    fn removes_flat_key_background() {
        let (w, h) = (8u32, 8u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let center = (3..5).contains(&x) && (3..5).contains(&y);
                if center {
                    rgba.extend_from_slice(&[220, 30, 30, 255]);
                } else {
                    rgba.extend_from_slice(&[0, 255, 0, 255]);
                }
            }
        }
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().unwrap();
            wr.write_image_data(&rgba).unwrap();
        }
        let (out, key) = cutout_png(&png_bytes, None).unwrap();
        assert_eq!(key, (0, 255, 0));
        let (decoded, _, _) = decode_rgba_png(&out).unwrap();
        assert_eq!(decoded[3], 0, "左上角应被抠掉");
        // 中心像素 (3,3) -> idx (3*8+3)*4
        let center = (3 * 8 + 3) * 4;
        assert_eq!(decoded[center + 3], 255, "主体应保持不透明");
    }

    #[test]
    fn already_transparent_is_detected() {
        let (w, h) = (2u32, 2u32);
        let rgba: Vec<u8> = vec![255, 0, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255];
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().unwrap();
            wr.write_image_data(&rgba).unwrap();
        }
        assert!(has_alpha(&png_bytes));
    }
}
