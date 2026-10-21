//! Bounded TTL/LRU cache with caller scopes and invalidation epochs.
use lru::LruCache;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    tool: String,
    arguments: String,
    scope: Option<String>,
}

impl CacheKey {
    fn new(tool: &str, args: &Value, scope: Option<&Value>) -> Self {
        Self {
            tool: tool.to_string(),
            arguments: canonical_json(args),
            scope: scope.map(canonical_json),
        }
    }
}

/// Stable even when another dependency enables serde_json's preserve_order.
pub(crate) fn canonical_json(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let mut entries: Vec<_> = object.iter().collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key.clone(), sorted(value)))
                        .collect(),
                )
            }
            Value::Array(array) => Value::Array(array.iter().map(sorted).collect()),
            _ => value.clone(),
        }
    }
    sorted(value).to_string()
}

struct CacheEntry {
    value: Value,
    created_at: Instant,
    ttl: Duration,
}
impl CacheEntry {
    fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= self.ttl
    }
}

pub struct ToolCache {
    inner: Mutex<CacheInner>,
}
struct CacheInner {
    entries: LruCache<CacheKey, CacheEntry>,
    default_ttl: Duration,
    max_size: usize,
    uncached_tools: HashSet<String>,
    custom_ttls: HashMap<String, u64>,
    total_hits: u64,
    total_lookups: u64,
    evictions: u64,
    generation: u64,
}
impl CacheInner {
    fn enabled(&self, tool: &str) -> bool {
        self.max_size > 0
            && !self.default_ttl.is_zero()
            && !self.uncached_tools.contains(tool)
            && self.custom_ttls.get(tool) != Some(&0)
    }
    fn remove_tool(&mut self, tool: &str) -> usize {
        let keys: Vec<_> = self
            .entries
            .iter()
            .filter(|(key, _)| key.tool == tool)
            .map(|(key, _)| key.clone())
            .collect();
        let count = keys.len();
        for key in keys {
            self.entries.pop(&key);
        }
        count
    }
}

impl ToolCache {
    /// Zero capacity or default TTL disables reads and writes, including overrides.
    pub fn new(default_ttl_secs: u64, max_size: usize) -> Self {
        Self {
            inner: Mutex::new(CacheInner {
                entries: LruCache::unbounded(),
                default_ttl: Duration::from_secs(default_ttl_secs),
                max_size,
                uncached_tools: HashSet::new(),
                custom_ttls: HashMap::new(),
                total_hits: 0,
                total_lookups: 0,
                evictions: 0,
                generation: 0,
            }),
        }
    }
    pub fn generation(&self) -> u64 {
        self.inner.lock().generation
    }

    /// Apply saved settings immediately; unchanged settings keep warm entries.
    pub fn reconfigure(&self, default_ttl_secs: u64, max_size: usize) {
        let mut inner = self.inner.lock();
        let ttl = Duration::from_secs(default_ttl_secs);
        if inner.default_ttl == ttl && inner.max_size == max_size {
            return;
        }
        inner.default_ttl = ttl;
        inner.max_size = max_size;
        inner.entries.clear();
        inner.generation = inner.generation.wrapping_add(1);
    }
    pub fn get(&self, tool: &str, args: &Value) -> Option<Value> {
        self.lookup(tool, args, None)
    }
    pub fn get_scoped(&self, tool: &str, args: &Value, scope: &Value) -> Option<Value> {
        self.lookup(tool, args, Some(scope))
    }
    fn lookup(&self, tool: &str, args: &Value, scope: Option<&Value>) -> Option<Value> {
        let key = CacheKey::new(tool, args, scope);
        let mut inner = self.inner.lock();
        inner.total_lookups += 1;
        if !inner.enabled(tool) {
            return None;
        }
        let entry = inner.entries.get(&key)?;
        if entry.is_expired() {
            inner.entries.pop(&key);
            return None;
        }
        let value = entry.value.clone();
        inner.total_hits += 1;
        Some(value)
    }
    pub fn set(&self, tool: &str, args: &Value, result: Value) {
        self.set_with_ttl(tool, args, result, None);
    }
    pub fn set_with_ttl(&self, tool: &str, args: &Value, result: Value, ttl: Option<Duration>) {
        self.insert(tool, args, None, result, ttl);
    }
    pub fn set_scoped(&self, tool: &str, args: &Value, scope: &Value, result: Value) {
        self.insert(tool, args, Some(scope), result, None);
    }
    fn insert(
        &self,
        tool: &str,
        args: &Value,
        scope: Option<&Value>,
        result: Value,
        ttl: Option<Duration>,
    ) {
        let key = CacheKey::new(tool, args, scope);
        let mut inner = self.inner.lock();
        if !inner.enabled(tool) {
            return;
        }
        let ttl = ttl.unwrap_or_else(|| {
            inner
                .custom_ttls
                .get(tool)
                .map(|seconds| Duration::from_secs(*seconds))
                .unwrap_or(inner.default_ttl)
        });
        if ttl.is_zero() {
            inner.entries.pop(&key);
            return;
        }
        inner.entries.put(
            key,
            CacheEntry {
                value: result,
                created_at: Instant::now(),
                ttl,
            },
        );
        while inner.entries.len() > inner.max_size {
            inner.entries.pop_lru();
            inner.evictions += 1;
        }
    }
    pub fn invalidate(&self, tool: &str) -> usize {
        let mut inner = self.inner.lock();
        inner.generation = inner.generation.wrapping_add(1);
        inner.remove_tool(tool)
    }
    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.generation = inner.generation.wrapping_add(1);
        inner.entries.clear();
    }
    pub fn set_uncached(&self, tools: &[&str]) {
        let mut inner = self.inner.lock();
        inner.generation = inner.generation.wrapping_add(1);
        for name in tools {
            inner.uncached_tools.insert((*name).into());
            inner.remove_tool(name);
        }
    }
    pub fn set_custom_ttl(&self, tool: &str, ttl_secs: u64) {
        let mut inner = self.inner.lock();
        inner.generation = inner.generation.wrapping_add(1);
        inner.custom_ttls.insert(tool.into(), ttl_secs);
        inner.remove_tool(tool);
    }
    pub fn cleanup(&self) -> usize {
        let mut inner = self.inner.lock();
        let keys: Vec<_> = inner
            .entries
            .iter()
            .filter(|(_, entry)| entry.is_expired())
            .map(|(key, _)| key.clone())
            .collect();
        let count = keys.len();
        for key in keys {
            inner.entries.pop(&key);
        }
        count
    }
    pub fn stats(&self) -> Value {
        let inner = self.inner.lock();
        let mut uncached: Vec<_> = inner.uncached_tools.iter().cloned().collect();
        uncached.sort();
        serde_json::json!({
            "total_entries": inner.entries.len(), "max_size": inner.max_size,
            "total_hits": inner.total_hits, "total_lookups": inner.total_lookups,
            "expired_entries": inner.entries.iter().filter(|(_, entry)| entry.is_expired()).count(),
            "hit_rate": if inner.total_lookups > 0 { inner.total_hits as f64 / inner.total_lookups as f64 } else { 0.0 },
            "uncached_tools": uncached, "evictions": inner.evictions, "generation": inner.generation,
        })
    }
}
pub fn default_cache() -> Arc<ToolCache> {
    Arc::new(ToolCache::new(300, 1000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn zero_capacity_and_ttl_disable_cache_even_with_overrides() {
        for cache in [ToolCache::new(60, 0), ToolCache::new(0, 10)] {
            cache.set_custom_ttl("probe", 60);
            cache.set_with_ttl("probe", &json!({}), json!(1), Some(Duration::from_secs(60)));
            assert!(cache.get("probe", &json!({})).is_none());
            assert_eq!(cache.stats()["total_entries"], 0);
        }
    }
    #[test]
    fn eviction_uses_recency_and_capacity_is_hard_bounded() {
        let cache = ToolCache::new(60, 2);
        cache.set("a", &Value::Null, json!(1));
        cache.set("b", &Value::Null, json!(2));
        assert_eq!(cache.get("a", &Value::Null), Some(json!(1)));
        cache.set("c", &Value::Null, json!(3));
        assert!(cache.get("b", &Value::Null).is_none());
        assert_eq!(cache.stats()["total_entries"], 2);
        assert_eq!(cache.stats()["evictions"], 1);
    }
    #[test]
    fn scopes_and_argument_types_never_share_results() {
        let cache = ToolCache::new(60, 10);
        let args = json!({"a": 1, "nested": {"z": 2, "b": 3}});
        cache.set_scoped("read", &args, &json!({"session": "a"}), json!(42));
        assert_eq!(
            cache.get_scoped(
                "read",
                &json!({"nested":{"b":3,"z":2},"a":1}),
                &json!({"session":"a"})
            ),
            Some(json!(42))
        );
        assert!(cache
            .get_scoped("read", &args, &json!({"session":"b"}))
            .is_none());
        assert!(cache.get("read", &args).is_none());
        assert!(cache
            .get_scoped("read", &json!({"a":"1"}), &json!({"session":"a"}))
            .is_none());
    }
    #[test]
    fn disabling_or_changing_ttl_removes_old_entries() {
        let cache = ToolCache::new(60, 10);
        cache.set("a", &Value::Null, json!(1));
        cache.set_custom_ttl("a", 0);
        assert!(cache.get("a", &Value::Null).is_none());
        cache.set("b", &Value::Null, json!(2));
        cache.set_uncached(&["b"]);
        assert_eq!(cache.stats()["total_entries"], 0);
        let generation = cache.generation();
        cache.clear();
        assert!(cache.generation() > generation);
    }
    #[test]
    fn ttl_expiry_and_tool_invalidation_cover_every_scope() {
        let cache = ToolCache::new(60, 10);
        cache.set_with_ttl(
            "short",
            &Value::Null,
            json!(1),
            Some(Duration::from_nanos(1)),
        );
        assert!(cache.get("short", &Value::Null).is_none());
        for scope in [json!("a"), json!("b")] {
            cache.set_scoped("read", &Value::Null, &scope, json!(2));
        }
        assert_eq!(cache.invalidate("read"), 2);
        assert_eq!(cache.stats()["total_entries"], 0);
    }

    #[test]
    fn hot_configuration_disables_and_resizes_without_restarting() {
        let cache = ToolCache::new(60, 10);
        cache.set("a", &Value::Null, json!(1));
        let generation = cache.generation();
        cache.reconfigure(60, 10);
        assert_eq!(cache.generation(), generation);
        assert_eq!(cache.get("a", &Value::Null), Some(json!(1)));
        cache.reconfigure(0, 10);
        cache.set_with_ttl("a", &Value::Null, json!(1), Some(Duration::from_secs(60)));
        assert_eq!(cache.stats()["total_entries"], 0);
        cache.reconfigure(60, 1);
        cache.set("a", &Value::Null, json!(1));
        cache.set("b", &Value::Null, json!(2));
        assert_eq!(cache.stats()["total_entries"], 1);
        assert!(cache.get("a", &Value::Null).is_none());
    }
}
