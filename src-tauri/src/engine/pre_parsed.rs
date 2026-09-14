//! 预解析数据模块 — 由 build.rs 生成，嵌入二进制。
//!
//! 提供 chibi 动作词汇表（`src/chibi/animations.json`）的查找接口。
//! 词汇表在构建期固化进二进制：它既不是运行时素材也不该进安装包，
//! 后端的动作名清单、情绪映射与空闲/事件触发均据此构建。

include!(concat!(env!("OUT_DIR"), "/manifest_data.rs"));
