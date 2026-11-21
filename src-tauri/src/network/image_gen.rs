//! 生图服务：OpenAI 兼容 `images/*` 与火山方舟 Seedream。
//!
//! 为什么单独一层而不是塞进 provider 体系：那套是围绕 chat completions 建的
//! （`ChatResponse` / `StreamEvent` / 工具调用重建），生图的入参出参形状完全不同。
//! 硬接进去会让路由层到处长 `if task == image`。
//!
//! 两个后端的能力边界不同，调用方必须知道：
//! - `openai_compat`：无参考图走 `/images/generations`；**有参考图**时改走
//!   `/images/edits`（multipart 多图），这是 OpenAI 官方的图生图入口。
//! - `seedream`：同一个端点，用 `image` 数组传参考图（data URL 或外链）。
//!
//! 结果一律解码成字节，不把 URL 交给上层——否则「生成成功」只是拿到一个会过期的链接。

use base64::Engine;
use serde_json::{json, Value};
use tauri::Manager;
use std::time::Duration;

use crate::network::proxy::ProxyConfig;

/// 一次生成请求。
pub struct ImageGenRequest {
    pub prompt: String,
    /// 参考图：(mime, bytes)。空表示纯文生图。
    pub references: Vec<(String, Vec<u8>)>,
    pub size: Option<String>,
    /// 质量档：low / medium / high / auto。草稿用 low，成品再升档——这是省时间和配额最有效的一档。
    pub quality: Option<String>,
    /// 要透明背景。后端支持就原生出 alpha；不支持则由调用方抠底。
    pub transparent: bool,
}

/// 生成结果。
pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    pub media_type: String,
    /// 后端改写后的 prompt（部分模型会返回，用于告诉用户"实际画的是什么"）
    pub revised_prompt: Option<String>,
}

/// 生图运行时配置（源自路由矩阵里的 `image_gen` 条目）。
///
/// 为什么和聊天路由共用一份配置：生图本质上也是一种"按任务选模型"，只是协议不同。
/// 放在矩阵里，用户只在一处管理端点/密钥/模型，不再多一个割裂的设置页。
pub struct ImageGenRuntime {
    /// `openai_compat` 或 `seedream`，由 provider_type 推导
    pub backend: String,
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    /// 调用方没给尺寸时用的默认值，按后端选
    pub default_size: String,
    pub timeout_secs: u64,
}

/// provider_type → 生图后端。
///
/// 路由矩阵的 provider_type 是给聊天用的（openai/gemini/doubao/custom…），
/// 生图只看两个事实：是不是方舟那套 `image[]` 入参。
fn backend_of(provider_type: &str) -> &'static str {
    match provider_type.trim().to_ascii_lowercase().as_str() {
        "doubao" | "volces" | "ark" | "seedream" | "bytedance" => "seedream",
        _ => "openai_compat",
    }
}

/// 读出生图运行时配置。配置不完整时返回 Ok(None)，由工具报具体缺哪项。
pub fn read_runtime() -> Option<(ImageGenRuntime, ProxyConfig)> {
    let handle = crate::network::web::current_app_handle()?;
    let cfg = handle
        .state::<std::sync::Arc<crate::state::AppState>>()
        .config
        .read()
        .get_all();
    let proxy = ProxyConfig::from_app_config(&cfg);

    // 不跟随 fallback 链：生图端点和聊天端点通常不是一回事，
    // 继承 chat 的配置只会让"写错了也能跑"变成更难查的问题。
    let route = cfg.routing_matrix.get("image_gen")?;
    let backend = backend_of(&route.provider_type).to_string();
    let default_size = if backend == "seedream" {
        "2K".to_string()
    } else {
        "1024x1024".to_string()
    };
    Some((
        ImageGenRuntime {
            backend,
            endpoint: route.endpoint.clone(),
            api_key: route.api_key.clone(),
            model: route.model.clone(),
            default_size,
            timeout_secs: 180,
        },
        proxy,
    ))
}

/// 配置缺项（用于给出可操作的报错，而不是笼统的"未启用"）。
pub fn missing_fields(rt: &ImageGenRuntime) -> Vec<&'static str> {
    let mut out = Vec::new();
    if rt.endpoint.trim().is_empty() {
        out.push("端点");
    }
    if rt.api_key.trim().is_empty() {
        out.push("API Key");
    }
    if rt.model.trim().is_empty() {
        out.push("模型");
    }
    out
}

fn build_client(cfg: &ImageGenRuntime, proxy: Option<&ProxyConfig>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs.clamp(20, 600)))
        .pool_max_idle_per_host(4);
    if let Some(pc) = proxy {
        match pc.effective_proxy_url() {
            Some(url) => {
                if let Ok(p) = reqwest::Proxy::all(&url) {
                    builder = builder.proxy(p);
                }
            }
            None => builder = builder.no_proxy(),
        }
    }
    builder.build().map_err(|e| format!("构造 HTTP 客户端失败：{e}"))
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// `/images/generations` → `/images/edits`
fn edits_endpoint(endpoint: &str) -> String {
    if let Some(base) = endpoint.strip_suffix("/images/generations") {
        format!("{base}/images/edits")
    } else if endpoint.ends_with("/images/edits") {
        endpoint.to_string()
    } else {
        format!("{}/images/edits", endpoint.trim_end_matches('/'))
    }
}

fn truncate_body(text: &str, max: usize) -> String {
    let cleaned: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        format!("{cleaned}…")
    } else {
        cleaned
    }
}

/// 解析 `data[].b64_json` / `data[].url`（Ark 与 OpenAI 都是这个形状）。
async fn parse_response(
    client: &reqwest::Client,
    value: &Value,
    rewritten: bool,
) -> Result<Vec<GeneratedImage>, String> {
    let items = value
        .get("data")
        .and_then(|v| v.as_array())
        .or_else(|| value.get("images").and_then(|v| v.as_array()))
        .ok_or_else(|| {
            format!(
                "响应里没有 data/images 数组：{}",
                truncate_body(&value.to_string(), 400)
            )
        })?;
    let mut out = Vec::new();
    for item in items {
        let revised = item
            .get("revised_prompt")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if let Some(b64) = item.get("b64_json").and_then(|v| v.as_str()) {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("b64 解码失败：{e}"))?;
            out.push(GeneratedImage {
                media_type: sniff_mime(&bytes),
                bytes,
                revised_prompt: revised,
            });
            continue;
        }
        // 有的实现把 base64 直接放在 url 字段里（历史上踩过）
        if let Some(url) = item.get("url").and_then(|v| v.as_str()) {
            if let Some(rest) = url.strip_prefix("data:") {
                if let Some((meta, b64)) = rest.split_once(',') {
                    let mime = meta.split(';').next().unwrap_or("image/png").to_string();
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .map_err(|e| format!("data URL 解码失败：{e}"))?;
                    out.push(GeneratedImage {
                        media_type: mime,
                        bytes,
                        revised_prompt: revised,
                    });
                    continue;
                }
            }
            let resp = client
                .get(url)
                .send()
                .await
                .map_err(|e| format!("下载生成结果失败：{e}"))?;
            if !resp.status().is_success() {
                return Err(format!("下载生成结果失败：HTTP {}", resp.status()));
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| format!("读取生成结果失败：{e}"))?
                .to_vec();
            out.push(GeneratedImage {
                media_type: sniff_mime(&bytes),
                bytes,
                revised_prompt: revised,
            });
        }
    }
    if out.is_empty() {
        let hint = if rewritten {
            "（已改用 /images/edits 走参考图路径；若该端点不支持编辑，请换支持图生图的后端）"
        } else {
            ""
        };
        return Err(format!(
            "响应里没有可用的图片数据{hint}：{}",
            truncate_body(&value.to_string(), 400)
        ));
    }
    Ok(out)
}

/// 按魔数判断图片类型：后端返回的 mime 不可靠，落盘要用真实类型。
pub fn sniff_mime(bytes: &[u8]) -> String {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return "image/png".into();
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "image/jpeg".into();
    }
    if bytes.starts_with(b"RIFF") && bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        return "image/webp".into();
    }
    if bytes.starts_with(b"GIF8") {
        return "image/gif".into();
    }
    "image/png".into()
}

/// 执行一次生成。
pub async fn generate(
    cfg: &ImageGenRuntime,
    proxy: Option<&ProxyConfig>,
    req: &ImageGenRequest,
) -> Result<Vec<GeneratedImage>, String> {
    let missing = missing_fields(cfg);
    if !missing.is_empty() {
        return Err(format!(
            "生图路由（设置 → 任务模型分配 → 图片生成）还缺：{}。填好后重试。",
            missing.join(" / ")
        ));
    }
    let client = build_client(cfg, proxy)?;
    let size = req
        .size
        .clone()
        .unwrap_or_else(|| cfg.default_size.clone());
    let backend = cfg.backend.to_ascii_lowercase();

    // 透明背景不是所有模型都支持（不同代际差别很大）。策略：先带着透明参数请求，
    // 被明确拒绝再退回"不指定背景"，由调用方抠底补 alpha。
    // 这样同一份路由配置在支持与不支持真透明的模型上都能给出可用结果，
    // 而不是一遇到不支持就整单失败。
    let want_native_transparent = req.transparent;

    let generation_body = |with_transparency: bool| {
        let mut body = json!({
            "model": cfg.model,
            "prompt": req.prompt,
            "n": 1,
            "size": size,
        });
        if let Some(q) = &req.quality {
            body["quality"] = json!(q);
        }
        if with_transparency && want_native_transparent {
            body["background"] = json!("transparent");
            body["output_format"] = json!("png");
        }
        body
    };

    fn edit_form(
        cfg: &ImageGenRuntime,
        req: &ImageGenRequest,
        size: &str,
        with_transparency: bool,
    ) -> Result<reqwest::multipart::Form, String> {
        let mut form = reqwest::multipart::Form::new()
            .text("model", cfg.model.clone())
            .text("prompt", req.prompt.clone())
            .text("size", size.to_string());
        if let Some(q) = &req.quality {
            form = form.text("quality", q.clone());
        }
        if with_transparency && req.transparent {
            form = form.text("background", "transparent")
                .text("output_format", "png");
        }
        for (mime, bytes) in &req.references {
            let part = reqwest::multipart::Part::bytes(bytes.clone())
                .file_name(format!("ref.{}", mime.rsplit('/').next().unwrap_or("png")))
                .mime_str(mime)
                .map_err(|e| format!("构造参考图 part 失败：{e}"))?;
            form = form.part("image", part);
        }
        Ok(form)
    }

    if matches!(backend.as_str(), "seedream" | "ark" | "doubao" | "volces" | "bytedance") {
        let mut body = json!({
            "model": cfg.model,
            "prompt": req.prompt,
            "size": size,
            "response_format": "b64_json",
            "watermark": false,
        });
        if !req.references.is_empty() {
            let images: Vec<String> = req
                .references
                .iter()
                .map(|(m, b)| data_url(m, b))
                .collect();
            body["image"] = json!(images);
        }
        let resp = client
            .post(cfg.endpoint.trim())
            .bearer_auth(cfg.api_key.trim())
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("请求生图服务失败：{e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "生图服务返回 HTTP {status}：{}",
                truncate_body(&text, 600)
            ));
        }
        let value: Value = serde_json::from_str(&text).map_err(|e| {
            format!("响应不是合法 JSON：{e}；原始内容：{}", truncate_body(&text, 300))
        })?;
        // 该后端没有真透明参数；调用方会按需抠底
        return parse_response(&client, &value, false).await;
    }

    if req.references.is_empty() {
        // 文生图
        let mut transparent = want_native_transparent;
        loop {
            let resp = client
                .post(cfg.endpoint.trim())
                .bearer_auth(cfg.api_key.trim())
                .json(&generation_body(transparent))
                .send()
                .await
                .map_err(|e| format!("请求生图服务失败：{e}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.is_success() {
                let value: Value = serde_json::from_str(&text).map_err(|e| {
                    format!("响应不是合法 JSON：{e}；原始内容：{}", truncate_body(&text, 300))
                })?;
                return parse_response(&client, &value, false).await;
            }
            if transparent && looks_like_unsupported_transparency(&text) {
                // 模型不支持透明参数：去掉它重试一次，alpha 交给调用方抠底
                transparent = false;
                continue;
            }
            return Err(format!(
                "生图服务返回 HTTP {status}：{}",
                truncate_body(&text, 600)
            ));
        }
    }

    // 有参考图 → 走 edits（multipart）。这是官方的图生图入口。
    let url = edits_endpoint(cfg.endpoint.trim());
    let mut transparent = want_native_transparent;
    loop {
        let form = edit_form(cfg, req, &size, transparent)?;
        let resp = client
            .post(&url)
            .bearer_auth(cfg.api_key.trim())
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("请求 {url} 失败：{e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_success() {
            let value: Value = serde_json::from_str(&text).map_err(|e| {
                format!("响应不是合法 JSON：{e}；原始内容：{}", truncate_body(&text, 300))
            })?;
            return parse_response(&client, &value, true).await;
        }
        if transparent && looks_like_unsupported_transparency(&text) {
            transparent = false;
            continue;
        }
        return Err(format!(
            "图生图返回 HTTP {status}（{url}）：{}。\
             若该端点只支持文生图，请改用支持参考图的后端（如 seedream）。",
            truncate_body(&text, 600)
        ));
    }
}

/// 判断失败是否因为后端不认识透明相关参数（而不是鉴权/额度/模型名错误）。
///
/// 只看这三个词：它们在鉴权与配额类报错里不会出现，误判概率低。
/// 判成"不支持"就降级重试一次，代价是一次多花的请求；判错成"支持"
/// 会让整个透明请求失败，所以这里宁可偏向重试。
fn looks_like_unsupported_transparency(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("background") || t.contains("transparent") || t.contains("output_format")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_endpoint_derives_from_generations() {
        assert_eq!(
            edits_endpoint("https://api.openai.com/v1/images/generations"),
            "https://api.openai.com/v1/images/edits"
        );
        assert_eq!(
            edits_endpoint("https://x/v1/images/edits"),
            "https://x/v1/images/edits"
        );
        assert_eq!(edits_endpoint("https://x/v1"), "https://x/v1/images/edits");
    }

    #[test]
    fn mime_detected_from_magic_bytes() {
        assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\nrest"), "image/png");
        assert_eq!(sniff_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), "image/jpeg");
        assert_eq!(sniff_mime(b"GIF89a"), "image/gif");
    }
}
