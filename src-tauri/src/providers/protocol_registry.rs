//! 协议注册表（ProtocolRegistry）—— 「provider 后端来自插件」的数据中枢
//!
//! 从插件目录装载 `protocols/*.json` 的声明式协议规格（ProtocolSpec），校验 + 预编译
//! 路径表达式为 CompiledSpec，按 `provider_type` 建立索引。`factory` 创建 provider 时
//! 先查本注册表：命中则返回 `DeclarativeProvider`（通用解释器），否则回退原生 adapter。
//!
//! 装载语义（对齐 `load_provider_presets` 的按需读盘）：
//! - 每次查询重新读盘——编辑插件 JSON 后重开或重连即生效，无需重启。
//! - 插件目录按名字典序遍历，同 `provider_type` **后到覆盖先到**（用户插件可覆盖
//!   内置 llm-providers 的协议）。
//! - 单个协议损坏只跳过该条（不影响其余协议与插件加载）。
//!
//! 内置 `chat_completions` 参考协议编译期嵌入，作为注册表的最底兜底（即使无插件
//! 也保证注册表非空，`factory` 可始终走声明式路径验证）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::providers::spec::{CompiledSpec, ProtocolSpec};

/// 内置 chat_completions 参考协议（编译期嵌入，保证注册表兜底非空）。
pub const BUILTIN_CHAT_COMPLETIONS_SPEC: &str = include_str!("../plugins/protocols/chat_completions.json");

/// 递归收集 base 目录下全部 `.json`。
fn glob_json(base: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            glob_json(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            out.push(path);
        }
    }
}

/// 从单个插件目录读取清单声明的协议 glob（缺省 `protocols/*.json`）。
///
/// glob 仅支持「目录前缀 + 通配符」形态（如 `protocols/*.json` / `specs/**/*.json`）；
/// 取第一个通配符之前的目录部分作为扫描根，无通配符时整串按目录（或单个文件）处理。
fn plugin_protocol_paths(pdir: &Path) -> Vec<PathBuf> {
    let glob = std::fs::read_to_string(pdir.join("plugin.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|m| m["protocols"].as_str().map(str::to_string));
    let sub = glob
        .as_deref()
        .filter(|g| crate::plugins::is_safe_manifest_path(g))
        .map(|g| g.trim().trim_start_matches("./"))
        .map(|g| match g.find('*') {
            Some(i) => g[..i]
                .trim_end_matches('/')
                .trim_end_matches("/**")
                .trim_end_matches('/')
                .to_string(),
            None => g.trim_end_matches("/*.json").trim_end_matches('/').to_string(),
        })
        .unwrap_or_else(|| "protocols".to_string());
    let base = if sub.is_empty() {
        pdir.to_path_buf()
    } else {
        pdir.join(sub)
    };
    // 无通配符且指向单个文件：仅允许插件目录内的真实文件。
    if base.is_file() {
        return if crate::plugins::is_safe_plugin_path(pdir, &base) {
            vec![base]
        } else {
            Vec::new()
        };
    }
    if !crate::plugins::is_safe_plugin_path(pdir, &base) {
        return Vec::new();
    }
    let mut out = Vec::new();
    glob_json(&base, &mut out);
    out
}

/// 从文件读取并编译一个 ProtocolSpec；失败返回 (路径, 原因)。
fn compile_file(path: &Path) -> Result<CompiledSpec, (String, String)> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        (path.display().to_string(), format!("读取失败 {e}"))
    })?;
    let spec: ProtocolSpec = serde_json::from_str(&text).map_err(|e| {
        (path.display().to_string(), format!("解析失败 {e}"))
    })?;
    CompiledSpec::compile(spec).map_err(|e| (path.display().to_string(), e))
}

/// 全量装载协议注册表（按 provider_type 索引；后到覆盖先到）。
///
/// 顺序：先内置参考协议兜底，再叠加插件目录（靠后的插件覆盖靠前的）。
pub fn load_all() -> HashMap<String, Arc<CompiledSpec>> {
    let mut map: HashMap<String, Arc<CompiledSpec>> = HashMap::new();

    // 1. 内置 chat_completions 兜底
    if let Ok(spec) = serde_json::from_str::<ProtocolSpec>(BUILTIN_CHAT_COMPLETIONS_SPEC) {
        match CompiledSpec::compile(spec) {
            Ok(c) => {
                map.insert(c.spec.provider_type.clone(), Arc::new(c));
            }
            Err(e) => tracing::error!("[Protocols] 内置 chat_completions 协议编译失败: {e}"),
        }
    }

    // 2. 插件目录（信任门禁：未信任插件的协议不注册）
    let root = crate::plugins::plugins_dir();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return map;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("plugin.json").exists())
        .collect();
    dirs.sort();
    for pdir in dirs {
        if !crate::plugins::is_plugin_dir_trusted(&pdir) {
            continue;
        }
        for path in plugin_protocol_paths(&pdir) {
            match compile_file(&path) {
                Ok(compiled) => {
                    tracing::info!(
                        "[Protocols] 注册声明式协议 {} → provider_type={}",
                        compiled.spec.id,
                        compiled.spec.provider_type
                    );
                    map.insert(compiled.spec.provider_type.clone(), Arc::new(compiled));
                }
                Err((p, reason)) => {
                    tracing::warn!("[Protocols] 跳过协议 {p}: {reason}");
                }
            }
        }
    }
    map
}

// ============================================================================
// 运行时注册表：惰性装载的文件协议缓存 + JS 插件动态覆盖层
// ============================================================================

/// 文件协议注册表缓存（None = 待装载）。invalidate 后下次查询重新读盘，
/// 支持运行时装载插件带来的 `protocols/*.json` 热更新。
static REGISTRY: RwLock<Option<Arc<HashMap<String, Arc<CompiledSpec>>>>> = RwLock::new(None);

/// JS 插件贡献的动态协议（provider_type → spec），叠加在文件协议之上。
///
/// 使用 OnceLock 避免在 Windows/rustc 组合下要求 HashMap 在静态初始化阶段
/// 调用非 const 构造函数。
static DYNAMIC: std::sync::OnceLock<RwLock<HashMap<String, Arc<CompiledSpec>>>> =
    std::sync::OnceLock::new();

fn dynamic_specs() -> &'static RwLock<HashMap<String, Arc<CompiledSpec>>> {
    DYNAMIC.get_or_init(|| RwLock::new(HashMap::new()))
}

/// 文件协议注册表（惰性装载；每次失效后首次查询重新读盘）。
fn file_specs() -> Arc<HashMap<String, Arc<CompiledSpec>>> {
    if let Some(m) = REGISTRY.read().clone() {
        return m;
    }
    let m = Arc::new(load_all());
    *REGISTRY.write() = Some(m.clone());
    m
}

/// 失效文件协议缓存（运行时插件装载/卸载后由 js_host 调用）。
pub fn invalidate() {
    *REGISTRY.write() = None;
}

/// 查询协议 spec：JS 动态贡献优先，其次插件文件协议与内置兜底。
///
/// 原生 provider_type 的拦截在 factory 层（NATIVE_PROVIDER_TYPES），
/// 本函数只管声明式路由面。
pub fn spec_for(provider_type: &str) -> Option<Arc<CompiledSpec>> {
    if let Some(s) = dynamic_specs().read().get(provider_type) {
        return Some(s.clone());
    }
    file_specs().get(provider_type).cloned()
}

/// 整体替换 JS 插件贡献的动态协议集；返回注册成功的 provider_type 列表。
///
/// 多个插件贡献同一 provider_type 时按插入顺序后到覆盖先到
/// （调用方按插件名字典序喂入，与文件协议语义一致）。
pub fn replace_dynamic_specs(specs: Vec<CompiledSpec>) -> Vec<String> {
    let mut m = HashMap::new();
    let mut types = Vec::new();
    for c in specs {
        types.push(c.spec.provider_type.clone());
        m.insert(c.spec.provider_type.clone(), Arc::new(c));
    }
    *dynamic_specs().write() = m;
    types
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_chat_completions_compiles() {
        let spec: ProtocolSpec =
            serde_json::from_str(BUILTIN_CHAT_COMPLETIONS_SPEC).expect("内置协议应可解析");
        assert_eq!(spec.provider_type, "chat_completions");
        assert!(CompiledSpec::compile(spec).is_ok());
    }

    #[test]
    fn registry_has_builtin_fallback() {
        let map = load_all();
        assert!(map.contains_key("chat_completions"));
    }

    #[test]
    fn dynamic_overlay_registers_and_clears() {
        let spec: ProtocolSpec = serde_json::from_value(serde_json::json!({
            "id": "dyn-t",
            "provider_type": "dyn_test_overlay_type",
            "request": { "message_format": "chat_completions" }
        }))
        .unwrap();
        let types = replace_dynamic_specs(vec![CompiledSpec::compile(spec).unwrap()]);
        assert!(types.contains(&"dyn_test_overlay_type".to_string()));
        assert!(spec_for("dyn_test_overlay_type").is_some());
        assert_eq!(
            spec_for("dyn_test_overlay_type").unwrap().spec.id,
            "dyn-t"
        );
        // 内置兜底不受动态层影响
        assert!(spec_for("chat_completions").is_some());

        // 清空动态层（测试隔离，恢复全局状态）
        replace_dynamic_specs(Vec::new());
        assert!(spec_for("dyn_test_overlay_type").is_none());
    }
}