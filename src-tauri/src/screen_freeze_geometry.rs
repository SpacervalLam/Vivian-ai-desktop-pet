/// One scale controls both axes, including when size limits are reached.
pub fn dimensions(width: u32, height: u32, dpi: f64, scale: f64) -> (u32, u32, f64) {
    let base_width = width as f64;
    let base_height = height as f64;
    let maximum = 8.0_f64.min(4096.0 / base_width).min(4096.0 / base_height).max(1.0);
    let minimum = 0.1_f64.max(160.0 * dpi / base_width).max(120.0 * dpi / base_height).min(1.0);
    let scale = scale.clamp(minimum, maximum);
    ((base_width * scale).round().max(1.0) as u32, (base_height * scale).round().max(1.0) as u32, scale)
}

/// Dragging a left/top edge keeps the opposite edge in place.
pub fn anchored_position(x: i32, y: i32, old_width: u32, old_height: u32, width: u32, height: u32, edge: &str) -> (i32, i32) {
    (x + if edge.contains('w') { old_width as i32 - width as i32 } else { 0 },
     y + if edge.contains('n') { old_height as i32 - height as i32 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_pixel_size_is_the_zoom_baseline_at_every_dpi() {
        for (w, h) in [(2, 2), (40, 20), (640, 480), (4000, 300), (8000, 2000)] {
            for dpi in [1.0, 1.5, 2.0] {
                assert_eq!(dimensions(w, h, dpi, 1.0), (w, h, 1.0));
            }
        }
    }
    #[test]
    fn limits_preserve_aspect_ratio_for_all_scales_and_dpi() {
        for (w, h) in [(640, 480), (4000, 300), (4, 2000), (2, 2)] {
            for dpi in [1.0, 1.5, 2.0] {
                for scale in [0.1, 0.8, 1.0, 8.0] {
                    let (width, height, _) = dimensions(w, h, dpi, scale);
                    assert!(width <= 4096 && height <= 4096);
                    // Integer rounding is the only permitted aspect-ratio error.
                    assert!((width as f64 * h as f64 - height as f64 * w as f64).abs() <= (w + h) as f64 / 2.0);
                }
            }
        }
    }
    #[test]
    fn left_and_top_edges_keep_the_opposite_corner_fixed() {
        for edge in ["n", "ne", "e", "se", "s", "sw", "w", "nw"] {
            let (x, y) = anchored_position(-800, 100, 640, 480, 800, 600, edge);
            assert_eq!(x, if edge.contains('w') { -960 } else { -800 });
            assert_eq!(y, if edge.contains('n') { -20 } else { 100 });
        }
    }
}
