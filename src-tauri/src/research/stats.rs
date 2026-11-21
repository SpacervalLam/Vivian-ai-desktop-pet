//! 统计引擎：从样本中提取时间/时长规律。

use std::f64::consts::PI;

/// 时间型数据的圆形统计结果
#[derive(Debug, Clone)]
pub struct CircularStats {
    /// 圆形均值（小时，0.0-24.0）
    pub mean_hour: f64,
    /// 圆形标准差（小时）
    pub stddev_hours: f64,
    /// 结果向量长度 R（0-1，越接近 1 越集中）
    pub resultant_length: f64,
}

/// 线性统计结果
#[derive(Debug, Clone)]
pub struct LinearStats {
    pub mean: f64,
    pub stddev: f64,
    pub count: usize,
}

/// 从 "HH:MM" 格式解析为小时浮点数
pub fn parse_time_to_hours(time_str: &str) -> Option<f64> {
    let parts: Vec<&str> = time_str.trim().split(':').collect();
    if parts.len() != 2 {
        return None;
    }
    let h: f64 = parts[0].trim().parse().ok()?;
    let m: f64 = parts[1].trim().parse().ok()?;
    if h >= 24.0 || m >= 60.0 {
        return None;
    }
    Some(h + m / 60.0)
}

/// 圆形均值和标准差（处理 23:59 vs 00:01 跨界）
///
/// 将小时映射到单位圆上的角度，计算 sin/cos 均值，
/// 再用 atan2 还原为小时，resultant length 衡量集中度。
pub fn circular_mean_stddev(hours: &[f64]) -> Option<CircularStats> {
    if hours.is_empty() {
        return None;
    }

    let n = hours.len() as f64;
    let mut sin_sum = 0.0f64;
    let mut cos_sum = 0.0f64;

    for &h in hours {
        let angle = h / 24.0 * 2.0 * PI;
        sin_sum += angle.sin();
        cos_sum += angle.cos();
    }

    let sin_mean = sin_sum / n;
    let cos_mean = cos_sum / n;
    let r = (sin_mean * sin_mean + cos_mean * cos_mean).sqrt();

    // 均值角度 → 小时
    let mut mean_angle = sin_mean.atan2(cos_mean);
    if mean_angle < 0.0 {
        mean_angle += 2.0 * PI;
    }
    let mean_hour = mean_angle / (2.0 * PI) * 24.0;

    // 圆形标准差：sqrt(-2 * ln(R))，转换为小时
    let stddev_hours = if r > 0.0 && r < 1.0 {
        (-2.0 * r.ln()).sqrt() / (2.0 * PI) * 24.0
    } else if r >= 1.0 {
        0.0
    } else {
        12.0 // R=0 表示完全分散
    };

    Some(CircularStats {
        mean_hour,
        stddev_hours,
        resultant_length: r,
    })
}

/// 线性均值和标准差（用于时长等数据）
pub fn linear_mean_stddev(values: &[f64]) -> Option<LinearStats> {
    if values.is_empty() {
        return None;
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance = if values.len() > 1 {
        values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0)
    } else {
        0.0
    };
    Some(LinearStats {
        mean,
        stddev: variance.sqrt(),
        count: values.len(),
    })
}

/// 计算置信度：样本量因子 × 集中度因子
///
/// - 样本量因子：min(1.0, count / 15)
/// - 集中度因子：时间型用 resultant_length，线性型用 1 - normalized_variance
pub fn compute_confidence(count: usize, concentration: f64) -> f64 {
    let count_factor = (count as f64 / 15.0).min(1.0);
    let concentration_factor = concentration.clamp(0.0, 1.0);
    count_factor * concentration_factor
}

/// 是否满足自动结论条件
pub fn should_conclude(confidence: f64, count: usize) -> bool {
    confidence >= 0.85 && count >= 8
}

/// 判断新值是否偏离均值超过 2 个标准差
pub fn deviates_2sigma(new_value: f64, mean: f64, stddev: f64) -> bool {
    if stddev <= 0.0 {
        return (new_value - mean).abs() > 0.01;
    }
    (new_value - mean).abs() > 2.0 * stddev
}

/// 将小时浮点数格式化为 "HH:MM" 字符串
pub fn format_hour(h: f64) -> String {
    let total_minutes = (h * 60.0).round() as i64;
    let hours = (total_minutes / 60) % 24;
    let minutes = total_minutes % 60;
    format!("{:02}:{:02}", hours, minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time() {
        assert_eq!(parse_time_to_hours("22:30"), Some(22.5));
        assert_eq!(parse_time_to_hours("0:15"), Some(0.25));
        assert_eq!(parse_time_to_hours("invalid"), None);
        assert_eq!(parse_time_to_hours("25:00"), None);
    }

    #[test]
    fn circular_mean_basic() {
        // 22:00, 23:00, 22:30 → 均值约 22:30
        let hours = vec![22.0, 23.0, 22.5];
        let stats = circular_mean_stddev(&hours).unwrap();
        assert!((stats.mean_hour - 22.5).abs() < 0.5);
        assert!(stats.resultant_length > 0.9);
    }

    #[test]
    fn circular_mean_wraparound() {
        // 23:30, 00:30 → 均值约 00:00（跨界）
        let hours = vec![23.5, 0.5];
        let stats = circular_mean_stddev(&hours).unwrap();
        // 均值应在 0 附近（或 24 附近）
        let dist_to_midnight = stats.mean_hour.min(24.0 - stats.mean_hour);
        assert!(dist_to_midnight < 0.5);
    }

    #[test]
    fn linear_stats_basic() {
        let values = vec![30.0, 45.0, 40.0, 35.0, 50.0];
        let stats = linear_mean_stddev(&values).unwrap();
        assert_eq!(stats.mean, 40.0);
        assert_eq!(stats.count, 5);
        assert!(stats.stddev > 0.0);
    }

    #[test]
    fn confidence_calculation() {
        // 15 个样本 + 完美集中 → 1.0
        assert!((compute_confidence(15, 1.0) - 1.0).abs() < 0.001);
        // 5 个样本 + 完美集中 → 5/15 = 0.333
        assert!((compute_confidence(5, 1.0) - 0.333).abs() < 0.01);
        // 15 个样本 + 分散 → 0.5
        assert!((compute_confidence(15, 0.5) - 0.5).abs() < 0.001);
    }

    #[test]
    fn conclude_threshold() {
        assert!(should_conclude(0.9, 10));
        assert!(!should_conclude(0.8, 10)); // 置信度不够
        assert!(!should_conclude(0.9, 5)); // 样本不够
    }

    #[test]
    fn deviation_check() {
        assert!(deviates_2sigma(100.0, 50.0, 10.0)); // 5σ 偏离
        assert!(!deviates_2sigma(55.0, 50.0, 10.0)); // 0.5σ 内
    }

    #[test]
    fn format_hour_test() {
        assert_eq!(format_hour(22.5), "22:30");
        assert_eq!(format_hour(0.0), "00:00");
        assert_eq!(format_hour(23.99), "23:59");
    }
}
