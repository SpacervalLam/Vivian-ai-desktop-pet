//! 本地活动分类器 —— 从前台窗口快照直接推断用户活动标签。
//!
//! 两层策略（A → B → None）：
//!
//! - **A: 精确进程名映射表**：已知进程名 → 细粒度活动标签，O(1) HashMap 查找，置信度 0.85~0.95
//! - **B: 嵌入分类器**：对未知进程名，用嵌入模型（对齐 live2D 表情的语料库思想）
//!   将窗口标题嵌入后，与丰富的活动语料库做 Top-K softmax 投票，置信度由票占比映射。
//!   语料库见 `activity_corpus`。
//! - **None**：两层都未命中时，留给 LLM 反思阶段补充
//!
//! 与 `proactive::activity_journal::classify_window_title` 共享分类逻辑，
//! 但此处输出完整的细粒度中文活动标签（2~4 字）而非粗粒度分类，直接供 `UserBehaviorLog` 消费。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::LazyLock;

use parking_lot::Mutex;

use crate::memory::embedding::{HashingMemoryEmbedding, MemoryEmbeddingProvider};
use crate::world::activity_corpus::{ActivityCorpusEntry, ACTIVITY_CORPUS};
use crate::world::foreground_window::ForegroundWindowSnapshot;

/// 批量插入进程名 → 标签映射的辅助宏。
/// 避免 `for p in &[...]` 产生 `&&str` 类型不匹配。
macro_rules! insert_many {
    ($map:expr, $label:expr, $conf:expr, $($name:expr),+ $(,)?) => {
        $(
            $map.insert($name, ($label, $conf));
        )+
    };
}

// ══════════════════════════════════════════════════════════════════════
// A: 精确进程名映射表
// ══════════════════════════════════════════════════════════════════════

/// 构建进程名 → (细粒度活动标签, 置信度) 映射表。
/// 键为去掉了 `.exe` 后缀的全小写进程名。
fn build_process_map() -> HashMap<&'static str, (&'static str, f64)> {
    let mut m = HashMap::new();

    // ── 写代码 ──
    insert_many!(m, "写代码", 0.95,
        "code", "cursor", "windsurf", "idea", "idea64", "pycharm",
        "pycharm64", "rust-rover", "goland", "webstorm", "clion",
        "devenv", "vim", "gvim", "nvim", "neovim", "emacs",
        "sublime_text", "subl", "android-studio", "androidstudio",
        "xcode", "intellij",
    );

    // ── 终端命令 ──
    insert_many!(m, "终端命令", 0.95,
        "terminal", "windows-terminal", "wt", "powershell",
        "cmd", "git-bash", "bash", "zsh", "kitty", "alacritty",
    );

    // ── 浏览网页 ──
    insert_many!(m, "浏览网页", 0.90,
        "chrome", "firefox", "msedge", "edge", "safari", "opera",
        "brave", "arc", "browser", "vivaldi", "tor",
    );

    // ── 玩游戏 ──
    insert_many!(m, "玩游戏", 0.90,
        "steam", "epic", "epicgameslauncher", "battle.net", "battlenet",
        "origin", "ea", "eaapp", "gog", "galaxy", "minecraft",
        "valorant", "league", "league of legends", "lol",
    );

    // ── 看视频 ──
    insert_many!(m, "看视频", 0.90,
        "bilibili", "potplayer", "vlc", "mpv", "mpc-hc", "netflix",
        "youtube", "iqiyi", "腾讯视频",
    );

    // ── 聊天 ──
    insert_many!(m, "聊天", 0.90,
        "wechat", "weixin", "qq", "telegram", "discord", "slack",
        "teams", "skype", "dingtalk", "line", "whatsapp",
    );

    // ── 写文档 ──
    insert_many!(m, "写文档", 0.90,
        "winword", "word", "wps", "wpp", "onenote", "libreoffice",
    );

    // ── 表格处理 ──
    insert_many!(m, "表格处理", 0.90,
        "excel", "et", "libreoffice calc",
    );

    // ── 演示文稿 ──
    insert_many!(m, "演示文稿", 0.90,
        "powerpnt", "powerpoint", "wpp",
    );

    // ── 记笔记 ──
    insert_many!(m, "记笔记", 0.85,
        "obsidian", "marktext", "typora", "logseq", "notion", "joplin",
    );

    // ── 处理邮件 ──
    insert_many!(m, "处理邮件", 0.90,
        "outlook", "foxmail", "thunderbird", "mail",
    );

    // ── 图片设计 ──
    insert_many!(m, "图片设计", 0.85,
        "photoshop", "photoshop.exe", "illustrator", "figma",
        "krita", "clipstudio",
    );

    // ── 视频剪辑 ──
    insert_many!(m, "视频剪辑", 0.85,
        "premiere", "afterfx", "after effects", "davinci-resolve",
    );

    // ── 听音乐 ──
    insert_many!(m, "听音乐", 0.85,
        "spotify", "foobar2000", "music",
    );

    // ── 文件管理 / 系统设置 ──
    insert_many!(m, "文件管理", 0.85,
        "explorer", "files", "everything",
    );
    insert_many!(m, "系统设置", 0.85,
        "taskmgr", "control", "regedit", "msconfig", "perfmon", "resmon",
    );

    m
}

static PROCESS_MAP: LazyLock<HashMap<&'static str, (&'static str, f64)>> =
    LazyLock::new(build_process_map);

/// 通过精确进程名匹配推断活动（A 层）。
fn classify_by_process(process: &str) -> Option<(&'static str, f64)> {
    let p = process.trim().to_lowercase();
    // 先试去掉 .exe 后缀
    if let Some(stripped) = p.strip_suffix(".exe") {
        if let Some(result) = PROCESS_MAP.get(stripped) {
            return Some(*result);
        }
    }
    // 再试原样
    PROCESS_MAP.get(p.as_str()).copied()
}

// ══════════════════════════════════════════════════════════════════════
// B: 嵌入分类器（对齐 live2D 表情的语料库 + Top-K softmax 投票）
// ══════════════════════════════════════════════════════════════════════

/// 嵌入 Top-K
const TOP_K: usize = 5;
/// 相似度阈值（低于此值视为噪声，不再纳入投票）
const SIMILARITY_THRESHOLD: f32 = 0.45;
/// softmax 温度
const SOFTMAX_TEMPERATURE: f32 = 0.1;
/// 查询缓存容量（LRU）
const QUERY_CACHE_CAPACITY: usize = 64;

/// 活动嵌入分类器
///
/// 结构与 `EmbeddingEmotionClassifier` 对齐：将语料库条目嵌入后，对查询文本
/// 做 Top-K softmax 加权投票，取票数最高的活动标签。使用本地 `HashingMemoryEmbedding`
/// （jieba 分词 + 特征哈希）保证前台窗口回调路径零网络、毫秒级实时。
struct ActivityEmbeddingClassifier {
    provider: Arc<dyn MemoryEmbeddingProvider>,
    corpus: Vec<ActivityCorpusEntry>,
    /// 语料嵌入（懒初始化）
    corpus_embeddings: Mutex<Option<Vec<Vec<f32>>>>,
    /// 查询缓存（LRU）
    query_cache: Mutex<VecDeque<(String, Option<(String, f64)>)>>,
}

impl ActivityEmbeddingClassifier {
    fn new() -> Self {
        Self {
            provider: Arc::new(HashingMemoryEmbedding::default()),
            corpus: ACTIVITY_CORPUS.to_vec(),
            corpus_embeddings: Mutex::new(None),
            query_cache: Mutex::new(VecDeque::with_capacity(QUERY_CACHE_CAPACITY)),
        }
    }

    /// 分类入口：返回 `(活动标签, 置信度)`，未命中返回 None。
    fn classify(&self, text: &str) -> Option<(String, f64)> {
        let trimmed = text.trim();
        if trimmed.len() < 3 {
            return None; // 太短，无有效特征
        }

        // 1. 查询缓存
        if let Some(cached) = self.get_cached(trimmed) {
            return cached;
        }

        // 2. 确保语料已嵌入（懒初始化）
        self.ensure_initialized();

        // 3. 嵌入查询文本
        let query_emb = self.provider.embed(trimmed).ok()?;

        // 4. Top-K softmax 投票
        let result = self.classify_by_embedding(&query_emb);

        // 5. 写入缓存
        self.put_cache(trimmed.to_string(), result.clone());

        result
    }

    /// 通过嵌入向量分类（Top-K softmax 加权投票）
    fn classify_by_embedding(&self, query_emb: &[f32]) -> Option<(String, f64)> {
        let corpus_embeddings = self.corpus_embeddings.lock();
        let embeddings = corpus_embeddings.as_ref()?;

        // 计算与所有语料的余弦相似度，取 Top-K
        let mut sims: Vec<(usize, f32)> = embeddings
            .iter()
            .enumerate()
            .map(|(i, emb)| (i, cosine_similarity(query_emb, emb)))
            .collect();
        sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let top_k: Vec<(usize, f32)> = sims.into_iter().take(TOP_K).collect();

        let dominant_sim = top_k.first().map(|(_, s)| *s).unwrap_or(0.0);

        // 最高相似度低于阈值 → 未命中
        if dominant_sim < SIMILARITY_THRESHOLD {
            return None;
        }

        // softmax 加权投票：weight = exp(sim / temperature)
        let mut votes: HashMap<&'static str, f32> = HashMap::new();
        let mut total_weight: f32 = 0.0;
        for (idx, sim) in &top_k {
            if *sim < SIMILARITY_THRESHOLD {
                break;
            }
            let activity = self.corpus[*idx].activity;
            let weight = (sim / SOFTMAX_TEMPERATURE).exp();
            *votes.entry(activity).or_insert(0.0) += weight;
            total_weight += weight;
        }

        // 按票数降序排列
        let mut sorted_votes: Vec<(&'static str, f32)> = votes.into_iter().collect();
        sorted_votes.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let (winner_activity, winner_weight) = sorted_votes.first().copied()?;

        // 置信度：最高票占总票比例
        let confidence = if total_weight > 0.0 {
            (winner_weight / total_weight).clamp(0.0, 1.0)
        } else {
            0.0
        };

        tracing::trace!(
            "[ActivityClassifier] B: Top-K 投票 → {} (confidence={:.3}, sim={:.3})",
            winner_activity,
            confidence,
            dominant_sim,
        );

        Some((winner_activity.to_string(), confidence as f64))
    }

    /// 懒初始化语料嵌入（幂等）。本地嵌入不会失败，失败时保持未初始化。
    fn ensure_initialized(&self) {
        if self.corpus_embeddings.lock().is_some() {
            return;
        }
        let texts: Vec<String> = self.corpus.iter().map(|e| e.text.to_string()).collect();
        match self.provider.embed_batch(&texts) {
            Ok(embeddings) => {
                tracing::debug!(
                    "[ActivityClassifier] 活动语料已嵌入: {} 条, model={}",
                    embeddings.len(),
                    self.provider.model_id()
                );
                *self.corpus_embeddings.lock() = Some(embeddings);
            }
            Err(e) => {
                tracing::warn!("[ActivityClassifier] 活动语料嵌入失败: {}", e);
            }
        }
    }

    /// 从缓存读取（命中时移到尾部实现 LRU）
    fn get_cached(&self, text: &str) -> Option<Option<(String, f64)>> {
        let mut cache = self.query_cache.lock();
        if let Some(pos) = cache.iter().position(|(t, _)| t == text) {
            let (key, result) = cache.remove(pos).unwrap();
            cache.push_back((key, result.clone()));
            Some(result)
        } else {
            None
        }
    }

    /// 写入缓存（LRU 淘汰）
    fn put_cache(&self, text: String, result: Option<(String, f64)>) {
        let mut cache = self.query_cache.lock();
        if cache.len() >= QUERY_CACHE_CAPACITY {
            cache.pop_front();
        }
        cache.push_back((text, result));
    }
}

/// 全局活动嵌入分类器实例（懒初始化）
static EMBEDDING_CLASSIFIER: LazyLock<ActivityEmbeddingClassifier> =
    LazyLock::new(ActivityEmbeddingClassifier::new);

/// 计算两个向量的余弦相似度。
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..n {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let norm = (na * nb).sqrt();
    if norm > 0.0 {
        dot / norm
    } else {
        0.0
    }
}

// ══════════════════════════════════════════════════════════════════════
// 公共入口
// ══════════════════════════════════════════════════════════════════════

/// 从前台窗口快照推断用户活动。
///
/// 策略优先级：
/// 1. **A（精确进程名匹配）**：已知进程名 → 直接返回，置信度 0.85~0.95
/// 2. **B（嵌入分类）**：对未知进程名，用嵌入模型 + 丰富活动语料库做 Top-K
///    softmax 投票，置信度由最高票占比给出
/// 3. **None**：两层都未命中，留给 LLM 反思阶段补充
pub fn classify_foreground_activity(fw: &ForegroundWindowSnapshot) -> Option<(String, f64)> {
    // A 层：精确进程名匹配
    if !fw.process.is_empty() {
        if let Some((label, confidence)) = classify_by_process(&fw.process) {
            tracing::trace!(
                "[ActivityClassifier] A: 进程名「{}」→ {} (confidence={})",
                fw.process,
                label,
                confidence
            );
            return Some((label.to_string(), confidence));
        }
    }

    // B 层：嵌入分类（丰富语料库 + Top-K softmax 投票）
    if !fw.title.is_empty() {
        if let Some((label, confidence)) = EMBEDDING_CLASSIFIER.classify(&fw.title) {
            return Some((label, confidence));
        }
    }

    // 两层都未命中
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::foreground_window::ForegroundWindowSnapshot;

    fn fw(title: &str, process: &str) -> ForegroundWindowSnapshot {
        ForegroundWindowSnapshot {
            title: title.to_string(),
            process: process.to_string(),
            pid: 12345,
        }
    }

    // ── A 层测试：精确进程名匹配 ──

    #[test]
    fn test_a_coding_exact() {
        let result = classify_foreground_activity(&fw("ignored title", "Code.exe"));
        assert_eq!(result.as_ref().map(|(l, _)| l.as_str()), Some("写代码"));
        assert_eq!(result.unwrap().1, 0.95);
    }

    #[test]
    fn test_a_coding_no_ext() {
        let result = classify_foreground_activity(&fw("ignored title", "cursor"));
        assert_eq!(result.as_ref().map(|(l, _)| l.as_str()), Some("写代码"));
        assert_eq!(result.unwrap().1, 0.95);
    }

    #[test]
    fn test_a_browser() {
        let result = classify_foreground_activity(&fw("ignored title", "chrome.exe"));
        assert_eq!(result.as_ref().map(|(l, _)| l.as_str()), Some("浏览网页"));
        assert_eq!(result.unwrap().1, 0.90);
    }

    #[test]
    fn test_a_chat() {
        let result = classify_foreground_activity(&fw("ignored title", "WeChat.exe"));
        assert_eq!(result.as_ref().map(|(l, _)| l.as_str()), Some("聊天"));
    }

    #[test]
    fn test_a_game() {
        let result = classify_foreground_activity(&fw("ignored title", "steam.exe"));
        assert_eq!(result.as_ref().map(|(l, _)| l.as_str()), Some("玩游戏"));
    }

    #[test]
    fn test_a_terminal() {
        let result = classify_foreground_activity(&fw(
            "Administrator: Windows PowerShell",
            "powershell.exe",
        ));
        assert_eq!(result.as_ref().map(|(l, _)| l.as_str()), Some("终端命令"));
    }

    #[test]
    fn test_a_notes() {
        let result = classify_foreground_activity(&fw("ignored title", "obsidian.exe"));
        assert_eq!(result.as_ref().map(|(l, _)| l.as_str()), Some("记笔记"));
    }

    // ── B 层测试：嵌入分类 ──

    #[test]
    fn test_b_unknown_process_familiar_title() {
        // 未知进程名，但标题明显是编程场景
        let result = classify_foreground_activity(&fw(
            "src/main.rs - MyEditor",
            "myeditor.exe",
        ));
        assert_eq!(result.as_ref().map(|(l, _)| l.as_str()), Some("写代码"));
    }

    #[test]
    fn test_b_unknown_process_chinese_title() {
        // 未知进程名，标题含中文聊天特征
        let result = classify_foreground_activity(&fw(
            "微信群聊(3)",
            "unknown.exe",
        ));
        assert_eq!(result.as_ref().map(|(l, _)| l.as_str()), Some("聊天"));
    }

    #[test]
    fn test_b_unknown_process_video() {
        let result = classify_foreground_activity(&fw(
            "【合集】新番推荐 - 哔哩哔哩",
            "browser.exe",
        ));
        assert_eq!(result.as_ref().map(|(l, _)| l.as_str()), Some("看视频"));
    }

    #[test]
    fn test_b_unknown_process_music() {
        let result = classify_foreground_activity(&fw(
            "我的歌单 - 网易云音乐",
            "musicplayer.exe",
        ));
        assert_eq!(result.as_ref().map(|(l, _)| l.as_str()), Some("听音乐"));
    }

    #[test]
    fn test_b_unknown_process_shopping() {
        let result = classify_foreground_activity(&fw(
            "购物车 - 淘宝",
            "webapp.exe",
        ));
        assert_eq!(result.as_ref().map(|(l, _)| l.as_str()), Some("网上购物"));
    }

    // ── 完全未知的窗口 ──

    #[test]
    fn test_short_title() {
        // 短标题无法提取有效特征
        let result = classify_foreground_activity(&fw("Hi", "unknown.exe"));
        assert!(result.is_none());
    }
}