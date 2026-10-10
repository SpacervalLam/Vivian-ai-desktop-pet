# Windows Rust 依赖检查（2026-10-10）

检查依据：Windows x86_64 MSVC 的 `cargo metadata --locked`、`cargo tree -e features`、应用源码调用和现有 release 库产物。复现：`npm run audit:rust-size`，输出 `tmp/rust-dependency-audit.json`。需要本地已有对应的 Cargo 依赖和 release/deps。

## 重型依赖及保留原因

| 依赖 | 实际用途 | 当前判断 |
| --- | --- | --- |
| boa_engine 0.21.1 | `providers/js_runtime.rs` 的可执行 JS 插件宿主 | 保留；删除会使 JS 插件失效。默认 float16/xsum 支撑 ECMAScript 运算，不能作为无行为变化的删除项 |
| jieba-rs 0.7.4 | 中文记忆检索、实体提取、对话评估 | 保留默认词典；禁用 default-dict 后必须提供替代词典，否则分词质量或启动行为改变 |
| tiktoken-rs 0.12.0 | `memory/time_stamped.rs` 的 cl100k_base Token 预算 | 保留；没有默认功能开关可直接精简，换成字符估算会改变上下文预算 |
| pdf-extract 0.7.12 | `commands/chat.rs` 的 PDF 附件提取 | 保留；若要外置需另做附件处理插件及未安装反馈 |
| rodio 0.19.0 | 本地 TTS 音频解码、播放 | 默认 FLAC/Vorbis/WAV/MP3 都已启用。裁剪 FLAC 需先规定所有 TTS 返回格式，不能直接删掉通用解码能力 |
| sysinfo 0.32.1 | 进程列表、CPU/内存/网速监测 | 有明确的未使用默认功能候选，见下节 |

## 可以进一步评估的默认功能

`sysinfo` 当前默认启用 `component,disk,network,system,user,multithread`。源码仅使用 `System`、进程刷新和 `Networks`，没有找到 `Components`、`Disks`、`Users` 调用。候选配置是：

```toml
sysinfo = { version = "0.32", default-features = false, features = ["system", "network", "multithread"] }
```

首次检查保留现有配置；后续已按同一份前端资源构建实测，结果见下文。LTO 已可能移除未调用代码，关闭这些默认功能不等于可执行文件一定缩小。不能用资源外置带来的体积变化冒充 Rust 默认功能裁剪收益。

## 已收敛的部分

- `reqwest` 已关闭默认功能，明确使用 Rustls、JSON、流式、HTTP/2、multipart。
- 解析后的 `rustls` 使用 ring/std/tls12，没有开启默认 AWS-LC 或后量子算法。
- `image` 解析后的格式仅 bmp/ico/png，没有启用全格式解码。
- 发布配置已经是 opt-level=s、fat LTO、单 codegen unit、strip=symbols。
- `tokio` 已明确列出运行时、文件、进程、网络等所需功能。

没有为省体积移除插件、中文分词、Token 计数或 PDF 功能。release/deps 的 `.rlib` 包含元数据和编译中间表示，且目录可能有多个构建变体；其大小不能相加当作安装包大小。若继续做代码级精简，应使用保留符号的专用分析构建与 cargo-bloat/链接图；当前发行 EXE 已剥离符号，不能可靠给出逐 crate 的最终贡献。

## 本轮验证记录

### 后续功能裁剪实测

在不改变前端资源的条件下，实际构建了上面的 `sysinfo` 精简配置（CPU/内存/进程、网络、多线程保留；磁盘、用户、传感器关闭）。对比期间 181 个前端文件 SHA-256 均未变化，Rust 版本、release 配置、NSIS LZMA 配置相同。构建使用 `cargo build --release --bin vivian --features tauri/custom-protocol`，再由 Tauri bundle 生成 NSIS。

| 项目 | 原配置 | 裁剪配置 | 差值 |
| --- | ---: | ---: | ---: |
| 主程序 EXE | 85,319,168 字节 | 85,319,168 字节 | 0 |
| NSIS 安装包 | 40,674,041 字节 | 40,665,411 字节 | -8,630 字节（约 0.02%） |

同一份裁剪 EXE 再次运行 bundle，安装包为 40,638,739 字节，比前一次打包相差 26,672 字节；这超过首轮裁剪的 8,630 字节差值。因此该差值不足以证明功能裁剪有效。

裁剪后的真实 CPU、内存、网络接口采集及源码原有进程聚合测试均通过（2 项）。主程序 release 编译及 NSIS 打包成功。该方案未带来有意义的包体缩减，已恢复 `sysinfo = "0.32"`，避免把配置复杂度当成优化收益。LTO 已可能消除未使用模块，依赖功能列表缩短不能代替最终产物测量。实验产物及日志位于 `tmp/size-experiment/`，不随应用发布。

图集扩展压缩另行测量，不能归入 Rust 裁剪收益；最终发布继续使用原来的 Rust 依赖功能配置。

最终图集优化版：主程序 84,022,272 字节；安装包 39,389,374 字节（37.56 MiB）。安装包相对原版节省 1,284,667 字节（3.16%），明显超过重复打包观察到的约 26 KiB 波动。四个可选资源 ZIP 校验值不变。最终构建首次遇到编译器内存分配失败，保持原优化参数、以单构建任务重试成功，未停用用户进程；日志为 `tmp/size-experiment/final-native-build-retry.log`。

前端 60 个测试脚本通过；Rust 编译检查和完整 Windows NSIS 构建成功。可选包安装测试覆盖三类包、更新、空格路径、文件与 ZIP 校验、非法路径及失败时保留旧资源。相同暂存输入重复生成 ZIP 的 SHA-256 一致。

全量 Rust 单元测试单独编译重试后执行：2036 通过、2 失败、9 忽略。资源包和贴纸相关测试通过。失败项为 `memory::conversation_archive::companion_optimization_tests::independent_archive_writers_reject_stale_disk_versions` 与 `proactive::behavior::structured_context_tests::proactive_context_keeps_history_roles_and_system_protocol`；前者单独复跑通过，后者单独复跑仍失败。本轮未修改这两个模块，不把全量测试描述为全部通过。首次测试编译与发行编译并行时遇到 LLVM 内存不足，重试改为发行构建结束后独立执行。

首次可选资源外置后的基础前端资源约 23.08 MiB，基础 NSIS 安装包约 38.79 MiB；体积下降来自此前图片压缩和可选资源外置，未改变 Rust 依赖功能配置。后续扩大图集压缩将基础前端降至 22,912,352 字节（21.85 MiB）；Rust 功能裁剪实验已撤回。

2026-10-10 删除桌面小游戏后，主程序为 83,817,472 字节，基础安装包为 39,344,644 字节（37.52 MiB）。游戏此前的美术资源已外置，因此这次基础安装包减少主要来自代码删除；独立游戏 ZIP 同时移除。现有发布清单包含基础安装包、3D 公寓、字体和贴纸四项，均已核对大小及 SHA-256。前端 56 个测试脚本、Rust 编译检查及 149 项运行时契约测试通过；发布编译和 NSIS 打包成功，未改变 Rust 依赖功能配置。日志位于 `tmp/game-removal-*.log`。
