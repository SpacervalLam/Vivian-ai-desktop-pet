//! 预解析数据模块 — 由 build.rs 生成，嵌入二进制。
//!
//! 提供对预解析的 motion 元数据、expression 元数据、model_manifest.json
//! 的查找接口。启用 encryptResources 后，所有运行时数据查询都通过此模块，
//! 不再直接读取资源文件。

include!(concat!(env!("OUT_DIR"), "/manifest_data.rs"));
