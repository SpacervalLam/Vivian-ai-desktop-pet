//! Hook 配置加载与解析
//!
//! 配置格式（JSON）：
//! ```json
//! {
//!   "hooks": {
//!     "PreToolUse": [
//!       {
//!         "matcher": "write_file|edit_file",
//!         "command": "python check_safety.py",
//!         "timeout_ms": 5000
//!       }
//!     ],
//!     "PostToolUse": [
//!       {
//!         "matcher": ".*",
//!         "command": "log_tool_usage.sh",
//!         "timeout_ms": 3000
//!       }
//!     ]
//!   }
//! }
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Deserialize;

use super::event::HookEventName;

/// 单个 Hook 配置项
#[derive(Debug, Clone)]
pub struct HookSpec {
    /// Hook 名称（从文件名或序号生成）
    pub name: String,
    /// 事件类型
    pub event: HookEventName,
    /// 工具名匹配正则
    pub matcher: Regex,
    /// 要执行的命令
    pub command: String,
    /// 超时时间（毫秒）
    pub timeout_ms: u64,
    /// 命令执行的工作目录
    pub source_dir: PathBuf,
}

/// JSON 配置文件反序列化结构
#[derive(Debug, Deserialize)]
struct HooksConfigFile {
    hooks: HashMap<String, Vec<HookEntryRaw>>,
}

#[derive(Debug, Deserialize)]
struct HookEntryRaw {
    matcher: String,
    command: String,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    5000
}

/// Hook 注册表：管理所有已配置的 Hook
#[derive(Debug, Clone)]
pub struct HookRegistry {
    /// 按事件类型分组的 Hook 列表
    pub hooks: HashMap<HookEventName, Vec<HookSpec>>,
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self {
            hooks: HashMap::new(),
        }
    }
}

impl HookRegistry {
    /// 从配置目录加载所有 *.json 文件
    ///
    /// 发现路径：
    /// 1. `~/.vivian/hooks/*.json`（全局）
    /// 2. `<项目>/.vivian/hooks/*.json`（项目级，合并）
    pub fn load(hook_dirs: &[PathBuf]) -> Self {
        let mut registry = Self::default();

        for dir in hook_dirs {
            if !dir.is_dir() {
                continue;
            }

            let entries = match std::fs::read_dir(dir) {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!("[HookRegistry] 无法读取 hook 目录 {:?}: {}", dir, err);
                    continue;
                }
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }

                match Self::load_file(&path, dir) {
                    Ok(specs) => {
                        for spec in specs {
                            registry
                                .hooks
                                .entry(spec.event)
                                .or_default()
                                .push(spec);
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            "[HookRegistry] 加载 hook 文件 {:?} 失败: {}",
                            path,
                            err
                        );
                    }
                }
            }
        }

        let total: usize = registry.hooks.values().map(|v| v.len()).sum();
        if total > 0 {
            tracing::info!("[HookRegistry] 已加载 {} 个 Hook", total);
        }

        registry
    }

    /// 加载默认路径的 Hook（全局 + 项目级）
    pub fn load_default() -> Self {
        let mut dirs = Vec::new();

        // 全局: ~/.vivian/hooks/
        if let Some(home) = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .ok()
            .map(PathBuf::from)
        {
            dirs.push(home.join(".vivian").join("hooks"));
        }

        Self::load(&dirs)
    }

    /// 获取指定事件类型 + 工具名匹配的 Hook 列表
    pub fn matching_hooks(&self, event: HookEventName, tool_name: &str) -> Vec<&HookSpec> {
        self.hooks
            .get(&event)
            .map(|hooks| {
                hooks
                    .iter()
                    .filter(|h| h.matcher.is_match(tool_name))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 是否有任何 PreToolUse Hook 配置
    pub fn has_pre_tool_hooks(&self) -> bool {
        self.hooks
            .get(&HookEventName::PreToolUse)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    /// 是否有任何 PostToolUse Hook 配置
    pub fn has_post_tool_hooks(&self) -> bool {
        self.hooks
            .get(&HookEventName::PostToolUse)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    fn load_file(path: &Path, source_dir: &Path) -> Result<Vec<HookSpec>, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("读取文件失败: {}", e))?;

        let config: HooksConfigFile = serde_json::from_str(&content)
            .map_err(|e| format!("JSON 解析失败: {}", e))?;

        let file_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("hook")
            .to_string();

        let mut specs = Vec::new();

        for (event_str, entries) in config.hooks {
            let event = match event_str.as_str() {
                "PreToolUse" => HookEventName::PreToolUse,
                "PostToolUse" => HookEventName::PostToolUse,
                other => {
                    tracing::warn!("[HookRegistry] 未知的 Hook 事件类型: {}", other);
                    continue;
                }
            };

            for (idx, entry) in entries.into_iter().enumerate() {
                let matcher = match Regex::new(&entry.matcher) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(
                            "[HookRegistry] 无效的正则表达式 '{}': {}",
                            entry.matcher,
                            e
                        );
                        continue;
                    }
                };

                specs.push(HookSpec {
                    name: format!("{}:{}", file_stem, idx),
                    event,
                    matcher,
                    command: entry.command,
                    timeout_ms: entry.timeout_ms,
                    source_dir: source_dir.to_path_buf(),
                });
            }
        }

        Ok(specs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry() {
        let registry = HookRegistry::default();
        assert!(!registry.has_pre_tool_hooks());
        assert!(!registry.has_post_tool_hooks());
        assert!(registry
            .matching_hooks(HookEventName::PreToolUse, "write_file")
            .is_empty());
    }

    #[test]
    fn matching_hooks_basic() {
        let mut registry = HookRegistry::default();
        registry.hooks.insert(
            HookEventName::PreToolUse,
            vec![
                HookSpec {
                    name: "test:0".to_string(),
                    event: HookEventName::PreToolUse,
                    matcher: Regex::new("write_file|edit_file").unwrap(),
                    command: "echo test".to_string(),
                    timeout_ms: 5000,
                    source_dir: PathBuf::from("."),
                },
            ],
        );

        let matches = registry.matching_hooks(HookEventName::PreToolUse, "write_file");
        assert_eq!(matches.len(), 1);

        let matches = registry.matching_hooks(HookEventName::PreToolUse, "read_file");
        assert!(matches.is_empty());
    }

    #[test]
    fn wildcard_matcher() {
        let mut registry = HookRegistry::default();
        registry.hooks.insert(
            HookEventName::PostToolUse,
            vec![HookSpec {
                name: "log:0".to_string(),
                event: HookEventName::PostToolUse,
                matcher: Regex::new(".*").unwrap(),
                command: "log.sh".to_string(),
                timeout_ms: 3000,
                source_dir: PathBuf::from("."),
            }],
        );

        assert_eq!(
            registry.matching_hooks(HookEventName::PostToolUse, "any_tool").len(),
            1
        );
    }

    #[test]
    fn parse_json_config() {
        let json = r#"{
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "write_file",
                        "command": "check.sh",
                        "timeout_ms": 3000
                    }
                ]
            }
        }"#;

        let config: HooksConfigFile = serde_json::from_str(json).unwrap();
        assert!(config.hooks.contains_key("PreToolUse"));
        let entries = &config.hooks["PreToolUse"];
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].matcher, "write_file");
        assert_eq!(entries[0].timeout_ms, 3000);
    }
}
