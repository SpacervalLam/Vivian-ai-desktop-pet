//! Takes 围栏表格 — append-only + supersede 语义
//!
//! 设计：
//! - 每个实体/主题对应一个 TakesTable（围栏表格）
//! - 行只能追加，永不删除（append-only）
//! - 新事实与旧事实冲突时，旧事实标记 superseded（渲染时加 ~~strikethrough~~），
//!   新事实追加到表末尾，row_num 永不复用
//! - 每行携带 source_memory_id 溯源
//!
//! 实现：内存 Vec + JSON 持久化；supersede 判定用 claim 归一化后字符串匹配做初筛。
//!   （未来可扩展为 LLM 仲裁）

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 单行 Take 状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TakeStatus {
    /// 当前有效
    Active,
    /// 已被新行取代
    Superseded,
}

/// Takes 表格中的一行
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TakesRow {
    /// 行号（从 1 开始，永不复用）
    pub row_num: u32,
    /// 主体实体名（表格归属键）
    pub subject: String,
    /// 声明（claim，如"职位"/"所在地"/"状态"）
    pub claim: String,
    /// 声明的值（如"CEO"/"北京"/"已离职"）
    pub value: String,
    /// 行状态
    pub status: TakeStatus,
    /// 来源记忆 ID（溯源）
    pub source_memory_id: String,
    /// 创建时间戳
    pub created_at: f64,
    /// 被取代时间戳（None 表示未取代）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_at: Option<f64>,
    /// 被哪一行取代（None 表示未取代）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<u32>,
}

/// 单个实体/主题的 Takes 表
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TakesTable {
    /// 主体名
    pub subject: String,
    /// 所有行（按 row_num 升序，append-only）
    pub rows: Vec<TakesRow>,
    /// 下一个可分配的 row_num
    pub next_row_num: u32,
}

impl TakesTable {
    pub fn new(subject: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            rows: Vec::new(),
            next_row_num: 1,
        }
    }

    /// 追加一行 take，若与已有 active 行的 (claim) 相同则自动 supersede 旧行
    ///
    /// 返回新行的 row_num。
    pub fn add_take(
        &mut self,
        claim: impl Into<String>,
        value: impl Into<String>,
        source_memory_id: impl Into<String>,
        created_at: f64,
    ) -> u32 {
        let claim = claim.into();
        let value = value.into();
        let source_memory_id = source_memory_id.into();

        // 查找同 claim 的 active 行，标记 superseded
        let new_row_num = self.next_row_num;
        for row in &mut self.rows {
            if row.status == TakeStatus::Active && normalize_claim(&row.claim) == normalize_claim(&claim) {
                row.status = TakeStatus::Superseded;
                row.superseded_at = Some(created_at);
                row.superseded_by = Some(new_row_num);
            }
        }

        // 追加新行
        self.rows.push(TakesRow {
            row_num: new_row_num,
            subject: self.subject.clone(),
            claim,
            value,
            status: TakeStatus::Active,
            source_memory_id,
            created_at,
            superseded_at: None,
            superseded_by: None,
        });
        self.next_row_num += 1;
        new_row_num
    }

    /// 获取当前所有 active 行
    pub fn active_takes(&self) -> Vec<&TakesRow> {
        self.rows
            .iter()
            .filter(|r| r.status == TakeStatus::Active)
            .collect()
    }

    /// 渲染为 Markdown 围栏表格
    ///
    /// superseded 行加 ~~strikethrough~~，active 行正常显示。
    /// 列：row_num | claim | value | status | source_memory_id
    pub fn render_markdown(&self) -> String {
        if self.rows.is_empty() {
            return String::new();
        }

        let mut out = String::new();
        out.push_str(&format!("### Takes: {}\n\n", self.subject));
        out.push_str("| # | claim | value | status | source |\n");
        out.push_str("|---|-------|-------|--------|--------|\n");

        for row in &self.rows {
            let claim_display = if row.status == TakeStatus::Superseded {
                format!("~~{}~~", row.claim)
            } else {
                row.claim.clone()
            };
            let value_display = if row.status == TakeStatus::Superseded {
                format!("~~{}~~", row.value)
            } else {
                row.value.clone()
            };
            let status_str = match row.status {
                TakeStatus::Active => "active",
                TakeStatus::Superseded => "superseded",
            };
            let source_short = &row.source_memory_id[..row.source_memory_id.len().min(12)];
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                row.row_num, claim_display, value_display, status_str, source_short
            ));
        }

        out
    }

    /// 渲染为紧凑文本（用于注入 LLM prompt）
    ///
    /// 只返回 active 行，格式：`[row=N] claim=value`
    pub fn render_compact(&self) -> String {
        let active: Vec<&TakesRow> = self.active_takes();
        if active.is_empty() {
            return String::new();
        }
        active
            .iter()
            .map(|r| format!("[row={}] {}={}", r.row_num, r.claim, r.value))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// claim 归一化：去空白、转小写，用于 supersede 匹配
fn normalize_claim(claim: &str) -> String {
    claim.trim().to_lowercase().replace([' ', '\t', '\n'], "")
}

/// Takes 围栏表格仓库 — 管理多个主体的表
pub struct TakesFence {
    inner: Arc<RwLock<TakesFenceData>>,
    store_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TakesFenceData {
    /// schema 版本
    version: u32,
    /// 主体名 → TakesTable
    tables: HashMap<String, TakesTable>,
}

impl TakesFenceData {
    fn new() -> Self {
        Self {
            version: 1,
            tables: HashMap::new(),
        }
    }
}

impl TakesFence {
    pub fn new(store_path: PathBuf) -> Self {
        let mut data = TakesFenceData::new();
        if store_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&store_path) {
                if let Ok(loaded) = serde_json::from_str::<TakesFenceData>(&content) {
                    data = loaded;
                    tracing::info!(
                        "[TakesFence] 加载 {} 个主体的 takes 表",
                        data.tables.len()
                    );
                }
            }
        }

        Self {
            inner: Arc::new(RwLock::new(data)),
            store_path,
        }
    }

    /// 添加一条 take，自动 supersede 同 claim 的旧 active 行
    ///
    /// 返回新行的 row_num。
    pub fn add_take(
        &self,
        subject: &str,
        claim: &str,
        value: &str,
        source_memory_id: &str,
        created_at: f64,
    ) -> u32 {
        let mut data = self.inner.write();
        let table = data
            .tables
            .entry(subject.to_string())
            .or_insert_with(|| TakesTable::new(subject));
        table.add_take(claim, value, source_memory_id, created_at)
    }

    /// 获取指定主体的 active takes（紧凑文本）
    pub fn render_compact(&self, subject: &str) -> String {
        let data = self.inner.read();
        data.tables
            .get(subject)
            .map(|t| t.render_compact())
            .unwrap_or_default()
    }

    /// 获取指定主体的完整 Markdown 表格
    pub fn render_markdown(&self, subject: &str) -> String {
        let data = self.inner.read();
        data.tables
            .get(subject)
            .map(|t| t.render_markdown())
            .unwrap_or_default()
    }

    /// 获取指定主体的 active 行
    pub fn active_takes(&self, subject: &str) -> Vec<TakesRow> {
        let data = self.inner.read();
        data.tables
            .get(subject)
            .map(|t| t.active_takes().into_iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 从记忆内容中提取 takes 并写入
    ///
    /// 简单策略：把记忆内容作为 subject 的一个 claim="mentioned" take。
    /// 未来可扩展为 LLM 抽取结构化 claim/value。
    pub fn ingest_from_memory(
        &self,
        subject: &str,
        content: &str,
        memory_id: &str,
        timestamp: f64,
    ) -> u32 {
        // 简单实现：把整条内容作为 "mentioned" take 的 value
        // 未来可用 LLM 抽取 (claim, value) 对
        let value = if content.chars().count() > 80 {
            format!("{}…", &content[..content.char_indices().take(80).last().map(|(i, _)| i).unwrap_or(content.len())])
        } else {
            content.to_string()
        };
        self.add_take(subject, "mentioned", &value, memory_id, timestamp)
    }

    /// 列出所有主体名
    pub fn list_subjects(&self) -> Vec<String> {
        self.inner.read().tables.keys().cloned().collect()
    }

    /// 主体数量
    pub fn subject_count(&self) -> usize {
        self.inner.read().tables.len()
    }

    /// 总行数（含 superseded）
    pub fn total_rows(&self) -> usize {
        self.inner
            .read()
            .tables
            .values()
            .map(|t| t.rows.len())
            .sum()
    }

    /// 持久化到磁盘
    pub fn save_to_disk(&self) -> Result<(), String> {
        let data = self.inner.read();
        let content = serde_json::to_string_pretty(&*data)
            .map_err(|e| format!("序列化 takes 表失败: {e}"))?;
        let tmp_path = self.store_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, content)
            .map_err(|e| format!("写入临时文件失败: {e}"))?;
        std::fs::rename(&tmp_path, &self.store_path)
            .map_err(|e| format!("重命名文件失败: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn make_test_fence() -> TakesFence {
        let path = temp_dir().join(format!("test_takes_{}.json", uuid::Uuid::new_v4()));
        TakesFence::new(path)
    }

    #[test]
    fn test_add_take_basic() {
        let fence = make_test_fence();
        let row_num = fence.add_take("腾讯", "职位", "CEO", "mem1", 1000.0);
        assert_eq!(row_num, 1);

        let active = fence.active_takes("腾讯");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].value, "CEO");
        assert_eq!(active[0].status, TakeStatus::Active);
    }

    #[test]
    fn test_supersede_same_claim() {
        let fence = make_test_fence();
        fence.add_take("腾讯", "职位", "CEO", "mem1", 1000.0);
        let row2 = fence.add_take("腾讯", "职位", "CTO", "mem2", 2000.0);

        // 旧行应被 supersede
        let active = fence.active_takes("腾讯");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].row_num, row2);
        assert_eq!(active[0].value, "CTO");
    }

    #[test]
    fn test_different_claims_coexist() {
        let fence = make_test_fence();
        fence.add_take("腾讯", "职位", "CEO", "mem1", 1000.0);
        fence.add_take("腾讯", "所在地", "深圳", "mem2", 2000.0);

        let active = fence.active_takes("腾讯");
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn test_row_num_never_reused() {
        let fence = make_test_fence();
        let r1 = fence.add_take("X", "claim", "v1", "mem1", 1000.0);
        let r2 = fence.add_take("X", "claim", "v2", "mem2", 2000.0);
        let r3 = fence.add_take("X", "claim", "v3", "mem3", 3000.0);

        assert_eq!(r1, 1);
        assert_eq!(r2, 2);
        assert_eq!(r3, 3);

        // 只有最后一行 active
        let active = fence.active_takes("X");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].row_num, 3);
    }

    #[test]
    fn test_render_compact() {
        let fence = make_test_fence();
        fence.add_take("腾讯", "职位", "CEO", "mem1", 1000.0);
        fence.add_take("腾讯", "所在地", "深圳", "mem2", 2000.0);

        let compact = fence.render_compact("腾讯");
        assert!(compact.contains("职位=CEO"));
        assert!(compact.contains("所在地=深圳"));
    }

    #[test]
    fn test_render_markdown() {
        let fence = make_test_fence();
        fence.add_take("腾讯", "职位", "CEO", "mem1", 1000.0);
        fence.add_take("腾讯", "职位", "CTO", "mem2", 2000.0);

        let md = fence.render_markdown("腾讯");
        assert!(md.contains("~~CEO~~")); // 旧行被划线
        assert!(md.contains("CTO")); // 新行正常
        assert!(md.contains("superseded"));
        assert!(md.contains("active"));
    }

    #[test]
    fn test_normalize_claim() {
        assert_eq!(normalize_claim("  职位  "), "职位");
        assert_eq!(normalize_claim("Job Title"), "jobtitle");
        assert_eq!(normalize_claim("职位\n"), "职位");
    }

    #[test]
    fn test_nonexistent_subject() {
        let fence = make_test_fence();
        assert!(fence.active_takes("不存在").is_empty());
        assert_eq!(fence.render_compact("不存在"), "");
    }

    #[test]
    fn test_save_and_load() {
        let path = temp_dir().join(format!("test_takes_save_{}.json", uuid::Uuid::new_v4()));
        {
            let fence = TakesFence::new(path.clone());
            fence.add_take("腾讯", "职位", "CEO", "mem1", 1000.0);
            fence.save_to_disk().unwrap();
        }
        let fence2 = TakesFence::new(path.clone());
        assert_eq!(fence2.subject_count(), 1);
        let active = fence2.active_takes("腾讯");
        assert_eq!(active.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_ingest_from_memory() {
        let fence = make_test_fence();
        let row = fence.ingest_from_memory("腾讯", "马化腾是创始人", "mem1", 1000.0);
        assert!(row >= 1);
        let active = fence.active_takes("腾讯");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].claim, "mentioned");
    }
}
