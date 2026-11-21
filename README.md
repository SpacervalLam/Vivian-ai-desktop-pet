# Vivian

Vivian 是一款面向 Windows 10 / 11 的 AI 桌宠。你可以和桌面上的角色私聊或群聊，让她记录约定、设置提醒，也可以在工作台中打开项目、查看文件差异和终端。每位角色分别保存对话、记忆、情绪和日记。

[下载安装包](https://github.com/SpacervalLam/Vivian-ai-desktop-pet/releases) · [观看 B 站演示](https://www.bilibili.com/video/BV1m3nS66EXn/) · [反馈问题](https://github.com/SpacervalLam/Vivian-ai-desktop-pet/issues)

## 安装与首次对话

1. 从发行页选择适用于 Windows x64 的安装程序。运行环境需要 WebView2。如需可选资源，请使用同一发行版说明中列出的资源包。
2. 启动应用，在设置中填写模型端点、模型名称和 API Key，使用连接检测确认配置，然后打开角色聊天窗口。
3. 当前源码默认提供 Vivian 和 Nana。在设置的「角色与资源」中可以导入兼容的角色 ZIP。仓库提供 ChatGPT、Claude 和 DeepSeek 三个可独立打包的角色。

## 主要功能

| 场景 | 可以做什么 |
| --- | --- |
| 桌面陪伴 | 拖动和轻触角色、观看空闲动画。开启桌面物理后，角色可以在窗口边缘落脚和漫步。 |
| 对话与记忆 | 流式私聊、群聊和跨角色交流。在心智界面查看记忆、关系、日记、笔记与约定。 |
| 工作台 | 阅读和修改项目文件、运行命令、检查 Git 差异与工具调用。按 Ctrl+K 搜索工作会话。 |
| 工具与扩展 | 配置提醒、网页检索、截图识别、语音输入与朗读。通过插件和 MCP 接入其他工具。[扩展指南](plugins/README.md) |
| 手机访问 | 开启「网络与远程访问」后，在手机浏览器中访问电脑提供的服务，继续聊天或查看工作进度。 |

## 数据与备份

默认数据目录为 `%APPDATA%\vivian\`。应用配置保存在 `config.yaml`，角色数据按 `characters/<角色 ID>/` 分开存放。模型、远程嵌入、搜索、天气和语音服务会按配置访问相应端点。在「设置 → 备份与恢复」可导出或导入 `.altn` 备份。

## 从源码运行

准备 Node.js 22 或更高版本、Rust 1.88 或更高版本、Windows MSVC C++ 构建工具和 WebView2。完整构建还需要可通过 `python` 调用的 Python 3.8 或更高版本来打包资源。

首次从 Git 检出后，先按 CI 的 **Fetch pinned sqlite-vec extension** 步骤下载并校验 Windows x64 扩展，将 `vec0.dll` 放入 `src-tauri/vendor/sqlite-vec/`。Rust 编译时会嵌入该文件。[查看下载地址和 SHA-256 校验命令](.github/workflows/ci.yml)

```powershell
npm ci
npm run tauri:dev
```

`npm run dev` 可单独启动 Vite 前端。窗口、文件系统和终端等原生功能需要通过 Tauri 运行。

| 命令 | 用途 |
| --- | --- |
| `npm run typecheck` | 检查主前端与公寓插件的 TypeScript 类型 |
| `npm run test:frontend` | 运行前端测试脚本 |
| `npm run test:rust` | 运行主程序 Rust 测试 |
| `npm run test:characters` | 打包并检查角色资源、导入流程和 Rust 角色包契约 |
| `npm run build` | 构建前端、压缩桌宠图集并打包资源与角色 |
| `npm run tauri:build` | 构建桌面程序及 NSIS 安装包 |

NSIS 安装包输出到 `src-tauri/target/release/bundle/nsis/`。角色 ZIP 和目录清单位于 `release/characters/`，可用 `npm run package:characters` 单独生成。

## 代码导航

| 路径 | 职责 |
| --- | --- |
| `src/` | React 窗口、设置界面、角色注册与桌宠动画 |
| `src-tauri/src/` | Rust 应用宿主、对话与记忆、工具执行和原生窗口 |
| `character-packs/` | 扩展角色的清单、提示词与美术资源 |
| `plugins/` | 插件接口、示例与独立构建的 [3D 公寓](plugins/3d-apartment/README.md) |
| `scripts/`、`tests/` | 构建打包入口与回归测试 |

模块职责和数据流见[代码 Wiki](CODE_WIKI.md)。项目采用 [MIT 许可证](LICENSE)。
