//! Web 搜索能力缝隙 —— 「工具 → 缝隙 → Provider」三层解耦。
//!
//! 参考 deepseek-harness 的 capability seam 设计，搜索是**可替换的供应商能力**：
//!
//! - **工具层**（`tools::builtin::web_search_tool`）只拥有模型可见的 schema、
//!   参数校验与结果格式化，绝不碰网络与 provider 选择
//! - **本模块（缝隙）**拥有 provider 注册表、执行时选择、并发扇出、结果合并、
//!   `max_results` 强制截断与结构化错误，是搜索能力的唯一所有者
//! - **Provider 层**（`providers/`）真正联网，从配置快照构建，可替换、可注册扩展。
//!   内置五个引擎：duckduckgo / searxng / tavily / bing / deepseek
//!   （deepseek = DeepSeek 官方原生搜索，一次搜索 = 一次 Anthropic 兼容
//!   Messages 模型调用 + `web_search_20250305` server tool）
//!
//! 选择语义（执行时解析，永不依赖注册顺序）：
//! - 配置 `web_search.providers` 列表（有序、去重）= 用户启用的引擎池，也是
//!   LLM 请求级选择的**授权范围**：单 provider 直通，多 provider 并发扇出后
//!   按链顺序合并去重（顺序 = 用户配置的优先级）
//! - 请求可带 `engines`（LLM 通过工具参数自主指定一个或多个引擎）→ 与已启用
//!   池取交集执行；交集空则回退全部已启用引擎
//! - 未配置 / 全部不可用 / 全部未注册 → 默认链 `["duckduckgo"]`
//!   （零配置保证：搜索始终可用）
//! - 空结果或全部失败时：配置了代理先直连重试一次（代理挂了不瘫痪搜索）；
//!   链不含 duckduckgo 时再用它兜底
//! - 有 provider 成功但无匹配 → `Ok(空结果)`（搜索成功，确实没有）
//! - 全部 provider 失败 → 聚合 `WebError`（诚实失败，工具层向模型报告原因）
//!
//! 与 LLM function calling 路径的关系：本缝隙只提供搜索后端能力，
//! 搜索决策（要不要搜、用哪个引擎）由 LLM 通过 `web_search` 工具自主判断。

pub mod providers;
pub mod types;

pub use types::{
    WebError, WebErrorCode, WebSearchRequest, WebSearchResult, WebSearchSource,
};

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use tauri::Manager;

use crate::config::WebSearchConfig;

// ============================================================================
// AppHandle 注入（读取 AppState 中的配置与代理）
// ============================================================================

/// 全局 AppHandle（由 lib.rs setup 注入）
static APP_HANDLE: Lazy<RwLock<Option<tauri::AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用一次）
pub fn set_app_handle(handle: tauri::AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 当前 AppHandle（工具层访问 AppState 用）
pub fn current_app_handle() -> Option<tauri::AppHandle> {
    APP_HANDLE.read().clone()
}

/// 读取当前 WebSearchConfig 与代理 URL。
///
/// AppHandle 未注入时返回 `(None, None)` → 缝隙走默认 DuckDuckGo 链
/// （保持零配置可用）。供工具层与流水线主动搜索共用。
pub fn read_search_config() -> (Option<WebSearchConfig>, Option<String>) {
    let handle_opt = APP_HANDLE.read().clone();
    handle_opt
        .map(|handle| {
            let cfg = handle
                .state::<Arc<crate::state::AppState>>()
                .config
                .read()
                .get_all();
            let web_search_config = cfg.web_search.clone();
            let proxy_config = crate::network::proxy::ProxyConfig::from_app_config(&cfg);
            let proxy_url = proxy_config.effective_proxy_url();
            (Some(web_search_config), proxy_url)
        })
        .unwrap_or((None, None))
}

// ============================================================================
// WebSearchProvider — provider 契约（缝隙拥有）
// ============================================================================

/// 一个可注册的搜索供应商。
///
/// 实现方从配置快照构建（见 `ProviderFactory`）。`available()` 必须是
/// 廉价本地检查（如 api_key / base_url 非空），**不得发网络请求**。
#[async_trait]
pub trait WebSearchProvider: Send + Sync {
    /// 稳定注册 id（如 "duckduckgo"）
    fn id(&self) -> &'static str;
    /// 廉价本地可用性检查；未配置必要参数返回 false
    fn available(&self) -> bool;
    /// 执行一次搜索；失败返回结构化 `WebError`
    async fn search(&self, request: &WebSearchRequest) -> Result<WebSearchResult, WebError>;
}

/// 从配置快照构建 provider 实例的工厂。
///
/// 缝隙在**每次搜索执行时**调用工厂 —— 等价于 deepseek-harness 的
/// resolveOptions thunk：配置变更无需重注册 provider，一次搜索也不会
/// 混用两个配置版本。
pub type ProviderFactory = fn(Option<&WebSearchConfig>, Option<&str>) -> Arc<dyn WebSearchProvider>;

// ============================================================================
// ProviderRegistry — 注册表
// ============================================================================

/// provider 工厂注册表（id → 工厂）。重复 id 拒绝注册。
#[derive(Default)]
pub struct ProviderRegistry {
    factories: Vec<(&'static str, ProviderFactory)>,
}

impl ProviderRegistry {
    /// 内置注册表：duckduckgo / searxng / tavily / bing / deepseek
    pub fn builtin() -> Self {
        let mut reg = Self::default();
        // 内置 id 唯一，注册失败直接 panic（程序错误而非运行时状态）
        reg.register("duckduckgo", providers::duckduckgo_factory)
            .expect("builtin provider ids are unique");
        reg.register("searxng", providers::searxng_factory)
            .expect("builtin provider ids are unique");
        reg.register("tavily", providers::tavily_factory)
            .expect("builtin provider ids are unique");
        reg.register("bing", providers::bing_factory)
            .expect("builtin provider ids are unique");
        reg.register("deepseek", providers::deepseek_factory)
            .expect("builtin provider ids are unique");
        reg
    }

    /// 注册一个工厂；id 重复返回 `WEB_DUPLICATE_PROVIDER`。
    pub fn register(&mut self, id: &'static str, factory: ProviderFactory) -> Result<(), WebError> {
        if self.factories.iter().any(|(fid, _)| *fid == id) {
            return Err(WebError::new(
                WebErrorCode::DuplicateProvider,
                format!("web provider「{id}」重复注册"),
            ));
        }
        self.factories.push((id, factory));
        Ok(())
    }

    /// 按 id 构建实例（不检查可用性）；未注册返回 `None`
    fn build(
        &self,
        id: &str,
        config: Option<&WebSearchConfig>,
        proxy_url: Option<&str>,
    ) -> Option<Arc<dyn WebSearchProvider>> {
        self.factories
            .iter()
            .find(|(fid, _)| *fid == id)
            .map(|(_, f)| f(config, proxy_url))
    }

    /// 已注册 id 列表（诊断日志用）
    pub fn ids(&self) -> Vec<&'static str> {
        self.factories.iter().map(|(id, _)| *id).collect()
    }
}

// ============================================================================
// WebSearchService — 服务缝隙（注册表 + 选择 + 扇出合并 + 兜底）
// ============================================================================

/// 一次供应商链执行的产出
struct ChainOutcome {
    /// 合并后有内容的结果；无内容为 `None`
    merged: Option<WebSearchResult>,
    /// 是否至少一个 provider 成功返回（即使 0 条）——区分「无匹配」与「不可用」
    any_ok: bool,
    /// 失败的 provider 错误列表
    errors: Vec<(&'static str, WebError)>,
}

/// Web 搜索服务：所有消费方（工具 / 流水线 / 后台任务）的统一入口。
pub struct WebSearchService {
    registry: RwLock<ProviderRegistry>,
}

static WEB_SEARCH_SERVICE: Lazy<WebSearchService> = Lazy::new(|| WebSearchService {
    registry: RwLock::new(ProviderRegistry::builtin()),
});

impl WebSearchService {
    /// 全局共享实例
    pub fn shared() -> &'static Self {
        &WEB_SEARCH_SERVICE
    }

    /// 运行时注册额外 provider（供插件 / 未来扩展使用）
    pub fn register_provider(
        &self,
        id: &'static str,
        factory: ProviderFactory,
    ) -> Result<(), WebError> {
        self.registry.write().register(id, factory)
    }

    /// 已注册的 provider id 列表
    pub fn registered_ids(&self) -> Vec<&'static str> {
        self.registry.read().ids()
    }

    /// 执行一次搜索。
    ///
    /// 执行流程：
    /// 1. 解析供应商链：请求级 `engines`（LLM 自主指定，与已启用 providers
    ///    取交集）优先，否则用配置列表（去重 / 过滤未知 id；空 → 默认 duckduckgo）
    /// 2. 按链执行：单 provider 直通，多 provider 并发扇出合并
    /// 3. 无内容时：配置了代理 → 直连重试一次；链不含 duckduckgo → 兜底
    /// 4. 最终有成功 provider → `Ok`（空结果也算成功）；全部失败 → 聚合 `Err`
    pub async fn search(
        &self,
        request: &WebSearchRequest,
        config: Option<&WebSearchConfig>,
        proxy_url: Option<&str>,
    ) -> Result<WebSearchResult, WebError> {
        let chain = self.resolve_chain(request, config);
        let mut outcome = self.run_chain(&chain, request, config, proxy_url).await;
        if outcome.merged.is_some() {
            return Ok(outcome.merged.expect("checked above"));
        }

        // 空结果或全失败：代理场景直连重试一次（代理不可用不瘫痪搜索）
        if proxy_url.is_some() {
            tracing::warn!("[WebSearch] 配置了代理但无结果，尝试直连重试");
            let retry = self.run_chain(&chain, request, config, None).await;
            if retry.merged.is_some() {
                return Ok(retry.merged.expect("checked above"));
            }
            // 用直连结果继续判定（代理路径的失败可能由代理导致）
            outcome = retry;
        }

        // 链不含 duckduckgo 时兜底（零配置引擎）
        if !chain.iter().any(|id| *id == "duckduckgo") {
            tracing::warn!("[WebSearch] 供应商链无结果，回退 DuckDuckGo 兜底");
            let fallback = self.run_chain(&["duckduckgo"], request, config, proxy_url).await;
            if fallback.merged.is_some() {
                return Ok(fallback.merged.expect("checked above"));
            }
            outcome = fallback;
        }

        finish(outcome)
    }

    /// 解析供应商链。
    ///
    /// 规则：
    /// 1. 基础链 = 配置 providers 列表（去重、过滤未注册 id）；空 → 默认 duckduckgo
    /// 2. 请求带 `engines`（LLM 工具调用时自主指定）→ 与基础链取交集；
    ///    交集非空则使用交集（请求级选择只允许在用户已启用的范围内），
    ///    交集为空（指定了未启用的引擎）→ 忽略请求级指定，回退基础链并记日志
    fn resolve_chain(
        &self,
        request: &WebSearchRequest,
        config: Option<&WebSearchConfig>,
    ) -> Vec<&'static str> {
        let registered = self.registry.read().ids();
        let configured: Vec<String> = config
            .map(|c| c.providers.clone())
            .unwrap_or_default();

        let mut chain: Vec<&'static str> = Vec::new();
        for id in &configured {
            if let Some(rid) = registered.iter().find(|r| **r == id.as_str()) {
                if !chain.contains(rid) {
                    chain.push(rid);
                }
            } else {
                tracing::warn!(
                    "[WebSearch] 配置的 provider「{id}」未注册（可用: {registered:?}），跳过"
                );
            }
        }

        if chain.is_empty() {
            chain = vec!["duckduckgo"];
        }

        // 请求级引擎指定（LLM 自主选择一个或多个）：与已启用引擎取交集
        if let Some(engines) = &request.engines {
            let requested: Vec<&str> = engines
                .iter()
                .map(|e| e.trim())
                .filter(|e| !e.is_empty())
                .collect();
            if !requested.is_empty() {
                let filtered: Vec<&'static str> = requested
                    .iter()
                    .filter_map(|e| chain.iter().find(|c| **c == *e).copied())
                    .collect();
                if filtered.is_empty() {
                    tracing::warn!(
                        "[WebSearch] 请求指定的引擎 {requested:?} 均未启用（已启用: {chain:?}），回退全部已启用引擎"
                    );
                } else {
                    return filtered;
                }
            }
        }

        chain
    }

    /// 按链执行一次：并发扇出 → 收集 Ok/Err → 合并去重截断
    async fn run_chain(
        &self,
        chain: &[&'static str],
        request: &WebSearchRequest,
        config: Option<&WebSearchConfig>,
        proxy_url: Option<&str>,
    ) -> ChainOutcome {
        // 构建链上的可用 provider 实例（不可用的记日志跳过，不让单个配置项瘫痪搜索）
        let instances: Vec<(&'static str, Arc<dyn WebSearchProvider>)> = {
            let registry = self.registry.read();
            chain
                .iter()
                .filter_map(|id| match registry.build(id, config, proxy_url) {
                    Some(p) => {
                        if !p.available() {
                            tracing::warn!(
                                "[WebSearch] provider「{id}」不可用: {}",
                                WebError::configured_unavailable(id)
                            );
                            None
                        } else {
                            Some((*id, p))
                        }
                    }
                    None => {
                        tracing::warn!(
                            "[WebSearch] provider「{id}」未注册: {}",
                            WebError::configured_missing(id)
                        );
                        None
                    }
                })
                .collect()
        };

        if instances.is_empty() {
            return ChainOutcome {
                merged: None,
                any_ok: false,
                errors: vec![(
                    "chain",
                    WebError::new(
                        WebErrorCode::ProviderUnavailable,
                        format!("供应商链 {chain:?} 无可用 provider"),
                    ),
                )],
            };
        }

        tracing::info!(
            "[WebSearch] 搜索: query={:?}, providers={:?}, max_results={:?}, proxy={}",
            request.query,
            chain,
            request.max_results,
            proxy_url.unwrap_or("none")
        );

        // 并发扇出（请求与配置均为借用，BoxFuture 生命周期绑定本次调用）
        let futures: Vec<
            futures::future::BoxFuture<'_, (&'static str, Result<WebSearchResult, WebError>)>,
        > = instances
            .into_iter()
            .map(|(id, p)| {
                Box::pin(async move { (id, p.search(request).await) })
                    as futures::future::BoxFuture<'_, _>
            })
            .collect();
        let raw = futures::future::join_all(futures).await;

        let mut ok: Vec<(&'static str, WebSearchResult)> = Vec::new();
        let mut errors = Vec::new();
        for (id, res) in raw {
            match res {
                Ok(r) => ok.push((id, r)),
                Err(e) => {
                    tracing::warn!("[WebSearch] provider「{id}」失败: {e}");
                    errors.push((id, e));
                }
            }
        }

        let any_ok = !ok.is_empty();
        // 有内容才算有效结果；Ok 但空 sources / 无 content 的结果由 search()
        // 的兜底链路（代理重试 / DDG 兜底）继续处理，最终由 finish() 判定
        let merged = if any_ok {
            let merged = merge_results(&ok, request.max_results);
            merged.has_content().then_some(merged)
        } else {
            None
        };

        ChainOutcome {
            merged,
            any_ok,
            errors,
        }
    }
}

/// 终局判定：有成功 provider → Ok（空结果 = 搜索成功无匹配）；全失败 → 聚合 Err
fn finish(outcome: ChainOutcome) -> Result<WebSearchResult, WebError> {
    if outcome.any_ok {
        Ok(WebSearchResult::empty())
    } else if let Some((_, e)) = outcome.errors.into_iter().next() {
        // 单 provider 失败直接传播；多 provider 全失败返回第一个（已逐个记日志）
        Err(e)
    } else {
        Err(WebError::new(
            WebErrorCode::ProviderUnavailable,
            "无可用搜索 provider",
        ))
    }
}

/// 合并扇出结果：按链顺序拼接（保留用户配置的优先级）+ URL 去重 + 强制截断
///
/// 截断语义（对齐 deepseek-harness 的 capSources）：缝隙无条件信任
/// `max_results` 上界，provider 侧条数参数只是省钱优化。
fn merge_results(
    ok: &[(&'static str, WebSearchResult)],
    max_results: Option<usize>,
) -> WebSearchResult {
    // content：拼接非空 provider 答案（当前内置 provider 均为 None，预留）
    let contents: Vec<String> = ok
        .iter()
        .filter_map(|(_, r)| r.content.clone().filter(|c| !c.is_empty()))
        .collect();
    let content = (!contents.is_empty()).then(|| contents.join("\n\n"));

    // sources：顺序拼接 + URL 去重 + 截断
    let mut seen = std::collections::HashSet::new();
    let mut sources = Vec::new();
    let mut truncated = false;
    'outer: for (_, r) in ok {
        truncated = truncated || r.truncated;
        for s in &r.sources {
            let key = providers::util::normalize_url(&s.url);
            // 空 URL 不参与去重（避免丢掉仅有的结果）
            if !key.is_empty() && !seen.insert(key) {
                continue;
            }
            if let Some(max) = max_results {
                if sources.len() >= max {
                    truncated = true;
                    break 'outer;
                }
            }
            sources.push(s.clone());
        }
    }

    WebSearchResult {
        content,
        sources,
        truncated,
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_builtin_and_duplicate() {
        let mut reg = ProviderRegistry::builtin();
        assert_eq!(
            reg.ids(),
            vec!["duckduckgo", "searxng", "tavily", "bing", "deepseek"]
        );

        let err = reg.register("bing", bing_factory_dup as ProviderFactory).unwrap_err();
        assert_eq!(err.code, WebErrorCode::DuplicateProvider);
    }

    fn bing_factory_dup(
        _c: Option<&WebSearchConfig>,
        _p: Option<&str>,
    ) -> Arc<dyn WebSearchProvider> {
        unreachable!("never called")
    }

    #[test]
    fn test_resolve_chain_default() {
        let svc = WebSearchService::shared();
        // 未配置 → 默认 duckduckgo
        assert_eq!(
            svc.resolve_chain(&WebSearchRequest::new("q"), None),
            vec!["duckduckgo"]
        );

        // 空列表 → 默认
        let mut cfg = WebSearchConfig::default();
        assert_eq!(
            svc.resolve_chain(&WebSearchRequest::new("q"), Some(&cfg)),
            vec!["duckduckgo"]
        );

        // 配置顺序保留 + 未知 id 过滤
        cfg.providers = vec!["tavily".into(), "unknown".into(), "bing".into()];
        assert_eq!(
            svc.resolve_chain(&WebSearchRequest::new("q"), Some(&cfg)),
            vec!["tavily", "bing"]
        );

        // 去重
        cfg.providers = vec!["bing".into(), "bing".into()];
        assert_eq!(
            svc.resolve_chain(&WebSearchRequest::new("q"), Some(&cfg)),
            vec!["bing"]
        );
    }

    #[test]
    fn test_resolve_chain_request_engines() {
        let svc = WebSearchService::shared();
        let mut cfg = WebSearchConfig::default();
        cfg.providers = vec!["bing".into(), "deepseek".into(), "tavily".into()];
        let base = WebSearchRequest::new("q");

        // 请求不带 engines → 全部已启用引擎
        assert_eq!(
            svc.resolve_chain(&base, Some(&cfg)),
            vec!["bing", "deepseek", "tavily"]
        );

        // 请求指定单个引擎（在已启用池内）→ 只用该引擎
        let req = WebSearchRequest::new("q").with_engines(vec!["deepseek".into()]);
        assert_eq!(svc.resolve_chain(&req, Some(&cfg)), vec!["deepseek"]);

        // 请求指定多个引擎 → 按请求顺序保留交集
        let req = WebSearchRequest::new("q").with_engines(vec!["tavily".into(), "deepseek".into()]);
        assert_eq!(
            svc.resolve_chain(&req, Some(&cfg)),
            vec!["tavily", "deepseek"]
        );

        // 指定未启用的引擎 → 交集空 → 回退全部已启用
        let req = WebSearchRequest::new("q").with_engines(vec!["searxng".into()]);
        assert_eq!(
            svc.resolve_chain(&req, Some(&cfg)),
            vec!["bing", "deepseek", "tavily"]
        );

        // 混合：部分启用部分未启用 → 只保留启用的部分
        let req = WebSearchRequest::new("q").with_engines(vec!["searxng".into(), "bing".into()]);
        assert_eq!(svc.resolve_chain(&req, Some(&cfg)), vec!["bing"]);

        // 空字符串 / 空列表 → 忽略请求级指定
        let req = WebSearchRequest::new("q").with_engines(vec![" ".into()]);
        assert_eq!(
            svc.resolve_chain(&req, Some(&cfg)),
            vec!["bing", "deepseek", "tavily"]
        );

        // 默认池（用户未启用任何）下指定 duckduckgo → 可用
        let req = WebSearchRequest::new("q").with_engines(vec!["duckduckgo".into()]);
        assert_eq!(svc.resolve_chain(&req, None), vec!["duckduckgo"]);
    }

    #[test]
    fn test_merge_results_order_dedup_and_cap() {
        let mk = |urls: Vec<&str>| WebSearchResult {
            content: None,
            sources: urls
                .into_iter()
                .map(|u| WebSearchSource::new(u))
                .collect(),
            truncated: false,
        };

        // 单 provider 直通语义
        let merged = merge_results(&[("a", mk(vec!["https://x.com/1", "https://x.com/2"]))], None);
        assert_eq!(merged.sources.len(), 2);
        assert!(!merged.truncated);

        // 多 provider 按链顺序拼接 + 去重（http/https 等价）
        let merged = merge_results(
            &[
                ("a", mk(vec!["https://x.com/1"])),
                ("b", mk(vec!["http://x.com/1", "https://y.com/2"])),
            ],
            None,
        );
        assert_eq!(merged.sources.len(), 2);
        assert_eq!(merged.sources[0].url, "https://x.com/1");
        assert_eq!(merged.sources[1].url, "https://y.com/2");

        // max_results 截断 + truncated 标记
        let merged = merge_results(&[("a", mk(vec!["https://x.com/1", "https://x.com/2"]))], Some(1));
        assert_eq!(merged.sources.len(), 1);
        assert!(merged.truncated);

        // provider 自报 truncated 传播
        let mut r = mk(vec!["https://x.com/1"]);
        r.truncated = true;
        let merged = merge_results(&[("a", r)], Some(5));
        assert!(merged.truncated);
    }

    #[test]
    fn test_unavailable_config_falls_back_to_default_chain() {
        // 配置了不可用的 provider（无 api_key）仍解析为该 id；
        // 不可用性在 run_chain 构建实例时跳过，链上无实例后由 search()
        // 的 DuckDuckGo 兜底步骤接管（与旧行为一致）
        let svc = WebSearchService::shared();
        let mut cfg = WebSearchConfig::default();
        cfg.providers = vec!["tavily".into()];
        assert_eq!(
            svc.resolve_chain(&WebSearchRequest::new("q"), Some(&cfg)),
            vec!["tavily"]
        );

        // 工厂构建 + 可用性检查（不发网络）
        let registry = ProviderRegistry::builtin();
        let p = registry.build("tavily", Some(&cfg), None).expect("registered");
        assert!(!p.available());

        // deepseek：无 key（测试环境无 AppHandle）→ 不可用
        let mut cfg2 = WebSearchConfig::default();
        cfg2.providers = vec!["deepseek".into()];
        let p = registry.build("deepseek", Some(&cfg2), None).expect("registered");
        assert!(!p.available());
    }
}
