//! 智能应用分类器。
//!
//! - 根据 window title / process name 分类应用（工作/娱乐/社交等）
//! - 用于 computer_control 的上下文感知
//! - LLM 驱动的语义理解（非关键词匹配）
//! - TTL 缓存
//!
//! 默认走基于规则的快速分类，可由调用方注入 LLM 回调以启用语义分类。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// 应用类别常量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppCategory {
    Game,
    Coding,
    Browser,
    Video,
    Chat,
    Office,
    Media,
    Utility,
    Other,
}

impl AppCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Game => "game",
            Self::Coding => "coding",
            Self::Browser => "browser",
            Self::Video => "video",
            Self::Chat => "chat",
            Self::Office => "office",
            Self::Media => "media",
            Self::Utility => "utility",
            Self::Other => "other",
        }
    }

    pub fn all() -> Vec<AppCategory> {
        vec![
            Self::Game,
            Self::Coding,
            Self::Browser,
            Self::Video,
            Self::Chat,
            Self::Office,
            Self::Media,
            Self::Utility,
            Self::Other,
        ]
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "game" => Some(Self::Game),
            "coding" => Some(Self::Coding),
            "browser" => Some(Self::Browser),
            "video" => Some(Self::Video),
            "chat" => Some(Self::Chat),
            "office" => Some(Self::Office),
            "media" => Some(Self::Media),
            "utility" => Some(Self::Utility),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// 分类详情。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationResult {
    pub category: String,
    pub confidence: f64,
    pub source: String,
}

/// 缓存条目。
struct CacheEntry {
    category: AppCategory,
    inserted_at: Instant,
}

/// LLM 推理回调（注入式，可由调用方注入云端 LLM 实现语义分类）。
pub type ClassifyCallback = Arc<dyn Fn(&str) -> Option<AppCategory> + Send + Sync>;

/// 智能应用分类器。
///
/// 默认基于规则快速分类，
/// 若注入了 LLM 回调则优先用回调。
pub struct SmartAppClassifier {
    cache: Mutex<HashMap<String, CacheEntry>>,
    cache_ttl: Duration,
    cache_max_size: usize,
    llm_callback: Option<ClassifyCallback>,
}

impl SmartAppClassifier {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            cache_ttl: Duration::from_secs(86400),
            cache_max_size: 1000,
            llm_callback: None,
        }
    }

    /// 注入 LLM 推理回调（builder 风格）。
    pub fn with_llm_callback(mut self, cb: ClassifyCallback) -> Self {
        self.llm_callback = Some(cb);
        self
    }

    /// 分类应用，返回类别字符串。
    pub fn classify(&self, app_name: &str) -> String {
        self.classify_with_details(app_name).category
    }

    /// 分类应用并返回详细信息。
    pub fn classify_with_details(&self, app_name: &str) -> ClassificationResult {
        // 1. 查缓存
        if let Some(cached) = self.get_from_cache(app_name) {
            return ClassificationResult {
                category: cached.as_str().to_string(),
                confidence: 1.0,
                source: "cache".to_string(),
            };
        }

        // 2. 优先用 LLM 回调
        let category = if let Some(cb) = &self.llm_callback {
            cb(app_name).unwrap_or_else(|| self.rule_based_classify(app_name))
        } else {
            self.rule_based_classify(app_name)
        };

        // 3. 写缓存
        self.set_to_cache(app_name, category);

        ClassificationResult {
            category: category.as_str().to_string(),
            confidence: 0.85,
            source: if self.llm_callback.is_some() {
                "llm"
            } else {
                "rule"
            }
            .to_string(),
        }
    }

    /// 判断应用是否属于指定类别。
    pub fn is_category(&self, app_name: &str, target: AppCategory) -> bool {
        let result = self.classify_with_details(app_name);
        AppCategory::from_str(&result.category) == Some(target)
    }

    /// 内部：从缓存获取。
    fn get_from_cache(&self, app_name: &str) -> Option<AppCategory> {
        let mut cache = self.cache.lock();
        if let Some(entry) = cache.get(app_name) {
            if entry.inserted_at.elapsed() < self.cache_ttl {
                return Some(entry.category);
            }
            // 过期，移除
            cache.remove(app_name);
        }
        // 周期性清理过期条目（简化：每次访问检查少量）
        if cache.len() > 10 {
            self.cleanup_expired_locked(&mut cache);
        }
        None
    }

    /// 内部：写入缓存。
    fn set_to_cache(&self, app_name: &str, category: AppCategory) {
        let mut cache = self.cache.lock();
        cache.insert(
            app_name.to_string(),
            CacheEntry {
                category,
                inserted_at: Instant::now(),
            },
        );
        // 限制大小
        while cache.len() > self.cache_max_size {
            // 移除最早的条目（简化：随机移除一个）
            if let Some(key) = cache.keys().next().cloned() {
                cache.remove(&key);
            } else {
                break;
            }
        }
    }

    fn cleanup_expired_locked(&self, cache: &mut HashMap<String, CacheEntry>) {
        let ttl = self.cache_ttl;
        cache.retain(|_, entry| entry.inserted_at.elapsed() < ttl);
    }

    /// 基于规则的快速分类（默认 fallback）。
    ///
    /// 无模型时的 fallback。
    fn rule_based_classify(&self, app_name: &str) -> AppCategory {
        let lower = app_name.to_lowercase();

        // 浏览器
        for kw in &["chrome", "firefox", "edge", "safari", "browser", "opera", "brave"] {
            if lower.contains(kw) {
                return AppCategory::Browser;
            }
        }

        // 开发工具
        for kw in &[
            "code",
            "vscode",
            "intellij",
            "pycharm",
            "idea",
            "terminal",
            "powershell",
            "cmd",
            "git",
            "vim",
            "emacs",
            "sublime",
            "atom",
            "devenv",
            "rust-rover",
            "goland",
            "webstorm",
        ] {
            if lower.contains(kw) {
                return AppCategory::Coding;
            }
        }

        // 游戏
        for kw in &["steam", "epic", "battle.net", "minecraft", "game", "gog", "origin"] {
            if lower.contains(kw) {
                return AppCategory::Game;
            }
        }

        // 视频
        for kw in &["bilibili", "youtube", "netflix", "video", "vlc", "potplayer", "mpv", "iqiyi"] {
            if lower.contains(kw) {
                return AppCategory::Video;
            }
        }

        // 社交
        for kw in &[
            "wechat",
            "qq",
            "telegram",
            "discord",
            "slack",
            "teams",
            "skype",
            "mail",
            "outlook",
            "foxmail",
        ] {
            if lower.contains(kw) {
                return AppCategory::Chat;
            }
        }

        // 办公
        for kw in &[
            "word", "excel", "powerpoint", "office", "wps", "notion", "onenote", "docs",
        ] {
            if lower.contains(kw) {
                return AppCategory::Office;
            }
        }

        // 媒体
        for kw in &["spotify", "music", "photoshop", "illustrator", "premiere", "ae", "foobar"] {
            if lower.contains(kw) {
                return AppCategory::Media;
            }
        }

        // 系统工具
        for kw in &[
            "explorer",
            "taskmgr",
            "control",
            "settings",
            "registry",
            "cmd",
            "powershell",
            "terminal",
            "system",
            "antivirus",
        ] {
            if lower.contains(kw) {
                return AppCategory::Utility;
            }
        }

        AppCategory::Other
    }
}

impl Default for SmartAppClassifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_based_browser() {
        let classifier = SmartAppClassifier::new();
        assert_eq!(classifier.classify("Google Chrome"), "browser");
        assert_eq!(classifier.classify("Mozilla Firefox"), "browser");
    }

    #[test]
    fn test_rule_based_coding() {
        let classifier = SmartAppClassifier::new();
        assert_eq!(classifier.classify("Visual Studio Code"), "coding");
        assert_eq!(classifier.classify("PowerShell 7"), "coding");
    }

    #[test]
    fn test_rule_based_game() {
        let classifier = SmartAppClassifier::new();
        assert_eq!(classifier.classify("Steam"), "game");
        assert_eq!(classifier.classify("Epic Games Launcher"), "game");
    }

    #[test]
    fn test_rule_based_chat() {
        let classifier = SmartAppClassifier::new();
        assert_eq!(classifier.classify("WeChat"), "chat");
        assert_eq!(classifier.classify("Discord"), "chat");
    }

    #[test]
    fn test_rule_based_other() {
        let classifier = SmartAppClassifier::new();
        assert_eq!(classifier.classify("UnknownApp123"), "other");
    }

    #[test]
    fn test_cache_hit() {
        let classifier = SmartAppClassifier::new();
        let r1 = classifier.classify_with_details("VS Code");
        let r2 = classifier.classify_with_details("VS Code");
        assert_eq!(r1.category, r2.category);
        assert_eq!(r2.source, "cache");
    }

    #[test]
    fn test_is_category() {
        let classifier = SmartAppClassifier::new();
        assert!(classifier.is_category("Chrome", AppCategory::Browser));
        assert!(!classifier.is_category("Chrome", AppCategory::Game));
    }

    #[test]
    fn test_llm_callback_injection() {
        let cb: ClassifyCallback = Arc::new(|_name| Some(AppCategory::Media));
        let classifier = SmartAppClassifier::new().with_llm_callback(cb);
        let result = classifier.classify_with_details("AnyApp");
        assert_eq!(result.category, "media");
        assert_eq!(result.source, "llm");
    }
}
