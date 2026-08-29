//! Speech Cache — 语音缓存
//!
//! 缓存 key: hash(text + voice + emotion + engine_name)
//! 缓存 value: 音频文件(存放在角色目录下的 cache 子目录)
//!
//! 命中缓存时直接播放本地音频,跳过 TTS 合成请求。
//! 对高频短文本("早安"/"晚安"/"欢迎回来"等)收益显著。
//!
//! 注意:缓存不包含 WordBoundary,命中时唇形同步退化为字符级估算。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::error::{VivianError, VivianResult};

use super::tts_backend::{AudioFormat, TtsSynthesisResult};

/// 语音缓存
pub struct SpeechCache {
    /// 缓存目录
    cache_dir: PathBuf,
    /// 内存索引:key → (文件路径, format)
    index: Arc<RwLock<HashMap<u64, (PathBuf, AudioFormat)>>>,
    /// 最大缓存条目数(LRU 淘汰)
    max_entries: usize,
}

impl SpeechCache {
    /// 创建缓存实例,目录为 `<character_data_dir>/sound/cache/`
    pub fn new(char_id: &str) -> VivianResult<Self> {
        let cache_dir = crate::utils::path::get_character_data_dir(char_id)
            .join("sound")
            .join("cache");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| VivianError::Speech(format!("创建缓存目录失败: {e}")))?;

        let cache = Self {
            cache_dir,
            index: Arc::new(RwLock::new(HashMap::new())),
            max_entries: 200,
        };
        cache.scan_existing()?;
        Ok(cache)
    }

    /// 降级构造：使用系统临时目录作为缓存目录，避免在 Default 路径上 panic。
    ///
    /// 仅在 `SpeechCache::new` 失败（如磁盘满/权限不足）时使用。
    pub fn fallback() -> Self {
        let cache_dir = std::env::temp_dir().join("vivian-tts-cache");
        if let Err(e) = std::fs::create_dir_all(&cache_dir) {
            tracing::warn!(
                "[SpeechCache] fallback 缓存目录创建失败 {:?}: {}，缓存将被禁用",
                cache_dir,
                e
            );
        }
        Self {
            cache_dir,
            index: Arc::new(RwLock::new(HashMap::new())),
            max_entries: 200,
        }
    }

    /// 扫描已有缓存文件,重建内存索引
    fn scan_existing(&self) -> VivianResult<()> {
        let entries = std::fs::read_dir(&self.cache_dir)
            .map_err(|e| VivianError::Speech(format!("读取缓存目录失败: {e}")))?;

        let mut index = self.index.write();
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_file() {
                    let path = entry.path();
                    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                        let format = match ext {
                            "mp3" => AudioFormat::Mp3,
                            "wav" => AudioFormat::Wav,
                            "pcm" => AudioFormat::Pcm,
                            "ogg" => AudioFormat::Ogg,
                            "aac" => AudioFormat::Aac,
                            _ => continue,
                        };
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            if let Ok(hash) = stem.parse::<u64>() {
                                index.insert(hash, (path, format));
                            }
                        }
                    }
                }
            }
        }
        tracing::debug!("[SpeechCache] 扫描到 {} 个缓存文件", index.len());
        Ok(())
    }

    /// 计算缓存 key
    ///
    /// key = hash(text + voice + emotion + engine_name + rate + volume + pitch)
    /// pitch 加入 key 是为了让 Emotion Prosody 不同音高的同文本分开缓存。
    fn compute_key(
        text: &str,
        voice: &str,
        emotion: Option<&str>,
        engine_name: &str,
        rate: f64,
        volume: f64,
        pitch: Option<f64>,
    ) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        voice.hash(&mut hasher);
        emotion.unwrap_or("").hash(&mut hasher);
        engine_name.hash(&mut hasher);
        rate.to_bits().hash(&mut hasher);
        volume.to_bits().hash(&mut hasher);
        pitch.map(|p| p.to_bits()).hash(&mut hasher);
        hasher.finish()
    }

    /// 查询缓存
    ///
    /// 命中时返回 TtsSynthesisResult(从文件读取音频)。
    /// 未命中返回 None。
    pub fn get(
        &self,
        text: &str,
        voice: &str,
        emotion: Option<&str>,
        engine_name: &str,
        rate: f64,
        volume: f64,
        pitch: Option<f64>,
    ) -> Option<TtsSynthesisResult> {
        let key = Self::compute_key(text, voice, emotion, engine_name, rate, volume, pitch);
        let index = self.index.read();
        if let Some((path, format)) = index.get(&key) {
            match std::fs::read(path) {
                Ok(audio) if !audio.is_empty() => {
                    tracing::debug!(
                        "[SpeechCache] 命中缓存: key={} path={}",
                        key,
                        path.display()
                    );
                    return Some(TtsSynthesisResult::new(audio, *format));
                }
                Ok(_) => {
                    tracing::warn!("[SpeechCache] 缓存文件为空: {}", path.display());
                }
                Err(e) => {
                    tracing::warn!(
                        "[SpeechCache] 读取缓存文件失败: {} err={}",
                        path.display(),
                        e
                    );
                }
            }
        }
        None
    }

    /// 写入缓存
    ///
    /// 将合成结果写入缓存文件,并更新内存索引。
    pub fn put(
        &self,
        text: &str,
        voice: &str,
        emotion: Option<&str>,
        engine_name: &str,
        rate: f64,
        volume: f64,
        pitch: Option<f64>,
        result: &TtsSynthesisResult,
    ) {
        let key = Self::compute_key(text, voice, emotion, engine_name, rate, volume, pitch);
        let ext = match result.format {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Wav => "wav",
            AudioFormat::Pcm => "pcm",
            AudioFormat::Ogg => "ogg",
            AudioFormat::Aac => "aac",
        };
        let filename = format!("{}.{}", key, ext);
        let path = self.cache_dir.join(&filename);

        match std::fs::write(&path, &result.audio) {
            Ok(()) => {
                let mut index = self.index.write();
                // LRU 淘汰:超过上限时删除最旧的文件
                if index.len() >= self.max_entries && !index.contains_key(&key) {
                    if let Some((&old_key, (old_path, _))) = index.iter().next() {
                        let _ = std::fs::remove_file(old_path);
                        index.remove(&old_key);
                    }
                }
                index.insert(key, (path, result.format));
                tracing::debug!("[SpeechCache] 写入缓存: key={} entries={}", key, index.len());
            }
            Err(e) => {
                tracing::warn!("[SpeechCache] 写入缓存失败: {}", e);
            }
        }
    }

    /// 清空缓存
    pub fn clear(&self) -> VivianResult<()> {
        let mut index = self.index.write();
        for (_, (path, _)) in index.drain() {
            let _ = std::fs::remove_file(&path);
        }
        Ok(())
    }
}
