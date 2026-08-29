//! 语料嵌入磁盘缓存
//!
//! 情绪分类（1680 条）与快速语义（4 维共 152 条）的语料是编译期常量，
//! 嵌入结果只依赖 (model_id, dim, 语料文本)，与运行状态无关——
//! 却因仅存内存而在每次启动时全量重嵌（bge-m3 下约 20-25 秒/角色）。
//! 此模块把嵌入结果落盘，键命中时直接加载，启动跳过全部嵌入调用。
//!
//! 缓存文件格式（小端）：
//! ```text
//! magic:   b"VECB"      4 bytes
//! version: u32          1
//! key:     u64          语料指纹（model + dim + 全部文本）
//! count:   u32          嵌入条数
//! dim:     u32          向量维度
//! data:    count × dim × f32
//! ```
//!
//! 写入采用临时文件 + rename 原子替换；损坏/不匹配的缓存按未命中处理，
//! 回退到实时嵌入并覆盖重写，任何失败都不阻塞主流程。

use std::path::PathBuf;

use sha2::{Digest, Sha256};

const MAGIC: &[u8; 4] = b"VECB";
const VERSION: u32 = 1;

/// 计算缓存键：模型 + 维度 + 语料全文的 SHA-256 截断。
/// 语料顺序参与哈希，条目增删/改序都会失效。
pub fn corpus_key(model_id: &str, dim: usize, texts: &[&str]) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(model_id.as_bytes());
    hasher.update(dim.to_le_bytes());
    hasher.update((texts.len() as u64).to_le_bytes());
    for t in texts {
        hasher.update((t.len() as u64).to_le_bytes());
        hasher.update(t.as_bytes());
    }
    let digest = hasher.finalize();
    u64::from_le_bytes(digest[..8].try_into().expect("SHA-256 前 8 字节"))
}

/// 缓存文件路径：`<user_data>/cache/corpus_embeddings/<name>.bin`
fn cache_path(name: &str) -> PathBuf {
    crate::utils::path::get_user_data_dir()
        .join("cache")
        .join("corpus_embeddings")
        .join(format!("{name}.bin"))
}

/// 尝试从磁盘加载缓存的语料嵌入。
///
/// `expected_count` 为语料条数；键/条数/维度任一不匹配或文件损坏返回 None。
pub fn load(name: &str, key: u64, expected_count: usize, dim: usize) -> Option<Vec<Vec<f32>>> {
    let path = cache_path(name);
    let bytes = std::fs::read(&path).ok()?;
    if bytes.len() < 24 {
        return None;
    }
    if &bytes[0..4] != MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    if version != VERSION {
        return None;
    }
    let stored_key = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    if stored_key != key {
        return None;
    }
    let count = u32::from_le_bytes(bytes[16..20].try_into().ok()?) as usize;
    let stored_dim = u32::from_le_bytes(bytes[20..24].try_into().ok()?) as usize;
    if count != expected_count || stored_dim != dim {
        return None;
    }
    let data_len = count.checked_mul(dim)?.checked_mul(4)?;
    if bytes.len() != 24 + data_len {
        return None;
    }

    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let start = 24 + i * dim * 4;
        let mut vec = Vec::with_capacity(dim);
        for j in 0..dim {
            let off = start + j * 4;
            let v = f32::from_le_bytes(bytes[off..off + 4].try_into().ok()?);
            vec.push(v);
        }
        out.push(vec);
    }
    Some(out)
}

/// 把语料嵌入写入缓存（临时文件 + rename 原子替换）。
/// 失败仅记录警告：缓存写不进去只影响下次启动速度，不影响本次运行。
pub fn save(name: &str, key: u64, embeddings: &[Vec<f32>], dim: usize) {
    let result = (|| -> Result<(), String> {
        let path = cache_path(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建缓存目录失败: {e}"))?;
        }

        let count = embeddings.len();
        let mut bytes = Vec::with_capacity(24 + count * dim * 4);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&key.to_le_bytes());
        bytes.extend_from_slice(&(count as u32).to_le_bytes());
        bytes.extend_from_slice(&(dim as u32).to_le_bytes());
        for emb in embeddings {
            if emb.len() != dim {
                return Err(format!("向量维度不匹配: {} != {dim}", emb.len()));
            }
            for v in emb {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }

        let tmp = path.with_extension("bin.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| format!("写入缓存失败: {e}"))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("缓存原子替换失败: {e}"))?;
        Ok(())
    })();

    if let Err(e) = result {
        tracing::warn!("[CorpusCache] 语料嵌入缓存写入失败（{name}）: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_changes_with_inputs() {
        let texts = vec!["你好", "世界"];
        let k1 = corpus_key("bge-m3", 1024, &texts);
        // 模型不同 → 键不同
        assert_ne!(k1, corpus_key("hashing", 1024, &texts));
        // 维度不同 → 键不同
        assert_ne!(k1, corpus_key("bge-m3", 768, &texts));
        // 文本不同 → 键不同
        assert_ne!(k1, corpus_key("bge-m3", 1024, &vec!["你好", "世界2"]));
        // 顺序不同 → 键不同
        assert_ne!(k1, corpus_key("bge-m3", 1024, &vec!["世界", "你好"]));
        // 相同输入 → 键稳定
        assert_eq!(k1, corpus_key("bge-m3", 1024, &texts));
    }

    /// 缓存名带 PID 避免与真实缓存冲突；读写回环验证
    #[test]
    fn save_then_load_roundtrip() {
        let name = format!("test_corpus_{}", std::process::id());
        let key = 0xABCD_1234u64;
        let dim = 4usize;
        let embeddings: Vec<Vec<f32>> = vec![
            vec![0.1, 0.2, 0.3, 0.4],
            vec![-0.5, 0.6, -0.7, 0.8],
        ];
        save(&name, key, &embeddings, dim);
        let loaded = load(&name, key, embeddings.len(), dim);
        assert!(loaded.is_some(), "写后应能读到");
        let loaded = loaded.unwrap();
        assert_eq!(loaded.len(), 2);
        for (a, b) in embeddings.iter().zip(loaded.iter()) {
            for (x, y) in a.iter().zip(b.iter()) {
                assert!((x - y).abs() < 1e-6);
            }
        }
        // 键不匹配 → None
        assert!(load(&name, key + 1, embeddings.len(), dim).is_none());
        // 条数不匹配 → None
        assert!(load(&name, key, 3, dim).is_none());
        // 维度不匹配 → None
        assert!(load(&name, key, embeddings.len(), 8).is_none());
        let _ = std::fs::remove_file(cache_path(&name));
    }
}
