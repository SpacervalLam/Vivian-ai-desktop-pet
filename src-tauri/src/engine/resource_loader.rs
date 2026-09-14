//! 资源加载器
//!
//! 负责扫描模型目录并分类加载纹理（chibi 图集与贴图）等资源。
//!
//! 动作/表情文件已随 Live2D 渲染后端移除，角色清单与动作词汇来自构建期嵌入的
//! chibi 动作词汇表（`pre_parsed::chibi_animations_json`），由前端按词汇表帧时长推进。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

// ===== 扩展名常量 =====

/// 纹理文件扩展名（角色图集与贴图）
pub const TEXTURE_EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".webp"];

/// 动作资源信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotionInfo {
    pub path: String,
    pub name: String,
    pub relative_path: String,
    pub extension: String,
    pub duration: f64,
    pub fps: u32,
    pub r#loop: bool,
    pub total_frames: u64,
    pub curve_count: usize,
}

/// 表情资源信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionInfo {
    pub path: String,
    pub name: String,
    pub relative_path: String,
    pub extension: String,
    pub expression_id: String,
    pub parameter_count: usize,
}

/// 纹理资源信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureInfo {
    pub path: String,
    pub name: String,
    pub relative_path: String,
    pub index: u32,
}

/// 预设资源信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetInfo {
    pub path: String,
    pub name: String,
    pub relative_path: String,
    #[serde(rename = "type")]
    pub preset_type: String,
}

/// 资源集合
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Resources {
    pub motions: HashMap<String, MotionInfo>,
    pub expressions: HashMap<String, ExpressionInfo>,
    pub textures: Vec<TextureInfo>,
    pub presets: HashMap<String, PresetInfo>,
}

/// 资源加载器 - 扫描模型目录并分类加载资源
pub struct ResourceLoader {
    base_dir: PathBuf,
    model_dir: PathBuf,
    /// 角色 ID（用于查找预解析元数据）
    char_id: String,
    resources: parking_lot::RwLock<Resources>,
    loaded: parking_lot::RwLock<bool>,
}

impl ResourceLoader {
    pub fn new(base_dir: impl Into<PathBuf>, model_dir: &str) -> Self {
        let base_dir = base_dir.into();
        let model_dir = base_dir.join(model_dir);
        // model_dir 的 file_name 即为角色 ID
        let char_id = model_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        Self {
            base_dir,
            model_dir,
            char_id,
            resources: parking_lot::RwLock::new(Resources::default()),
            loaded: parking_lot::RwLock::new(false),
        }
    }

    /// 加载完整资源
    pub fn load(&self) -> Resources {
        debug!("[ResourceLoader] load() 方法被调用");
        if self.model_dir.exists() {
            self.scan_directory(true);
        } else {
            self.scan_embedded(true);
        }
        *self.loaded.write() = true;

        let res = self.resources.read();
        info!(
            "[ResourceLoader] 完整资源加载完成: Motions={}, Expressions={}, Textures={}, Presets={}",
            res.motions.len(),
            res.expressions.len(),
            res.textures.len(),
            res.presets.len()
        );
        res.clone()
    }

    /// 仅加载关键资源（纹理、模型、物理、CDI）
    pub fn load_critical(&self) -> Resources {
        debug!("[ResourceLoader] load_critical() 方法被调用");
        if self.model_dir.exists() {
            self.scan_directory(false);
        } else {
            self.scan_embedded(false);
        }
        *self.loaded.write() = false;

        let res = self.resources.read();
        debug!(
            "[ResourceLoader] 关键资源加载完成: Textures={}, Presets={}",
            res.textures.len(),
            res.presets.len()
        );
        res.clone()
    }

    /// 后台加载完整资源
    pub fn load_background(&self) -> Resources {
        debug!("[ResourceLoader] load_background() 方法被调用");
        if self.model_dir.exists() {
            self.scan_directory(true);
        } else {
            self.scan_embedded(true);
        }
        *self.loaded.write() = true;

        let res = self.resources.read();
        info!(
            "[ResourceLoader] 完整资源加载完成: Motions={}, Expressions={}, Textures={}, Presets={}",
            res.motions.len(),
            res.expressions.len(),
            res.textures.len(),
            res.presets.len()
        );
        res.clone()
    }

    /// 重新加载资源
    pub fn reload(&self) -> Resources {
        *self.resources.write() = Resources::default();
        *self.loaded.write() = false;
        self.load()
    }

    /// 扫描目录并加载资源（递归遍历子目录）
    fn scan_directory(&self, load_all: bool) {
        let mut resources = self.resources.read().clone();

        debug!(
            "[ResourceLoader] 开始扫描目录: {}, load_all: {}",
            self.model_dir.display(),
            load_all
        );

        self.walk_dir(self.model_dir.as_path(), load_all, &mut resources);

        // 纹理按 index 排序
        resources.textures.sort_by_key(|t| t.index);

        *self.resources.write() = resources;
    }

    /// 从 Bundle 索引扫描（release 模式，文件系统无资源文件）
    fn scan_embedded(&self, load_all: bool) {
        let mut resources = self.resources.read().clone();

        let prefix = format!("{}/", self.char_id);
        for virtual_path in crate::bundle_reader::list_assets_by_prefix(&self.char_id) {
            // virtual_path 形如 "Vivian/vivian-atlas.png"
            // 构造相对于 model_dir 的虚拟路径
            let relative = virtual_path
                .strip_prefix(&prefix)
                .unwrap_or(virtual_path);
            // 构造虚拟绝对路径（base_dir/char_id/relative）
            let virtual_full = self.model_dir.join(relative);
            self.classify_file(&virtual_full, load_all, &mut resources);
        }

        resources.textures.sort_by_key(|t| t.index);

        let counts = (
            resources.motions.len(),
            resources.expressions.len(),
            resources.textures.len(),
            resources.presets.len(),
        );

        *self.resources.write() = resources;

        debug!(
            "[ResourceLoader] 嵌入资源扫描完成 (char={}): Motions={}, Expressions={}, Textures={}, Presets={}",
            self.char_id,
            counts.0,
            counts.1,
            counts.2,
            counts.3
        );
    }

    /// 递归遍历目录，对每个文件调用 `classify_file` 分类
    fn walk_dir(&self, dir: &Path, load_all: bool, resources: &mut Resources) {
        let entries = match std::fs::read_dir(dir) {
            Ok(it) => it,
            Err(e) => {
                warn!(
                    "[ResourceLoader] 读取目录失败: {} - {}",
                    dir.display(),
                    e
                );
                return;
            }
        };

        for entry in entries.flatten() {
            let file_path = entry.path();
            if file_path.is_dir() {
                self.walk_dir(&file_path, load_all, resources);
                continue;
            }
            if !file_path.is_file() {
                continue;
            }
            self.classify_file(&file_path, load_all, resources);
        }
    }

    /// 对单个文件进行分类加载
    fn classify_file(&self, file_path: &Path, _load_all: bool, resources: &mut Resources) {
        let file_name = match file_path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => return,
        };

        let ext = file_path
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default();

        let relative_path = file_path
            .strip_prefix(&self.model_dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| file_name.clone());

        let path_str = file_path.to_string_lossy().to_string();

        // 角色图集 / 贴图：按文件名中的 texture_ 索引归类
        if TEXTURE_EXTENSIONS.contains(&ext.as_str()) {
            if file_name.contains("texture_") {
                let already = resources.textures.iter().any(|t| t.name == file_name);
                if !already {
                    let index = Self::extract_texture_index(&file_name);
                    resources.textures.push(TextureInfo {
                        path: path_str,
                        name: file_name.clone(),
                        relative_path,
                        index,
                    });
                    debug!("[ResourceLoader] 加载纹理: {}", file_name);
                }
            }
        }
    }

    /// 提取纹理索引（texture_10.png -> 10）
    fn extract_texture_index(filename: &str) -> u32 {
        let stripped = filename.replace("texture_", "").replace(".png", "");
        stripped.parse::<u32>().unwrap_or(0)
    }

    /// 获取文件名（不含最后一个扩展名）
    fn file_stem(path: &Path) -> String {
        path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// 获取动作信息
    pub fn get_motion(&self, name: &str) -> Option<MotionInfo> {
        self.resources.read().motions.get(name).cloned()
    }

    /// 获取表情信息
    pub fn get_expression(&self, name: &str) -> Option<ExpressionInfo> {
        self.resources.read().expressions.get(name).cloned()
    }

    /// 获取预设配置
    pub fn get_preset(&self, preset_type: &str) -> Option<PresetInfo> {
        self.resources.read().presets.get(preset_type).cloned()
    }

    /// 获取所有动作
    pub fn get_all_motions(&self) -> HashMap<String, MotionInfo> {
        self.resources.read().motions.clone()
    }

    /// 获取所有表情
    pub fn get_all_expressions(&self) -> HashMap<String, ExpressionInfo> {
        self.resources.read().expressions.clone()
    }

    /// 随机获取动作
    pub fn get_random_motion(&self) -> Option<MotionInfo> {
        let motions = self.resources.read().motions.clone();
        if motions.is_empty() {
            return None;
        }
        let values: Vec<MotionInfo> = motions.into_values().collect();
        random_choice(&values)
    }

    /// 随机获取表情
    pub fn get_random_expression(&self) -> Option<ExpressionInfo> {
        let expressions = self.resources.read().expressions.clone();
        if expressions.is_empty() {
            return None;
        }
        let values: Vec<ExpressionInfo> = expressions.into_values().collect();
        random_choice(&values)
    }

    /// 列出所有动作名称
    pub fn list_motion_names(&self) -> Vec<String> {
        self.resources.read().motions.keys().cloned().collect()
    }

    /// 列出所有表情名称
    pub fn list_expression_names(&self) -> Vec<String> {
        self.resources
            .read()
            .expressions
            .keys()
            .cloned()
            .collect()
    }

    /// 是否已加载
    pub fn is_loaded(&self) -> bool {
        *self.loaded.read()
    }

    /// 获取模型目录路径
    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    /// 获取基础目录路径
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

/// 从切片中随机选取一个元素（基于系统时间纳秒作为简易随机源）
fn random_choice<T: Clone>(items: &[T]) -> Option<T> {
    if items.is_empty() {
        return None;
    }
    let idx = random_u64() as usize % items.len();
    Some(items[idx].clone())
}

/// 生成一个简易的伪随机 u64（基于系统时间纳秒）
fn random_u64() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_texture_index() {
        assert_eq!(ResourceLoader::extract_texture_index("texture_0.png"), 0);
        assert_eq!(ResourceLoader::extract_texture_index("texture_10.png"), 10);
        assert_eq!(ResourceLoader::extract_texture_index("texture_abc.png"), 0);
        assert_eq!(ResourceLoader::extract_texture_index("random.png"), 0);
    }

    #[test]
    fn test_random_choice_empty() {
        let items: Vec<i32> = vec![];
        assert!(random_choice(&items).is_none());
    }

    #[test]
    fn test_random_choice_nonempty() {
        let items = vec![1, 2, 3];
        let picked = random_choice(&items);
        assert!(picked.is_some());
        assert!(items.contains(&picked.unwrap()));
    }
}