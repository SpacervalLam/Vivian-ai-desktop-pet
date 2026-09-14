//! 翻译服务：在 TTS 合成前将文本从显示语言翻译为 TTS 语言
//!
//! 支持的翻译服务：DeepL、Google Translate、LLM（大语言模型）
//! DeepL/Google 按句切分逐句翻译，带内存缓存避免重复请求
//! LLM 整体翻译，利用模型自身的上下文理解能力
//!
//! 上下文优化：DeepL/Google 维护滑动窗口，每句翻译时将前 2 句原文作为 context 传入，
//! 让翻译引擎理解上下文，解决单句翻译的语序/歧义问题。
//! DeepL 原生支持 context 参数；Google 不支持，接受质量略低。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OnceCell, RwLock};

use crate::error::VivianError;
use crate::providers::base::LLMRequest;
use crate::providers::router::ModelRouter;
use crate::types::response::ChatMessage;
use crate::utils::fnv1a_64;

/// 翻译服务单例
static TRANSLATION_SERVICE: OnceCell<TranslationService> = OnceCell::const_new();

pub async fn translation_service() -> &'static TranslationService {
    TRANSLATION_SERVICE
        .get_or_init(|| async { TranslationService::new() })
        .await
}

pub struct TranslationService {
    client: reqwest::Client,
    cache: Arc<RwLock<HashMap<String, String>>>,
}

impl TranslationService {
    fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        Self {
            client,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 翻译文本：按句切分，逐句翻译（带缓存 + 上下文），拼接返回
    ///
    /// - `from`/`to`: 语言代码（小写，如 "zh"、"ja"、"en"）
    /// - `provider`: "deepl" | "google"
    /// - `api_key`: 翻译服务 API Key
    /// - `endpoint`: 自定义端点（留空使用官方端点）
    pub async fn translate(
        &self,
        text: &str,
        from: &str,
        to: &str,
        provider: &str,
        api_key: &str,
        endpoint: Option<&str>,
    ) -> Result<String, VivianError> {
        if text.trim().is_empty() || from == to {
            return Ok(text.to_string());
        }

        let sentences = split_sentences_for_translation(text);
        let mut results = Vec::with_capacity(sentences.len());
        // 滑动上下文窗口：保留前 2 句原文
        let mut context_window: Vec<&str> = Vec::with_capacity(2);

        for sentence in &sentences {
            if sentence.trim().is_empty() {
                results.push(sentence.clone());
                continue;
            }

            let context = context_window.join("\n");
            // 缓存 key 包含上下文 hash，避免不同上下文下相同句子缓存冲突
            let context_hash = fnv1a_64(&context);
            let cache_key = format!("{sentence}|{from}|{to}|{context_hash}");

            {
                let cache = self.cache.read().await;
                if let Some(cached) = cache.get(&cache_key) {
                    results.push(cached.clone());
                    // 更新滑动窗口
                    if context_window.len() >= 2 {
                        context_window.remove(0);
                    }
                    context_window.push(sentence);
                    continue;
                }
            }

            let translated = match provider {
                "deepl" => {
                    self.translate_deepl(sentence, from, to, api_key, endpoint, &context)
                        .await?
                }
                "google" => {
                    self.translate_google(sentence, from, to, api_key, endpoint)
                        .await?
                }
                other => {
                    return Err(VivianError::Other(format!(
                        "未知翻译服务: {other}"
                    )))
                }
            };

            {
                let mut cache = self.cache.write().await;
                if cache.len() > 500 {
                    let keys: Vec<String> =
                        cache.keys().take(250).cloned().collect();
                    for k in keys {
                        cache.remove(&k);
                    }
                }
                cache.insert(cache_key, translated.clone());
            }

            results.push(translated);

            // 更新滑动窗口
            if context_window.len() >= 2 {
                context_window.remove(0);
            }
            context_window.push(sentence);
        }

        Ok(results.join("\n"))
    }

    async fn translate_deepl(
        &self,
        text: &str,
        from: &str,
        to: &str,
        api_key: &str,
        endpoint: Option<&str>,
        context: &str,
    ) -> Result<String, VivianError> {
        if api_key.is_empty() {
            return Err(VivianError::Other("DeepL API Key 未配置".into()));
        }

        let url = endpoint.unwrap_or_else(|| {
            if api_key.ends_with(":fx") {
                "https://api-free.deepl.com/v2/translate"
            } else {
                "https://api.deepl.com/v2/translate"
            }
        });

        let mut form = vec![
            ("text".to_string(), text.to_string()),
            ("source_lang".to_string(), from.to_uppercase()),
            ("target_lang".to_string(), to.to_uppercase()),
        ];
        // DeepL 原生 context 参数：传入前文句子作为上下文，影响翻译但不被翻译
        if !context.is_empty() {
            form.push(("context".to_string(), context.to_string()));
        }

        let resp = self
            .client
            .post(url)
            .header("Authorization", format!("DeepL-Auth-Key {api_key}"))
            .form(&form)
            .send()
            .await
            .map_err(|e| VivianError::Other(format!("DeepL 请求失败: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(VivianError::Other(format!(
                "DeepL 返回 {status}: {body}"
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| VivianError::Other(format!("DeepL 响应解析失败: {e}")))?;

        body["translations"][0]["text"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| VivianError::Other("DeepL 响应格式异常".into()))
    }

    async fn translate_google(
        &self,
        text: &str,
        from: &str,
        to: &str,
        api_key: &str,
        endpoint: Option<&str>,
    ) -> Result<String, VivianError> {
        if api_key.is_empty() {
            return Err(VivianError::Other("Google API Key 未配置".into()));
        }

        let base = endpoint.unwrap_or("https://translation.googleapis.com/language/translate/v2");
        let url = format!("{base}?key={api_key}");

        let body = serde_json::json!({
            "q": text,
            "source": from,
            "target": to,
            "format": "text",
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| VivianError::Other(format!("Google 翻译请求失败: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(VivianError::Other(format!(
                "Google 翻译返回 {status}: {body}"
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| VivianError::Other(format!("Google 翻译响应解析失败: {e}")))?;

        body["data"]["translations"][0]["translatedText"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| VivianError::Other("Google 翻译响应格式异常".into()))
    }

    /// LLM 翻译：整体翻译，利用模型上下文理解能力
    ///
    /// 通过路由矩阵的 `translation` 任务 provider 调用 LLM，
    /// 翻译提示词指导模型仅输出译文，解析流程去除可能的冗余包装。
    pub async fn translate_llm(
        &self,
        text: &str,
        from: &str,
        to: &str,
        router: &ModelRouter,
    ) -> Result<String, VivianError> {
        if text.trim().is_empty() || from == to {
            return Ok(text.to_string());
        }

        let cache_key = format!("llm|{from}|{to}|{}", fnv1a_64(text));
        {
            let cache = self.cache.read().await;
            if let Some(cached) = cache.get(&cache_key) {
                return Ok(cached.clone());
            }
        }

        let from_name = lang_code_to_name(from);
        let to_name = lang_code_to_name(to);

        let system_prompt = format!(
            "你是专业翻译引擎。将用户输入从{from_name}翻译为{to_name}。\n\
             要求：\n\
             1. 仅输出翻译结果，不输出任何解释、注释或原文\n\
             2. 保持原文的语气和情感色彩\n\
             3. 标点符号遵循目标语言习惯（中文日文用全角，英文用半角）\n\
             4. 自然流畅，符合目标语言的表达习惯"
        );

        let messages = vec![
            ChatMessage::system(&system_prompt),
            ChatMessage::user(text),
        ];

        let request = LLMRequest::new("translation", messages).with_temperature(0.3);

        let response = router
            .generate(request)
            .await
            .map_err(|e| VivianError::Other(format!("LLM 翻译请求失败: {e}")))?;

        let translated = parse_llm_translation(&response);

        {
            let mut cache = self.cache.write().await;
            if cache.len() > 500 {
                let keys: Vec<String> = cache.keys().take(250).cloned().collect();
                for k in keys {
                    cache.remove(&k);
                }
            }
            cache.insert(cache_key, translated.clone());
        }

        Ok(translated)
    }
}

/// 语言代码转可读名称（用于翻译提示词）
fn lang_code_to_name(code: &str) -> &str {
    match code {
        "zh" => "中文",
        "ja" => "日本語",
        "en" => "English",
        "ko" => "한국어",
        "fr" => "Français",
        "de" => "Deutsch",
        "es" => "Español",
        "ru" => "Русский",
        _ => code,
    }
}

/// 解析 LLM 翻译响应：去除 markdown 代码块、引号等冗余包装
fn parse_llm_translation(raw: &str) -> String {
    let mut result = raw.trim().to_string();

    if result.starts_with("```") {
        if let Some(end) = result.find('\n') {
            result = result[end + 1..].to_string();
        }
        if result.ends_with("```") {
            result = result[..result.len() - 3].to_string();
        }
        result = result.trim().to_string();
    }

    let len = result.chars().count();
    if len >= 2 {
        let first = result.chars().next().unwrap();
        let last = result.chars().last().unwrap();
        if (first == '"' && last == '"')
            || (first == '"' && last == '"')
            || (first == '「' && last == '」')
            || (first == '\'' && last == '\'')
        {
            result = result.chars().skip(1).take(len - 2).collect();
        }
    }

    result.trim().to_string()
}

/// 按句末标点切分文本（用于逐句翻译）
///
/// 切分字符：。！？!?；; 和换行符。保留标点在句尾。
fn split_sentences_for_translation(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        if matches!(
            ch,
            '。' | '！' | '？' | '.' | '!' | '?' | ';' | '；' | '\n' | '\r'
        ) {
            if !current.trim().is_empty() {
                sentences.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
    }
    if !current.trim().is_empty() {
        sentences.push(current);
    }

    if sentences.is_empty() {
        vec![text.to_string()]
    } else {
        sentences
    }
}
