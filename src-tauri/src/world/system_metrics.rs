//! 系统硬件指标采集 —— CPU 占用、内存占用、网速。
//!
//! 进程级内存明细（`top_memory_processes`）属于**按需采集**：
//! 常规 10s 轮询只刷新总量指标，进程枚举只在系统压力提醒等
//! 需要"告诉用户谁在吃内存"的时刻才执行一次。

use std::time::Instant;

use serde::{Deserialize, Serialize};
use sysinfo::{Networks, ProcessRefreshKind, ProcessesToUpdate, System};

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

/// 按可执行名聚合后的进程内存占用（供"谁在吃内存"类提示注入）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessMemoryAgg {
    /// 可执行文件名（如 chrome.exe），保留原始大小写
    pub name: String,
    /// 同名进程数（Chrome 每个标签页一个进程，聚合后体现总量）
    pub process_count: u32,
    /// 合计物理内存（字节）
    pub total_memory_bytes: u64,
    /// 单个进程最大物理内存（字节）
    pub peak_memory_bytes: u64,
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

    /// 按需采集进程内存并按可执行名聚合，返回内存占用最高的前 `n` 组。
    ///
    /// - 排除 `exclude_pid`（调用方自身进程，防止智能体建议"杀掉自己"）
    /// - 只刷新进程内存，不碰 CPU/网络，单次成本可控；
    ///   但仍是全量进程枚举（Windows 上约几十到几百毫秒），
    ///   只允许在系统压力提醒等低频时刻调用，禁止放入常规轮询。
    /// - 进程名按小写聚合（Windows 进程名大小写不稳定），展示保留首个原始名。
    pub fn top_memory_processes(&mut self, n: usize, exclude_pid: u32) -> Vec<ProcessMemoryAgg> {
        use std::collections::HashMap;
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::new().with_memory(),
        );

        // key: 小写进程名 → (展示名, 进程数, 合计内存, 单进程峰值)
        let mut agg: HashMap<String, (String, u32, u64, u64)> = HashMap::new();
        for (pid, p) in self.sys.processes() {
            if pid.as_u32() == exclude_pid {
                continue;
            }
            let raw = p.name().to_string_lossy();
            if raw.is_empty() {
                continue;
            }
            let mem = p.memory();
            if mem == 0 {
                continue;
            }
            let entry = agg
                .entry(raw.to_lowercase())
                .or_insert_with(|| (raw.to_string(), 0, 0, 0));
            entry.1 += 1;
            entry.2 += mem;
            entry.3 = entry.3.max(mem);
        }

        let mut items: Vec<ProcessMemoryAgg> = agg
            .into_iter()
            .map(|(_, (name, process_count, total_memory_bytes, peak_memory_bytes))| {
                ProcessMemoryAgg {
                    name,
                    process_count,
                    total_memory_bytes,
                    peak_memory_bytes,
                }
            })
            .collect();
        items.sort_by(|a, b| b.total_memory_bytes.cmp(&a.total_memory_bytes));
        items.truncate(n);
        items
    }
}

impl Default for SystemMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 聚合语义：非空、按内存降序、字段合理性、n 截断生效
    #[test]
    fn top_memory_processes_sorted_and_aggregated() {
        let mut collector = SystemMetricsCollector::new();
        let top = collector.top_memory_processes(5, std::process::id());

        // 系统里必然有进程在跑
        assert!(!top.is_empty(), "进程枚举不应为空");
        assert!(top.len() <= 5, "应被 n 截断");

        // 按合计内存降序
        for w in top.windows(2) {
            assert!(
                w[0].total_memory_bytes >= w[1].total_memory_bytes,
                "应按内存降序: {:?} < {:?}",
                w[0].total_memory_bytes,
                w[1].total_memory_bytes
            );
        }

        // 字段合理性：进程数 ≥ 1、内存 > 0、峰值 ≥ 单进程平均
        for p in &top {
            assert!(p.process_count >= 1);
            assert!(p.total_memory_bytes > 0);
            assert!(p.peak_memory_bytes >= p.total_memory_bytes / p.process_count as u64);
            assert!(!p.name.is_empty());
        }
    }
}
