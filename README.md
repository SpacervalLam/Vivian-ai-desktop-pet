<div align="center">

# Vivian

**具备记忆、情绪与主动性的 AI 桌面宠物**

Rust · Tauri 2 · React 18 · Q 版 Sprite 动画

</div>

Vivian 是面向 Windows 的多角色桌面陪伴应用。Vivian 与 Nana 可以同时在线，各自拥有独立的记忆、人格、心理状态和对话历史，也能彼此交谈。除日常陪伴外，它还提供工作智能体、语音、联网检索、桌面工具及可选的 3D 公寓。

角色数据保存在本地；模型、远程嵌入、搜索、天气和语音等服务按配置访问外部端点。代码实现与数据流见 [代码 Wiki](CODE_WIKI.md)。

## 能做什么

| 能力 | 说明 |
| --- | --- |
| 多角色陪伴 | 流式私聊、群聊、跨角色对话、主动开口；支持中文、英文和日文界面。 |
| 记忆与成长 | 跨会话记忆、用户画像、关系、日记与笔记；相处方式可从真实用户记忆中逐步调整，保留来源并支持撤回。 |
| 情绪与感知 | 时间、天气、媒体、前台应用与活动观察参与回复；心情影响表达，并在对话后更新。 |
| 桌面互动 | 拖动、轻触反馈、长按互动、动作与表情、鼠标跟随及桌面气泡。 |
| 工具协作 | 文件、应用、媒体、截图识别、待办、提醒、搜索和网页读取；按权限执行，由角色根据实际结果回复。 |
| 工作智能体 | 代码阅读与修改、命令执行、工作区、终端、计划、子任务和成果回流；工作模型独立配置；工作页支持会话全文搜索与键盘快速切换。 |
| 扩展能力 | 技能按需加载，支持自建工具、插件和 MCP；浏览器扩展桥连接真实标签页。 |
| 语音与远程 | 语音识别、TTS、实时语音及手机端 Web 访问；按所选服务配置。 |
| 3D 公寓 | 独立可选插件，包含房间与街区场景；基础程序可单独安装和运行。 |

陪伴侧由主智能体保留角色与记忆、决定工具调用；多步执行使用隔离的中性上下文，工具回执交回角色表达。前台查询、截图等直接等待实际结果。前台窗口与最近外部窗口有明确区分，运行中的录屏软件不等于已经确认正在录制；截图识别还取决于所配置的视觉模型是否可用。

工作页点击搜索图标或按 `Ctrl+K` 可搜索会话标题、消息内容和工作区；用方向键选择、`Enter` 打开，`Alt+1–9` 快速打开前九项，`Esc` 关闭。记忆页的匹配内容会高亮显示。

## 快速开始

### 使用安装包

从 [Releases](https://github.com/SpacervalLam/Vivian-ai-desktop-pet/releases) 获取对应发行包。基础安装程序使用 NSIS；如需 3D 公寓，将配套公寓 ZIP 与 setup 放在同一目录，并在安装时勾选。安装后可在「设置 → 通用」启用或禁用公寓。

首次运行，在设置中配置模型端点、模型名和 API Key，使用一键检测检查连接。再按需要配置工作模型、语音、搜索和外部连接。当前支持 Windows 10 / 11，需要 WebView2。

### 从源码运行

准备 Rust **1.88 或以上**、Node.js **22 或以上**、Windows MSVC C++ 构建工具与 WebView2。角色图集随仓库分发，开发模式无需另行准备角色模型。

```powershell
git clone https://github.com/SpacervalLam/Vivian-ai-desktop-pet.git
cd Vivian-ai-desktop-pet
npm install
npm run tauri:dev
```

`npm run dev` 只启动前端 Vite 服务；完整桌面功能应使用 `tauri:dev`。

### 构建与检查

以下命令均在仓库根目录运行：

```powershell
# 主前端：类型检查、构建及公寓外置边界检查
npm run build

# Rust 编译检查
cargo check --manifest-path src-tauri/Cargo.toml

# 基础程序 NSIS 安装包
npm run tauri:build

# 独立构建公寓插件 ZIP
npm run build:apartment

# 生成基础 setup、配套公寓 ZIP 和校验信息
npm run package:small
```

基础安装包位于 `src-tauri/target/release/bundle/nsis/`，配套发行文件位于 `release/`。主程序的 `npm run build` 不构建公寓，Three.js 和场景资源由插件独立管理；安装与预览方式见 [公寓插件说明](plugins/3d-apartment/README.md)。

发布版使用加密美术资源包。打包前须准备匹配的 `src-tauri/vivian.bundle.enc`、`vivian.bundle.index.json` 与 `asset_key.bin`；密钥不进入版本管理。资源更新后也需重新生成匹配的包、索引与密钥，不能把缺少密钥时可编译的开发兜底当作可用发行包。

## 配置与数据

- **模型与用量**：可为对话、推理、记忆、反思、视觉等任务分配路由；工作模型覆盖仅作用于工作智能体。用量页按任务查看调用与 Token 消耗。
- **搜索与网页**：支持 DuckDuckGo、SearXNG、Tavily 和 DeepSeek 搜索后端，按启用状态使用。搜索默认陪伴侧 5 条、工作侧 10 条；可配置结果数、引擎和过滤条件，网页正文支持分页续读。旧 Bing 配置只保留兼容提示。
- **工具权限**：风险等级、访问级别、工作区与确认策略共同约束调用；执行上下文不会因工具返回内容获得额外授权。浏览器桥和 MCP 在外部连接页管理。
- **本地数据**：默认位于 `%APPDATA%\vivian\`，配置为 `config.yaml`，角色数据按 `characters/<角色 ID>/` 分隔。
- **备份与恢复**：「设置 → 备份与恢复」可导出 `.altn`、导入备份或恢复初始状态；导入和重置需要确认并重启。

恢复初始状态会清除记忆、历史、心理与成长记录、笔记、工作会话、截图，以及用户数据目录中的自建技能、插件和 MCP 配置；保留主配置、凭据、安全白名单及必要运行设施。重启时先完成清扫再初始化，删除失败时保留重置标记并阻止读取旧数据。需要保留使用期数据时，先导出备份。

## 开发参考

- [代码 Wiki](CODE_WIKI.md)：模块职责、关键函数、数据流与并发约定。
- [语义路由与提示词准备](docs/semantic-prompt-routing-2026-09-30.md)、[提示词协调](docs/prompt-coordination-2026-10-01.md)：如何选择当前对话需要的上下文。
- [工具执行隔离](docs/tool-execution-isolation-2026-10-01.md)、[陪伴与工作工具对齐](docs/companion-tool-parity-2026-10-01.md)：执行边界、回执和验证范围。
- [搜索与网页读取](docs/web-search-upgrade-2026-10-01.md)、[运行资源与人设成长](docs/runtime-persona-optimization-2026-10-01.md)：相关实现与受控测量。

修改代码后运行对应编译检查和相关测试；发布前还需实际检查安装、窗口交互和所配置模型的表现。专题文档中的历史测试结果不代表当前所有模块均通过。

问题反馈：[GitHub Issues](https://github.com/SpacervalLam/Vivian-ai-desktop-pet/issues)；联系：spacervallam@gmail.com。

采用 [MIT 许可证](LICENSE)。
