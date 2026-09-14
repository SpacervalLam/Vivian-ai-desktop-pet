//! 嵌入模型注册表
//!
//! 集中管理内置已知嵌入模型的元数据（模型 ID、向量维度、来源、展示名），
//! 供 `build_embedding` 在构造嵌入服务时自动校正向量维度，也供前端设置表单
//! 在用户选择模型时自动填充维度，避免"维度填错导致索引反复重建"。
//!
//! 注册表仅提供"已知默认值"；用户自定义模型（不在表中）时回退到配置里显式填写的维度。

use serde::Serialize;

/// 嵌入模型来源
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingSource {
    /// 云端 OpenAI 兼容接口
    Cloud,
    /// 本地 Ollama
    Local,
}

/// 一条嵌入模型规格
#[derive(Debug, Clone, Serialize)]
pub struct EmbeddingModelSpec {
    /// 模型 ID（与配置 `memory.embedding.model` / `ollama_model` 匹配）
    pub id: &'static str,
    /// 向量维度
    pub dimension: usize,
    /// 模型来源（用于前端区分 cloud / local 预设）
    pub source: EmbeddingSource,
    /// 展示名（可含提供商前缀）
    pub display_name: &'static str,
}

/// 内置模型注册表（按来源分组）
const CLOUD_MODELS: &[EmbeddingModelSpec] = &[
    EmbeddingModelSpec {
        id: "BAAI/bge-m3",
        dimension: 1024,
        source: EmbeddingSource::Cloud,
        display_name: "BAAI/bge-m3 (1024)",
    },
    EmbeddingModelSpec {
        id: "BAAI/bge-large-zh-v1.5",
        dimension: 1024,
        source: EmbeddingSource::Cloud,
        display_name: "BAAI/bge-large-zh-v1.5 (1024)",
    },
    EmbeddingModelSpec {
        id: "BAAI/bge-small-zh-v1.5",
        dimension: 512,
        source: EmbeddingSource::Cloud,
        display_name: "BAAI/bge-small-zh-v1.5 (512)",
    },
    EmbeddingModelSpec {
        id: "text-embedding-3-small",
        dimension: 1536,
        source: EmbeddingSource::Cloud,
        display_name: "OpenAI text-embedding-3-small (1536)",
    },
    EmbeddingModelSpec {
        id: "text-embedding-3-large",
        dimension: 3072,
        source: EmbeddingSource::Cloud,
        display_name: "OpenAI text-embedding-3-large (3072)",
    },
];

const LOCAL_MODELS: &[EmbeddingModelSpec] = &[
    EmbeddingModelSpec {
        id: "bge-m3",
        dimension: 1024,
        source: EmbeddingSource::Local,
        display_name: "bge-m3 (1024)",
    },
    EmbeddingModelSpec {
        id: "nomic-embed-text",
        dimension: 768,
        source: EmbeddingSource::Local,
        display_name: "nomic-embed-text (768)",
    },
    EmbeddingModelSpec {
        id: "bge-large-zh-v1.5",
        dimension: 1024,
        source: EmbeddingSource::Local,
        display_name: "bge-large-zh-v1.5 (1024)",
    },
    EmbeddingModelSpec {
        id: "bge-small-zh-v1.5",
        dimension: 512,
        source: EmbeddingSource::Local,
        display_name: "bge-small-zh-v1.5 (512)",
    },
];

/// 按模型 ID 精确匹配（忽略空白与大小写差异不敏感的部分）
pub fn lookup(id: &str) -> Option<&'static EmbeddingModelSpec> {
    let trimmed = id.trim();
    CLOUD_MODELS
        .iter()
        .chain(LOCAL_MODELS.iter())
        .find(|s| s.id == trimmed)
}

/// 解析某模型的真实维度；未知模型返回 `None`（由调用方回退到配置值）
pub fn resolve_dimension(id: &str) -> Option<usize> {
    lookup(id).map(|s| s.dimension)
}

/// 返回全部已知模型元数据（供前端渲染候选项）
pub fn all_models() -> Vec<EmbeddingModelSpec> {
    CLOUD_MODELS
        .iter()
        .chain(LOCAL_MODELS.iter())
        .cloned()
        .collect()
}

/// 对配置中的维度做校正：
/// - 配置维度为 0（未填）且模型已知 → 采用模型真实维度
/// - 配置维度与已知模型不符 → 采用模型真实维度并打 WARN（避免维度错配反复触发索引重建）
/// - 其它情况（未知模型）→ 保留配置值
pub fn normalize_dimension(model_id: &str, configured: usize) -> usize {
    let Some(real) = resolve_dimension(model_id) else {
        return configured;
    };
    if configured == real {
        return real;
    }
    if configured == 0 {
        tracing::info!(
            "[EmbeddingRegistry] 模型 {} 未填维度，自动采用 {}",
            model_id,
            real
        );
    } else {
        tracing::warn!(
            "[EmbeddingRegistry] 模型 {} 配置维度 {} 与已知维度 {} 不符，自动校正",
            model_id,
            configured,
            real
        );
    }
    real
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_known_models() {
        assert_eq!(lookup("bge-m3").map(|s| s.dimension), Some(1024));
        assert_eq!(lookup("nomic-embed-text").map(|s| s.dimension), Some(768));
        assert_eq!(
            lookup("text-embedding-3-small").map(|s| s.dimension),
            Some(1536)
        );
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup("my-custom-model").is_none());
    }

    #[test]
    fn normalize_corrects_wrong_dim() {
        assert_eq!(normalize_dimension("bge-m3", 768), 1024);
        assert_eq!(normalize_dimension("bge-m3", 0), 1024);
        assert_eq!(normalize_dimension("bge-m3", 1024), 1024);
    }

    #[test]
    fn normalize_keeps_unknown() {
        assert_eq!(normalize_dimension("my-custom", 64), 64);
    }
}