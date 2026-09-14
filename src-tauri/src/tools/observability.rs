//! 工具可观测性系统 - 收集工具调用的指标、执行时间、失败率监控、调用历史

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// 工具调用记录（一次调用开始后即创建，结束时填充结果）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub call_id: String,
    pub input_data: Value,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    pub duration_ms: f64,
    pub success: bool,
    pub output_data: Option<Value>,
    pub error: Option<String>,
}

impl ToolCallRecord {
    fn new(tool_name: &str, input_data: Value) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            call_id: Uuid::new_v4().to_string()[..8].to_string(),
            input_data,
            start_time_ms: chrono::Utc::now().timestamp_millis(),
            end_time_ms: 0,
            duration_ms: 0.0,
            success: false,
            output_data: None,
            error: None,
        }
    }
}

/// 工具聚合指标
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolMetrics {
    pub tool_name: String,
    pub total_calls: u64,
    pub successful_calls: u64,
    pub failed_calls: u64,
    pub total_duration_ms: f64,
    pub min_duration_ms: f64,
    pub max_duration_ms: f64,
    pub total_input_chars: u64,
    pub total_output_chars: u64,
    pub last_called_at_ms: i64,
}

impl ToolMetrics {
    fn new(tool_name: &str) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            total_calls: 0,
            successful_calls: 0,
            failed_calls: 0,
            total_duration_ms: 0.0,
            min_duration_ms: f64::MAX,
            max_duration_ms: 0.0,
            total_input_chars: 0,
            total_output_chars: 0,
            last_called_at_ms: 0,
        }
    }

    pub fn success_rate(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.successful_calls as f64 / self.total_calls as f64
        }
    }

    pub fn avg_duration_ms(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.total_duration_ms / self.total_calls as f64
        }
    }

    pub fn failure_rate(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.failed_calls as f64 / self.total_calls as f64
        }
    }
}

/// 可观测性记录句柄，由 `start_call` 返回，结束调用时传入 `end_call`
pub struct ObsRecord {
    pub inner: ToolCallRecord,
    start: Instant,
}

/// 工具可观测性管理器
///
/// 功能：
/// - 记录每次工具调用的详细信息
/// - 聚合工具级别的指标统计
/// - 支持查询和分析
pub struct ToolObservability {
    inner: RwLock<ObsInner>,
}

struct ObsInner {
    /// 每个工具的调用记录
    records: HashMap<String, Vec<ToolCallRecord>>,
    /// 每个工具的聚合指标
    metrics: HashMap<String, ToolMetrics>,
    /// 每个工具最大记录数
    max_records_per_tool: usize,
    /// 启动时间
    global_start: Instant,
}

impl ToolObservability {
    pub fn new(max_records_per_tool: usize) -> Self {
        Self {
            inner: RwLock::new(ObsInner {
                records: HashMap::new(),
                metrics: HashMap::new(),
                max_records_per_tool,
                global_start: Instant::now(),
            }),
        }
    }

    /// 开始记录工具调用
    pub fn start_call(&self, tool_name: &str, input_data: Value) -> ObsRecord {
        let mut inner = self.inner.write();
        inner
            .metrics
            .entry(tool_name.to_string())
            .or_insert_with(|| ToolMetrics::new(tool_name));
        drop(inner);

        let record = ToolCallRecord::new(tool_name, input_data);
        ObsRecord {
            inner: record,
            start: Instant::now(),
        }
    }

    /// 结束记录工具调用
    pub fn end_call(
        &self,
        record: ObsRecord,
        success: bool,
        output: Option<Value>,
        error: Option<String>,
    ) {
        let duration_ms = record.start.elapsed().as_secs_f64() * 1000.0;
        let end_time_ms = chrono::Utc::now().timestamp_millis();

        let mut record = record;
        record.inner.end_time_ms = end_time_ms;
        record.inner.duration_ms = duration_ms;
        record.inner.success = success;
        record.inner.output_data = output.clone();
        record.inner.error = error;

        let tool_name = record.inner.tool_name.clone();
        let input_chars = record.inner.input_data.to_string().len() as u64;
        let output_chars = output
            .as_ref()
            .map(|o| o.to_string().len() as u64)
            .unwrap_or(0);

        let mut inner = self.inner.write();
        let metrics = inner
            .metrics
            .entry(tool_name.clone())
            .or_insert_with(|| ToolMetrics::new(&tool_name));
        metrics.total_calls += 1;
        if success {
            metrics.successful_calls += 1;
        } else {
            metrics.failed_calls += 1;
        }
        metrics.total_duration_ms += duration_ms;
        if duration_ms < metrics.min_duration_ms {
            metrics.min_duration_ms = duration_ms;
        }
        if duration_ms > metrics.max_duration_ms {
            metrics.max_duration_ms = duration_ms;
        }
        metrics.total_input_chars += input_chars;
        metrics.total_output_chars += output_chars;
        metrics.last_called_at_ms = end_time_ms;

        let max = inner.max_records_per_tool;
        let records = inner
            .records
            .entry(tool_name)
            .or_insert_with(Vec::new);
        records.push(record.inner);
        if records.len() > max {
            let drop_count = records.len() - max;
            records.drain(..drop_count);
        }
    }

    /// 获取工具指标
    pub fn get_metrics(&self, tool_name: &str) -> Option<ToolMetrics> {
        self.inner.read().metrics.get(tool_name).cloned()
    }

    /// 获取所有工具指标
    pub fn get_all_metrics(&self) -> HashMap<String, ToolMetrics> {
        self.inner.read().metrics.clone()
    }

    /// 获取最近的调用记录
    pub fn get_recent_records(
        &self,
        tool_name: &str,
        limit: usize,
        success_only: bool,
    ) -> Vec<ToolCallRecord> {
        let inner = self.inner.read();
        match inner.records.get(tool_name) {
            Some(records) => {
                let filtered: Vec<_> = if success_only {
                    records.iter().filter(|r| r.success).cloned().collect()
                } else {
                    records.clone()
                };
                filtered.into_iter().rev().take(limit).collect()
            }
            None => Vec::new(),
        }
    }

    /// 获取错误调用记录
    pub fn get_error_records(&self, tool_name: &str, limit: usize) -> Vec<ToolCallRecord> {
        let inner = self.inner.read();
        match inner.records.get(tool_name) {
            Some(records) => records
                .iter()
                .filter(|r| !r.success)
                .rev()
                .take(limit)
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    }

    /// 获取可观测性摘要
    pub fn summary(&self) -> Value {
        let inner = self.inner.read();
        let total_calls: u64 = inner.metrics.values().map(|m| m.total_calls).sum();
        let total_failures: u64 = inner.metrics.values().map(|m| m.failed_calls).sum();
        let total_duration: f64 = inner
            .metrics
            .values()
            .map(|m| m.total_duration_ms)
            .sum();
        let tools_tracked = inner.metrics.len();
        let uptime_seconds = inner.global_start.elapsed().as_secs_f64();

        let mut slowest_tool = String::new();
        let mut max_avg = 0.0;
        for (name, m) in &inner.metrics {
            let avg = m.avg_duration_ms();
            if avg > max_avg {
                max_avg = avg;
                slowest_tool = name.clone();
            }
        }

        let mut most_failing_tool = String::new();
        let mut max_fail_rate = 0.0;
        for (name, m) in &inner.metrics {
            if m.total_calls >= 3 && m.failure_rate() > max_fail_rate {
                max_fail_rate = m.failure_rate();
                most_failing_tool = name.clone();
            }
        }

        let overall_success_rate = if total_calls > 0 {
            1.0 - (total_failures as f64 / total_calls as f64)
        } else {
            0.0
        };

        serde_json::json!({
            "uptime_seconds": uptime_seconds,
            "total_calls": total_calls,
            "total_failures": total_failures,
            "overall_success_rate": overall_success_rate,
            "total_duration_ms": total_duration,
            "tools_tracked": tools_tracked,
            "slowest_tool": slowest_tool,
            "most_failing_tool": most_failing_tool,
        })
    }

    /// 重置所有数据
    pub fn reset(&self) {
        let mut inner = self.inner.write();
        inner.records.clear();
        inner.metrics.clear();
        inner.global_start = Instant::now();
    }
}

/// 创建默认的可观测性实例
pub fn default_observability() -> Arc<ToolObservability> {
    Arc::new(ToolObservability::new(1000))
}
