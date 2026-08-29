//! 系统硬件指标采集 —— CPU 占用、内存占用、网速。

use std::time::Instant;

use serde::{Deserialize, Serialize};
use sysinfo::{Networks, System};

/// 系统硬件指标快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    /// CPU 总占用百分比（0-100）
    pub cpu_usage: f32,
    /// 物理内存总量（字节）
    pub memory_total: u64,
    /// 已用物理内存（字节）
    pub memory_used: u64,
    /// 内存占用百分比（0-100）
    pub memory_usage_pct: f32,
    /// 下载速度（字节/秒）
    pub net_download_bps: u64,
    /// 上传速度（字节/秒）
    pub net_upload_bps: u64,
}

/// 系统指标采集器（持有 sysinfo 句柄，跨轮询复用）
pub struct SystemMetricsCollector {
    sys: System,
    networks: Networks,
    /// 上一次网络采样 (总接收字节, 总发送字节, 时刻)
    prev_net: Option<(u64, u64, Instant)>,
}

impl SystemMetricsCollector {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();

        let mut networks = Networks::new();
        networks.refresh_list();

        Self {
            sys,
            networks,
            prev_net: None,
        }
    }

    /// 刷新并返回当前系统指标快照
    pub fn refresh(&mut self) -> SystemMetrics {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.networks.refresh();

        let cpu_usage = self.sys.global_cpu_usage();

        let memory_total = self.sys.total_memory();
        let memory_used = self.sys.used_memory();
        let memory_usage_pct = if memory_total > 0 {
            (memory_used as f64 / memory_total as f64 * 100.0) as f32
        } else {
            0.0
        };

        let mut total_rx: u64 = 0;
        let mut total_tx: u64 = 0;
        for (_name, data) in self.networks.iter() {
            total_rx += data.total_received();
            total_tx += data.total_transmitted();
        }

        let now = Instant::now();
        let (net_download_bps, net_upload_bps) =
            if let Some((prev_rx, prev_tx, prev_time)) = self.prev_net {
                let elapsed = now.duration_since(prev_time).as_secs_f64();
                if elapsed > 0.1 {
                    (
                        ((total_rx.saturating_sub(prev_rx)) as f64 / elapsed) as u64,
                        ((total_tx.saturating_sub(prev_tx)) as f64 / elapsed) as u64,
                    )
                } else {
                    (0, 0)
                }
            } else {
                (0, 0)
            };
        self.prev_net = Some((total_rx, total_tx, now));

        SystemMetrics {
            cpu_usage,
            memory_total,
            memory_used,
            memory_usage_pct,
            net_download_bps,
            net_upload_bps,
        }
    }
}

impl Default for SystemMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}
