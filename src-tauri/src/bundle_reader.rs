//! Bundle 读取器
//!
//! 从 vivian.bundle.enc 读取资源：AES 解密 → zstd 解压 → 按索引查找。
//! Bundle 文件由 scripts/encrypt-assets.mjs 生成，索引由 build.rs 嵌入二进制。
//! 运行时一次性解密+解压整个 bundle 到内存，后续访问零成本。

use once_cell::sync::OnceCell;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;

/// Bundle 索引项（由 build.rs 生成到 OUT_DIR/bundle_index.rs）
#[derive(Debug, Clone)]
pub struct BundleEntry {
    pub path: &'static str,
    pub offset: u64,
    pub size: u64,
}

// 编译期嵌入的索引数据和压缩格式
include!(concat!(env!("OUT_DIR"), "/bundle_index.rs"));

/// 全局 Bundle 缓存（启动时加载一次，后续零成本访问）
static BUNDLE: OnceCell<RwLock<BundleData>> = OnceCell::new();

struct BundleData {
    /// 解密+解压后的完整 bundle 字节流
    data: Vec<u8>,
    /// 数据段在 bundle 中的起始偏移（magic + count + index 段之后）
    data_start: usize,
    /// 路径 → 索引项 的哈希表
    index: HashMap<&'static str, BundleEntry>,
}

/// 初始化 Bundle：读取文件 → AES 解密 → zstd 解压 → 解析索引
///
/// 在 lib.rs setup 中调用一次。如果 bundle 文件不存在（dev 模式），返回 Ok(()) 但不初始化。
pub fn init(bundle_path: &PathBuf) -> Result<(), String> {
    if !bundle_path.exists() {
        tracing::info!("[bundle] 文件不存在，跳过初始化: {}", bundle_path.display());
        return Ok(());
    }

    let ciphertext = std::fs::read(bundle_path)
        .map_err(|e| format!("读取 bundle 失败: {}", e))?;

    // AES 解密
    let compressed = crate::asset_crypto::decrypt(&ciphertext)?;

    // zstd 解压
    let plaintext = if BUNDLE_COMPRESSION_FORMAT == "zstd" {
        zstd::decode_all(compressed.as_slice())
            .map_err(|e| format!("zstd 解压失败: {}", e))?
    } else {
        return Err(format!("不支持的压缩格式: {} (仅支持 zstd)", BUNDLE_COMPRESSION_FORMAT));
    };

    // 校验 magic
    if plaintext.len() < 8 || &plaintext[0..4] != b"VBL1" {
        return Err("Bundle magic 校验失败".to_string());
    }

    // 解析 bundle 内嵌索引段，计算数据段起始偏移
    // 格式: magic(4) + count(4) + [pathLen(4) + path + offset(8) + size(8)] * count
    let count = u32::from_le_bytes([
        plaintext[4], plaintext[5], plaintext[6], plaintext[7],
    ]) as usize;
    let mut data_start = 8usize;
    for _ in 0..count {
        if data_start + 4 > plaintext.len() {
            return Err("Bundle 索引段解析越域".to_string());
        }
        let path_len = u32::from_le_bytes([
            plaintext[data_start], plaintext[data_start + 1],
            plaintext[data_start + 2], plaintext[data_start + 3],
        ]) as usize;
        data_start += 4 + path_len + 8 + 8; // pathLen + path + offset + size
    }

    // 构建索引哈希表（用编译期嵌入的 BUNDLE_ENTRIES 常量）
    let mut index = HashMap::with_capacity(BUNDLE_ENTRIES.len());
    for entry in BUNDLE_ENTRIES.iter() {
        index.insert(entry.path, entry.clone());
    }

    tracing::info!(
        "[bundle] 初始化完成: {} 个文件, 解压后 {:.2} MB, 数据段偏移 {}",
        index.len(),
        plaintext.len() as f64 / 1024.0 / 1024.0,
        data_start
    );

    let _ = BUNDLE.set(RwLock::new(BundleData {
        data: plaintext,
        data_start,
        index,
    }));

    Ok(())
}

/// 检查 Bundle 是否已初始化
pub fn is_initialized() -> bool {
    BUNDLE.get().is_some()
}

/// 根据路径获取资源字节
///
/// 路径格式: "Vivian/nana.model3.json"
pub fn get(path: &str) -> Option<Vec<u8>> {
    let cell = BUNDLE.get()?;
    let bundle = cell.read();
    let entry = bundle.index.get(path)?;
    let start = bundle.data_start + entry.offset as usize;
    let end = start + entry.size as usize;
    if end > bundle.data.len() {
        tracing::error!(
            "[bundle] 资源范围越界: {} (offset={}, size={}, data_start={}, data_len={})",
            path, entry.offset, entry.size, bundle.data_start, bundle.data.len()
        );
        return None;
    }
    Some(bundle.data[start..end].to_vec())
}

/// 列出所有资源路径
pub fn list_assets() -> Vec<&'static str> {
    BUNDLE_ENTRIES.iter().map(|e| e.path).collect()
}

/// 列出属于指定前缀的资源路径
pub fn list_assets_by_prefix(prefix: &str) -> Vec<&'static str> {
    let prefix_with_slash = if prefix.ends_with('/') {
        prefix.to_string()
    } else {
        format!("{}/", prefix)
    };
    BUNDLE_ENTRIES
        .iter()
        .filter(|e| e.path.starts_with(&prefix_with_slash))
        .map(|e| e.path)
        .collect()
}

/// 根据文件扩展名猜测 Content-Type
pub fn content_type(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.ends_with(".model3.json")
        || lower.ends_with(".exp3.json")
        || lower.ends_with(".motion3.json")
        || lower.ends_with(".physics3.json")
        || lower.ends_with(".cdi3.json")
        || lower.ends_with(".vtube.json")
        || lower.ends_with(".json")
    {
        "application/json"
    } else if lower.ends_with(".moc3") {
        "application/octet-stream"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".mtn") {
        "application/octet-stream"
    } else {
        "application/octet-stream"
    }
}
