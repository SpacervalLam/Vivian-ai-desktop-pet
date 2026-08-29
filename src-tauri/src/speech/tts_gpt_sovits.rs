//! GPT-SoVITS TTS 后端
//!
//! 对齐真实 API: https://github.com/RVC-Boss/GPT-SoVITS
//!
//! - v2 (api_v2.py): 端点 `/tts`,字段 `text_lang`/`ref_audio_path`/`prompt_lang`/`speed_factor`/`media_type`
//! - v1 (api.py):   端点 `/`,  字段 `text_language`/`refer_wav_path`/`prompt_language`/`speed`
//!
//! v2 优先(功能更全,支持流式);v2 端点不可用时回退 v1
//! - `ref_audio_path` 与 `prompt_text` 必需(若用户未配置,后端启动时应用 -dr/-dt/-dl 指定默认参考音频)
//! - `prompt_lang` 必需,默认与 `text_lang` 相同
//! - 输出 WAV 音频
//! - 不支持 WordBoundary

use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use crate::error::{VivianError, VivianResult};

use super::tts::{TtsConfig, VoiceInfo};
use super::tts_backend::{AudioFormat, TtsBackend, TtsSynthesisResult};

pub struct GptSoVitsBackend {
    client: reqwest::Client,
}

impl GptSoVitsBackend {
    pub fn new() -> Self {
        Self {
            // 禁用代理：GPT-SoVITS 服务在 localhost，走代理会得到 502
            // 超时 60s：GPT-SoVITS 首次推理（冷启动）需加载 BERT 特征、构建 jieba 词典等，
            // 实测耗时可达 20s+；后续请求会被缓存，耗时仅数秒。
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .no_proxy()
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    fn base_url(config: &TtsConfig) -> VivianResult<&str> {
        config
            .gpt_sovits_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| VivianError::Speech("GPT-SoVITS 未配置服务地址".to_string()))
    }

    /// 推断文本语言(zh/en/ja/ko/yue)
    fn detect_language(text: &str) -> &'static str {
        let has_cjk = text.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c));
        let has_jp = text
            .chars()
            .any(|c| ('\u{3040}'..='\u{30FF}').contains(&c));
        if has_jp && !has_cjk {
            "ja"
        } else if has_cjk {
            "zh"
        } else {
            "en"
        }
    }

    /// 从 Content-Type 推断音频格式
    fn format_from_content_type(content_type: &str) -> AudioFormat {
        if content_type.contains("wav") {
            AudioFormat::Wav
        } else if content_type.contains("ogg") {
            AudioFormat::Ogg
        } else if content_type.contains("aac") {
            AudioFormat::Aac
        } else {
            // GPT-SoVITS 默认输出 WAV
            AudioFormat::Wav
        }
    }
}

#[async_trait]
impl TtsBackend for GptSoVitsBackend {
    fn name(&self) -> &'static str {
        "gpt-sovits"
    }

    async fn synthesize(&self, text: &str, config: &TtsConfig) -> VivianResult<TtsSynthesisResult> {
        let base = Self::base_url(config)?;
        let lang = Self::detect_language(text);

        // 优先尝试 v2 端点(`/tts`),失败时回退 v1(`/`)
        match self.try_v2(text, config, base, lang).await {
            Ok(result) => Ok(result),
            Err(e) => {
                // v2 失败:仅在端点不存在(404/405)时回退 v1,其他错误直接返回
                let msg = e.to_string();
                if msg.contains("404") || msg.contains("Not Found") || msg.contains("405") {
                    tracing::info!("[TTS] GPT-SoVITS v2 不可用,回退 v1");
                    self.try_v1(text, config, base, lang).await
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn list_voices(&self, _config: &TtsConfig) -> VivianResult<Vec<VoiceInfo>> {
        // GPT-SoVITS 没有"语音列表"概念:它通过参考音频+文本驱动音色克隆
        // 模型切换通过 /set_gpt_weights 和 /set_sovits_weights 端点,而非语音 ID
        Ok(Vec::new())
    }

    async fn health_check(&self, config: &TtsConfig) -> bool {
        Self::base_url(config).is_ok()
    }
}

impl GptSoVitsBackend {
    /// v2 端点 `/tts` — 对齐 api_v2.py
    async fn try_v2(
        &self,
        text: &str,
        config: &TtsConfig,
        base: &str,
        lang: &str,
    ) -> VivianResult<TtsSynthesisResult> {
        let url = format!("{}/tts", base.trim_end_matches('/'));

        // prompt_lang: 用户配置的参考音频语种,默认与 text_lang 相同
        let prompt_lang = config
            .gpt_sovits_prompt_lang
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(lang);

        // 文本切分方式:用户可在配置中选择,默认 cut5(按标点切)
        let text_split_method = config
            .gpt_sovits_text_split_method
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("cut5");

        // 并行推理:默认 true
        let parallel_infer = config.gpt_sovits_parallel_infer.unwrap_or(true);

        // 构建请求体:严格对齐 api_v2.py 的 TTS_Request 模型
        let mut body = json!({
            "text": text,
            "text_lang": lang,
            "prompt_lang": prompt_lang,
            "media_type": "wav",
            "text_split_method": text_split_method,
            "speed_factor": config.rate,
            "parallel_infer": parallel_infer,
        });

        // 主参考音频(必填):决定音色 — 校验文件存在性，缺失时跳过避免 400 错误
        if let Some(ref_audio) = config.gpt_sovits_ref_audio.as_deref() {
            if !ref_audio.is_empty() {
                if std::path::Path::new(ref_audio).exists() {
                    body["ref_audio_path"] = json!(ref_audio);
                } else {
                    tracing::warn!(
                        "[TTS] GPT-SoVITS 参考音频文件不存在，已跳过: {}",
                        ref_audio
                    );
                }
            }
        }
        // 参考音频文本(可选)
        if let Some(prompt_text) = config.gpt_sovits_prompt_text.as_deref() {
            if !prompt_text.is_empty() {
                body["prompt_text"] = json!(prompt_text);
            }
        }
        // 辅助参考音频(可选):多参考音频音色融合 — 过滤不存在的路径
        if let Some(aux_paths) = config.gpt_sovits_aux_ref_audios.as_ref() {
            let valid_aux: Vec<&String> = aux_paths
                .iter()
                .filter(|p| !p.is_empty() && std::path::Path::new(p).exists())
                .collect();
            if !valid_aux.is_empty() {
                body["aux_ref_audio_paths"] = json!(valid_aux);
            }
            let invalid: Vec<&String> = aux_paths
                .iter()
                .filter(|p| !p.is_empty() && !std::path::Path::new(p).exists())
                .collect();
            for p in invalid {
                tracing::warn!("[TTS] GPT-SoVITS 辅助参考音频不存在，已跳过: {}", p);
            }
        }
        // 高级采样参数(可选)
        if let Some(top_k) = config.gpt_sovits_top_k {
            body["top_k"] = json!(top_k);
        }
        if let Some(top_p) = config.gpt_sovits_top_p {
            body["top_p"] = json!(top_p);
        }
        if let Some(temperature) = config.gpt_sovits_temperature {
            body["temperature"] = json!(temperature);
        }

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| VivianError::Speech(format!("GPT-SoVITS v2 请求失败: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(VivianError::Speech(format!(
                "GPT-SoVITS v2 失败 [{}]: {}",
                status, body
            )));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let audio = resp
            .bytes()
            .await
            .map_err(|e| VivianError::Speech(format!("读取 GPT-SoVITS v2 音频失败: {e}")))?
            .to_vec();

        let format = Self::format_from_content_type(&content_type);

        Ok(TtsSynthesisResult::new(audio, format))
    }

    /// v1 端点 `/` — 对齐 api.py
    async fn try_v1(
        &self,
        text: &str,
        config: &TtsConfig,
        base: &str,
        lang: &str,
    ) -> VivianResult<TtsSynthesisResult> {
        let url = format!("{}/", base.trim_end_matches('/'));

        // v1 字段名与 v2 不同: text_language / prompt_language / refer_wav_path / speed
        let mut body = json!({
            "text": text,
            "text_language": lang,
            "prompt_language": lang,
            "speed": config.rate,
        });

        if let Some(ref_audio) = config.gpt_sovits_ref_audio.as_deref() {
            if !ref_audio.is_empty() {
                body["refer_wav_path"] = json!(ref_audio);
            }
        }
        if let Some(prompt_text) = config.gpt_sovits_prompt_text.as_deref() {
            if !prompt_text.is_empty() {
                body["prompt_text"] = json!(prompt_text);
            }
        }

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| VivianError::Speech(format!("GPT-SoVITS v1 请求失败: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(VivianError::Speech(format!(
                "GPT-SoVITS v1 失败 [{}]: {}",
                status, body
            )));
        }

        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let audio = resp
            .bytes()
            .await
            .map_err(|e| VivianError::Speech(format!("读取 GPT-SoVITS v1 音频失败: {e}")))?
            .to_vec();

        let format = Self::format_from_content_type(&content_type);

        Ok(TtsSynthesisResult::new(audio, format))
    }
}
