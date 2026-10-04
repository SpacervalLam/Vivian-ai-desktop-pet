//! Shared search service for companion and work agents.
//!
//! Enabled engines form the request authorization pool. Explicit unsupported or
//! disabled engines are rejected before resolution. Fast mode queries the pool
//! sequentially and keeps the first engine that returns content; research mode
//! queries the pool concurrently. Reciprocal-rank fusion, source diversity and URL
//! provenance preserve useful results across engines.
//!
//! Sequential fast mode exists because engine availability cannot be known without
//! issuing a request: a configured key may be revoked, rate-limited or rejected.
//! Trimming the pool up front would leave nobody to take over such a failure.
//!
//! Domain/date filters, short-lived caches and a whole-request deadline apply across
//! retries and fallback. Partial failures remain visible; no match is distinct from
//! transport or parsing failure. Automatic selection can fall back to DuckDuckGo;
//! explicit selection cannot silently expand to other engines.

pub mod providers;
pub mod types;

pub use types::{WebError, WebErrorCode, WebSearchRequest, WebSearchResult, WebSearchSource};

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
/// 缝隙在**每次搜索执行时**调用工厂：配置变更无需重注册 provider，
/// 一次搜索也不会混用两个配置版本。
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
    /// 内置注册表：duckduckgo / searxng / tavily / deepseek
    pub fn builtin() -> Self {
        let mut reg = Self::default();
        // 内置 id 唯一，注册失败直接 panic（程序错误而非运行时状态）
        reg.register("duckduckgo", providers::duckduckgo_factory)
            .expect("builtin provider ids are unique");
        reg.register("searxng", providers::searxng_factory)
            .expect("builtin provider ids are unique");
        reg.register("tavily", providers::tavily_factory)
            .expect("builtin provider ids are unique");
        reg.register("deepseek", providers::deepseek_factory)
            .expect("builtin provider ids are unique");
        for (id, factory) in [
            ("exa", providers::exa_factory as ProviderFactory),
            ("perplexity", providers::perplexity_factory as ProviderFactory),
            ("openai", providers::openai_factory as ProviderFactory),
            ("xai", providers::xai_factory as ProviderFactory),
            ("anthropic", providers::anthropic_factory as ProviderFactory),
        ] {
            reg.register(id, factory).expect("builtin provider ids are unique");
        }
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
    empty_result: Option<WebSearchResult>,
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
type SearchCache = std::collections::HashMap<u64, (std::time::Instant, WebSearchResult)>;
static SEARCH_CACHE: Lazy<parking_lot::Mutex<SearchCache>> =
    Lazy::new(|| parking_lot::Mutex::new(SearchCache::new()));

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
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        let mut cache_request = request.clone();
        cache_request.refresh = false;
        serde_json::to_string(&cache_request)
            .unwrap_or_default()
            .hash(&mut hash);
        serde_json::to_string(&config)
            .unwrap_or_default()
            .hash(&mut hash);
        proxy_url.hash(&mut hash);
        let cache_key = hash.finish();
        let ttl = std::time::Duration::from_secs(if request.recency_days.is_some() {
            30
        } else {
            120
        });
        if !request.refresh {
            if let Some((at, result)) = SEARCH_CACHE.lock().get(&cache_key) {
                if at.elapsed() < ttl {
                    let mut result = result.clone();
                    result.cached = true;
                    return Ok(result);
                }
            }
        }
        if let Some(engines) = &request.engines {
            let enabled = config
                .map(|c| c.providers.clone())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| vec!["duckduckgo".into()]);
            // 已退役或未在配置中启用的引擎一律拒绝，不静默换人：
            // 模型明确点了某家，换成别家会给出它没预期的来源。
            if engines.iter().any(|e| !enabled.contains(e)) {
                return Err(WebError::new(
                    WebErrorCode::ProviderConfiguredUnavailable,
                    "请求引擎未启用或已退役；不会静默扩大到其他引擎",
                ));
            }
        }
        let mut prepared = request.clone();
        prepared.language = request
            .language
            .clone()
            .or_else(|| config.and_then(|c| c.language.clone()));
        if !request.include_domains.is_empty() {
            prepared.query.push_str(&format!(
                " ({})",
                request
                    .include_domains
                    .iter()
                    .map(|d| format!("site:{d}"))
                    .collect::<Vec<_>>()
                    .join(" OR ")
            ));
        }
        for domain in &request.exclude_domains {
            prepared.query.push_str(&format!(" -site:{domain}"));
        }
        match tokio::time::timeout(
            std::time::Duration::from_secs(request.timeout_secs.clamp(5, 50)),
            self.search_inner(&prepared, config, proxy_url),
        )
        .await
        {
            Ok(result) => result.map(|r| {
                let r = filter_sources(r, request);
                if !r.sources.is_empty() {
                    let mut cache = SEARCH_CACHE.lock();
                    cache.retain(|_, (at, _)| at.elapsed() < std::time::Duration::from_secs(120));
                    if cache.len() >= 64 {
                        if let Some(key) = cache
                            .iter()
                            .min_by_key(|(_, (at, _))| *at)
                            .map(|(key, _)| *key)
                        {
                            cache.remove(&key);
                        }
                    }
                    cache.insert(cache_key, (std::time::Instant::now(), r.clone()));
                }
                r
            }),
            Err(_) => Err(WebError::new(
                WebErrorCode::ProviderError,
                "联网搜索达到总时间预算；尚未核实，请调整查询或报告证据缺口",
            )),
        }
    }

    async fn search_inner(
        &self,
        request: &WebSearchRequest,
        config: Option<&WebSearchConfig>,
        proxy_url: Option<&str>,
    ) -> Result<WebSearchResult, WebError> {
        let chain = self.resolve_chain(request, config);
        // fast 模式（未指定 engines、非 research）走顺序降级：只成功调用一家引擎，
        // 但第一家跑不通时下一家接手。research 模式并发扇出，合并多源提升覆盖率。
        let sequential = request.engines.is_none() && !request.research;
        let mut outcome = if sequential {
            self.run_chain_sequential(&chain, request, config, proxy_url)
                .await
        } else {
            self.run_chain(&chain, request, config, proxy_url).await
        };
        if let Some(mut result) = outcome.merged.take() {
            result
                .warnings
                .extend(outcome.errors.iter().map(|(_, e)| e.to_string()));
            return Ok(result);
        }

        // 空结果或全失败：代理场景直连重试一次（代理不可用不瘫痪搜索）
        if proxy_url.is_some() {
            tracing::warn!("[WebSearch] 配置了代理但无结果，尝试直连重试");
            let mut retry = if sequential {
                self.run_chain_sequential(&chain, request, config, None)
                    .await
            } else {
                self.run_chain(&chain, request, config, None).await
            };
            if let Some(mut result) = retry.merged.take() {
                result
                    .warnings
                    .push("代理路径未取得结果，已直连重试".into());
                result
                    .warnings
                    .extend(retry.errors.iter().map(|(_, e)| e.to_string()));
                return Ok(result);
            }
            // 用直连结果继续判定（代理路径的失败可能由代理导致）
            outcome = retry;
        }

        // 链不含 duckduckgo 时兜底（零配置引擎）
        if request.engines.is_none() && !chain.iter().any(|id| *id == "duckduckgo") {
            tracing::warn!("[WebSearch] 供应商链无结果，回退 DuckDuckGo 兜底");
            let mut fallback = self
                .run_chain(&["duckduckgo"], request, config, proxy_url)
                .await;
            if let Some(mut result) = fallback.merged.take() {
                result
                    .warnings
                    .extend(outcome.errors.iter().map(|(_, e)| e.to_string()));
                result
                    .warnings
                    .push("已使用 DuckDuckGo 兜底；配置引擎未取得可用结果".into());
                return Ok(result);
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
        let configured: Vec<String> = config.map(|c| c.providers.clone()).unwrap_or_default();

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

        // fast 模式（未指定 engines 且非 research）：链上只用一个引擎，按成本排序取第一个
        // 「本地检查可用」的。返回完整链——是否只用一个由 run_chain 的顺序降级负责，
        // 不能在这里截断：`available()` 只看凭据非空（不发请求），运行时故障
        // （限流 / 鉴权失效 / 请求被判非法）不会反映到它上面，截断后就无人接手。
        chain
    }

    /// 顺序尝试链上引擎，取第一个取得内容的成功结果。
    ///
    /// 用于 fast 模式：成本上仍只成功调用一家（失败调用不计结果），但把
    /// 「凭据存在却跑不通」的情况交给下一家兜底，而不是整条搜索直接失败。
    /// 按配置顺序尝试——用户把谁排在前面，就优先用谁。
    async fn run_chain_sequential(
        &self,
        chain: &[&'static str],
        request: &WebSearchRequest,
        config: Option<&WebSearchConfig>,
        proxy_url: Option<&str>,
    ) -> ChainOutcome {
        let mut errors = Vec::new();
        let mut any_ok = false;
        let mut empty_result: Option<WebSearchResult> = None;

        for id in chain {
            let outcome = self.run_chain(&[*id], request, config, proxy_url).await;
            let failed_here = !outcome.errors.is_empty();
            errors.extend(outcome.errors);
            if outcome.merged.is_some() {
                if failed_here && chain.len() > 1 {
                    tracing::warn!(
                        "[WebSearch] 前序引擎不可用，已降级到「{id}」并取得结果"
                    );
                }
                return ChainOutcome {
                    merged: outcome.merged,
                    any_ok: true,
                    empty_result: None,
                    errors,
                };
            }
            if outcome.any_ok {
                any_ok = true;
                if empty_result.is_none() {
                    empty_result = outcome.empty_result;
                }
            }
        }

        ChainOutcome {
            merged: None,
            any_ok,
            empty_result,
            errors,
        }
    }

    /// 并发扇出执行：收集 Ok/Err → 合并去重截断
    async fn run_chain(
        &self,
        chain: &[&'static str],
        request: &WebSearchRequest,
        config: Option<&WebSearchConfig>,
        proxy_url: Option<&str>,
    ) -> ChainOutcome {
        let mut errors = Vec::new();
        // 构建链上的可用 provider 实例（不可用的记日志跳过，不让单个配置项瘫痪搜索）
        let instances: Vec<(&'static str, Arc<dyn WebSearchProvider>)> = {
            let registry = self.registry.read();
            chain
                .iter()
                .filter_map(|id| match registry.build(id, config, proxy_url) {
                    Some(p) => {
                        if !p.available() {
                            errors.push((*id, WebError::configured_unavailable(id)));
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
                        errors.push((*id, WebError::configured_missing(id)));
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
                empty_result: None,
                errors,
            };
        }

        tracing::info!(
            "[WebSearch] 搜索: query={:?}, providers={:?}, max_results={:?}, proxy={}",
            request.query,
            chain,
            request.max_results,
            if proxy_url.is_some() {
                "configured"
            } else {
                "direct"
            }
        );

        // 并发扇出（请求与配置均为借用，BoxFuture 生命周期绑定本次调用）
        let futures: Vec<
            futures::future::BoxFuture<'_, (&'static str, Result<WebSearchResult, WebError>)>,
        > = instances
            .into_iter()
            .map(|(id, p)| {
                Box::pin(async move {
                    let budget =
                        std::time::Duration::from_secs((request.timeout_secs * 3 / 4).max(2));
                    let result = match tokio::time::timeout(budget, p.search(request)).await {
                        Ok(result) => result,
                        Err(_) => Err(WebError::provider_error(id, "搜索引擎达到本次时间预算")),
                    };
                    (id, result)
                }) as futures::future::BoxFuture<'_, _>
            })
            .collect();
        let raw = futures::future::join_all(futures).await;

        let mut ok: Vec<(&'static str, WebSearchResult)> = Vec::new();
        for (id, res) in raw {
            match res {
                Ok(r) => ok.push((id, filter_sources(r, request))),
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
            empty_result: any_ok.then(|| merge_results(&ok, request.max_results)),
            errors,
        }
    }
}

/// 终局判定：有成功 provider → Ok（空结果 = 搜索成功无匹配）；全失败 → 聚合 Err
fn finish(outcome: ChainOutcome) -> Result<WebSearchResult, WebError> {
    if outcome.any_ok {
        let mut result = outcome.empty_result.unwrap_or_else(WebSearchResult::empty);
        result
            .warnings
            .extend(outcome.errors.iter().map(|(_, e)| e.to_string()));
        Ok(result)
    } else if !outcome.errors.is_empty() {
        let mut errors = outcome.errors.into_iter();
        let (_, mut first) = errors.next().expect("nonempty errors");
        for (engine, error) in errors {
            first.message.push_str(&format!("; {engine}: {error}"));
        }
        Err(first)
    } else {
        Err(WebError::new(
            WebErrorCode::ProviderUnavailable,
            "无可用搜索 provider",
        ))
    }
}

/// 合并扇出结果：倒数排名融合、URL 去重、来源多样性和强制截断
///
/// 截断语义：缝隙无条件信任
/// `max_results` 上界，provider 侧条数参数只是省钱优化。
fn merge_results(
    ok: &[(&'static str, WebSearchResult)],
    max_results: Option<usize>,
) -> WebSearchResult {
    let mut candidates: std::collections::HashMap<String, (WebSearchSource, f64, usize)> =
        std::collections::HashMap::new();
    let mut order = 0;
    for (engine, result) in ok {
        let mut engine_seen = std::collections::HashSet::new();
        for (rank, source) in result.sources.iter().enumerate() {
            if crate::network::url_fetcher::validate_url(&source.url).is_err() {
                continue;
            }
            let key = providers::util::normalize_url(&source.url);
            if !engine_seen.insert(key.clone()) {
                continue;
            }
            let score = 1.0 / (60.0 + rank as f64);
            let entry = candidates.entry(key).or_insert_with(|| {
                order += 1;
                (providers::util::annotate_source(source.clone()), 0.0, order)
            });
            entry.1 += score;
            if !entry.0.engines.iter().any(|e| e == engine) {
                entry.0.engines.push((*engine).into());
            }
            if source.snippet_text().len() > entry.0.snippet_text().len() {
                entry.0.snippet = source.snippet.clone();
            }
            if entry.0.published_at.is_none() {
                entry.0.published_at = source.published_at.clone();
            }
        }
    }
    let mut ranked: Vec<_> = candidates.into_values().collect();
    fn source_weight(source: &WebSearchSource) -> f64 {
        match source.source_tier.as_str() {
            "P0" => 1.15,
            "P1" => 1.08,
            _ => 1.0,
        }
    }
    ranked.sort_by(|a, b| {
        (b.1 * source_weight(&b.0))
            .total_cmp(&(a.1 * source_weight(&a.0)))
            .then_with(|| a.2.cmp(&b.2))
    });
    let available = ranked.len();
    let max = max_results.unwrap_or(available);
    let mut counts = std::collections::HashMap::<String, usize>::new();
    let mut sources = vec![];
    let mut deferred = vec![];
    for (mut source, _, _) in ranked {
        source.snippet = source.snippet.map(|s| s.chars().take(1500).collect());
        source.raw_content = source.raw_content.map(|s| s.chars().take(2000).collect());
        let host = reqwest::Url::parse(&source.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default();
        let count = counts.entry(host).or_default();
        if *count >= 2 {
            deferred.push(source);
        } else {
            *count += 1;
            sources.push(source);
        }
    }
    sources.extend(deferred);
    sources.truncate(max);
    let contents: Vec<_> = ok
        .iter()
        .filter_map(|(_, r)| r.content.clone())
        .filter(|c| !c.is_empty())
        .collect();
    WebSearchResult {
        content: (!contents.is_empty()).then(|| contents.join("\n\n")),
        sources,
        truncated: available > max || ok.iter().any(|(_, r)| r.truncated),
        warnings: ok.iter().flat_map(|(_, r)| r.warnings.clone()).collect(),
        engines_used: ok.iter().map(|(id, _)| (*id).into()).collect(),
        cached: false,
    }
}

fn filter_sources(mut result: WebSearchResult, request: &WebSearchRequest) -> WebSearchResult {
    let cutoff = request
        .recency_days
        .map(|n| chrono::Utc::now() - chrono::Duration::days(n as i64));
    let mut undated = false;
    result.sources.retain(|source| {
        if !request.include_domains.is_empty()
            && !providers::util::domain_matches(&source.url, &request.include_domains)
        {
            return false;
        }
        if providers::util::domain_matches(&source.url, &request.exclude_domains) {
            return false;
        }
        if let Some(cutoff) = cutoff {
            let date = source.published_at.as_deref().and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .or_else(|| chrono::DateTime::parse_from_rfc2822(s).ok())
                    .map(|d| d.with_timezone(&chrono::Utc))
                    .or_else(|| {
                        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                            .ok()
                            .and_then(|d| d.and_hms_opt(0, 0, 0))
                            .map(|d| d.and_utc())
                    })
            });
            if let Some(date) = date {
                return date.date_naive() >= cutoff.date_naive();
            } else {
                undated = true;
            }
        }
        true
    });
    if undated {
        result.warnings.push("部分来源无可解析发布时间：引擎已收到时间条件，但不能保证这些来源在指定时间窗内；获取时间不是发布时间".into());
    }
    if request.language.is_some() || request.country.is_some() {
        result
            .warnings
            .push("语言/地区为查询偏好，不是结果内容的硬保证".into());
    }
    result
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    struct MockProvider {
        name: &'static str,
        delay: u64,
    }
    #[async_trait]
    impl WebSearchProvider for MockProvider {
        fn id(&self) -> &'static str {
            self.name
        }
        fn available(&self) -> bool {
            true
        }
        async fn search(&self, _: &WebSearchRequest) -> Result<WebSearchResult, WebError> {
            tokio::time::sleep(std::time::Duration::from_secs(self.delay)).await;
            Ok(WebSearchResult {
                sources: vec![WebSearchSource::new("https://docs.rs/regex/latest")],
                ..Default::default()
            })
        }
    }
    fn fast_factory(_: Option<&WebSearchConfig>, _: Option<&str>) -> Arc<dyn WebSearchProvider> {
        Arc::new(MockProvider {
            name: "mock_fast",
            delay: 0,
        })
    }
    fn slow_factory(_: Option<&WebSearchConfig>, _: Option<&str>) -> Arc<dyn WebSearchProvider> {
        Arc::new(MockProvider {
            name: "mock_slow",
            delay: 4,
        })
    }
    fn mock_service() -> (WebSearchService, WebSearchConfig) {
        let mut registry = ProviderRegistry::default();
        registry.register("mock_fast", fast_factory).unwrap();
        registry.register("mock_slow", slow_factory).unwrap();
        let mut cfg = WebSearchConfig::default();
        cfg.providers = vec!["mock_fast".into(), "mock_slow".into()];
        (
            WebSearchService {
                registry: RwLock::new(registry),
            },
            cfg,
        )
    }
    #[tokio::test]
    async fn web_partial_timeout_preserves_success() {
        let (svc, cfg) = mock_service();
        let mut req = WebSearchRequest::new("partial-engine-timeout-regression");
        req.research = true;
        req.timeout_secs = 1;
        req.refresh = true;
        let result = svc.search(&req, Some(&cfg), None).await.unwrap();
        assert_eq!(result.sources.len(), 1);
        assert!(result.warnings.iter().any(|w| w.contains("mock_slow")));
        assert_eq!(result.engines_used, vec!["mock_fast"]);
    }
    #[tokio::test]
    async fn web_cache_refresh_and_strict_engine_selection() {
        let (svc, mut cfg) = mock_service();
        cfg.providers = vec!["mock_fast".into()];
        let mut req = WebSearchRequest::new("cache-refresh-regression");
        req.engines = Some(vec!["mock_fast".into()]);
        assert!(!svc.search(&req, Some(&cfg), None).await.unwrap().cached);
        assert!(svc.search(&req, Some(&cfg), None).await.unwrap().cached);
        req.refresh = true;
        assert!(!svc.search(&req, Some(&cfg), None).await.unwrap().cached);
        // 未在配置中启用的引擎：请求级指定应被直接拒绝，而不是静默换人
        req.engines = Some(vec!["mock_slow".into()]);
        assert!(svc.search(&req, Some(&cfg), None).await.is_err());
        // 未注册的引擎 id 同理
        req.engines = Some(vec!["no_such_engine".into()]);
        assert!(svc.search(&req, Some(&cfg), None).await.is_err());
    }
    #[test]
    fn web_filters_dates_and_domain_boundaries() {
        let mut old = WebSearchSource::new("https://docs.rs/old");
        old.published_at = Some("2000-01-01".into());
        let mut fresh = WebSearchSource::new("https://docs.rs/current");
        fresh.published_at = Some(chrono::Utc::now().to_rfc2822());
        let unknown = WebSearchSource::new("https://sub.docs.rs/undated");
        let fake = WebSearchSource::new("https://docs.rs.evil.test/article");
        let mut req = WebSearchRequest::new("dates");
        req.recency_days = Some(7);
        req.include_domains = vec!["docs.rs".into()];
        let result = filter_sources(
            WebSearchResult {
                sources: vec![old, fresh, unknown, fake],
                ..Default::default()
            },
            &req,
        );
        assert_eq!(result.sources.len(), 2);
        assert!(result
            .warnings
            .iter()
            .any(|w| w.contains("无可解析发布时间")));
    }
    #[test]
    fn web_fusion_retains_other_engines_and_provenance() {
        let a = WebSearchResult {
            sources: (0..10)
                .map(|n| WebSearchSource::new(format!("https://a.test/{n}")))
                .collect(),
            ..Default::default()
        };
        let b = WebSearchResult {
            sources: vec![
                WebSearchSource::new("https://b.test/evidence"),
                WebSearchSource::new("https://a.test/0"),
            ],
            ..Default::default()
        };
        let result = merge_results(&[("a", a), ("b", b)], Some(3));
        assert!(result.sources.iter().any(|s| s.url.contains("b.test")));
        assert_eq!(result.sources[0].engines.len(), 2);
        assert_eq!(result.sources[0].confidence, "UNVERIFIED");
    }

    #[test]
    fn test_registry_builtin_and_duplicate() {
        let mut reg = ProviderRegistry::builtin();
        assert_eq!(
            reg.ids(),
            vec!["duckduckgo", "searxng", "tavily", "deepseek", "exa", "perplexity", "openai", "xai", "anthropic"]
        );

        let err = reg
            .register("deepseek", deepseek_factory_dup as ProviderFactory)
            .unwrap_err();
        assert_eq!(err.code, WebErrorCode::DuplicateProvider);
    }

    fn deepseek_factory_dup(
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

        // 配置顺序保留 + 未注册 id 过滤
        cfg.providers = vec!["tavily".into(), "unknown".into(), "searxng".into()];
        assert_eq!(
            svc.resolve_chain(&WebSearchRequest::new("q"), Some(&cfg)),
            vec!["tavily", "searxng"]
        );

        // 去重
        cfg.providers = vec!["tavily".into(), "tavily".into()];
        assert_eq!(
            svc.resolve_chain(&WebSearchRequest::new("q"), Some(&cfg)),
            vec!["tavily"]
        );
    }

    #[test]
    fn test_resolve_chain_request_engines() {
        let svc = WebSearchService::shared();
        let mut cfg = WebSearchConfig::default();
        cfg.providers = vec!["searxng".into(), "deepseek".into(), "tavily".into()];
        let mut base = WebSearchRequest::new("q");
        base.research = true;

        // 请求不带 engines → 全部已启用引擎
        assert_eq!(
            svc.resolve_chain(&base, Some(&cfg)),
            vec!["searxng", "deepseek", "tavily"]
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
        let req = WebSearchRequest::new("q").with_engines(vec!["duckduckgo".into()]);
        assert_eq!(
            svc.resolve_chain(&req, Some(&cfg)),
            vec!["searxng", "deepseek", "tavily"]
        );

        // 混合：部分启用部分未启用 → 只保留启用的部分
        let req = WebSearchRequest::new("q").with_engines(vec!["duckduckgo".into(), "tavily".into()]);
        assert_eq!(svc.resolve_chain(&req, Some(&cfg)), vec!["tavily"]);

        // 空字符串 / 空列表 → 忽略请求级指定
        let mut req = WebSearchRequest::new("q").with_engines(vec![" ".into()]);
        req.research = true;
        assert_eq!(
            svc.resolve_chain(&req, Some(&cfg)),
            vec!["searxng", "deepseek", "tavily"]
        );

        // 默认池（用户未启用任何）下指定 duckduckgo → 可用
        let req = WebSearchRequest::new("q").with_engines(vec!["duckduckgo".into()]);
        assert_eq!(svc.resolve_chain(&req, None), vec!["duckduckgo"]);
    }

    #[test]
    fn test_merge_results_order_dedup_and_cap() {
        let mk = |urls: Vec<&str>| WebSearchResult {
            content: None,
            sources: urls.into_iter().map(|u| WebSearchSource::new(u)).collect(),
            truncated: false,
            ..Default::default()
        };

        // 单 provider 直通语义
        let merged = merge_results(
            &[("a", mk(vec!["https://x.com/1", "https://x.com/2"]))],
            None,
        );
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
        let merged = merge_results(
            &[("a", mk(vec!["https://x.com/1", "https://x.com/2"]))],
            Some(1),
        );
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
        let p = registry
            .build("tavily", Some(&cfg), None)
            .expect("registered");
        assert!(!p.available());

        // deepseek：无 key（测试环境无 AppHandle）→ 不可用
        let mut cfg2 = WebSearchConfig::default();
        cfg2.providers = vec!["deepseek".into()];
        let p = registry
            .build("deepseek", Some(&cfg2), None)
            .expect("registered");
        assert!(!p.available());
    }
}
