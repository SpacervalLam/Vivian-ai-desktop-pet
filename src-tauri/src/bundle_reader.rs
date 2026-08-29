//! Bundle 读取器
//!
//! 从 vivian.bundle.enc 读取资源：按需读取单个文件的密文段 → AES 解密 → zstd 解压。
//! Bundle 文件由 scripts/encrypt-assets.mjs 生成（VBL2 per-file 格式），索引由 build.rs 嵌入二进制。
//! 运行时只加载索引段，不将整个解压内容常驻内存；热资源经小容量 LRU 缓存复用。

use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

/// Bundle 索引项（由 build.rs 生成到 OUT_DIR/bundle_index.rs）
#[derive(Debug, Clone)]
pub struct BundleEntry {
    pub path: &'static str,
    /// 密文段内偏移（含 nonce + tag）
    pub offset: u64,
    /// 密文长度（nonce + 密文 + tag）
    pub size: u64,
    /// 解压后明文长度
    pub plain_size: u64,
}

// 编译期嵌入的索引数据
include!(concat!(env!("OUT_DIR"), "/bundle_index.rs"));

/// 解压后热资源缓存容量上限
const CACHE_CAP: usize = 32 * 1024 * 1024;

/// 全局 Bundle（启动时初始化一次）
static BUNDLE: OnceCell<BundleData> = OnceCell::new();

struct BundleData {
    /// 密文文件句柄（按需 seek + read）
    file: Mutex<File>,
    /// 数据段（密文）在文件中的起始偏移
    data_start: u64,
    /// 路径 → 索引项
    index: HashMap<&'static str, &'static BundleEntry>,
    /// 解压后明文的 LRU 缓存
    cache: Mutex<AssetCache>,
}

/// 简易 LRU：总字节数超限时淘汰最久未使用条目
struct AssetCache {
    map: HashMap<String, Arc<Vec<u8>>>,
    order: VecDeque<String>,
    total: usize,
}

impl Default for AssetCache {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            total: 0,
        }
    }
}

impl AssetCache {
    fn get(&mut self, path: &str) -> Option<Arc<Vec<u8>>> {
        let value = self.map.get(path)?.clone();
        if let Some(pos) = self.order.iter().position(|p| p == path) {
            self.order.remove(pos);
            self.order.push_back(path.to_string());
        }
        Some(value)
    }

    fn put(&mut self, path: &str, data: Arc<Vec<u8>>) {
        if self.map.contains_key(path) {
            return;
        }
        self.total += data.len();
        self.map.insert(path.to_string(), data);
        self.order.push_back(path.to_string());
        while self.total > CACHE_CAP && self.order.len() > 1 {
            if let Some(old) = self.order.pop_front() {
                if let Some(v) = self.map.remove(&old) {
                    self.total -= v.len();
                }
            }
        }
    }
}

/// 初始化 Bundle：读取并校验磁盘索引段，保持文件句柄供按需读取
///
/// 在 lib.rs setup 中调用一次。如果 bundle 文件不存在（dev 模式），返回 Ok(()) 但不初始化。
pub fn init(bundle_path: &PathBuf) -> Result<(), String> {
    if !bundle_path.exists() {
        tracing::info!("[bundle] 文件不存在，跳过初始化: {}", bundle_path.display());
        return Ok(());
    }

    let mut file = std::fs::File::open(bundle_path)
        .map_err(|e| format!("打开 bundle 失败: {}", e))?;

    // 读 magic + count
    let mut header = [0u8; 8];
    file.read_exact(&mut header)
        .map_err(|e| format!("读取 bundle 头部失败: {}", e))?;
    let magic = &header[0..4];
    if magic == b"VBL1" {
        return Err(
            "检测到旧版 VBL1 bundle（整体解压常驻内存格式），请重新运行 npm run encrypt:assets 生成 VBL2 按需解压格式".to_string(),
        );
    }
    if magic != b"VBL2" {
        return Err(format!("Bundle magic 校验失败: {:?}", magic));
    }
    let count = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;

    // 逐条读磁盘索引: pathLen(4) + path + offset(8) + size(8) + plainSize(8)
    let mut disk_index: HashMap<String, (u64, u64, u64)> = HashMap::with_capacity(count);
    for _ in 0..count {
        let mut plen_buf = [0u8; 4];
        file.read_exact(&mut plen_buf)
            .map_err(|e| format!("读取索引路径长度失败: {}", e))?;
        let plen = u32::from_le_bytes(plen_buf) as usize;
        let mut path_buf = vec![0u8; plen];
        file.read_exact(&mut path_buf)
            .map_err(|e| format!("读取索引路径失败: {}", e))?;
        let mut nums = [0u8; 24];
        file.read_exact(&mut nums)
            .map_err(|e| format!("读取索引数值失败: {}", e))?;
        let path = String::from_utf8_lossy(&path_buf).into_owned();
        let offset = u64::from_le_bytes(nums[0..8].try_into().unwrap());
        let size = u64::from_le_bytes(nums[8..16].try_into().unwrap());
        let plain_size = u64::from_le_bytes(nums[16..24].try_into().unwrap());
        disk_index.insert(path, (offset, size, plain_size));
    }

    // 数据段起点 = 索引段读完后的文件位置
    let data_start = file
        .stream_position()
        .map_err(|e| format!("定位数据段失败: {}", e))?;

    // 与编译期嵌入索引交叉校验，防止 bundle 文件与二进制版本不匹配
    if BUNDLE_ENTRIES.len() != count {
        return Err(format!(
            "bundle 与编译期索引不匹配: 磁盘 {} 条, 编译期 {} 条 (请重新运行 npm run encrypt:assets 并重新编译)",
            count,
            BUNDLE_ENTRIES.len()
        ));
    }
    let mut plain_total = 0u64;
    for entry in BUNDLE_ENTRIES.iter() {
        match disk_index.get(entry.path) {
            Some(&(o, s, p)) if o == entry.offset && s == entry.size && p == entry.plain_size => {
                plain_total += p;
            }
            _ => {
                return Err(format!(
                    "bundle 索引条目不匹配: {} (请重新运行 npm run encrypt:assets 并重新编译)",
                    entry.path
                ));
            }
        }
    }

    // 校验文件长度覆盖全部密文段
    let file_len = file
        .metadata()
        .map_err(|e| format!("读取 bundle 元数据失败: {}", e))?
        .len();
    if let Some(last) = BUNDLE_ENTRIES.iter().max_by_key(|e| e.offset + e.size) {
        let end = data_start + last.offset + last.size;
        if end > file_len {
            return Err(format!(
                "bundle 文件长度不足: 需要 {} 字节, 实际 {} 字节",
                end, file_len
            ));
        }
    }

    tracing::info!(
        "[bundle] 初始化完成: {} 个文件, 密文 {:.2} MB, 明文总量 {:.2} MB (按需解压, 常驻仅索引)",
        BUNDLE_ENTRIES.len(),
        file_len as f64 / 1024.0 / 1024.0,
        plain_total as f64 / 1024.0 / 1024.0
    );

    let index: HashMap<&'static str, &'static BundleEntry> = BUNDLE_ENTRIES
        .iter()
        .map(|e| (e.path, e))
        .collect();

    let _ = BUNDLE.set(BundleData {
        file: Mutex::new(file),
        data_start,
        index,
        cache: Mutex::new(AssetCache::default()),
    });

    Ok(())
}

/// 检查 Bundle 是否已初始化
pub fn is_initialized() -> bool {
    BUNDLE.get().is_some()
}

/// 根据路径获取资源明文字节
///
/// 路径格式: "Vivian/nana.model3.json"。
/// 命中缓存直接返回；否则读取该文件密文段 → AES 解密 → zstd 解压 → 写入缓存。
pub fn get(path: &str) -> Option<Vec<u8>> {
    let bundle = BUNDLE.get()?;

    {
        let mut cache = bundle.cache.lock();
        if let Some(v) = cache.get(path) {
            return Some((*v).clone());
        }
    }

    let entry = *bundle.index.get(path)?;
    let start = bundle.data_start + entry.offset;
    let end = start + entry.size;
    if end <= start {
        tracing::error!("[bundle] 资源范围非法: {} (offset={}, size={})", path, entry.offset, entry.size);
        return None;
    }

    // 读密文段（锁内只做 IO，解密解压在锁外）
    let mut ciphertext = vec![0u8; entry.size as usize];
    {
        let mut f = bundle.file.lock();
        f.seek(SeekFrom::Start(start)).ok()?;
        f.read_exact(&mut ciphertext).ok()?;
    }

    let compressed = crate::asset_crypto::decrypt(&ciphertext).ok()?;
    let plain = zstd::decode_all(compressed.as_slice()).ok()?;
    if plain.len() != entry.plain_size as usize {
        tracing::error!(
            "[bundle] 解压长度校验失败: {} (期望 {}, 实际 {})",
            path, entry.plain_size, plain.len()
        );
        return None;
    }

    let arc = Arc::new(plain);
    let mut cache = bundle.cache.lock();
    cache.put(path, arc.clone());
    Some((*arc).clone())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 端到端：init 真实 VBL2 bundle → get 解密解压 → 缓存命中
    /// 工作区无 bundle 文件时（未跑打包脚本）自动跳过。
    #[test]
    fn test_vbl2_roundtrip() {
        let bundle_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vivian.bundle.enc");
        if !bundle_path.exists() {
            eprintln!("[test] 跳过: bundle 文件不存在");
            return;
        }

        init(&bundle_path).expect("init 应成功");
        assert!(is_initialized());

        // 取第一个文件做解密解压验证
        let first = BUNDLE_ENTRIES
            .first()
            .expect("索引非空");
        let data = get(first.path).expect("get 应返回明文");
        assert_eq!(data.len(), first.plain_size as usize, "解压长度应与索引 plain_size 一致");

        // 再次 get 应命中缓存且内容一致
        let cached = get(first.path).expect("缓存 get 应成功");
        assert_eq!(data, cached);

        // 不存在的路径返回 None
        assert!(get("__no_such_file__.png").is_none());
    }
}
