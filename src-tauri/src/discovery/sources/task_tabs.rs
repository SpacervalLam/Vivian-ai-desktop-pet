//! 隔离任务 tab 自动发现 — 小红书 / 抖音 / 知乎（非 LLM 驱动的后台采集）
//!
//! 模型：经浏览器桥的 `browser_task_tab` 工具在后台静默创建 inactive 标签
//! （不抢占用户焦点、立即静音），加载平台页面后在页面上下文执行同源
//! fetch / DOM 提取，完成后自动关闭标签。标签与浏览器同 profile，天然
//! 携带平台登录 Cookie，实现需登录态平台的后台发现。
//!
//! 采集本身为纯脚本（无 LLM 参与），候选仍走引擎统一 LLM 评估入库。
//! 每平台按角色独立冷却（默认 3 小时），且要求扩展已上报该平台登录态。

use serde_json::Value;

use super::ContentCandidate;
use crate::discovery::engine;

/// 任务 tab 平台冷却（秒）
const TASK_TAB_COOLDOWN_SECS: i64 = 3 * 3600;
/// 单次任务 tab 工具预算（页面加载 15s + SPA 渲染 + 提取）
const TASK_TAB_TIMEOUT_MS: u64 = 80_000;
/// 每平台单轮采集上限
const TASK_TAB_LIMIT: usize = 20;

/// 一个平台的后台采集配置
struct TaskTabPlan {
    platform: &'static str,
    /// 打开的平台页 URL
    url: &'static str,
    /// 页面加载后的额外渲染等待（毫秒）
    wait_ms: u64,
    /// 页面上下文执行的提取脚本（返回 JSON 字符串）
    code: &'static str,
    /// 是否要求扩展上报该平台已登录
    needs_login: bool,
}

const ZHIHU_CODE: &str = r#"(async () => {
  try {
    const r = await fetch('/api/v3/feed/topstory/hot-lists/total?limit=20', { credentials: 'include' });
    const j = await r.json();
    const list = (j && j.data) || [];
    return JSON.stringify(list.map((x, i) => {
      const t = x.target || {};
      return {
        id: 'zhihu-hot-' + (x.id || i),
        title: x.title || '',
        desc: (t.excerpt_area && t.excerpt_area.text) || x.detail_text || '',
        url: t.url || '',
        author: (t.author && t.author.name) || '',
        cover: (t.thumbnail || (t.excerpt_area && t.excerpt_area.thumbnail)) || '',
        views: 0,
        likes: 0,
      };
    }).filter(x => x.title));
  } catch (e) { return JSON.stringify([]); }
})()"#;

const XHS_CODE: &str = r#"(() => {
  const seen = new Set();
  const items = [];
  document.querySelectorAll('a[href*="/explore/"], a[href*="/search_result/"]').forEach((a) => {
    const href = a.getAttribute('href') || '';
    const m = href.match(/\/(?:explore|search_result)\/([0-9a-f]{8,})/);
    if (!m) return;
    const id = m[1];
    if (seen.has(id)) return;
    const img = a.querySelector('img');
    const title = ((a.getAttribute('title') || a.textContent || (img && img.alt) || '') + '').trim().slice(0, 80);
    if (!title) return;
    seen.add(id);
    items.push({
      id, title, desc: title,
      url: 'https://www.xiaohongshu.com/explore/' + id,
      author: '', cover: (img && img.src) || '', views: 0, likes: 0,
    });
  });
  return JSON.stringify(items.slice(0, 20));
})()"#;

const DOUYIN_CODE: &str = r#"(() => {
  const seen = new Set();
  const items = [];
  document.querySelectorAll('a[href*="/video/"]').forEach((a) => {
    const href = a.getAttribute('href') || '';
    const m = href.match(/\/video\/(\d+)/);
    if (!m) return;
    const id = m[1];
    if (seen.has(id)) return;
    const img = a.querySelector('img');
    const title = ((a.textContent || (img && img.alt) || '') + '').trim().slice(0, 80);
    if (!title) return;
    seen.add(id);
    items.push({
      id, title, desc: title,
      url: 'https://www.douyin.com/video/' + id,
      author: '', cover: (img && img.src) || '', views: 0, likes: 0,
    });
  });
  return JSON.stringify(items.slice(0, 20));
})()"#;

fn task_tab_plans() -> Vec<TaskTabPlan> {
    vec![
        TaskTabPlan {
            platform: "zhihu",
            url: "https://www.zhihu.com/hot",
            wait_ms: 1200,
            code: ZHIHU_CODE,
            needs_login: false,
        },
        TaskTabPlan {
            platform: "xiaohongshu",
            url: "https://www.xiaohongshu.com/explore",
            wait_ms: 3000,
            code: XHS_CODE,
            needs_login: true,
        },
        TaskTabPlan {
            platform: "douyin",
            url: "https://www.douyin.com/?recommend=1",
            wait_ms: 3000,
            code: DOUYIN_CODE,
            needs_login: true,
        },
    ]
}

/// 平台冷却账本："{char_id}:{platform}" → 上次采集时间戳
static LAST_TASK_TAB: once_cell::sync::Lazy<parking_lot::RwLock<std::collections::HashMap<String, i64>>> =
    once_cell::sync::Lazy::new(|| parking_lot::RwLock::new(std::collections::HashMap::new()));

fn task_tab_due(char_id: &str, platform: &str) -> bool {
    let map = LAST_TASK_TAB.read();
    match map.get(&format!("{}:{}", char_id, platform)) {
        Some(last) => chrono::Utc::now().timestamp() - last >= TASK_TAB_COOLDOWN_SECS,
        None => true,
    }
}

fn record_task_tab(char_id: &str, platform: &str) {
    LAST_TASK_TAB
        .write()
        .insert(format!("{}:{}", char_id, platform), chrono::Utc::now().timestamp());
}

/// 任务 tab 结果文本（untrusted 包裹 + 可能的双重 JSON 字符串）→ 原始条目数组
fn parse_task_tab_items(text: &str) -> Vec<Value> {
    // 剥离 <UNTRUSTED_PAGE_CONTENT ...> 包裹与转义字符串双重包装
    let Some(value) = crate::discovery::parse_json_tolerant(text) else {
        return Vec::new();
    };
    let value = if let Some(inner) = value.as_str() {
        crate::discovery::parse_json_tolerant(inner).unwrap_or(Value::Null)
    } else {
        value
    };
    value.as_array().cloned().unwrap_or_default()
}

/// 原始条目 → 统一候选
fn raw_to_candidate(platform: &str, item: &Value, source: &str) -> Option<ContentCandidate> {
    let id = item.get("id").and_then(|v| v.as_str())?.to_string();
    let title = item
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() || title.is_empty() {
        return None;
    }
    let url = item
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|u| u.starts_with("http"))
        .unwrap_or("")
        .to_string();
    Some(ContentCandidate {
        platform: platform.to_string(),
        content_id: id,
        title,
        description: item
            .get("desc")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(150)
            .collect(),
        author: item
            .get("author")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        url,
        cover_url: item
            .get("cover")
            .and_then(|v| v.as_str())
            .filter(|c| c.starts_with("http"))
            .unwrap_or("")
            .to_string(),
        duration_secs: 0,
        view_count: item.get("views").and_then(|v| v.as_u64()).unwrap_or(0),
        like_count: item.get("likes").and_then(|v| v.as_u64()).unwrap_or(0),
        pubdate: 0,
        source: source.to_string(),
    })
}

/// 知识采集维护顺带的隔离任务 tab 自动发现（小红书/抖音/知乎）
///
/// 每平台独立冷却 + 登录态门槛；扩展未连接 / 平台未登录 / 冷却未到时静默跳过。
/// 采集为纯脚本驱动（无 LLM），候选统一走引擎评估入库。
pub async fn refresh_from_task_tabs(char_id: &str) {
    let Some(bridge) = crate::browser_bridge::tools::global_bridge() else {
        return;
    };
    let platform_status = bridge.platform_status().map(|(_, m)| m);

    for plan in task_tab_plans() {
        if !task_tab_due(char_id, plan.platform) {
            continue;
        }
        // 登录态门槛：需登录平台在扩展上报未登录时跳过（不打开必然空白的任务 tab）
        if plan.needs_login {
            let logged_in = platform_status
                .as_ref()
                .and_then(|m| m.get(plan.platform).copied())
                .unwrap_or(false);
            if !logged_in {
                continue;
            }
        }

        let args = serde_json::json!({
            "url": plan.url,
            "code": plan.code,
            "waitMs": plan.wait_ms,
        });
        let result = bridge
            .request_tool(
                "browser_task_tab",
                &args,
                std::time::Duration::from_millis(TASK_TAB_TIMEOUT_MS),
                Some("discovery-tasktab"),
            )
            .await;

        // 无论成败都记录冷却，避免布局改版/登录失效时反复开 tab 打扰
        record_task_tab(char_id, plan.platform);

        let Ok(text) = result else {
            tracing::debug!("[discovery:{}] 任务 tab 采集失败（{}）", char_id, plan.platform);
            continue;
        };
        let candidates: Vec<ContentCandidate> = parse_task_tab_items(&text)
            .iter()
            .filter_map(|item| raw_to_candidate(plan.platform, item, "task_tab"))
            .take(TASK_TAB_LIMIT)
            .collect();
        if candidates.is_empty() {
            tracing::debug!("[discovery:{}] 任务 tab 采集为空（{}）", char_id, plan.platform);
            continue;
        }
        let admitted = engine::admit_candidates(char_id, candidates).await;
        tracing::info!(
            "[discovery:{}] 任务 tab 采集（{}）：入库 {} 条",
            char_id,
            plan.platform,
            admitted
        );
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_task_tab_items_wrapped() {
        let inner = r#"[{"id":"a1","title":"t","desc":"d","url":"https://www.zhihu.com/question/1","author":"z","cover":"","views":0,"likes":0}]"#;
        let wrapped = format!("<UNTRUSTED_PAGE_CONTENT nonce=\"x\">\n{}\n</UNTRUSTED_PAGE_CONTENT>", inner);
        let items = parse_task_tab_items(&wrapped);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "a1");
    }

    #[test]
    fn test_parse_task_tab_items_double_wrapped() {
        let inner = r#"[{"id":"a1","title":"t"}]"#.to_string();
        let once = serde_json::to_string(&inner).unwrap();
        let wrapped = format!("<UNTRUSTED_PAGE_CONTENT nonce=\"x\">\n{}\n</UNTRUSTED_PAGE_CONTENT>", once);
        let items = parse_task_tab_items(&wrapped);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "a1");
    }

    #[test]
    fn test_raw_to_candidate() {
        let item = serde_json::json!({
            "id": "note1",
            "title": "标题",
            "desc": "描述",
            "url": "https://www.xiaohongshu.com/explore/note1",
            "author": "作者",
            "cover": "https://img.example/1.jpg",
        });
        let c = raw_to_candidate("xiaohongshu", &item, "task_tab").unwrap();
        assert_eq!(c.platform, "xiaohongshu");
        assert_eq!(c.content_id, "note1");
        assert_eq!(c.url, "https://www.xiaohongshu.com/explore/note1");
    }
}