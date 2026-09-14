//! 工具结果缓存 - 为只读工具调用结果提供 TTL + LRU 缓存

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::Value;

/// 缓存条目
struct CacheEntry {
    /// 缓存值
    value: Value,
    /// 创建时间
    created_at: Instant,
    /// TTL（秒）
    ttl: Duration,
    /// 命中次数
    hits: u64,
    /// 最近访问时间
    last_accessed: Instant,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }
}

/// 工具调用结果缓存
///
/// 特性：
/// - 基于工具名称 + 输入的 MD5 键缓存
/// - 支持 TTL 过期
/// - 支持 LRU 淘汰策略
/// - 缓存命中统计
/// - 可按工具粒度控制缓存
pub struct ToolCache {
    inner: Mutex<CacheInner>,
}

struct CacheInner {
    /// 缓存条目
    entries: HashMap<String, CacheEntry>,
    /// 访问顺序（LRU）
    access_order: VecDeque<String>,
    /// 默认 TTL（秒）
    default_ttl: Duration,
    /// 最大条目数
    max_size: usize,
    /// 不参与缓存的工具
    uncached_tools: std::collections::HashSet<String>,
    /// 工具自定义 TTL（秒）
    custom_ttls: HashMap<String, u64>,
    /// 总命中数（用于统计）
    total_hits: u64,
    /// 总查询数（用于统计）
    total_lookups: u64,
}

impl ToolCache {
    /// 创建新的缓存
    ///
    /// - `default_ttl_secs`: 默认 TTL（秒）
    /// - `max_size`: 最大条目数
    pub fn new(default_ttl_secs: u64, max_size: usize) -> Self {
        Self {
            inner: Mutex::new(CacheInner {
                entries: HashMap::new(),
                access_order: VecDeque::new(),
                default_ttl: Duration::from_secs(default_ttl_secs),
                max_size,
                uncached_tools: std::collections::HashSet::new(),
                custom_ttls: HashMap::new(),
                total_hits: 0,
                total_lookups: 0,
            }),
        }
    }

    /// 获取缓存结果
    pub fn get(&self, tool_name: &str, args: &Value) -> Option<Value> {
        let key = make_key(tool_name, args);
        let mut inner = self.inner.lock();
        inner.total_lookups += 1;

        if inner.uncached_tools.contains(tool_name) {
            return None;
        }

        let entry = inner.entries.get_mut(&key)?;
        if entry.is_expired() {
            inner.entries.remove(&key);
            inner.access_order.retain(|k| k != &key);
            return None;
        }

        entry.hits += 1;
        entry.last_accessed = Instant::now();
        let value = entry.value.clone();

        inner.total_hits += 1;

        // 更新访问顺序：移到末尾
        inner.access_order.retain(|k| k != &key);
        inner.access_order.push_back(key);

        Some(value)
    }

    /// 设置缓存结果
    pub fn set(&self, tool_name: &str, args: &Value, result: Value) {
        self.set_with_ttl(tool_name, args, result, None);
    }

    /// 设置缓存结果（可指定 TTL）
    pub fn set_with_ttl(
        &self,
        tool_name: &str,
        args: &Value,
        result: Value,
        ttl: Option<Duration>,
    ) {
        let key = make_key(tool_name, args);
        let mut inner = self.inner.lock();

        if inner.uncached_tools.contains(tool_name) {
            return;
        }

        let effective_ttl = ttl
            .unwrap_or_else(|| {
                let secs = inner
                    .custom_ttls
                    .get(tool_name)
                    .copied()
                    .unwrap_or(inner.default_ttl.as_secs());
                Duration::from_secs(secs)
            });

        // 容量淘汰
        if inner.entries.len() >= inner.max_size && !inner.entries.contains_key(&key) {
            inner.evict_lru();
        }

        let now = Instant::now();
        inner.entries.insert(
            key.clone(),
            CacheEntry {
                value: result,
                created_at: now,
                ttl: effective_ttl,
                hits: 0,
                last_accessed: now,
            },
        );
        inner.access_order.retain(|k| k != &key);
        inner.access_order.push_back(key);
    }

    /// 使指定工具的所有缓存失效
    pub fn invalidate(&self, tool_name: &str) -> usize {
        let prefix = format!("{}:", tool_name);
        let mut inner = self.inner.lock();
        let keys_to_remove: Vec<String> = inner
            .entries
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        let count = keys_to_remove.len();
        for key in keys_to_remove {
            inner.entries.remove(&key);
            inner.access_order.retain(|k| k != &key);
        }
        count
    }

    /// 清空所有缓存
    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.entries.clear();
        inner.access_order.clear();
    }

    /// 设置不参与缓存的工具
    pub fn set_uncached(&self, tool_names: &[&str]) {
        let mut inner = self.inner.lock();
        for name in tool_names {
            inner.uncached_tools.insert((*name).to_string());
        }
    }

    /// 设置工具自定义 TTL
    pub fn set_custom_ttl(&self, tool_name: &str, ttl_secs: u64) {
        let mut inner = self.inner.lock();
        inner.custom_ttls.insert(tool_name.to_string(), ttl_secs);
    }

    /// 清理过期条目
    pub fn cleanup(&self) -> usize {
        let mut inner = self.inner.lock();
        let expired: Vec<String> = inner
            .entries
            .iter()
            .filter_map(|(k, v)| if v.is_expired() { Some(k.clone()) } else { None })
            .collect();
        let count = expired.len();
        for key in expired {
            inner.entries.remove(&key);
            inner.access_order.retain(|k| k != &key);
        }
        count
    }

    /// 获取缓存统计
    pub fn stats(&self) -> Value {
        let inner = self.inner.lock();
        let total_entries = inner.entries.len();
        let expired_count = inner.entries.values().filter(|e| e.is_expired()).count();
        let total_hits = inner.total_hits;
        let total_lookups = inner.total_lookups;
        let hit_rate = if total_lookups > 0 {
            total_hits as f64 / total_lookups as f64
        } else {
            0.0
        };

        serde_json::json!({
            "total_entries": total_entries,
            "max_size": inner.max_size,
            "total_hits": total_hits,
            "total_lookups": total_lookups,
            "expired_entries": expired_count,
            "hit_rate": hit_rate,
            "uncached_tools": inner.uncached_tools.iter().cloned().collect::<Vec<_>>(),
        })
    }
}

impl CacheInner {
    fn evict_lru(&mut self) {
        // 优先淘汰命中次数为 0 的最旧条目
        let mut victim: Option<String> = None;
        for key in &self.access_order {
            if let Some(entry) = self.entries.get(key) {
                if entry.hits == 0 {
                    victim = Some(key.clone());
                    break;
                }
            }
        }
        let victim = victim.unwrap_or_else(|| self.access_order.front().cloned().unwrap_or_default());
        if !victim.is_empty() {
            self.entries.remove(&victim);
            self.access_order.retain(|k| k != &victim);
            tracing::debug!("LRU 淘汰缓存条目: {}...", &victim[..victim.len().min(8)]);
        }
    }
}

/// 生成缓存键
fn make_key(tool_name: &str, args: &Value) -> String {
    use std::collections::hash_map::DefaultHasher;

    let content = format!("{}:{}", tool_name, args);
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{}:{:016x}", tool_name, hash)
}

/// 创建默认的缓存实例
pub fn default_cache() -> Arc<ToolCache> {
    Arc::new(ToolCache::new(300, 1000))
}
