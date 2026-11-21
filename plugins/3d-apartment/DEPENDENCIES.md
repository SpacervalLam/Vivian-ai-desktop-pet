# 依赖与发行边界

| 分类 | 归属 | 处理 |
| --- | --- | --- |
| Three.js、Three 类型 | 公寓插件 | 从根依赖清单移除，由插件 workspace 声明；主前端不含场景或 Three.js |
| React、ReactDOM、Tauri JS API | 宿主与插件各自需要 | 插件打成独立 IIFE，自带运行时，不依赖宿主 React 全局变量 |
| 场景、导航、角色动画、天气映射、启动遮罩 | 公寓插件 | 全部移入插件 `src/` |
| 7 个 GLB 模型 | 公寓插件 | 只进入单独 ZIP，基础 setup 不携带 |
| Blender 工程、贴图中间产物、验证工具 | 公寓开发目录 | 移入 `art/`、`tools/`；发行 ZIP 不携带 |
| 插件状态、通用设置、窗口联动与原生 ESC | 宿主接口 | 宿主拥有桌宠窗口，必须保留这些轻量接口；没有 3D 渲染和场景逻辑 |
| Mermaid npm 包 | 未使用 | 当前笔记本仅展示 Mermaid 条目，没有调用渲染库；移除直接依赖 |
| `@mongodb-js/zstd` | 旧构建依赖 | 原加密生成脚本已移除，无引用；运行时 Rust zstd 保留 |
| `@types/dompurify` | 冗余开发依赖 | DOMPurify 自带类型；移除类型占位包，保留 DOMPurify |
| Tokio | 宿主 | 将 `full` 改为显式运行时、宏、定时器、同步、文件、进程、IO、网络功能；传递依赖仍可以开启其所需功能 |
| 文档、表格、终端、音频及数据库依赖 | 宿主 | 都有现有功能使用，保留 |

npm 锁文件包条目从 356 降到 224。workspace 可能将 Three.js 提升到根 `node_modules`，这是开发环境的包布局；`node_modules` 不进入安装包，主程序依赖清单及主前端产物不包含 Three.js。

Rust release 改为 `opt-level = "s"`，保留 fat LTO、单代码生成单元、符号剥离及原有 panic/unwind 行为。NSIS 使用 LZMA，仅生成 setup。WebView2 按 Tauri 默认机制处理，不捆绑固定版本运行时。

验证：主程序前端构建、插件独立类型检查与构建、Rust `cargo check`、90 条导航路线与对向角色仿真、固定步长及 sprite 加载清理测试、独立发行脚本首帧/7 个模型加载/卸载、PowerShell 组件安装/更新/错误哈希拒绝均已通过。浏览器运行验证使用 Tauri 命令桩；没有将完整安装程序安装进用户现有系统环境。

基础 setup、组件 ZIP、SHA256 和安装说明由 `npm run package:small` 输出到仓库根 `release/`。

2026-09-30 本地发行结果：基础 setup 41,208,375 字节（39.30 MiB），独立公寓 ZIP 5,183,392 字节（4.94 MiB）。与工作区原有上一份 setup 的 49,344,332 字节相比，基础 setup 减小约 16.49%。正式 release 构建和 NSIS 打包成功，生成的 NSIS 文件清单没有公寓脚本或模型资源；公寓只由安装钩子从安装程序旁的 ZIP 导入。
