# Vivian 代码 Wiki

本文档为 Vivian 项目的代码架构百科，按模块层次组织，记录关键模块职责、核心数据结构、关键函数与跨模块数据流。配合 [README.md](file:///g:/vivian-rs/README.md) 使用——README 侧重功能特性，本 Wiki 侧重代码实现。

> 项目根目录：`g:\vivian-rs\`
> 后端入口：[`src-tauri/src/lib.rs`](file:///g:/vivian-rs/src-tauri/src/lib.rs)
> 前端入口：[`src/main.tsx`](file:///g:/vivian-rs/src/main.tsx) + [`src/App.tsx`](file:///g:/vivian-rs/src/App.tsx)
> 工具权限矩阵：`ToolRiskTier`（风险等级，6 级）× `AgentAccessLevel`（访问级别，4 级）经 `policy_for()` 决定 `allow`/`ask`/`deny`；定级规则与当前分布见下方「沙箱 / 风险等级申报」节（工具 `risk()` trait 缺省 `Safe` 即放行，新工具须显式声明）。

---

## 目录

- [顶层架构](#顶层架构)
- [前端架构](#前端架构)
  - [前端构建（多窗口按需加载）](#前端构建多窗口按需加载)
  - [桌宠长按手势与施法动画](#桌宠长按手势与施法动画)
  - [心智观察器页面合并（MindInspector）](#心智观察器页面合并mindinspector)
    - [置顶摘要（PinnedSummary）](#置顶摘要pinnedsummary)
  - [暖纸主题（UI 视觉统一）](#暖纸主题ui-视觉统一)
  - [3D 公寓窗口](#3d-公寓窗口)
  - [ConnectionsPanel.tsx —— 外部连接页](#connectionspaneltsx--外部连接页)
- [核心数据结构](#核心数据结构)
- [模块详解](#模块详解)
  - [brain/ —— 大脑核心](#brain--大脑核心)
  - [coding_agent/ —— 编程智能体](#codingagent--编程智能体)
  - [task_service —— 自治任务与后台回流](#task_service--自治任务与后台回流)
  - [pipeline/ —— 对话流水线](#pipeline--对话流水线)
  - [cross_character.rs —— 跨角色通信总线](#cross_characterrs--跨角色通信总线)
  - [conversation/ —— 会话生命周期](#conversation--会话生命周期)
  - [memory/ —— 三层记忆系统](#memory--三层记忆系统)
  - [mind/ —— 心智合成层](#mind--心智合成层)
  - [psychology/ —— 心理学因果链](#psychology--心理学因果链)
  - [proactive/ —— 主动对话编排](#proactive--主动对话编排)
  - [skills/ —— 技能服务](#skills--技能服务)
  - [tools/ —— 工具系统](#tools--工具系统)
  - [自建工具系统（custom_tools）—— 能力自进化的执行侧](#自建工具系统custom_tools--能力自进化的执行侧)
  - [providers/ —— 多 Provider 路由](#providers--多-provider-路由)
  - [notebook/ —— 笔记系统](#notebook--笔记系统)
  - [network/ —— 网络基础设施与搜索后端](#network--网络基础设施与搜索后端)
  - [discovery/ —— 多平台内容发现与推荐](#discovery--多平台内容发现与推荐)
  - [browser_bridge/ —— 浏览器自动化桥](#browser_bridge--浏览器自动化桥)
  - [world/ —— 真实世界感知](#world--真实世界感知)
  - [dialogue/ —— 对话历史管理](#dialogue--对话历史管理)
  - [engine/ —— 桌宠表现层](#engine--桌宠表现层)
  - [presence/ —— 在场状态与后台任务](#presence--在场状态与后台任务)
  - [speech/ —— 语音系统](#speech--语音系统)
  - [commands/ —— Tauri 命令层](#commands--tauri-命令层)
  - [remote/ —— 远程访问 HTTP 服务](#remote--远程访问-http-服务)
  - [persona/ —— 人格定义与场景](#persona--人格定义与场景)
  - [emotion/ —— 情绪分类](#emotion--情绪分类)
  - [utils/ —— 通用工具](#utils--通用工具)
- [关键数据流](#关键数据流)
- [持久化模式](#持久化模式)
- [并发与锁策略](#并发与锁策略)

---

## 顶层架构

```mermaid
flowchart TD
    FE["前端 (React + TS + Zustand)"]
    FE_SUB["App.tsx / ChatWindow / MemoryWindow / MindInspector / ConfigWindow"]
    CMD["commands/ (37 个 Tauri 命令模块)"]
    APP["AppState (state.rs)"]
    APP_SUB["characters: HashMap&lt;char_id, CharacterInstance&gt;<br/>session_coordinator / shared_resources / world / ..."]
    BRAIN["Brain (大脑核心)"]
    PET["PetController (桌宠控制)"]
    VOICE["Realtime Voice"]
    MOD1["pipeline/ — 对话流水线"]
    MOD2["memory/ — 记忆系统"]
    MOD3["mind/ — 心智合成"]
    MOD4["psychology/ — 心理因果链"]
    MOD5["dialogue/ — 对话历史"]
    MOD6["proactive/ — 主动对话"]
    MOD7["persona/ — 人格定义"]
    MOD8["providers/ — LLM Provider"]
    MOD9["tools/ — 工具系统"]
    MOD10["presence/ — 在场状态"]
    MOD11["network/ — 网络基础设施与搜索后端"]
    MOD12["world/ — 世界感知"]

    FE --- FE_SUB
    FE_SUB -.->|Tauri IPC| CMD
    CMD --> APP
    APP --- APP_SUB
    APP --> BRAIN
    APP --> PET
    APP --> VOICE
    BRAIN --> MOD1 & MOD2 & MOD3 & MOD4 & MOD5 & MOD6
    BRAIN --> MOD7 & MOD8 & MOD9 & MOD10 & MOD11 & MOD12
```

### CharacterInstance（角色实例）

定义于 [`state.rs`](file:///g:/vivian-rs/src-tauri/src/state.rs)。每个角色独立持有一份完整资源：

| 字段 | 类型 | 职责 |
|------|------|------|
| `id` | `String` | 角色 ID（`"vivian"` / `"nana"`） |
| `name` | `String` | 显示名称 |
| `brain` | `Arc<Brain>` | 大脑核心，持有 memory/dialogue/psychology/persona/proactive 等所有子系统 |
| `pet_controller` | `Arc<PetController>` | 桌宠控制器，管理桌宠窗口与状态机 |
| `manifest` | `Arc<ResourceManifest>` | 模型清单，表情/动作映射 |
| `realtime_voice` | `Arc<RealtimeVoice>` | 实时语音会话 |
| `think_lock` | `Arc<Mutex<()>>` | 思考互斥锁，串行化 think 调用 |
| `online` | `RwLock<bool>` | 在线状态 |

### AppState（全局状态）

```rust
pub struct AppState {
    pub characters: Arc<RwLock<HashMap<String, CharacterInstance>>>,
    pub active_character_id: RwLock<String>,
    pub session_coordinator: SessionCoordinator,         // 跨角色/用户/proactive turn 协调
    pub world: Arc<EnvironmentContext>,                  // 世界快照
    pub shared_resources: Arc<SharedResources>,          // 跨角色共享资源
    pub config: Arc<RwLock<Config>>,                     // 全局配置
    pub model_router: Arc<ModelRouter>,                  // LLM 路由矩阵
    pub tool_system: Arc<ToolSystem>,                    // 工具系统
    // ... 更多共享资源
}
```

---

## 前端架构

### 前端构建（多窗口按需加载）

桌宠由多个 Tauri 窗口组成（桌宠角色窗口 / Chat / Memory / Config / Bubble / Toast / SideChat / MessageBanner 等），每个窗口通过 `?view=` 参数加载不同的 React 组件。

- **逐窗口动态 import**（[`src/main.tsx`](file:///g:/vivian-rs/src/main.tsx)）：`main.tsx` 不再静态导入全部窗口组件，而是按 `view` 参数对各自组件做 `await import(...)`。主窗口（无 view）只加载 App，不再打包 Chat / Memory / MindInspector / Config 等它用不到的代码，显著降低主窗口首帧解析量。
- **vendor 拆包**（[`vite.config.ts`](file:///g:/vivian-rs/vite.config.ts)）：`build.rollupOptions.output.manualChunks` 将稳定依赖拆成独立 chunk —— `react`（react/react-dom/zustand）、`tauri`（@tauri-apps）、`i18n`（i18next/react-i18next）。多窗口共享这些 chunk 的高效缓存、并行加载。
- **target es2022**：WebView2 为常青 Chromium，无需为旧浏览器降级转译，减少产物体积。
- **按需 chunk 兜底**：其余依赖（mermaid 等）保持 Vite 默认基于动态 import 的按需拆包，不合并成单一巨型 vendor 包（避免本来懒加载的库被提前加载）。
- **Main 控制器窗口瘦身**（[`src/main.tsx`](file:///g:/vivian-rs/src/main.tsx)）：隐藏控制器分支（`view=hidden_controller`）直接 return 不渲染任何 React 组件，同时跳过 i18n 初始化与 global.css 加载，仅保留最小 Tauri IPC 桥接层，消除控制器窗口的 UI 渲染开销。

### 桌宠长按手势与施法动画

桌宠窗口支持「按住不拖动满 1 秒**开关心智观察器**」手势，纯前端实现（Rust 零改动）：

- **动作是开关，不是「打开」**（`App.tsx` 的 `toggleMemory`）：窗口此刻已经摆在屏幕上（可见且未最小化）→ 最小化；其余情况（没开过 / 已最小化 / 被 hide）→ 走 `openMemory`（打开、提到前台、播入场动画）。判据用「在不在屏上」而不是窗口焦点：按下桌宠那一刻焦点就被桌宠窗口抢走了，子窗口的失焦回调还会顺手把它降回非置顶，等 1 秒长按成立时它早已不是前台窗口——按焦点判断的话最小化这条路永远触发不了。
- **托盘菜单与全局快捷键仍走 `openMemory`**（`onOpenMemory` / `window:shortcut` 的 `memory`），语义是明确的「打开」，不跟着变开关；`openMemory` 本身也保持原语义（打开/提到前台 + 入场动画）。

- **手势判定**（[`App.tsx`](file:///g:/vivian-rs/src/App.tsx)）：挂在背景层根 `mousedown`（`handleBackgroundMouseDown`，宠物本体点击会冒泡到根 div；双角色窗口共用 App，天然覆盖所有桌宠）。`startHold` 启动 200ms 定时器 → 到点挂载环形进度槽并调 `startCast(800)`；进度环填满（总 1s）触发 `completeHold` → `stop_window_drag` 清后端 DRAG_OFFSET（用户仍按着左键，防止松手被甩飞采样解析）+ `holdActionRef`（心智观察器开关 `toggleMemory`，见上）；`holdCompletedAtRef` 吞掉触发后 500ms 内的 click 余波（不触发摸头台词）
- **环形进度槽**（[`HoldProgressRing.tsx`](file:///g:/vivian-rs/src/components/HoldProgressRing.tsx)）：SVG 双圆环（半透明深色轨道 + 白色进度弧），白色弧用 **Web Animations API**（`el.animate`）把 `strokeDashoffset` 从整圈周长线性跑到 0，从 12 点方向顺时针填满；完成信号取动画的 `finished`（动画被 cancel 时它会 reject，这种情况不算完成），系统开了「减弱动态效果」或拿不到动画能力时直接判满，保证长按不会被卡住；`pointerEvents: none` 不影响拖拽/穿透判定。**这里必须是 WAAPI，不能退回 CSS 声明式动画**——CSS 要求元素的 `animation-name` 在**首次样式计算时**就能匹配到 `@keyframes`，而本组件的时长与圆周长都由 props 和圆几何在运行时算出，规则后到的话 Chromium 不会为它回溯启动动画，进度弧会一直停在 0 长度（只剩一条空槽），最后只有兜底定时器在收尾
- **取消路径三合一**（`cancelHold`）：`window mouseup` / `onMoved` 窗口位移超 `HOLD_MOVE_TOLERANCE_PX`(10 物理px) / 后端 `drag:cancelled` watchdog。拖动判定必须用窗口位移——拖拽时窗口跟随光标移动，client 坐标的 mousemove 检测不到；`startHold` 记录按下时 `outerPosition` 作基准。时长/容差常量集中在 App.tsx 顶部：`HOLD_RING_DELAY_MS=200` / `HOLD_OPEN_TOTAL_MS=1000` / `HOLD_MOVE_TOLERANCE_PX=10`
- **施法动画会话**（[`ChibiPetCanvas.tsx`](file:///g:/vivian-rs/src/components/ChibiPetCanvas.tsx)，`startCast` / `cancelCast` / `stopCast`）：施法与进度环同步——进度环出现时起播，每帧时长按 `durationMs / 素材总时长` 等比缩放（下限 16ms，保持原作节奏），约 0.8s 播完与环填满对齐；12 帧 4×3 雪碧图帧推进走 `sequenceTokenRef` 令牌，会话 `{token, 当前帧, 帧时长表}` 记在 `castSessionRef`。取消时从当前帧倒放回初始帧再归位 idle；`stopCast` 供完成路径兜底归位（不倒放）；`startHold` 开头先 `stopCast`，避免上一段取消倒放与新会话叠加
- **窗口已在时不重建、由子窗口自己显形**（`App.tsx` 的 `openWindow` + [`utils/petReveal.ts`](file:///g:/vivian-rs/src/utils/petReveal.ts)）：从「不在屏上」的状态长按（没开过 / 已最小化 / 被 hide，含托盘菜单与快捷键入口）时，既不重建也不 navigate（前者 label 冲突，后者整页 reload 会丢页签与输入），而是「提到前台 + 让子窗口自己再播一遍入场」。桌宠把当前窗口矩形随 `pet:reveal` 事件发给 memory 窗口，**播哪一条动画由子窗口按「此刻在不在屏上」自己选**——只有它看得到自己的最小化状态，两条路径对「显形」的假设又正好相反：

  - **在屏上（可见且未最小化）→ `replayPetReveal`**：先 180ms 收拢回桌宠矩形、再 340ms 展开，展开段与首开逐帧一致（差别只在多了一段收拢）。内容已经在屏上，直接压到起手帧会是一次「全屏啪地塌成小卡片」的可见跳变，而 hide/show 又会闪、会抖 Z 序，所以只能靠收拢过渡。
  - **不在屏上（被最小化 / 被 hide）→ `playPetReveal`**：与首开同一条路，先把首帧摆成桌宠大小再显形。这里没有「别跳变」的约束，但有一个必须避开的坑：若照旧先还原窗口，那次还原本身就是一次呼出，随后的收拢展开是第二次——看起来就是「呼出了两回」。`playPetReveal` 因此自己负责还原（`unminimize` 夹在「摆好首帧」与「show」之间，最小化时未最小化窗口是无操作，可无条件调用）。

  显形时机因此整个归子窗口：`raiseWindow` 对这类窗口带 `selfReveal`，只做置顶与聚焦，**完全不碰可见性**（连 `unminimize` 都不做，「等还原落定」那段轮询也只在非 selfReveal 时跑）；两个入口（创建 / 复用）都用 `armSelfRevealFallback` 兜底——事件丢失或子窗口脚本异常时 1.6s 后强制显示，宁可直接显形没动画，也不能让长按毫无反应。重复通知用 `replayBusyRef` 互斥。复用判定只有一条规则：`isVisible()` **抛异常**才算引用失效（窗口已销毁）；它返回 false 只说明窗口被 hide 过，仍按复用处理、由 `raiseWindow` show 回来。窗口还在却去重建必然 label 冲突，而 Tauri 对冲突只在 console 打一行 `tauri://error`，对外表现就是「点了毫无反应」——同理，复用分支里任何窗口动作（设 topmost / 聚焦 / 改属性）都必须单独兜异常，不能让它的失败把活着的窗口判成死的

### 心智观察器页面合并（MindInspector）

Memory 窗口（`MemoryWindow`，默认全屏大小）内嵌 [`MindInspector.tsx`](file:///g:/vivian-rs/src/components/mind-inspector/MindInspector.tsx)，侧边栏导航合并为 3 项，整个心智观察器使用「手账暖纸 + 纸胶带 + 点阵底纹」视觉体系：

- **外壳结构**（`MindInspector.tsx`）：纵向 = 顶部封面条（`.mind-sb-cover`，「Mind Scrapbook | 当前页名」+ 日期印章）+ 主体（左贴纸导航栏 `mind-nav-rail` 3 项 + 右内容区 `mind-page-content`）。窗口顶部原生标题栏已删除——封面条标题区即窗口拖拽区（`data-tauri-drag-region`），最小化/关闭按钮直接置于封面条右侧；页面经 `NavigationContext.setHeaderExtra` 注入的工具栏（如 DiaryPage 的角色切换/日期筛选）与日期印章、窗口按钮并排显示在封面条右侧，不再单独占一行
- **综合页（overview）** = [`OverviewPage.tsx`](file:///g:/vivian-rs/src/components/mind-inspector/pages/OverviewPage.tsx)：页内顶部手账 Tab 切换 `mind`（MindPage）/ `world`（WorldPage）/ `graph`（GraphPage）/ `profile`（UserProfilePage），缓存上次选择；页头 = 大标题「综合」+ 铅笔虚线 + 当前子视图胶囊
- **记忆图谱页（graph）** = [`GraphPage.tsx`](file:///g:/vivian-rs/src/components/mind-inspector/pages/GraphPage.tsx)：时间线 + 迷你地图 + 类型多选筛选 + 会话圈；底部「角色成长记录」区块（[`graph/EvolutionSection.tsx`](file:///g:/vivian-rs/src/components/mind-inspector/pages/graph/EvolutionSection.tsx)）——消费 `get_persona_evolution`（已注册进 invoke_handler，返回 `entries` + `candidates`），手账贴纸风展示人格自进化覆盖层：已生效调整（日期戳 + 语气/性格贴纸 + 调整内容 + ↳ 依据 + 印证 ×N 红章）与「酝酿中」候选（虚线弱化态 + `count/total` 支持进度），随角色切换、带手动刷新；加载失败静默降级空态，不打断图谱
  - **类型筛选（多选，会重建时间轴）**：筛选状态是 `Set<NodeType>`，「全部」清空回全显、user/agent 核心节点恒显。UI 上「对话」「微信」合并为「聊天」chip（覆盖 `dialogue`+`wechat` 两类底层节点，计数取两者之和），i18n 走 `type_chat` 三语。筛选**不是渲染层过滤，而是重建时间轴**——以可见节点时间戳重建压缩比例尺（复用 `buildTimeScale`，节点间距 `clamp(gap×K, 56, 140)`），节点按时间紧凑排列、画布高度随可见节点最低点刷新收缩（实测 vivian 全量 20900px → 筛「聊天」4973px，约 4.2×）。三条必须保持的不变式：① 折叠的 `summarized` 子节点必须排除出比例尺与防碰撞，否则堆在端点把可见节点顶下去、重新撑出留白（vivian 聊天类 78 条中含 32 条 summarized）；② 懒加载在筛选态下须拉满全量，压缩比例尺只覆盖已加载时间窗，否则自我封闭——全量记忆 JSON 仅 ~0.27MB(vivian)/0.2MB(nana)，可接受；③ 骨架索引 `skIndex` 在筛选态失效（骨架是全集），一律改按时间戳定位。会话圈（同一次会话的手绘圈）**仅当选中「聊天」时渲染**，其余筛选态隐藏；筛选切换按视口中心时间重锚滚动，避免高度骤变导致视口跳变。选中态 chip 用实色填充 + 白字 + 白点 + 轻投影（不加粗——手写体无真 bold，合成加粗会糊）。
- **创作页（journal）** = [`JournalPage.tsx`](file:///g:/vivian-rs/src/components/mind-inspector/pages/JournalPage.tsx)：子 tab 切换 `diary`（DiaryPage）/ `notebook`（NotebookPage）/ `planner`（PlannerPage，待办+定时合并），支持 `memory:navigate` 事件定位；页头同样为「创作」大标题 + 子视图胶囊
- **工作页（code）**：Codex 布局 + 手账风格三栏工作台，由 [`CodeAgentPageNew.tsx`](file:///g:/vivian-rs/src/components/mind-inspector/pages/CodeAgentPageNew.tsx) 提供实际实现（左栏会话/工作区管理 / 中栏对话流 + 单轮工作过程分组折叠 / 右栏检查器：概览 + 轨迹 + 内嵌终端）

#### 置顶摘要（PinnedSummary）

主工作区右侧的信息列，仿 Codex 的「环境信息」面板，实现于 [`pages/PinnedSummary.tsx`](file:///g:/vivian-rs/src/components/mind-inspector/pages/PinnedSummary.tsx) + [`CodeAgentPage.css`](file:///g:/vivian-rs/src/components/mind-inspector/pages/CodeAgentPage.css) 的 `.codex-pinned*`：

- **形态**：**贴在主工作区右缘的一条信息列**（`position: absolute`，`right: var(--codex-sb-w)` 紧贴对话滚动条的左边），不是占位的 flex 列。对话区 / 输入区 / 统计行 / 「回到底部」按钮统一用 `padding-right` 让出 `--codex-pinned-reserve`，所以面板既不盖正文，又不会把滚动条挤到自己左边（滚动条属于铺满整条宽度的 `.codex-chat`，恒在最右侧）。面板与对话区之间**没有分隔线**（`border-left` 已删）——分隔靠底色与留白，不靠线。`codex-main-col` 带 `position: relative`，同时给 `.codex-pinned` 和 `.codex-to-bottom` 当定位基准
- **宽度策略**：三个常量构成优先级——`PINNED_W_IDEAL`(250) / `PINNED_W_MIN`(186) / `CHAT_MIN_W`(430)：先保证对话区 border-box 至少 430（= 工作区宽 − 面板宽），剩下的才给面板；面板自己也不低于 186（再窄「提交或推送」那排按钮会换行）。父组件用 `ResizeObserver` 量 `codex-main-body`，把宽度写成 `.codex-main-col` 上的四个 CSS 变量：`--codex-pinned-w`（动画值，收起为 0）、`--codex-pinned-w-expanded`（展开值，收起期间不变，内层靠它维持展开宽度做裁切）、`--codex-pinned-reserve`（= 面板 + 呼吸缝 `PINNED_GAP_W`(18)，收起为 0）、`--codex-sb-w`（实测滚动条宽）。首帧用 `useLayoutEffect` 同步量一次，否则初始那次宽度变化会被当成动画播一遍。**面板宽度不再走 prop**，宽度策略只该有一个出处
- **让位怎么合成**（`CodeAgentPage.css`）：四处统一用 `max(基准内边距, --codex-pinned-reserve)`。展开时 reserve 远大于基准（≥186+18），正文右缘恰好停在面板左侧 `PINNED_GAP_W`=18px 处（就是「刚好不遮挡」）；收起时 reserve 归零，`max()` 取回基准，左右内边距重新对称。**基准值必须由主题声明成变量**（`--codex-chat-pad-x` / `--codex-composer-pad-x` / `--codex-to-bottom-right`），不能写死在让位规则里——让位规则是 `.codex-main-col .codex-chat`（0,2,0），而主题的 `.mind-main[data-ui-style="minimal"] .codex-chat` 是 0,3,0、窄屏那条 `.mind-inspector-root.is-work-page .workbench-root .codex-chat` 是 0,4,0，写死会被整条盖掉（实测展开时右内边距仍是 34px、窄窗下正文被压住 196px、输入卡片被压住 205px）。滚动条槽在 border 与 padding 之间，正文可用宽本来就减掉了它，所以 reserve **不加**滚动条宽；只有浮动按钮的 `right` 要额外加 `--codex-sb-w` 才能和正文列右缘对齐
- **收放动画（两轴同时）**：320ms `cubic-bezier(0.4,0,0.2,1)`（与两侧边栏同一条曲线），两条轴并行——**横向** `width` 走 0 ↔ 250，**纵向** `clip-path: inset()` 把可见区**从上往下揭开**（呼出）/ **从下往上收掉**（收起）。横向那条必须留着：让位靠 `--codex-pinned-reserve`，面板盒宽恒小于它，同步走才保证任何一帧都不压住正文。纵向两端形状要一致才可插值，所以展开态写 `inset(0 0 0 0)` 而不是 `none`，收起态 `inset(0 0 100% 0)`（底边内缩 100% ⇒ 可见高度归零）；实测顶边内缩全程恒为 0，即「锚在顶边往下揭开」。内层 `.codex-pinned-inner` 固定展开宽度并 `align-items: flex-end` **靠右对齐**——外层变窄时它从左侧被 `overflow` 切掉，内容不横向平移（若默认左对齐，整块会跟着外层左缘左移 250px，读起来就变成「从右边滑进来」了）。四处让位（`padding-right` / `right`）**必须同曲线同步**——不同步的话收起期间正文与面板会互相错位。外加 `visibility 0s` 延迟切换（`visibility` 是阶跃插值，配延迟就能让内容动画播完才真正隐藏，顺带退出 tab 序列）。**不随收起卸载**（`if (!visible) return null` 已删——卸载了就没有元素可做动画）。拖拽调宽期间 `.codex-main-body.resizing` 一次性关掉面板（含 `clip-path`）与四处让位的过渡，否则宽度被缓动拖住
- **输入框不再有蓝色焦点框**：`.codex-composer-textarea` 自身写了 `outline: none`（0,1,0），但 `MindInspectorThemes.css` 的通用 `.mind-inspector-root .workbench-root textarea:focus-visible { outline: 2px solid var(--codex-accent) }`（0,3,1）压过它——手账主题的 accent 是印章青蓝 `#537d96`，于是聚焦时卡片里凭空多一个青蓝框。现在按 0,4,0 加了一条例外把 `.codex-composer-textarea` / `.codex-pinned-input` / `.codex-pinned-select` 设回 `outline: none`，焦点提示改由 `.codex-composer:focus-within`（描边色 + 阴影）承担
- **底色**：`--codex-pinned-bg`，极简主题下覆写为纯白（浅色 `#fff`）/ `#242424`（深色）；其余主题退回 `--codex-paper-card`。**深色那两块（`@media prefers-color-scheme` 与 `:root[data-theme="dark"]`）都要声明**——浅色块同样命中深色环境，漏一处卡片会保持纯白，在深色界面里刺眼
- **显隐开关**：顶栏「模式下拉」与「右侧检查器按钮」之间的便签按钮（`StickyNote`，`aria-pressed` 同步状态）控制整块面板的收起 / 呼出。可见性落盘 `localStorage['vivian.code_agent.pinned_summary_visible']`，**默认展开**——只有显式存过 `'0'` 才默认收起，把「用户主动关过」和「从没设置过」区分开。面板不可见、或环境信息卡收起时不轮询 git。每张卡片内部仍可单独折叠；环境信息卡收起时若工作区脏则显示暖色圆点，避免收起来就失明。注意极简主题把 `.codex-icon-btn` 背景统一压成 `transparent !important`，`MindInspectorThemes.css` 里按 `aria-pressed`/`aria-expanded` 把「已开启」态补回来，否则看不出按没按
- **环境信息**（git 仓库状态）：变更 `+N -M` 与改动文件数 / 本地（仓库目录名）/ 分支（detached 时显示「游离 HEAD」）/ 同步（领先 · 落后 · 未设置上游分支）/ 最近提交（短 hash + 说明 + 时间）。数据来自 `git_repo_status`，工作区或折叠态变化时立即拉一次，之后**仅窗口可见时**每 8 秒轮询（`document.visibilityState`）；请求带序号（`reqSeq`），工作区切得快时旧响应不会覆盖新状态
- **写操作**：`提交或推送`（内联表单，Enter 提交 / 按钮提交并推送，**执行前弹确认框**并写明「会暂存全部改动」与目标仓库名）、`比较分支`（拉 `git_list_branches` 填下拉，默认选中推测基准，出 `git_branch_diff` 的领先/落后/增删/提交列表）。无未提交改动时提交按钮禁用
- **来源**：`list_plugins` / `list_skills` / `list_mcp_servers` 三份清单并行拉取后汇总计数，展开可看插件明细（绿=trusted 生效 / 黄=changed 待重认 / 灰=untrusted）；「已装载」只数 `status === 'loaded'`，跳过的插件不计入
- **不做 PR 集成**：Vivian 没有 PR 状态源，就不摆一行「无法获取 Pull Request 状态」凑数——同步行如实汇报上游分支情况
- **后端**：[`commands/git.rs`](file:///g:/vivian-rs/src-tauri/src/commands/git.rs)，全部走系统 `git` CLI（`-C <dir>` 指定目录，不依赖进程 cwd），不引入 libgit2/git2。`-c core.quotepath=false` 必须带，否则中文路径会变成八进制转义串；Windows 下 `CREATE_NO_WINDOW` 必须加，否则每次轮询闪一次黑框。未跟踪文件的行数单独统计（`git diff --numstat HEAD` 看不见它们），计入「变更 +N」，带 200 文件 / 2MB 预算上限防大目录卡顿

**兼容跳转**：`MindInspector` 的 `resolveNav` 把合并前的子视图跳转（`navigateTo('mind'/'world'/'graph'/'profile'/'diary'/'notebook'/'todo'/'scheduler')`、URL 参数 `nav=...`、`nb_id`、`memory:navigate` 事件）统一映射为「合并页主键 + `pageParams.sub`」，由合并页跟随切换子 tab。导航定义与 `NavKey` 在 [`design-system.ts`](file:///g:/vivian-rs/src/components/mind-inspector/design-system.ts)。

### 暖纸主题（UI 视觉统一）

心智观察器与设置窗口（`ConfigWindow`）共用一套「暖纸信纸」视觉基调，由三处集中 token 驱动：

- **颜色 token**（[`global.css`](file:///g:/vivian-rs/src/styles/global.css)）：`--panel-*` 三块（深色默认 / 浅色跟随 / 浅色强制）重映射为「纸本 + 墨 + 印章青蓝」，对齐宣纸质感——纸张 `#F5EFE4` / 浮起卡 `#FBF7EE` / 侧边栏 `#EFE8DB`；墨色 5 档（浓墨 `#2A2622` → 极淡墨 `#8F867B`）；分割线 `#D8CFBE`；唯一强调色「印章青蓝」`#537D96`（hover `#3F6179`）；语义色克制墨染（成功墨绿 `#4A6B4A` / 危险深朱 `#8B2C1F`）
- **质感**：`.scrapbook-bg` 用多层 `radial-gradient` 模拟宣纸颗粒与暖斑（无需外部图片）；`.scrapbook-card` 收为 1px 细边框 + 3px 极小圆角；全局滚动条改用 `--panel-scrollbar` token（浅色下不再不可见）
- **排版**（[`design-system.ts`](file:///g:/vivian-rs/src/components/mind-inspector/design-system.ts)）：正文切衬线（`Noto Serif SC` / 宋体家族），英文/数字装饰标题保留手写体（`Caveat`）作点缀；圆角 token 极方化（控件「印章取方」：xs 2px / md 4px / xl 8px），呼应信纸邀纸感

页面侧边栏外壳在 `MindInspector.tsx` 挂 `scrapbook-bg` 纸纹底、激活高亮线用印章青蓝；`ConfigWindow` 根容器同样挂纸纹并切衬线字体。视觉整体由 token 层驱动，切换深/浅主题时暖纸调性保持一致。

### 3D 公寓窗口

基于 Three.js 的日式动漫风 3D 宿舍房间（赛璐璐着色 + 描边分级 + finish 材质分档 + 泛光/雾后处理；微缩底座已拆，房间站在雨夜街区中，窗外是真 3D 街景），独立全屏无边框窗口（`?view=room`），入口在心智观察器封面条右上角「进入公寓」胶囊按钮（`mind-sb-cover-apartment`）与主窗口快捷键，两处共用 [`src/utils/roomWindow.ts`](file:///g:/vivian-rs/src/utils/roomWindow.ts)（label 常量 `room`，复用判定走 `WebviewWindow.getByLabel`，跨 WebView 一致）。

- **窗口形态**：屏幕尺寸 + 无边框 + **不透明** + 不可缩放——全屏透明窗口 + WebGL 在 Windows（尤其 N 卡驱动对透明通道处理有缺陷）是 GPU 进程崩溃高危组合。渲染负载控制：`setPixelRatio` 封顶 1.5（2x 的 4 倍像素会逼崩 GPU）、阴影 `mapSize` 1024、`powerPreference: 'high-performance'`（双显卡机器用独显，避免集显 context lost）
- **双模式**（[`RoomScene.tsx`](file:///g:/vivian-rs/src/components/room/RoomScene.tsx)）：
  - **观察者模式**（默认）：OrbitControls 自由旋转/缩放/平移，墙面单向透视形成「剖面娃娃屋」，进房间即全景概览——透视**不是靠背面剔除**（墙材质恒 `DoubleSide`），而是渲染循环按「相机在墙法线哪一侧」切 `wallMeshes[i].visible` / `ws.decor.visible`；凸入室内的部分（窗台/窗帘/盆栽）挂 `ws.roomSideDecor` 永远可见，天花按 `camY <= roomH + 0.3` 显隐。**Enter 进入第一人称**——观察者模式需要鼠标旋转/缩放，不能用点击画面（会跟 OrbitControls 冲突），键盘事件同样满足 requestPointerLock 的手势要求
  - **第一人称**（[`fpsControls.ts`](file:///g:/vivian-rs/src/components/room/anime/fpsControls.ts)）：PointerLock + WASD，移动模型对齐 Quake/Source 摩擦模型——加速向「归一化方向 × 上限」1-exp 平滑逼近（无硬 clamp，斜向不超速、掉头自然急停）、停止走地面摩擦 + stopspeed 强停区（低速快速归零不滑冰）、空中无摩擦保惯性；重力 20 / 跳速 6 / 蹲视高 0.8 / 走 2.2 / 跑 4.5 / 蹲 1.2 m/s；Shift 或鼠标右键加速；head bob 相位不累积进 camera.y。**鼠标防漂移三件套**（浏览器 Pointer Lock 经典问题）：`requestPointerLock({ unadjustedMovement: true })` 去 OS 加速（不支持退回普通锁定）+ 锁定后 120ms 恢复窗口跳过初始巨型 delta + 单次 |movement|>200px 的 spike 过滤（Windows Chromium 下 500-1000Hz 鼠标会随机把 movementX 从个位数跳到几百）
  - **墙的透视按模式分**（改墙体/挂墙装饰前先确认目标模式，别拿剖面娃娃屋的假设套第一人称）：观察者模式=单面透视（上述按相机侧显隐）；**第一人称=双面不透视**——`wallMat` 恒 `DoubleSide`，相机在房间内任何角度都看得到墙的正面，上面那套显隐剔除根本不跑
- **布局数据**（[`dormLayout.json`](file:///g:/vivian-rs/src/components/room/dormLayout.json)）：户型（room/shell）/ 家具 / 门窗洞口（openings）/ 调色板（palette）/ 灯光（lighting）/ 后处理（postfx）/ 天气（weather）全部数据驱动，JSON 热调不重建场景
- **门系统**（[`props.ts`](file:///g:/vivian-rs/src/components/room/anime/props.ts) + [`collider.ts`](file:///g:/vivian-rs/src/components/room/anime/collider.ts)）：
  - **三类门扇**：平开门（`door` / `entryDoor`，门扇挂 `door-leaf` 铰链 pivot、门框 `door-frame`）、玻璃推拉门（`glassDoor`，两扇把手镜像在洞口中央相接处，`depthWrite:false` 半透明）、日式推拉门（`fusuma`，固定扇 + 动扇滑行至重合，净开口半洞）。门**不参与合批/冻结**（合批会把门扇并进静态 mesh、冻结锁死 matrix）；描边在 add 到 scene 前做（addOutline 靠 `updateMatrixWorld` 烘焙相对变换，未挂载时 matrixWorld==本地矩阵最稳）
  - **开关触发 = 近距 + 穿过意图**：先按「玩家 camera + 角色 PetAgent 到门洞中心最近距离 < `DOOR_NEAR`(1.3m)」门控——远处路过不开、有意图但还远也不开；再判穿过意图，观察者模式取 A* 路径前方 `PATH_LOOKAHEAD`(0.9m) 段做线段-门洞相交，第一人称取移动方向前瞻；穿过后 `DOOR_HOLD`(1.4s) 宽限保持开启再关（平开门方向由 `heldTarget` 带符号记住，宽限期内不翻转）。门扇动画手动 `updateMatrix()+updateMatrixWorld(true)`（挂在 freeze 的 decor 组下），门动时置 `key.shadow.needsUpdate` 让投影跟随；关门态门扇 collider 挡 FPS
  - **视觉开口 ≠ 可通过洞口**：推拉门/玻璃门的墙 `shell.walls` 开口按「视觉整段」给（门框渲染满洞），可通过半边用开口上的 `navCut: 'east'|'west'` 标记在 nav 栅格与 FPS 洞口推导两处同源切半；禁止收窄墙开口来制造半边门洞（墙网格会填满另一半，门看着像墙）
- **碰撞三同源**（[`collider.ts`](file:///g:/vivian-rs/src/components/room/anime/collider.ts)）：墙面渲染、导航栅格、FPS 碰撞同一数据源——墙碰撞 `buildWallColliders` 直接吃 `shell.walls` 墙定义（开放 LDK 无墙处不长幻影墙；阳台三面无墙靠 `railings` 参数补薄盒闭合计），家具碰撞 `buildFurnitureColliders` 按 dormLayout.json 标称尺寸绕 Y 取 AABB（实际 mesh 外廓比标称大，用标称才能让 check-layout 的连通性校验同时覆盖 FPS）；FPS 洞口判定 = 洞口两侧 0.3m 落在房间内（外墙入户门外无房间 → 保持实心防走出楼）；`collidesAt` 跳过头顶高（`min.y > headY`）与脚踝低（`max.y < footY+0.12`）的盒子。`PLAYER_R=0.15`
- **视觉管线**（[`toon.ts`](file:///g:/vivian-rs/src/components/room/anime/toon.ts) + [`merge.ts`](file:///g:/vivian-rs/src/components/room/anime/merge.ts)）：
  - **后处理**：`RenderPass`（HalfFloat + 自建 MSAA RT samples:4）→ `UnrealBloomPass` → `OutputPass`，顺序不能反；`NeutralToneMapping`（非 ACES）；`scene.background` 是世界底色（`postfx.background`，恒夜 `#1b2230`），**雾色必须 ≈ 背景色**——远处物体是融进夜空，雾色偏了会露地平线接缝；相机 far=220（世界地面角距相机可达 ~140m，裁掉会露「世界裂缝」）；`composer.dispose()` 显式调用；切换 scene.fog 会触发全场材质重编译（第一下卡一帧）
  - **材质 finish 分档**：`toon()` 的 `finish` 参数（`matte/soft/metal/glass/wet`）经 `onBeforeCompile` 注入风格化高光 + 边缘光——只用 built-in 量、禁自定义 uniform，必须 `customProgramCacheKey='room-toon-'+finish` 防不同 finish 串编译缓存；`toonCache` key 含 finish；主光方向由 RoomScene 调 `setToonKeyLight` 告知（在造任何家具之前）
  - **描边分级**：weight 打在材质 `userData.outlineWeight` 上（2 主结构全厚 / 1 次结构 ×0.55 细线 / 0 不描），`addOutline` 按材质分桶各建一壳；默认 matte/soft→2、metal/wet→1、glass/emissive→0（黑壳会把 bloom 光晕闷死在轮廓里），ToonOpts/emissive 可显式覆盖，覆盖值进 toonCache key
  - **合批**（`mergeByMaterial`）：按材质桶压 mesh，`noMerge` / `noOutline` 标记走特殊路径——`noOutline` 是 OR 传播（同桶一件带标记整桶不描边），需要描边的件必须同时标 `noMerge` 防被吞；倒角基元 `box()` = `RoundedBoxGeometry`（BEVEL_RADIUS=0.008 全屋统一 8mm）是非索引几何，桶内混有非索引时合批前把索引几何统一 `toNonIndexed()`，否则 mergeGeometries 拒合整桶退化
- **室外层**（[`exterior.ts`](file:///g:/vivian-rs/src/components/room/anime/exterior.ts)）：微缩底座已拆，房间站在雨夜街区——170m 湿沥青世界地面（居中跟房间包围盒走，边缘被雾吞掉）+ 近景街道层（对面 9 栋低模楼带双色温窗灯、3 盏路灯 `halo:false` 守零新增透明预算、街道湿地光斑）+ 公寓楼外壳（201 所在这栋三层，201 段外皮由房间 shell 自己承担不重复建）+ 街角便利店（静态部分走标准装配，招牌灯箱 / 自动门 / 红绿灯动效独立挂载每帧更新）。外景用世界绝对坐标不跟房间包围盒居中、不进 FPS 碰撞（阳台栏杆拦着玩家出不去）；窗外 sky plane 已拆——窗外就是真 3D 街景，窗玻璃水痕层保留
- **天气**（dormLayout.json `weather`）：落雨禁区 = 有顶房间（`ceiling!==false`）的并集包围盒自动算出，阳台不算禁区（雨要落进阳台）；`rain.margin` 盖住南街；`buildRain` 的 `update(elapsed)` 挂渲染循环，`buildWetGround` 光斑 / 碎光点支持 `center` 指定（不指定会以原点铺开洒进室内）
- **布局验收哨兵**（改布局 / 碰撞 / 门洞 / 墙后必跑）：观察者导航栅格校验（越界 / 互穿 / 洞口错配 / 房间连通——家具 AABB 必须整体落在单房间 interior 内，是硬错不是软碰撞）+ FPS 碰撞盒完整复刻（出生点 BFS 全屋可达 / 不走出建筑 / 幻影墙自检；与 nav 栅格是两套口径，`PLAYER_R`/`WALL_HALF`/`RAIL_HALF`/`MIN_H` 参数必须与运行时同步）
- **资源释放**（cleanup 顺序敏感）：`renderer.forceContextLoss()` 先于 `renderer.dispose()`（先断 GPU 上下文再释放，防资源悬挂）；遍历灯光显式 `light.dispose()` + `light.shadow.map.dispose()`（阴影 RT 惰性创建，不随 renderer 自动释放）；`toonCache` / `sharedOutlineMaterial` 缓存材质以 `userData.__roomCached` 打标，cleanup 跳过（跨重建复用，释放反而造成下次重建闪断）；toonifyModel 材质化收齐全部 10 个贴图槽进释放 sink，只收 map/normalMap 会漏
- **互斥渲染**（[`window.rs`](file:///g:/vivian-rs/src-tauri/src/commands/window.rs) `set_room_mode_internal`）：进出房间统一收口——角色窗口 hide + TrySuspend 冻结（渲染进程挂起）、心智观察器只 hide。关键时序：
  - 进房间：roomWindow.ts 的 tauri://created 回调**先 set_room_mode(true) 再 show**（RoomWindow 挂载再调一次，幂等）
  - 退房间：`lib.rs` on_window_event 的 CloseRequested / Destroyed 只做 set_room_mode(false)。**CloseRequested 里绝不能对 room 窗口本身调 hide() 等操作**——事件处理返回后 close 流程才继续销毁，中途操作窗口会引入竞态、导致 ESC 关不掉窗口（已踩）
  - `freeze_webview` 里**不能查 `is_visible()`**——hide 走主线程 FIFO 队列是异步的，hide 后立即查 is_visible 还是旧值 true，会把正常冻结误判成「窗口还可见」而跳过，桌宠后台空转。防快速 hide→show 竞态只靠 `WEBVIEW_FREEZE_GEN` 代计数器
- **ESC 关闭**：PointerLock 下浏览器吞 ESC（用于退出锁定、不派发 keydown），Rust `watch_room_escape` 线程用 GetAsyncKeyState 轮询 ESC 下降沿（20ms，快于人类点按最短时长）+ `is_room_foreground`（is_focused 优先 + GetForegroundWindow 兜底，任一 true 即前台）关窗口；观察者模式由前端 keydown（capture 阶段 + `e.code === 'Escape'`）兜底。幂等靠 `ROOM_MODE_ACTIVE`（前端卸载 invoke + Rust 窗口事件两条路径都到达）

### ConnectionsPanel.tsx —— 外部连接页

设置页的「外部连接」页（`src/components/ConnectionsPanel.tsx`）合并了原先分散的两处入口：独立的「浏览器」页 + 工具页里的 MCP 区块。合并依据是两者同属「给 AI 接外部能力的连接」，只是方向相反——内置连接器由扩展反向连入，MCP server 由 app 拉起子进程。

- **内置连接器区**：浏览器桥卡片（连接状态 + 从 `list_tools` 实时取 `mcp__browser__*` 工具清单 + 扩展安装引导），未连接时展开三步引导
- **凭据状态子区**（卡片内，不与 MCP Servers 并列）：平台登录态网格。**它不是连接、也不提供任何工具**——只是扩展 Cookie 哨兵探测出的 `HashMap<String, bool>`（`server.rs::report_platform_status`），唯一消费方是 `discovery/sources/*` 的被动采集器。放在桥卡片内可避免用户误以为「登录某平台即获得该平台工具」
- **MCP Servers 区**：原工具页的 MCP 增删改 UI，状态与加载逻辑一并从 `ConfigWindow` 迁入本组件（`ConfigWindow` 不再持有 `mcpServers` / `mcpEditing` / `mcpSaving`）

---

## 核心数据结构

### ChatMessage（对话消息）

定义于 [`types/response.rs`](file:///g:/vivian-rs/src-tauri/src/types/response.rs)。

```rust
pub struct ChatMessage {
    pub role: String,         // "user" / "assistant" / "system"
    pub content: String,
    pub meta: Option<MessageMeta>,
}

pub struct MessageMeta {
    pub channel: String,           // "wechat" / "direct" / "proactive" / "cross_character"
    pub speaker: Option<String>,   // 说话者 ID
    pub listener: Option<String>,  // 听话者 ID
    pub timestamp: Option<f64>,
    pub images: Vec<MessageImage>,
    // ... 文件元数据 / 工具调用标记等
}
```

### AiResponse（LLM 响应）

```rust
pub struct AiResponse {
    pub text: String,
    pub intent: String,
    pub response_mode: String,     // speak / non_verbal / internal / ignore
    pub tool_calls: Vec<ToolCall>,
    pub expression: String,
    pub motion: String,
    pub control_actions: Vec<ControlAction>,
    // ...
}
```

### PipelineState（流水线状态）

定义于 [`pipeline/state.rs`](file:///g:/vivian-rs/src-tauri/src/pipeline/state.rs)。73 个字段贯穿全链，主要字段：

```rust
pub struct PipelineState {
    pub user_input: String,
    pub messages: Vec<ChatMessage>,           // 对话历史
    pub memory_text: String,                  // 检索后的记忆上下文
    pub world_brief: WorldBrief,              // 世界快照
    pub character_block: String,              // 人格块
    pub user_model_text: String,              // 用户认知模型文本（UserModel → PromptBuildingStep）
    pub tools: Vec<ToolSchema>,               // 可用工具
    pub response: Option<AiResponse>,         // LLM 响应
    pub metadata: PipelineMetadata,           // 跳过标志/检索结果等
    // ... 50+ 字段
}
```

### MemoryItem（记忆条目）

定义于 [`memory/types.rs`](file:///g:/vivian-rs/src-tauri/src/memory/types.rs)。

```rust
pub struct MemoryItem {
    pub id: String,
    pub content: String,
    pub memory_type: MemoryType,              // ShortTerm / MidTerm / LongTerm / Knowledge 等
    pub importance: f64,
    pub evidence_score: f64,                  // 证据驱动可信度
    pub created_at: f64,
    pub last_accessed: f64,
    pub tags: Vec<String>,
    pub metadata: serde_json::Value,          // speaker/listener/perspective 等元数据
    pub description: Option<String>,          // LLM 抽取的摘要
    pub protected: bool,                      // 永不归档
}
```

---

## 模块详解

### brain/ —— 大脑核心

[`brain/brain.rs`](file:///g:/vivian-rs/src-tauri/src/brain/brain.rs) 是角色的"大脑容器"，聚合所有子系统。

#### 核心方法

| 方法 | 职责 |
|------|------|
| `Brain::build(char_id, config, manifest)` | 构造 Brain，注入 manifest 到 4 个依赖（PsychologyManager / EmotionBridge / ResponseParsingRunnable / ExpressionManager） |
| `brain.think(user_input, stream)` | 用户对话主入口，调用 `think_inner(input, stream, false, true)` |
| `brain.think_cross_character(input, stream)` | 跨角色对话专用入口，跳过异步反思：`think_inner(input, stream, false, false)` |
| `brain.think_proactive(input, stream)` | 主动对话入口，跳过对话历史写入：`think_inner(input, stream, true, true)` |
| `brain.think_inner(input, stream, skip_dialogue_write, run_reflection)` | 内部统一实现，执行完整 pipeline |
| `brain.generate_startup_greeting()` | 生成启动问候。不再区分首次/回归分支，统一走完整对话流水线（`chain.ainvoke_greeting`）——与一般直接渠道对话同一套提示词（含记忆检索→种子记忆进入 prompt），仅在用户消息前加一句"这是首次见面"/"用户回来了"的提示。写入记忆库前自动补 `build_speaker_prefix(char_id, "user", char_id)` 前缀（`[I say to User]`），与主对话入库格式统一（`commands/engine.rs::try_wake_greeting` 的唤醒问候同样处理） |
| `chain.ainvoke_greeting(user_input)` | 启动问候专用流水线入口。走完整 `prepare_pipeline_state` + `execute_pipeline_and_build_response`（含记忆检索→种子记忆在场），但设置 `skip_memory_save` 门控让 UserMemorySavingRunnable / MemorySavingRunnable 跳过写入，避免把合成的问候指令当作用户消息污染记忆库。对话写回与记忆写入由调用方独立后处理 |

#### 子模块

| 文件 | 职责 |
|------|------|
| [`chat_chain.rs`](file:///g:/vivian-rs/src-tauri/src/brain/chat_chain.rs) | LangChain 风格 Runnable 链，拆分为 `prepare_pipeline_state` / `execute_pipeline_and_build_response` / `ainvoke` 三步 |
| [`async_reflection.rs`](file:///g:/vivian-rs/src-tauri/src/brain/async_reflection.rs) | 异步反思，每 5 轮或 30 分钟触发，合并意识更新与活动抽取 |
| [`augment_reply_service.rs`](file:///g:/vivian-rs/src-tauri/src/brain/augment_reply_service.rs) | 主对话后异步补充回复服务，slow 检索召回 fast 路径遗漏的重要记忆 |
| [`focus_mode.rs`](file:///g:/vivian-rs/src-tauri/src/brain/focus_mode.rs) | 凝神/专注模式状态机，漏桶累积器 + 迟滞设计 |
| [`rate_limiter.rs`](file:///g:/vivian-rs/src-tauri/src/brain/rate_limiter.rs) | Token bucket 限流器 |
| [`cognitive_tick.rs`](file:///g:/vivian-rs/src-tauri/src/brain/cognitive_tick.rs) | 认知 tick 运行器，每 5 分钟消费 `pending_conflicts` 队列 |
| [`tool_leak_filter.rs`](file:///g:/vivian-rs/src-tauri/src/brain/tool_leak_filter.rs) | 流式过滤 `<tool_call>` 等泄露标记 |
| [`topic_signal.rs`](file:///g:/vivian-rs/src-tauri/src/brain/topic_signal.rs) | 话题信号检测，驱动话题切换 |
| [`subagent_context.rs`](file:///g:/vivian-rs/src-tauri/src/brain/subagent_context.rs) | 子代理上下文，支持 LLM 调用其他角色 |
| [`coding_agent.rs`](file:///g:/vivian-rs/src-tauri/src/brain/coding_agent.rs) | 编程智能体服务（会话式 agent loop，详情见[编程智能体](#codingagent--编程智能体)） |
| [`task_service.rs`](file:///g:/vivian-rs/src-tauri/src/brain/task_service.rs) | 自治任务执行（ctx.tasks 能力缝）：LLM 逐步决策执行工具直到完成/达最大步数；`TaskEvent` 广播到事件总线，报告回流陪伴对话（详情见[task_service —— 自治任务与后台回流](#task_service--自治任务与后台回流)） |
| [`budget.rs`](file:///g:/vivian-rs/src-tauri/src/brain/budget.rs) | 轮次产出预算与收益递减检测（`OutputBudgetTracker`）：每轮按 LLM 输出 token（无 usage 场景用工具结果摘要字符 `record_chars` 近似）+ 实质进展标志记录，连续 3 轮低产出（token<500 / 字符<120）且无进展 → `StopDiminishing` 提前停机提示，防 agent 循环空转烧配额；与 `DoomLoopTracker`（同签名重复）互补 |

### coding_agent/ —— 编程智能体

[`brain/coding_agent.rs`](file:///g:/vivian-rs/src-tauri/src/brain/coding_agent.rs) 提供会话式的结对编程能力，前端在记忆观察器的「工作」页签操作。

#### 数据模型

| 类型 | 说明 |
|------|------|
| `CodingSession` | 会话：`session_id`（`code-{uuid}`）/ `char_id` / `working_directory` / `extra_workspaces[]` / `title` / `messages[]` / `status` + 会话级配置（`permission` / `model_id` / `reasoning_level` / `goal` / `plan_mode` / `plan` / `feedback` / `compacted` / `deliverables` / `message_feedback` / `work_todos`，serde default 兼容旧数据） |
| `ExtraWorkspace` | 附加工作区：`path`（绝对路径）+ `read_only`（true 时拒绝写入与删除，读取不受影响）。一个会话 = **主工作区 + N 个附加工作区**，取并集作为可访问范围 |
| `CodingMessage` | 会话消息：`role`（user/assistant/tool_use/tool_result/error）+ `content` + 工具字段（`tool_name`/`tool_arguments`/`tool_success`/`tool_call_id`）+ `timestamp` + 扩展字段（`id` / `images` / `file_refs`，serde default） |
| `CodingImage` | 单张图片：`media_type`（MIME）+ `data`（base64 数据，不含前缀）+ `name`（可选文件名）——用户/助手消息均可含多张图片，随会话持久化 |
| `CodingFileRef` | 文件引用：`path`（绝对路径）+ `content`（读取内容，可空）+ `error`（读取失败原因，可空）——输入框 `@` 选择文件注入上下文 |
| `CodingStatus` | Idle / Running / Canceled |
| `CodingWorkspace` | 工作区项：`id`（=path）/ `name`（basename）/ `path` |

##### 主工作区 vs 附加工作区

会话可以挂多个工作区，但两者地位不同：

| | 主工作区（`working_directory`） | 附加工作区（`extra_workspaces[]`） |
|---|---|---|
| 数量 | 唯一（空串 = 无工作区模式） | 任意个 |
| 决定什么 | 相对路径解析、项目记忆（`.vivian/memory.md`）、终端 cwd、system prompt 环境块的主目录 | 只扩大可访问范围 |
| 可写性 | 由会话 `permission`（访问级别）决定，**没有独立只读标记** | 每个自带 `read_only` |
| 文件链接 | 回复里可用相对路径（前端按主工作区还原） | **必须用绝对路径** |

沙箱与权限层对「是否在授权范围内」只有一个判定口径：`tools::types::is_path_within_any(path, primary, extras)`，
由 `ToolUseContext::is_path_authorized` 委托（主工作区为空串时恒真 —— 见下方「无工作区模式」）。

路径比较统一走 `brain::coding_agent::workspace_key()`（统一分隔符、去尾部斜杠、Windows 忽略大小写），
加挂去重与委派校验共用它，不重写用户传入的原始写法。

会话级配置字段：
- `permission`：`read_only` / `workspace_write` / `full_access`（缺省 workspace_write），经 `permission_to_access_level()` 映射为工具系统 `AgentAccessLevel`（ReadOnly / FsWrite / FullControl）
- `model_id`：会话选中的工作智能体模型 id（与 `config.active_work_model` 同步），None 跟随默认路由
- `reasoning_level`：`low` / `medium` / `high`（缺省 high），low 关闭思维链（`LLMRequest.reasoning=false`）
- `goal`：会话目标（`/goal` 设置，注入 system prompt「# 当前目标」段）
- `plan_mode`：计划模式开关（`/plan` 切换，开启时注入只读研究策略 `PLAN_MODE_POLICY`）
- `plan`：已批准执行方案（`/plan approve` 固化，注入 system prompt「# 已批准方案」段；`/plan off` 清除）
- `feedback`：反馈记录数组（`/feedback` 追加，含时间戳）
- `compacted`：较早历史的 LLM 压缩摘要（`/compact` 生成，注入 system prompt「# 历史摘要」段）
- `deliverables`：产物文件（write_file / edit_file 成功写入的绝对路径，去重，驱动前端产物面板）
- `message_feedback`：单条消息级反馈（消息下标 → `"up"` / `"down"`，`coding_set_message_feedback` 写入）
- `work_todos`：工作待办清单（`Vec<WorkTodo>`，`{content, status}` 两字段；`work_todo_write` 整表替换写入，注入每轮上下文作为执行计划；全部完成时新一轮对话归档清空，否则跨轮保留）

#### Agent Loop

```
用户消息(推入历史 + 置 Running)
   → build_llm_messages(裁剪60条历史 + system prompt)
   → ModelRouter.generate_stream_with_tools(仅白名单编程工具, 原生function calling)
   → has_tool_calls?
       否 → 记录assistant回复 → 置Idle → 完成轮次
       是 → 记录assistant.tool_calls(带id关联) → 逐个 execute_tool_use
             → 结果写入历史(摘要截断6000字符, 保留tool_call_id)
             → 回到循环开始(预算=config.tools.max_coding_rounds, 默认48, 命令层传入)
               ├ 软预算提醒: 用到 2/3、5/6 时注入系统提示"评估是否收尾/方案是否有效"
               ├ 停滞检测: 相同工具+相同参数重复≥3(DoomLoopTracker)
               │            / 同工具连续失败且错误摘要相同≥3 → 注入"重新分析/停止重试"
               ├ 预算耗尽且有实质进展(成功写/改/执行) → 自动续轮一次(+base/3, 封顶96)
               └ 耗尽且无进展 → 硬停止 → 前端弹去向选择条
   → 收尾: summarize_turn_to_memory(本轮对话摘要入库记忆)
```

关键点：
- **工具复用主对话链路**：`execute_tool_use` 自动经过沙箱 `is_path_safe` / 守卫 / 审批矩阵
- **会话级权限接入**：`ToolUseContext` 新增 `access_level: Option<AgentAccessLevel>` 字段（serde default None），`execute_tool_use` 权限检查优先用 `context.access_level.unwrap_or(runtime_cfg.access_level)`——编程 agent 按会话 `permission` 设置覆盖，实现会话粒度的工具放行控制（None 时回退全局 runtime config，不影响其他 agent）
- **工作区写入免确认**：执行器权限检查构建 `PermissionContext` 时，把 `context.working_directory` 注册为已授权工作目录（`add_working_directory`，read_only 会话注册为只读）——工作目录内的读写操作在 `check_file_permission` 中直接 `allow`，不再落到「路径不在已授权目录需确认」分支；路径范围仍由各工具 `validate_input` 的沙箱校验限制在工作目录内，`read_only` 会话写入仍被矩阵拒绝
- **沙箱确认回调**：编程会话的工具执行传入 `coding_sandbox_confirm(has_workspace, has_responder)`——`write_file`/`edit_file` 内置档案 `requires_confirmation=true` 且 Cautious 模式前 3 次需要确认，而执行器在无回调时会直接返回 `SandboxConfirmationRequired` 错误（无弹窗），导致工作区写文件被误拦，故有工作区时恒放行（真正边界仍由路径沙箱 + 权限矩阵 + 命令黑名单把守）；无工作区时没有路径边界，改为"有应答者就弹确认、没有应答者就拒绝"，详见「沙箱确认回调（`coding_sandbox_confirm`）」小节
- **推理等级**：run_loop 从会话读取 `reasoning_level`，`LLMRequest.reasoning = reasoning_level != "low"`（standard/code 两条路径一致）
- **多轮工具调用**：历史中 assistant 的 `tool_calls` 结构完整回传 LLM（`ChatMessage::assistant_with_tool_calls`），tool 结果经 `ChatMessage::tool_result` 按 `tool_call_id` 关联，满足原生 function calling 的多轮上下文协议
- **任务执行期间发消息：插话 / 引导标注**：编程页输入框在智能体工作时仍可发送——消息**不打断**当前任务，进入输入区上方「排队中」卡片（可编辑 / 删除），当前任务结束后按序自动补发：
  - `CodingMessage.interjected`：普通排队插话（前端排水时传 `interjected: true`）。`build_llm_messages` 加 `[系统标注] 用户在你处理上一条消息期间发来了消息…` + `<user_message>` 包裹，帮助模型区分「对当前任务的补充/修正」与「全新对话」。
  - `CodingMessage.guided`：排队消息上的「引导」按钮（原「立刻推送」）**不再取消当前任务**，而是把该条标记为引导（`guided: true`），在当前 run 结束后**优先**于其他排队消息发送；`build_llm_messages` 加 `[系统标注] 用户在你工作期间给出了引导，这是对你当前工作的引导/指示（不是全新任务）…` 标注，让模型明确这是对工作的引导而非全新指令。
  - 两种标注仅注入 LLM 上下文层，UI 展示保持原文纯净
- **流式输出**：`StreamEvent::Text` 增量经 `coding:chunk` 逐字转发前端打字机；`StreamEvent::Thinking` 增量经 `coding:thinking_chunk` 在「思考占位」内渐进展开灰色推理文本（不入库）
- **上下文控制**：历史最多注入 60 条；工具结果经 `prune_head_tail`（executor 公共函数）头尾裁剪——超 6000 字符保留头部 2/3 + 尾部 1/6、中段以 `…[中段 N 字符已折叠]…` 标记折叠（尾部退出码/报错/diff 收尾不丢失），`summarize_result` 与重放历史两条路径一致；错误消息作为 `[系统提示]` 注入让 LLM 感知失败并调整策略
- **工作流与 LSP 工具**：`CODING_TOOLS` 白名单含 `run_workflow`（多步编排，连续 parallel 步骤扇出并发）与 `lsp_query`（定义/引用/实现/hover 语义查询），definition 按白名单顺序输出保持 API tools 前缀缓存稳定
- **图片工具（`send_image`）**：智能体把本地图片发送到聊天界面——编程会话下经 `CodingAgent::push_agent_image` 作为 assistant 消息追加（`images` base64 内联，随会话持久化、恢复会话前端直接重渲染），并 emit `coding:assistant_message`（携带 images）供编程页实时渲染；伴随的 caption 作为可选说明文本。图片副本同时保存到 `<user_data_dir>/images/` 供历史复用
- **可视化组件工具（`show_widget`）**：工作智能体把 SVG 流程图/架构图/时序图/状态图渲染为编程页内联卡片，落「强约束 / 载荷隔离 / 受控渲染」三原则——约束系统（viewBox 固定 680、颜色显式填、暖纸色板、禁渐变阴影/emoji/script/事件、字体≥11px）内嵌在工具 `description`（随 schema 每轮注入，等效每次调用前读一遍规范）；SVG 经 `CodingWidget{title,kind,code}` 走 `push_agent_widget` 推给前端（`widgets` 字段，不进 LLM 上下文）；前端 `WidgetCard` 经 dompurify sanitize 后内联渲染、失败降级为可展开源码卡片。与 `send_image` 同通道、随会话持久化，历史恢复零成本
- **能力进化工具**：`CODING_TOOLS` 白名单末位为 `create_skill` / `use_skill` / `search_skill` / `create_tool` / `create_plugin`——工作智能体是能力进化事件的**执行主体**：沉淀方法论（create_skill，写入即注册）与构建新工具（create_tool，stdin/stdout 函数协议）、打包完整插件（create_plugin，四类贡献原子落盘即装载）。system prompt 明确该职责并引导「任务中总结出值得复用的做法时沉淀技能、确认缺少可执行原语时构建工具、一组相关能力要整体装卸时打包插件」。`create_tool` / `create_plugin` 创建经 executor 能力进化门强制弹预览卡片（宿主自动放行回调不绕过），且属于 `WORK_AGENT_ONLY_TOOLS`——陪伴侧工具面/检索域/执行层三层收口（见 custom_tools 小节），`agent_kind="work"` 才可发起
- **轮次预算与循环保护**（`run_loop_inner`）：预算来自 `config.tools.max_coding_rounds`（默认 48，设置-工具可调，命令层从 `config` 读取后经 `send_message` → `run_loop` → `run_loop_inner` 传入；`DEFAULT_MAX_TOOL_ROUNDS=48` 兜底）。
  - 软预算提醒：`rounds_used` 达到 `budget*2/3`、`budget*5/6` 时向本轮请求注入 `[系统提示]`（只引导、不落库）。
  - 停滞检测①：`DoomLoopTracker`（复用 `pipeline::doom_loop`）记录 (tool_name, canonical_args)，同一签名累计 ≥3 次 → 注入「停止重复、换方法或告知障碍」。
  - 停滞检测②：同一工具连续失败且错误摘要（summary）hash 相同 ≥3 次 → 注入「停止重试、重新分析根因」；一旦有任何成功即清空失败计数。
  - 收益递减检测：`brain/budget.rs::OutputBudgetTracker` 每轮记录 LLM 输出 token（无 usage 场景用工具结果摘要字符近似 `record_chars`）+ 实质进展标志，连续 3 轮低产出（token < 500 / 字符 < 120）且无进展 → 判定空转，提前提示收尾停机；与 `DoomLoopTracker`（同签名重复）互补，抓"调用各不相同但都毫无产出"。
  - 自动续轮：预算耗尽时若 `made_progress`（本轮任一成功 write_file/edit_file/run_command），`budget = (base + base/3).min(96)` 续轮一次（仅一次），并向历史写一条 `CodingRole::Error` 提示「自动续轮 N 轮」；无进展则 `break` 硬停止。
  - 硬停止：写 Error 消息 + `coding:error` 事件（含实际上限数字），前端据此弹去向选择条。
- **角色化 system prompt**：按 `char_id` 注入人设（Vivian/Nana），限定 Windows + PowerShell、工作目录沙箱，"先看再动、局部用 edit_file、改后跑命令验证"。**无工作区模式**：会话未绑定目录（如陪伴侧 `delegate_to_work_agent` 未指定与无历史工作区时）时，system prompt 改为"未选择（无工作区模式）：文件操作使用绝对路径；未绑定工作区，无目录沙箱，写入前会请求用户确认"——文件工具沙箱边界（`is_path_within_working_directory` 空 base 恒通过）配合权限矩阵兜底，轻量/进化类任务无需先选工作区即可工作；轮次摘要 `build_turn_transcript` 同步标注"未选择（无工作区模式）"
- **会话摘要入库记忆**（`summarize_turn_to_memory`）：run_loop 结束后异步执行——从最后一条用户消息起切片本轮消息，经 `router.generate`（memory 路由，`TURN_SUMMARY_SYSTEM_PROMPT` 压缩为 2-4 句中文摘要；LLM 失败退化为 `rule_turn_digest` 规则摘要兜底），写入会话所属角色的 `MemoryManager`（ShortTerm，importance 0.4，tags `coding_session`/`work`，metadata 含 `source=session_id`/`working_directory`/`speaker`/`listener`）。内容前缀 `[编程会话]`，与主对话记忆体系打通，角色可在后续对话中回忆自己做过的编程工作
- **项目记忆（工作区级 memory.md）**：跨会话沉淀的项目约定与教训，存储在**工作区内** `.vivian/memory.md`（项目级随项目走；`build_llm_messages` 每轮重读注入 system prompt「# 项目记忆（跨会话沉淀）」，超 8000 字符截断保尾部，文件未变时字节一致不影响缓存）。产出链路：`/compact` 归档时自动蒸馏（`distill_project_memory`，尽力而为）、`/memory 提炼` 手动蒸馏、`/memory <内容>` 手动追加；超过 `PROJECT_MEMORY_MERGE_LINES`(100) 行时蒸馏自动转为全文重写合并去重（`rewrite_project_memory`，LLM 返回空视为失败保留原文件）。`/memory` 无参查看（附路径）、`/memory 清除` 清空。旧版 appdata 存储（`coding_memory/<工作目录编码>/project_memory.md`）在 `read_project_memory_raw` 首次读取时一次性迁移到工作区（工作区已有文件优先，旧文件保留作备份）

#### 斜杠命令（Slash Commands）

输入 `/` 在前端弹出命令菜单（`SlashCommandMenu`，按命令名/标签字母模糊筛选，↑↓ + Enter 选择、Esc 关闭；选中命令插入输入框并保持聚焦，命令名后输入空格自动收起菜单便于补参数）。后端 `send_message` 检测消息以 `/` 开头时**不走 agent loop**，改由 `handle_slash_command` 拦截分发（同步命令即时处理，`/compact` 异步走 LLM），结果以 assistant/error 消息写入会话并复用 `coding:assistant_message` / `coding:error` / `coding:turn_done` 广播，前端消息流直接展示，不消耗 agent loop 轮次。

| 命令 | 处理器 | 行为 |
|------|--------|------|
| `/goal [目标]` / `/goal 清除` | `cmd_goal` | 无参查看当前目标，有参设置会话目标（`CodingSession.goal`，注入 system prompt「# 当前目标」段）；`清除` / `-clear` 移除目标 |
| `/plan` / `/plan approve` / `/plan off` | `cmd_plan` | 无参切换计划模式开关（`plan_mode`，开启注入 `PLAN_MODE_POLICY` 只读研究策略）；`approve` 把最近一条 assistant 方案消息固化为已批准方案（`CodingSession.plan`，注入「# 已批准方案」段并保持计划模式）；`off` 退出计划模式并清除已批准方案 |
| `/compact` | `cmd_compact` | 把较早历史（保留最近 `COMPACT_KEEP_MESSAGES=24` 条，旧消息不足 `COMPACT_MIN_MESSAGES=8` 提示无需压缩）交 LLM（`COMPACT_SYSTEM_PROMPT`）压缩为摘要，与既有 `compacted` 合并后写入会话并从历史裁剪 |
| `/permission [等级]` | `cmd_permission` | 无参查看当前权限，有参切换 `read_only` / `workspace_write` / `full_access` |
| `/feedback <内容>` | `cmd_feedback` | 把带时间戳的反馈追加进 `feedback` 数组 |
| `/export` | `cmd_export` | 将会话导出为 Markdown 到 `<用户数据目录>/coding_exports/`（含目标/已批准方案/计划模式/历史摘要/反馈/全部消息记录） |

关键点：
- **system prompt 注入**：`build_llm_messages` 在静态人设 prompt 之后按序追加「# 当前目标」（goal）、「# 已批准方案」（plan，仅 `plan` 非 None 时注入）、计划模式策略（plan_mode）、「# 历史摘要」（compacted）；四者默认缺失时 system prompt 与既有完全一致，不破坏前缀缓存
- **标题保护**：`send_message` 与 `push_message` 均跳过以 `/` 开头的首条消息作为会话标题
- **未知命令**：返回「未知命令」错误并列出可用命令

#### 事件广播（`coding:*`）

| 事件 | 载荷 |
|------|------|
| `coding:user_message` | `{session_id, content}` |
| `coding:assistant_message` | `{session_id, content}` |
| `coding:tool_call` | `{session_id, id, name, arguments}` |
| `coding:tool_result` | `{session_id, id, name, success, result, duration_ms}`（前端按 `id` 回填本地 `tool_use` 为结果卡；找不到对应 `tool_use`（刷新/切会话错过事件）时兜底追加独立结果卡，避免结果丢失） |
| `coding:deliverable` | `{session_id, path}`（write_file / edit_file 成功写入的新产物，增量驱动前端产物面板） |
| `coding:thinking` | `{session_id, thinking: true}`（生成期占位提示） |
| `coding:chunk` | `{session_id, content}`（流式文本增量） |
| `coding:thinking_chunk` | `{session_id, content}`（推理链增量） |
| `coding:turn_done` | `{session_id}`（恢复 Idle + 持久化，前端权威同步） |
| `coding:error` | `{session_id, message}` |
| `work_todo:changed` | `{session_id, items}`（待办清单变更，右栏面板实时刷新） |
| `coding:question` | `{question_id, session_id, question, context, options[], multi_select}`（方向询问卡片） |
| `coding:subagent` | `{session_id, phase: started/finished, depth, task/rounds/tool_calls, stop}`（子 agent 运行指示条） |
| `coding:job` | `{session_id, job_id, status: started/completed/failed/canceled}`（后台任务计数） |

#### 持久化

会话写入 `%APPDATA%\Vivian\coding_sessions.json`（保留最近 30 个，`serde_json` 序列化）；启动时 `load_from_disk` 恢复并将所有会话重置为 Idle，避免上次中断的 Running 会话残留。会话级配置（`permission` / `model_id` / `reasoning_level` / `goal` / `plan` / `deliverables` / `message_feedback`）随会话一同持久化，`switchSession` 时前端自动恢复。

#### 会话级配置命令（`commands/coding_agent.rs`）

| 命令 | 说明 |
|------|------|
| `coding_list_workspaces` | 历史会话中出现过的工作区列表（去重，按最近使用倒序） |
| `coding_set_workspace` | 切换会话**主工作区**（目录必须存在；运行中拒绝）。只扩大范围请用 `coding_add_workspace` |
| `coding_add_workspace` | 挂载附加工作区 `(session_id, path, read_only)`；目录必须存在，已挂载则更新其只读标记，**重复挂主工作区直接报错**（避免同一目录出现两份互相矛盾的权限记录）；运行中拒绝；返回最新列表 |
| `coding_remove_workspace` | 卸载附加工作区 `(session_id, path)`；运行中拒绝；返回最新列表 |
| `coding_set_workspace_read_only` | 切换附加工作区的只读标记 `(session_id, path, read_only)`；对主工作区无效（主工作区可写性由访问级别决定）；返回最新列表 |
| `coding_set_permission` | 设置会话权限等级（read_only / workspace_write / full_access；运行中拒绝） |
| `coding_set_model` | 设置会话工作模型 id（与 `select_work_model` 热切换同步；运行中拒绝） |
| `coding_set_reasoning_level` | 设置推理等级（low / medium / high；运行中拒绝） |
| `coding_list_available_models` | 可用工作模型列表（复用 `config.work_models`，返回 `{id, name}`） |
| `coding_get_work_todos` | 读取当前会话工作待办清单（供右栏面板只读展示） |
| `coding_respond_question` | 回传用户对方向询问的回答（唤醒挂起中的 `work_ask_user`） |
| `coding_pending_question` | 取当前会话未回答的询问（面板重挂载时恢复卡片） |

`CodingAgentService` 对应方法（`list_workspaces` / `set_workspace` / `add_workspace` / `remove_workspace` / `set_workspace_read_only` / `set_permission` / `set_model` / `set_reasoning_level`）均走「校验 → 更新 → persist」模式，与既有 `set_mode` 同构。

三个工作区命令都**返回最新列表**而不是 `()`：前端拿到结果直接 patch 本地会话状态，不必整表刷新
（`patchSessionWorkspaces`）。`coding_fork_session` 会一并复制 `extra_workspaces`。

#### 前端编程页（CodeAgentPage）

[`CodeAgentPageNew.tsx`](file:///g:/vivian-rs/src/components/mind-inspector/pages/CodeAgentPageNew.tsx) 实现 Codex 布局 + 手账风格三栏界面，`MindInspector.tsx` 直接导入该文件：

- **左栏（会话/工作区管理）**：「新会话」按钮（主点击 + 下拉箭头，见下）、会话按工作区分组或单列表，可按最近更新/手动排序，支持搜索会话；工作区分组标题提供三点菜单（重命名 / 删除工作区）和新建当前工作区会话的加号按钮；主工作区非空时展示工作区文件树
- **中栏（flex:1）**：空态 hero / 会话顶栏（会话标题 + **工作区芯片** + 运行状态 + 模式切换）/ 消息流（消息按角色区分渲染，文件类工具 read/write/edit 以手账风格代码块 + diff 高亮展示；非文件工具 `ToolCallCard` 紧凑展示；用户/助手消息含图片时渲染为图片缩略图气泡，点击经 `onOpenImage` 打开大图）+ 底部输入卡片。空态下 `canSend` 只看输入内容（不再要求已有会话），直接发送会经 `ensureSession` 先建会话再继续发。
- **中栏内部结构**：顶栏以下是一条横向带 `.codex-main-body`，里面只有一个 `.codex-main-col`（对话区 + 回到底部 + 输入区 + 统计 + 置顶摘要）。摘要是**绝对定位贴在右缘的信息列**（`right: var(--codex-sb-w)`，紧贴滚动条左边），不参与 flex 分配——四处内容靠 `padding-right: max(基准, --codex-pinned-reserve)` 让位，所以正文永远不会被面板盖住，同时滚动条能留在整条工作区的最右侧
- **右栏（检查器，可整体收纳）**：概览统计（轮次/步数/LLM 与工具耗时/首 token/缓存命中/token 用量）+ 内嵌终端标签页；标签名沿用工作区目录名，支持多开/关闭；左右侧边栏均可拖拽调整宽度
- **侧边栏收放过渡**：两侧收起/呼出走 320ms `cubic-bezier(0.4,0,0.2,1)` 宽度缓动；拖拽调宽期间挂 `.resizing` 关掉过渡（否则每帧目标宽度被缓动拖住，手感变成橡皮筋追鼠标）。右栏内层 `.codex-inspector-inner` 保持展开宽度、由外层 `overflow:hidden` 裁切，动画期间内容整块滑出而不逐帧重排；左栏收起后仍留 54px 窄条，故不裁切，改为内容 `opacity` 快速淡出 + 品牌标题/新建按钮收掉占位。两侧内容均**不随收起卸载**（否则动画一开始内容就消失，只剩空栏在缩）；拖拽手柄也常驻渲染，收起时淡出，避免它消失时布局瞬跳 6px。已用无头 Chrome 逐帧采样宽度验证 14 项（含「拖拽 0 中间帧」「内层宽度恒定」），脚本见 `.workbuddy-ai/tmp/sidebar-transition-verify.mjs`
- **置顶摘要的收放过渡**：与两侧边栏同一条曲线（320ms），同样「宽度归零 + 内层固定宽度裁切 + 不卸载」；**两轴同时**——横向宽度 0 ↔ 250，纵向由 `clip-path: inset()` 从上往下揭开 / 从下往上收掉（内层靠右对齐，动画期间内容不横向平移）；额外用 `visibility` 延迟切换让收起后退出 tab 序列。宽度不是固定值，而是按主工作区实测宽度动态算（先保对话区 `CHAT_MIN_W`=430，面板在 250~186 之间自适应）。四处让位（`padding-right` / `right`）必须与面板宽度过渡同曲线同步，否则收起期间正文与面板会互相错位。脚本见 `.workbuddy-ai/tmp/pinned-dock-verify.mjs`（46 项，覆盖滚动条在面板右侧、无分隔线、呼吸缝恒为 18px、四个让位元素在 1440/900/640/560 四个视口下都不被遮挡、三档主题基准内边距、真实鼠标点击聚焦后无描边 + 对照元素仍有描边、手账主题同样成立，以及纵向方向：底边内缩 0%↔100% 单调、顶边内缩恒为 0、内层未裁切左缘恒定）

**「新会话」按钮（`handleCreate` / `createSessionByDefault`）**：主点击**不弹目录选择框**，直接建会话：

| 情形 | 行为 |
|---|---|
| 设置里配了默认工作区 | 建在该目录 |
| 未配置默认工作区 | 建成**无工作区模式**会话（`workingDirectory: ''`） |
| 配了默认工作区但目录已失效 | 退回目录选择框让用户重新指向（配置过期不静默降级成无沙箱会话） |

按钮右侧的下拉箭头保留手动入口：「使用默认工作区」（未配置时文案变为「无工作区新建」）与「选择工作区…」。
`ensureSession`（首次发送消息时若还没会话）走同一规则，不再有第二个隐藏的弹框触发点。
默认工作区由设置项 `default_workspace` 提供（见 README「配置系统」），工作页监听 `config:saved` 即时生效。

**工作区芯片（`WorkspaceDropdown`）**：会话顶栏的目录名不是纯文本，而是可点芯片（`+N` 徽标提示附加工作区数量），
展开后是该会话的工作区管理菜单——列出主工作区与各附加工作区（含只读开关、逐个移除），
底部两个动作：「挂载目录…」（`coding_add_workspace`，默认可写）与「更换主工作区…」（`coding_set_workspace`）。

更换主工作区时，**原主工作区会降级为附加工作区（可写）而不是被丢弃**——换主目录不该让 agent 静默失去对原目录的访问权，
菜单里有 hint 文案说明；真不想要了可以手动移除那个附加目录。会话运行中芯片禁用（与后端「运行中拒绝」一致）。

**常驻目标/计划条（`GoalPlanBar`）**：消息流顶部常驻条，展示会话目标（内联编辑发送 `/goal <新目标>` / 清除 `/goal 清除`）与计划模式状态——未批准时显示「计划模式」标签 + 「批准方案」按钮（回传 `/plan approve` 固化最近方案为执行依据）+ 「退出计划」（`/plan off`）；已批准时展示方案摘要。只有 goal 或 plan_mode 非空时才渲染，不挤占空布局。

**@-mention 文件引用（`FileRefMenu`）**：输入框当前词以 `@` 开头时弹出工作目录文件选择菜单（`loadFileTree` 拉文件列表，标签/路径模糊筛选，↑↓+Enter 选中，选中把 `@路径` 插入输入）；发送时解析 `@引用` 为 `draftRefs`，随 `coding_send_message` 的 `fileRefs` 传后端——[`resolve_file_refs`](file:///g:/vivian-rs/src-tauri/src/brain/coding_agent.rs) 相对路径拼工作目录、沙箱校验、读取内容截断（单文件 `FILE_REF_MAX_CHARS`，至多 `FILE_REF_MAX_COUNT` 个），存进 `CodingMessage.file_refs`；用户消息气泡以文件图标 + 路径展示引用，读取失败显示 `path（错误）`。文件内容经 `build_llm_messages` 以 `<file_refs>` 块注入上下文，历史重放时持续有效。

**产物面板（`DeliverablesCard`）**：右栏概览展示会话产物清单（`session.deliverables`，write_file/edit_file 成功写入的绝对路径去重），按工作目录转相对路径渲染；`coding:deliverable` 事件增量追加（不重复）。

**消息操作（`MessageRow` hover 动作）**：操作条位于消息气泡**下方独立一行**（文档流内，不再绝对定位悬浮遮挡正文，hover 淡入），提供复制 / 有帮助 / 没帮助（`coding_set_message_feedback` 写入 `message_feedback`，消息下标 → up/down）/ 从此处派生新会话（`coding_fork_session` 复制该消息为止的历史为独立会话，刷新并切换）；用户气泡右对齐时操作条 `margin-left:auto` 跟随右对齐。助手消息渲染经 `MarkdownText`（[`codeMarkdown.tsx`](file:///g:/vivian-rs/src/components/mind-inspector/pages/codeMarkdown.tsx)，见「Markdown 正文渲染」小节）。

**工具卡片（`ToolCallCard`）紧凑展示**：数据流由 `coding:tool_call`（`role:'tool_use'`，`content:''`、参数在 `tool_arguments`）与 `coding:tool_result`（按 `tool_call_id` 回填 `tool_success` / `content`=后端 `summarize_result` 摘要）驱动。卡片头部带「工具调用」徽标 + 明确状态文字（`运行中… / ✓ 已完成 / ✕ 失败`），取代旧式裸 `✓/✕` 图标。
- **摘要行默认可见（仅折叠态）**：`toolArgSummary` 取关键参数（command/pattern/path…），`toolResultSummary` 按工具类型提取要点——`grep_search` → `找到 N 处匹配 · 扫描 M 个文件`、`run_command` → `✓ 成功 / ✕ 退出码 N · 首行输出`、`list_dir` → `N 个条目`、`edit_file` → `+N −M`（解析结果 JSON 内嵌 `diff` 的增减行数）、`write_file` → `路径 x`、通用取前两个非空键；未展开即可一瞥 Agent 在做什么。展开后摘要行隐藏（由完整 IN/OUT 取代，避免双虚线占位）。
- **展开详情**：完整 IN/OUT 等宽块，仅在有内容时渲染（`argumentsJson` / `result` 非空），空参数/空结果不再显示「（空）」大框；IN/OUT 文本 `trim()` 后渲染，杜绝 `pre-wrap` 下字符串首尾换行造成空行；`codex-tool-detail` 容器内边距紧凑（5px/6px/7px）。
- **空值处理**：`tool_arguments` 为 null/undefined/""/0/false 时摘要返回 `''`（不占行）、展开态按「无参数与返回内容 / 运行中 / 执行失败（无返回内容）」显示小字，区分「没参数」与「有参数但为空」。
- **running 挂钩会话状态**：`running = (role==='tool_use') && 会话运行中`——会话结束后任何卡片（含 code 模式「组合程序」卡）不再残留「运行中」；`coding:tool_result` 找不到对应 `tool_use`（刷新/切会话错过 `tool_call` 事件）时兜底追加独立结果卡片，避免结果凭空丢失。

**单轮工作过程分组（`ToolProcessGroup`）**：`groupChatMessages(messages, running)` 把消息流切分为渲染项——**连续 ≥2 条 tool_use/tool_result 消息聚为一组**（单张卡片直接渲染），其余消息透传。
- **自动收纳**：组后出现 assistant 总结 / 下一轮 user 消息 / 会话停止运行即判定 `settled`，进行中默认展开实时观察、总结出现瞬间自动折叠成一行摘要（`工作过程 · N 步 · M 个文件 · 耗时` + `✓ 已完成 / 进行中…` 状态 + `✕ 失败数`），之后用户自由开合；历史会话加载的组默认折叠。
- **展开/折叠动画**：CSS grid `grid-template-rows: 0fr→1fr` 过渡高度（无需测量实际高度）+ 内容 opacity 淡入 + 箭头旋转 -90°↔0°，250ms `cubic-bezier(0.25,0.6,0.3,1)`；内容始终挂载（仅视觉收起），不丢滚动位置与卡片内部展开态；折叠时上边框虚线透明化。
- **组内渲染**：复用原工具卡片渲染逻辑（含 diff / 工作流 / LSP 可视化卡），组内卡片收紧间距去阴影以体现层级；后端聚合落库的空壳 `tool_use` 桩消息（无 `tool_name`）仍跳过。

**工作流可视化卡片（`WorkflowVizCard`）**：`run_workflow` 的 tool_result（`{name, total, succeeded, failed, steps[]}`）解析成功后渲染为可视化卡片——头部（名称 + 成功/总数 + 状态章「全部成功 / N 步失败」）+ 进度条（成功占比，失败着色）+ 步骤区按连续 `parallel` 标记聚为「顺序 / 并行组」（并行组带「并行组 · N 步」标签），每步显示序号/工具/✓✕/结果要点。解析失败（如结果被裁剪）自动回退普通 `ToolCallCard`。

**LSP 语义导航卡片（`LspVizCard`）**：`lsp_query` 的 tool_result（`{kind, result}`）解析成功后渲染——`go_to_definition` / `find_references` / `go_to_implementation` 的位置（file URI + range.start）归一为 1 基 `path:line:col`、按文件分组（相对工作目录展示），每条为可点击导航行，点击经 `@tauri-apps/plugin-shell` 用系统默认编辑器打开（真实语义跳转）；`hover` 提取可读文本（兼容字符串/数组/`{value}`/markdown）渲染为滚动等宽块。解析失败自动回退普通工具卡片。

**三段式输入卡片**（`inputBar`，空态居中 / 有消息贴底复用）：
- 空态顶部：`WorkspaceDropdown`（`coding_list_workspaces` 缓存列表 + 添加工作区）+ `ModeDropdown`（标准/代码/极简 + 说明）
- 中间：多行 `textarea`，输入 `/` 触发 `SlashCommandMenu`（按命令名/标签字母模糊筛选，键盘 ↑↓ 选择 + Enter 插入 + Esc 关闭；选中命令插入输入框并保持聚焦，命令名后输入空格自动收起菜单便于补参数）
- 底部工具栏：附件上传（图片草稿 `AttachmentRail`）+ `PermissionDropdown` + `ModelDropdown`（模型 + 推理等级双区）+ 发送/停止

**轮次预算耗尽的去向选择条（`BudgetStopBanner`）**：当 `coding:error` 消息同时含「已达到单轮最大工具调用轮数」与「自动停止/可发送新消息继续」时，前端判定为预算耗尽硬停止（区别于「自动续轮」提示——那只是中途扩额、任务仍在继续），在输入区上方弹出横幅：
- 展示本轮进展（`computeTurnProgress`：自最后一条用户消息起统计 `tool_result` 次数 / 失败数 / 工具调用分布 / 涉及文件 ≤5）
- 三个动作：**继续**（`sendContinuation('请继续完成当前任务')` 开新一轮，LLM 带完整历史继续）/ **补充说明后继续**（聚焦输入框，补充后正常发送）/ **停止任务**（仅关横幅）
- 发送新消息（`handleSend`/队列/`sendContinuation`）或切换会话时自动清除横幅与聚焦提示

各下拉组件共用 `DROPDOWN_MENU_STYLE` / `DROPDOWN_OPTION_STYLE`，点击外部关闭（document mousedown）；会话切换（`switchSession`）与初次加载时从 `CodingSession` 恢复 `permission` / `reasoning_level` / `model_id`。模型下拉以 id 作为选中值，触发器显示映射的模型名。**无工作模型空态**：`coding_list_available_models` 返回空（或拉取失败）时，模型下拉渲染「尚未配置工作模型」+「去设置 LLM」按钮，按钮经 `openLlmSettings` 打开设置窗口并跳 AI/LLM 页（已开则聚焦 + `config:open-tab` 热切页签）；`handleSend` 在 `loadActiveModelId()` 为 null 时**不发送**，改为置 `modelHighlight` 高亮模型下拉（触发器脉动圆环 + 自动展开，8 秒自动熄灭）并页内提示，引导先配置工作模型。

**内嵌终端**：终端位于右栏检查器的「终端」页签内，右栏整体可收纳（不卸载：宽度归零 + 外层 `overflow:hidden` 裁切，而不是 `display:none`——终端既保持 ConPTY 会话，又不会被逐帧压窄到 0 列）；终端标签名沿用工作区目录名，支持多开/关闭。终端实例为 [`TerminalPanel.tsx`](file:///g:/vivian-rs/src/components/mind-inspector/pages/TerminalPanel.tsx)（xterm.js + ConPTY，懒加载），主题跟随浅色/深色手账配色，字号 11 默认等宽字体。

**源码查看/编辑视图（`SourceFileView`，文件 read 结果的文本分支）**：文件类工具 `read` 返回的文本文件（`coding_read_file`），非 Markdown 时经此组件渲染——只读态用 highlight.js 语法高亮 + 行号 gutter；编辑态为等宽 textarea + 行号，保存走 `coding_write_file`。超大文件初始只收首段，`coding_read_file_lines` 分页「加载更多」（单次行数 `CHUNK_LINES` 与后端 `coding_read_file_lines` 的 count 上限对齐）。高亮产物经 DOMPurify 白名单（`span` / `class`）过滤后再注入，与 `WidgetCard` 同一套安全链路。

- **语法映射**：扩展名 → hljs 语言名集中在 `EXT_LANG`（`ts/tsx/js/jsx/mjs/cjs`、`css/scss/less`、`json/yaml/yml`、`html/htm/xml/svg/vue`、`md/markdown/mdx`、`rs`、`py`、`sh/bash/zsh/ps1/bat/cmd`、`c/h`、`cpp`、`java`、`go`、`sql`、`diff/patch`、`ini/env` 等）。各语言模块从 `highlight.js/lib/languages/*` 按需**静态 `import`** 并在模块顶部 `registerLanguage`，避免运行时异步加载。
- **TOML 由 ini 语法原生覆盖**：`highlight.js` 的 `ini` 语法（`ini.js`）`name` 为「TOML, also INI」且 `aliases: ['toml']`，注册 `ini` 后 `getLanguage('toml')` 即命中该语法，TOML 高亮来自 ini 语法本身；**不要补 `import 'highlight.js/lib/languages/toml'`**——该文件不在 `highlight.js` 发布物内，静态 import 会让 Vite 预转换直接抛 `Failed to resolve import`（历史上已因此崩过构建）。
- **未知语言退化为转义纯文本**：`highlightHtml` 先查 `hljs.getLanguage(lang)`，未注册语言走 `escapeHtml` 转义后由 DOMPurify 过滤，内容不丢也不报错。

**Markdown 正文渲染（`codeMarkdown.tsx` / `MarkdownText`）**：助手回复以 markdown 为主，直接产出 React 节点（不拼 HTML、不走 dangerouslySetInnerHTML，零新增依赖）。块级——围栏代码块（语言标签 + 复制）、ATX 标题（1.5/1.25/1.125/1 倍递减）、分割线、引用（左侧竖条）、有序/无序列表（disc→circle→square 嵌套）、任务清单（`- [ ]` / `- [x]`，无项目符号 + 方框，完成项转灰）、表格、段落；行内——`code`、`**粗**`、`*斜*`、`~~删~~`、链接。
- **有意不解析 `_` 系**（`_斜_` / `__粗__`）：`some_var_name`、`__init__` 这类标识符在工作回复里太常见，误伤成本高于收益。`*` / `**` 两侧都是 ASCII 字母数字时按运算符处理（`2*3*4`、`2**3` 保持字面量），中文两侧不受限（`这是*斜体*文字` 正常）。`**` 找不到闭合时整对定界符按字面量消费，避免第二个 `*` 落进斜体分支吞掉后续整段。
- **本地文件链接卡片**：`[标签](路径)` 的目标判定为本地路径（`file://` / 盘符 / UNC / `/abs` / 含路径分隔符且带已知扩展名的相对路径）时，渲染为图标 + 标签的文件卡片（图标按扩展名挑：代码 / `#` / 花括号 / 文档 / 图片），点击经 `MarkdownFileContext` 的 `onOpenFile` 回调送右侧预览打开（相对路径按会话工作区补全，见 `resolveWorkspacePath`）；其余按外链。宿主经 React context 注入回调，不在 `MessageRow` → `MessageBubble` 层层透传。
- **流式逐块淡入**：`animate` prop 打开时根节点挂 `data-md-animated`，子块 opacity 0→1 并注入递增 `animation-delay`；块 key 由下标决定，React 复用已有 DOM，已存在块不重放动画。仅流式文本启用，历史消息静态。
- **正文阅读宽度**：正文块（段落/标题/列表/引用）`max-width: 680px`，代码块与表格突破该限占满整列。

#### 编程工具集（`tools/builtin/coding_tools.rs`）

| 工具 | 风险 | 说明 |
|------|------|------|
| `read_file` | FsRead | 读取文件内容（UTF-8/GBK/Shift-JIS 自动检测），沙箱校验工作目录 |
| `write_file` | FsWrite（破坏性） | UTF-8 写入，自动建父目录，沙箱校验工作目录 |
| `edit_file` | FsWrite（破坏性） | 精确字符串替换，要求唯一匹配或 `replace_all`，防止误改；成功后结果 JSON 内嵌 unified `diff`（`build_edit_diff`：每处替换一行 hunk 含 3 行上下文、间隙 ≤6 行自动合并、6 hunk/100 行/4000 字符体积受控），供前端 diff 渲染与 LLM 感知改动 |
| `run_command` | Shell | PowerShell 非交互执行，120s 超时，输出截断 8000 字符，破坏性命令黑名单，`CREATE_NO_WINDOW` |
| `grep_search` | FsRead | 递归内容搜索，跳过依赖目录与二进制，最多 50 处匹配 |
| `list_dir` | FsRead | 树状列目录（深度上限 4），跳过依赖目录 |
| `run_workflow` | Shell | 多步编排：steps 为 `{tool, arguments, parallel}` 数组，连续 `parallel:true` 扇出并发执行，经沙箱/审批管线，逐步结果含 `parallel` 标记（驱动前端可视化分组） |
| `lsp_query` | Safe（只读） | 经语言服务器语义查询：`go_to_definition` / `find_references` / `go_to_implementation` / `hover`，按文件位置（0 基行/列），需 `lsp.json` 配置对应扩展的语言服务器 |
| `notify_companion` | Safe | 阶段成果播报：`{title?, message}` 发给陪伴人格——异步走 `brain.think_with_options(input, false, true)` 完整陪伴管线生成人设化播报，经 `proactive:bubble` 事件投递（前端 TTS + 气泡 + 聊天记录），写入对话历史（channel=proactive）与记忆（trigger=work_report）；每角色 60s 节流（`work_agent_tools.rs::LAST_NOTIFY`），节流期内仅记录不即时播报 |
| `send_image` | Safe（`send_image_tool.rs`） | 发送本地图片到聊天界面（编程页/微信面板双通道路由，据 `ToolUseContext.session_id` 是否命中编程会话）：编程侧走 `push_agent_image` 进入会话；聊天侧镜像 `send_image_message` 管线写历史 + emit `chat:assistant_image`，窗口不可见时弹横幅 |
| `show_widget` | Safe（`show_widget_tool.rs`） | 渲染 SVG 可视化组件为编程页内联卡片（流程图/架构图/时序图/状态图）：约束内嵌 `description`，call 先做防御性校验（`looks_like_svg` + `contains_forbidden` 拒 script/事件/foreignObject）再经 `push_agent_widget` 推给前端；`CodingWidget{title,kind,code}` 随会话持久化 |

#### 工作待办清单（`work_todo_write`）

`CodingSession.work_todos`（`Vec<WorkTodo>`，`{content, status}` 两字段）随会话持久化，是工作智能体的「当前执行计划」：

- **整表替换**：单工具 `work_todo_write` 每次提交完整清单（`todos: [{content, status}]`，`additionalProperties:false`），没有局部更新、没有按下标的单条编辑——模型每调一次就把整个计划重述一遍，计划因此持续锚定在上下文里
- **强制规则内嵌在工具描述**：每步先建 todo / 完成即标不得批量 / 至多一项 in_progress（多开直接拒绝）/ 简单单步任务跳过清单 / 计划有变就重写整表
- **校验一律拒绝而非静默降级**：trim、非空、去重、状态合法、条数与长度上限（`WORK_TODO_MAX_ITEMS=30` 条 / `WORK_TODO_MAX_CONTENT_CHARS=200` 字）
- **注入每轮上下文**：`render_work_plan` 把清单渲染为「# 工作待办（当前执行计划）」段拼进 system prompt，模型无需主动回读；空清单不注入（不破坏缓存前缀）
- **loop 消费**：清单全部 completed 但循环仍在跑时，注入一次收尾提醒（`plan_done_hinted` 保证只提示一次）
- **生命周期**：新一轮对话开始时若清单已全部完成则归档清空（`maybe_clear_completed_plan`），否则跨轮保留（长任务可多轮推进）；清理点在 `send_message`、斜杠命令拦截之后
- 前端：编程页右栏「待办」页签（`WorkTodosCard`）是**只读面板**——清单的唯一写入方是工作智能体的 `work_todo_write` 工具，用户不参与增删改（面板没有输入框、勾选与删除按钮，也没有回写命令），只订阅 `work_todo:changed` 如实展示智能体当前的计划

#### 方向询问（`work_ask_user`）

`brain/work_question.rs` 注册表 + `coding:question` 事件。推进方向确实分叉、且读代码/跑命令无法判断时，给 2-4 个选项问用户：

- **挂起等待**：工具 `call()` `await` oneshot，`Sender` 存注册表；等待期间不产生 token、不消耗轮次预算，loop 停在那一行
- **答案回流**：以工具返回值（`{selected, custom}`，存 label 而非下标）进入上下文，不伪装成用户消息，上下文保持 append-only
- **三种收尾**：正常回答 / 取消（sender 被 drop，返回 success 让模型自行收尾）/ 30 分钟超时（TTL 与注册表一致）——后两者都不会被停滞检测误判成工具失败
- **校验**：选中 label 必须属于本题选项；单选时选项与自由输入互斥；两者皆空视为跳过（模型自行拍板并在总结里说明）
- **取消联动**：会话 `cancel()` 调用 `cancel_session()` 撤销挂起问题，否则 loop 永久挂起、会话卡在 Running 无法接收新消息
- 命令：`coding_respond_question` / `coding_pending_question`；前端 composer 上方渲染选择卡片

#### 子 agent 委派（`work_delegate` / `work_job`）

`brain/coding_subagent.rs` 执行器 + `brain/work_jobs.rs` 后台注册表：

- **独立上下文**：子 agent 有自己完整的消息历史与工具循环，看不到父会话；`task` + `context` 必须一次交代清楚
- **只回传最终文本**：中间探索步骤不进父上下文——这是委派的价值；前台模式 `await` 到结果
- **后台模式**：`background: true` 登记任务（`WorkJobRegistry`，每会话并发上限 `MAX_JOBS_PER_SESSION=8`）后立即返回 `job_id`；任务终结后结算经 `drain_settlements` 收件箱**主动注入下一轮上下文**（`render_settlement` 渲染），模型不必记得回查；`work_job` 提供 `output`（取回结果）/ `cancel`（放弃，协作式取消在轮次边界生效）/ `list`。终态任务记 `finished_at`，`create()` 机会性执行 `cleanup_terminal_jobs`（终态保留 30 分钟供取回，过期释放——注册表不无界累积，也不新增轮询循环）
- **深度限制**：`ToolUseContext.subagent_depth`（0 = 顶层），`SUBAGENT_MAX_DEPTH=2`，越界返回明确错误让模型自己做或交回上层
- **子 agent 不能向用户提问**：`work_ask_user` 在 `subagent_depth > 0` 时返回 `SubagentCannotAsk`，错误文案指引模型把未决问题写进最终结果带回父级
- **工具边界**：默认只读探索 + `run_command`（`SUBAGENT_DEFAULT_TOOLS`）；委派 / 询问 / 播报 / 进化类工具恒定剔除，模型可经 `tools` 参数显式追加
- **工作区范围由父会话决定**：`work_delegate` 的 `workspaces` 参数 —— 省略 = 继承父会话全部；给数组 = 只用给定的这些；`[]` = 一个都不给。见下方「子 agent 的工作区范围」
- 事件：`coding:subagent`（started / finished）、`coding:job`（started / completed / failed / canceled）
| 工具 | 风险 | 说明 |
|------|------|------|
| `work_todo_write` | Safe | 工作待办清单整表替换（含约束校验），清单注入每轮上下文驱动执行（见下小节） |
| `work_ask_user` | Safe | 方向分叉时出 2-4 个选项问用户，挂起等待回答（见下小节） |
| `work_delegate` | Safe | 委派自包含子任务给独立子 agent（前台 await / `background` 后台派发） |
| `work_job` | Safe | 后台子任务收集 / 取消 / 列表 |

##### 子 agent 的工作区范围（`work_delegate.workspaces`）

父会话可以限定子 agent 能用哪些工作区——只能从**父会话自己拥有的**工作区里选。

| `workspaces` | 子 agent 的工作区 |
|---|---|
| 省略 / `null` | 继承父会话全部（历史行为） |
| `["D:\\a", "D:\\b"]` | 只用给定的这些 |
| `[]` | 一个都不给 |

两条硬约束，方向都是**权限收缩**，实现里是拒绝式校验：

1. **只能选父会话自己拥有的路径。** 传了不属于本会话的目录 → 直接报错，并把本会话的工作区列表回给模型。
   否则子 agent 就拿到了父会话没有的目录，等于绕过父会话的沙箱边界。
2. **主工作区必须可写，所以只读工作区只能进附加列表。** 会话模型里主工作区没有独立只读标记
   （可写性由 `access_level` 决定），把只读目录提成主根会凭空放大授权。
   规则：遍历选中集合，**第一个可写**的当主工作区，其余进 `extra_workspaces`（保留各自只读标记）；
   一个可写的都没有 → 主工作区为空串（无工作区模式）+ 全部进附加列表。

`SubagentRequest.workspaces_restricted` 记录「上层是否显式限定过」，用于提示词：
受限的子 agent 被告知把「需要别处的信息」写进最终回复交回上层，而不是把轮次烧在反复被沙箱拒绝上。

**零工作区的子 agent = 不允许修改任何文件**（可读、不可写、不可执行命令）。这不是额外加的标志位，
而是下面「沙箱确认回调（`coding_sandbox_confirm`）」那条规则的自然结果：无工作区时确认一律被拒
（子 agent 面前没有应答者）。

### task_service —— 自治任务与后台回流

[`brain/task_service.rs`](file:///g:/vivian-rs/src-tauri/src/brain/task_service.rs) 是自治任务执行服务（`ctx.tasks` 能力缝），支撑陪伴对话直接用工作侧能力派活（`run_job` / `spawn_subagent` / `run_workflow` / `delegate_to_work_agent`），并负责把任务状态与完成报告**回流到陪伴对话**。

#### Agent-loop 形态

给定 directive，LLM 逐步决策「下一步调用哪个工具」并执行，直到 `done=true`、某工具声明 `goal_completed`，或达到 `MAX_TASK_STEPS=8`。每步经 `execute_tool_use` 复用主对话沙箱 / 守卫 / 审批矩阵；`TaskEvent`（Started / Step / Completed / Failed / Canceled）经 `ctx.emit_serial` 广播到事件总线（跨角色分享等前端订阅）。

#### 谱系与报告

- `TaskState` 含 `parent` / `children`（子代理谱系）、`report`（子代理回传文本）、`report_consumed`（报告是否已注入陪伴对话）；`run_loop` 的 `tool_ctx.session_id` 携带任务 id，`subagent_report` 据此回写报告。
- 成功结束但模型未调 `subagent_report` 时，`set_fallback_report` 用末尾 3 步摘要自动生成兜底报告，保证回流段始终有内容。
- 命令层 `commands/tasks.rs`：`list_agent_tasks` / `get_agent_task`（含后代谱系树）/ `cancel_agent_task`，供外部查询与取消自治任务。
- 全局句柄：`AppState::new` 里 `task_service::set_global` 注册（[state.rs](file:///g:/vivian-rs/src-tauri/src/state.rs)），供管线步骤等无 AppState 上下文的代码经 `task_service::global()` 访问。

#### 报告回流陪伴对话（收件箱语义）

1. **注入**（`pipeline/steps/prompt.rs::build_background_tasks_section`）：每轮构建 prompt 时查询 `running_top_level_for`（运行中顶级任务）+ `unconsumed_reports_for`（已完成、报告未消费、2 小时窗口内的顶级任务），渲染为「后台任务」动态段——运行中任务显示指令+步数，刚完成未汇报的显示报告并附带「用你的口吻主动向用户汇报」引导。三语标题（`section_heading("background_tasks")`）。
2. **移交**：`prompt.rs::ainvoke` 把待汇报任务 id 写入 `state.metadata["bg_report_task_ids"]`。
3. **消费**（`pipeline/steps/generation.rs`）：动态便签进入请求后，`mark_reports_consumed` 标记该批任务为已消费——**每份报告只注入一次**，后续轮次不再重复注入（便签/整体 system 两条切分路径均覆盖）。
4. **位置**：后台任务段在动态区紧随 Self State（"我正在做什么"的延伸），动态区整体位于历史之后、用户输入之前，不破坏前缀缓存。

#### 结果裁剪

陪伴链路的工具反馈历史（`tool_call_manager.rs::build_feedback_prompt`）与编程侧统一使用 `tools/executor.rs::prune_head_tail`（头 2/3 + 尾 1/6 + 中段折叠标记），超长工具结果的尾部（退出码/报错/diff 收尾）不再丢失。

### pipeline/ —— 对话流水线

[`pipeline/`](file:///g:/vivian-rs/src-tauri/src/pipeline) 实现 LangChain 风格的 Runnable 流水线。

#### 流水线步骤

```
PreProcessing → UserMemorySaving → [QueryRewrite ∥ FastSemantic] → MemoryRetrieval → WebContext
    → PromptBuilding → Generation → ResponseParsing → Validation → ExpressionMotion
    → PsychologyInsight → MoodUpdate → MemorySaving
```

#### 核心文件

| 文件 | 职责 |
|------|------|
| [`base.rs`](file:///g:/vivian-rs/src-tauri/src/pipeline/base.rs) | `Runnable` trait 与组合子（`\|` / `RunnableBranch` / `RunnableRetry` / `RunnableWithFallbacks`） |
| [`state.rs`](file:///g:/vivian-rs/src-tauri/src/pipeline/state.rs) | `PipelineState` 73 字段贯穿全链 |
| [`advisor.rs`](file:///g:/vivian-rs/src-tauri/src/pipeline/advisor.rs) | Advisor 拦截器链（日志/限流/Re2/循环检测） |
| [`prompt_modules.rs`](file:///g:/vivian-rs/src-tauri/src/pipeline/prompt_modules.rs) | Prompt 模块构建器；resolve_prompt_budget 按路由模型上下文窗口、用户等级（0–4）与任务类型计算软预算（`window/2 × 等级% × 任务%`，硬上限 `window×3/5`，绝对上限 262144），随后按 rank 裁剪——**预算必须显著高于静态区**，否则动态区每轮被裁空，详见下方「静态区体积与提示词预算」；其余构建函数含 `build_memory_block`（记忆块 + 英文忠实度/时间感知指引）、`build_memory_group_section`（记忆合并组：Episode+关系日志+记忆本体）、`build_user_profile_group_section`（画像合并组）、`build_tools_block`、`build_agent_status_bar`（Agent 状态栏）、`build_tool_minimal_identity`（工具精简人设 + PERSONA_LOAD + 语言约束）、`tool_minimal_output_format`（工具输出格式 + 按界面语言的语言约束）等；framework 规则加载函数（`safety_rules`/`output_format`/`session_rules` 等）统一加载英文标记化模板、无 lang 参数 |
| [`template_engine.rs`](file:///g:/vivian-rs/src-tauri/src/pipeline/template_engine.rs) | Prompt 模板引擎，`section_schema()` 定义 32 个 section 的结构元数据（9 静态 + 23 动态），`build_prompt_with_sections()` 产出 prompt + 逐 section 元数据（char_count / token_estimate / present） |
| [`context_compress.rs`](file:///g:/vivian-rs/src-tauri/src/pipeline/context_compress.rs) | 多级上下文压缩（Soft Trim → 原子组丢弃 → Reminder）+ 上下文感知压缩（LLM 摘要工具结果） |
| [`compaction_reminder.rs`](file:///g:/vivian-rs/src-tauri/src/pipeline/compaction_reminder.rs) | 压缩后提醒，从丢弃消息提取活跃工具名与最后话题 |
| [`doom_loop.rs`](file:///g:/vivian-rs/src-tauri/src/pipeline/doom_loop.rs) | 死循环检测，追踪 `(tool_name, args)` 签名连续出现次数 |
| [`react.rs`](file:///g:/vivian-rs/src-tauri/src/pipeline/react.rs) | 原生 FC 共享 ReAct 循环骨架；承载压缩、doom loop、goal_completed、轮次上限和 `DialoguePhase`。陪伴侧复杂请求可通过无副作用的 `continue_thinking` 在完整人格上下文中继续分析/核验/规划/反思，最多 4 轮后强制收尾；普通闲聊不续轮 |
| [`inline_tag_scanner.rs`](file:///g:/vivian-rs/src-tauri/src/pipeline/inline_tag_scanner.rs) | 内联标签扫描器，流式剥离 `<e>/<m>` 标签驱动桌宠表情/动作 |

**内联标签字母表（只认这两个）**：`<e name="…" dur="ms"/>` 表情、`<m name="…"/>` 动作。
扫描器的前瞻判定是 `matches!(bytes[after_lt], b'e' | b'm')`——**其它字母一律按普通文本原样保留**，
包括 2026-09-12 下线的 `<s name="…"/>` 贴纸标签（对应回归测试
`test_unknown_tag_letter_kept_as_text`）。加新标签必须同时改前瞻判定、`parse_tag` 的
match 臂和 `commands/chat.rs` 里的 `chat:inline_meta` 映射，三处漏一处就静默失效。

#### 静态区体积与提示词预算（2026-09-15 重定）

提示词是**总量预算**：静态区（人设 + 框架 + 示例）先占，剩下的才是动态区
（情绪 / 记忆 / 当下事件）额度。而静态区**不参与** `trim_sections_to_budget`，
所以它一旦顶满预算，被裁掉的全是动态区。

实测静态区（cl100k，vivian Full 档，`prompts/` 出厂文件）：

| 静态组件 | tokens |
|---|---|
| `[CHARACTER]` Full 档（identity / personality / background / interests / appearance / speech / canon_quotes / relationships） | 10,550 |
| `[EXAMPLES]` Few-shot | 2,468 |
| `【PERSONA_CONFIG】` + `【PERSONA_RULES】` | 1,659 |
| `[FRAMEWORK]`（human_feel / safety / chat_style / session / address / rhythm / prefix） | 1,454 |
| `[FORMAT SPEC]` | 670 |
| `[PERSONA_PROTOCOL]` §1–§5 | 381 |
| `[STYLE]` 预设 | 91 |
| **合计** | **≈ 17,270** |

**旧算法的问题**：`base = window / 8`（128K 窗口 → 16,384）再乘 level0 的 85% = **13,926**，
比静态区本身还小 3,300 —— 于是每轮都把 rank≥1 的段落丢光，只剩 rank=0 骨架。
这正是「陪伴侧活人感不足」的直接机制（`trim_sections_to_budget` 末尾那段
"静态区过大…回复会退回通用寒暄" 的告警就是为此写的）。

**新算法**：`base = window / 2`，硬上限 `window × 3 / 5`，绝对上限
`MAX_PROMPT_BUDGET_TOKENS = 262_144`。128K 窗口 chat 场景为 55,705（level0）～78,643（level4）。
**预算是上限不是目标**——调大不会让 prompt 变长，只是不再丢段落，因此在 token 成本上免费。

**记忆上下文上限**：`MEMORY_CONTEXT_MAX_TOKENS` 1250 → 3000。它同样是**从前往后硬切**
（`steps/memory.rs` 的截断循环），被切掉的记忆当轮完全不可见。上游召回已先截到 9 条，
9 条约 900–1500 tokens，旧值会稳定切掉尾部若干条。

**回归测试**：`test_dynamic_prompt_budget_uses_window_level_and_task` 断言
`resolve_prompt_budget(baseline) == 65_536`（window/2 × level2 × chat）与 `> 25_000`
（必须显著高于静态区）。改 `base` 除数时这两条会一起失败——这是故意的，别只改数字了事。

**量测脚本**：`.workbuddy-ai/tmp/measure_static_region.py`（引 tiktoken cl100k，
与运行时 `estimate_tokens` 同一编码器）。改人设文件后重跑即可看体积变化。

#### `is_first_meeting` 判据（2026-09-15 修正）

`PromptParts.is_first_meeting` 的字段文档（`prompt_modules.rs`）写明
「由持久记忆库状态独立判定，**不能从本轮空召回推断**」，`brain.rs::generate_startup_greeting`
也是正确实现（`memory.non_seed_count() == 0 && dialogue.get_history_length() == 0`）。

但 `steps/prompt.rs` 曾写成 `state.memory_text.is_empty()` —— 把"这轮没召回到高相关记忆"
误判成"第一次见面"，触发 `human_feel.en.md` 的 `NO_ONBOARDING` 反例（自我介绍 / 破冰脚本）。
而召回为空远比真·首次见面常见（短查询、query rewrite 跳过、分数低于 `min_score` 被过滤都会命中），
所以这是陪伴侧"机器感"的一个高频来源。

现改为与 `brain.rs` 同源判据：`memory.non_seed_count() == 0 && state.messages.is_empty()`。
两个要点：
- `state.messages` 在 `PromptBuildingStep` 运行时**只含历史**——本轮用户消息要到
  `chat_chain.rs` 末尾的 `add_message_with_metadata` 才入历史，所以"历史为空"这个判据可靠。
- `memory` 未注入时取 `false`（宁可漏掉一次破冰，也不要误判出自我介绍）。

#### 关键函数

```rust
// prompt_modules.rs
pub fn build_memory_block(memory_text: &str, lang: &str) -> String
// 在记忆块末尾追加忠实度约束 + 时间感知指引：
// - 记忆可能过时，与用户矛盾时以用户为准
// - 每条记忆带时间戳，需与「## 你周围正在发生什么」中的当前时间对比
// - 区分已发生/正在发生/未来计划（"下周要做xx"那一周没到就是未来计划）

// prompt_modules.rs —— Agent 状态栏
pub fn build_agent_status_bar(messages: &[ChatMessage], user_input: &str, focus_active: bool) -> Option<String>
// 以 <agent_status> 键值对（当前时间 / 本次对话轮数 / 最近工具调用 / 专注模式）作为
// user-role 元消息追加在用户输入之后、紧邻生成位置；末尾附"读数 + 操作策略"成对指令。
// 计数由代码确定性维护，不依赖 LLM 统计。

// context_compress.rs —— 上下文感知压缩
pub async fn compress_conversation_context_aware(
    router: &ModelRouter, task_type: &str, messages: &mut Vec<ChatMessage>,
    threshold_tokens: usize, keep_recent: usize, query: &str,
) -> CompressResult
// 在确定性压缩之上，对被丢弃的工具调用组用 LLM 结合 query 生成针对性摘要（三语），
// 失败回退到确定性预览；分组/原子性/阈值逻辑与 compress_conversation 一致。
```

#### steps/ 子目录

| 文件 | 步骤 | 职责 |
|------|------|------|
| `pre_processing.rs` | PreProcessing | 输入预处理、speaker prefix 解析、channel 路由 |
| `query_rewrite.rs` | QueryRewrite | LLM 查询重写，含 `should_skip_retrieval` 启发式（跳过"嗯/你好/好的"等闲聊） |
| `fast_semantic_step.rs` | FastSemantic | 嵌入语义分类 + 同步计算认知知识需求评估（EpistemicAssessment）与日程通知信号评估（ScheduleAssessment），与 QueryRewrite 并行执行 |
| `memory.rs` | MemoryRetrieval | 混合检索 + 置信度标记 + Verifier 二分类过滤 + 多跳关联检索（注入 user_model 时展开关联话题二次召回）。同文件内 `UserMemorySavingRunnable` / `MemorySavingRunnable` 提供 `skip_memory_save` 元数据门控——启动问候等内部指令设置后跳过用户消息/AI 回复的记忆写回，避免合成的问候指令被当作真实用户消息污染记忆库 |
| `web_context.rs` | WebContext | 基于 KnowledgeDecision 驱动主动搜索，结果注入 prompt |
| `prompt.rs` | PromptBuilding | U 型注意力布局组装 prompt（含认知信号 + 日程信号 + 主动搜索结果） |
| `generation.rs` | Generation | 首轮响应生成（`call_llm_native_fc` / `call_llm_native_fc_stream`），拿到首轮文本与工具调用后委托 `pipeline/react.rs` 的共享循环继续多轮；`push_stream_chunk` 统一 emitter 推送 |
| `validation.rs` | Validation | 空文本检测 + 长度截断 + 幻觉检测（注入对话历史防误报） |
| `reflection.rs` | ReflectionRunnable | 反思调用，产出表情/动作/control_actions + 心理状态 + world_update + goal_updates + evolution（自我进化）+ **memory_note**（长期记忆笔记沉淀：复用主对话 system_prompt = 同模型亲笔口吻 + 笔记当前全文在场天然去重；只收相处约定/承诺/教训/梗四类，事实偏好走 long_term_memory；`apply_memory_note` 经 `append_memory_md` 落盘，写侧字符预算溢出自动驱逐最旧日期分节；主路径与内联模式两条路径都应用，失败仅记日志） |
| `mood.rs` | MoodUpdate | 心理状态更新 |

#### direct 渠道纯文本约定（气泡不露 markdown）

同一份回复文本有三条渲染通道、规范各不相同：ChatWindow 富文本渲染（markdown 语法被消费）、桌宠气泡（`MessageBubble`）纯文本容器（语法会**裸露**）、记忆图谱按入库原文展示（不剥语法）。直接对话（direct）因此双管齐下：

- **prompt 禁源**：`prompt_modules.rs::build_channel_style_guide("direct")` 的 `[CHANNEL_STYLE]` 规则区声明 `no markdown formatting ever`（无 ** 加粗 / * 斜体 / 反引号 / # 标题 / 列表）——话语落入纯文本气泡并按原样写入记忆；`wechat` / `wechat_group` 分支不加此限制（保留富文本能力）。guide 经 `PromptParts.channel`（`steps/prompt.rs` 由 `state.current_channel` 填充）注入静态区
- **前端显示层兜底**（`src/utils/stripMarkdown.ts`）：剥 markdown 语法保留正文——反引号 → 加粗/删除线（`**`/`__`/`~~`）→ 斜体（带守卫：定界符紧贴内容且内容非纯数字，防 `3 * 4` 误伤）→ 链接 `[t](url)→t` → 行首 `#`/引用/列表符；代码围栏行丢弃、正文保留
- **接入点**（`src/controllers/BubbleController.ts`）：四个文本写入入口（`showBubble` / `updateBubble` / `showStreamingBubble` / `settleSegment`）收文本先 `stripMarkdown` 再上屏——打字机逐字揭示的是净化后文本，不闪半个 `*`；`currentBubble` 全仓写入点仅此四处，无绕过路径

记忆入库仍是回复原文（对话记忆由 brain 层存 `response.text`），若 direct 禁源效果不足，图谱侧需另做入库前净化。

#### ReAct 工具调用循环（react.rs）

原生 Function Calling 的多轮 ReAct 循环由 `pipeline/react.rs` 承载——压缩、`DoomLoopTracker`、goal_completed、round-limit 等逻辑只在骨架里存在一份：

- **统一入口** `run_react_loop(router, tool_call_manager, emitter, ReactParams) -> (String, Vec<ToolCallResult>, usize, Option<f64>)`；`ReactLoop::react_round(...) -> RoundOutcome` 处理单轮（无工具调用→阶段迁移 / 工具执行并追加 / goal_completed→`wrap_up` / doom loop→`wrap_up` / 延迟工具注入 / 首轮进入执行态）
- **两个入口只差"首轮怎么拿响应"**：`generation.rs` 的两个函数各自只负责首轮（`generate_with_tools` 或流式首包解析 + 重试/回退），把 `first_content` / `first_calls` 连同 `messages` / `tools` / `task_type` / `channel` / `max_rounds` 塞进 `ReactParams` 委托给 `run_react_loop`
- **执行态 / 表达态一等概念** `DialoguePhase { Persona, Execution }`：真正调用外部工具时切换执行态；纯 `continue_thinking` checkpoint 保持完整人格，确保推演后的最终文本仍以角色口吻输出。内部推演最多 4 轮，达到上限恢复人格并强制收尾；不暴露完整思维链。
- **工具语义自声明** `ToolSemantics { Retrieval, Action }`：`Tool` trait 默认方法 `semantics()` 由 `is_read_only()` 推导（读→Retrieval 需 relay 转述，写→Action 直接确认），个别工具可覆盖（如 `observe_user` 显式 `Retrieval`）
- **统一 emitter 推送** `push_stream_chunk`（`generation.rs`）：抽 `catch_unwind` 防护的 chunk 推送，供各调用点复用（`proactive/` 也复用）
- **统一轮次来源** `config.tools.max_rounds`（默认 20，`0`=无限外部工具轮次）；`continue_thinking` 另有 4 轮内部推演上限，避免无界自我反思。
- **行为保证**：doom loop 检测覆盖流式与非流式两条路径；relay 文案与 round-limit 统一为渠道感知；`goal_completed` 在每轮均检查；首轮压缩逻辑对齐；抽取文本加防御

#### WebContext —— 认知知识需求驱动的主动搜索

[`web_context.rs`](file:///g:/vivian-rs/src-tauri/src/pipeline/steps/web_context.rs) 基于认知知识需求评估（Epistemic Assessment）驱动主动搜索。Web Search 是认知能力而非用户显式调用的工具——当系统检测到用户输入可能需要外部知识验证时，在生成前预搜索，结果作为上下文注入 prompt。

**设计理念**：替代单一置信度阈值，转向多维知识需求评估。核心问题不是"我有多确定"，而是"为了给出可靠回答，是否需要从外部世界获得证据"。

**评估流程**：

1. **FastSemantic 阶段**（`fast_semantic.rs`）同步计算 `EpistemicAssessment`，产出四维评分：
   - `semantic_clarity`：语义清晰度（我理解用户在说什么吗？）
   - `factual_dependence`：外部事实依赖度（回答是否依赖外部事实？）
   - `temporal_sensitivity`：时效敏感性（事实是否可能随时间变化？）
   - `interpretation_risk`：解释风险（不搜索自行解释是否容易误解用户？）
   - `knowledge_gap`：知识缺口（模型是否有足够知识？）

2. **规则映射**（`evaluate_epistemic_state`，纯规则，不调用 LLM）：
   - 模糊指代（"那个瓜""你听说了"）→ 降低 clarity，提高 risk
   - 矛盾描述（"被"+"攻击"）→ 降低 clarity，提高 risk、factual
   - 网络梗/流行语 → 提高 risk、factual
   - 多专有名词组合 → 提高 gap、factual、risk
   - 时效性内容（"最近""今天"+非问候语）→ 提高 temporal、factual
   - 复杂问句（>15字+问号）→ 提高 factual、gap

3. **决策映射**（`KnowledgeDecision`）：
   ```
   temporal ≥ 0.7 → SearchRequired
   risk ≥ 0.7 → SearchRequired
   factual ≥ 0.7 && gap ≥ 0.5 → SearchRequired
   clarity < 0.4 → SearchPreferred
   factual ≥ 0.5 && temporal ≥ 0.3 → SearchPreferred
   factual ≥ 0.4 → SearchOptional
   其他 → NoSearch
   ```

4. **WebContext 步骤**读取 `state.epistemic_assessment`，在 `SearchRequired` / `SearchPreferred` 时执行搜索，结果写入 `PipelineState.web_context`。

5. **PromptBuilding 步骤**同时注入：
   - `epistemic_signals_section`：认知信号段落（"系统检测到用户输入可能存在以下特征"），让 LLM 感知是否需要搜索，辅助自主调用 `web_search` 工具
   - `proactive_search_section`：主动搜索结果（已搜索完成时注入），附带"不要假装本来就知道"的指导

与 LLM function calling 互补：预搜索在生成前完成，LLM 生成时仍可自主调用 `web_search` 工具做进一步搜索。

#### ScheduleSignal —— 日程通知信号驱动的助理剧本

[`fast_semantic.rs`](file:///g:/vivian-rs/src-tauri/src/emotion/fast_semantic.rs) 的 `evaluate_schedule_signal` 判定用户输入是否像一份**转来的、含时间安排的通知材料**（群通知 / 会议安排 / 活动须知），命中时在 prompt 注入助理引导段，让角色按真人助理的方式处理：抽取事件 → 高置信直接 `add_todo` 建待办并告知（用户可一句话撤销）→ 到点经 Scheduler 主动提醒。

**为什么走规则而不是嵌入分类**：嵌入语料全是短句，拿 500 字通知去比相似度会被细节稀释、全类掉到 unknown；正则恰恰相反——文本越长，日期/特征词命中越多，判定越准。日程检测是**内容属性**而非用户意图（转发通知的 intent 本来就是 sharing），故与 EpistemicAssessment 一样纯规则、零嵌入零推理。

**判定规则**（`evaluate_schedule_signal`）：

- 日期模式（`X月X日`、`星期X/周X`、ISO 日期、明天/下周等相对词）+ 时间模式（`HH:MM`、`X点`）计数为 `temporal_hits`
- 通知特征词（通知 / @所有人 / 签到 / 入场 / 携带 / 地点 / 会场 / 主讲人 / 届时 等）命中列表
- 满足任一即判定 `is_schedule_like`：`temporal_hits ≥ 3`；或 `≥2 + 特征词 ≥2`；或 `≥1 + 特征词 ≥3`；或 `≥1 + 长文(≥200字) + 特征词 ≥1`

**数据流**：`analyze()` 同步计算 → `FastPerceptionResult.schedule_assessment` → `PipelineState.schedule_assessment` → PromptBuilding 的 `format_schedule_signals` 仅在 `is_schedule_like` 时产出 `schedule_signals_section`（动态区 rank=1，位于 epistemic_signals 之后）。引导段内容：把输入当作转来的材料而非用户本人在说话；多时间点通知按事件分别建待办或询问要记哪些；**事件时间与提醒时间分离**（见下）。

**提醒时间的智能提前量**：`add_todo` 的 `event_time` 参数记录事件本身的开始时间（如典礼 09:00），`due_date` 承载提醒触发时间——引导段要求 LLM 结合画像组（user_facts 的住址/通勤/作息，每轮已注入）预留路程与准备时间，把提醒定在 `event_time` 之前；记忆不足时先问用户，答复经 auto_extractor 沉淀回 user_facts，同类事件的提前量随使用变准。到点提醒由 Scheduler 联动完成（桌面通知 + 聊天窗主动开口），无需额外机制。

### cross_character.rs —— 跨角色通信总线

[`cross_character.rs`](file:///g:/vivian-rs/src-tauri/src/cross_character.rs) 实现角色间对话。

#### 核心结构

```rust
pub static CROSS_CHARACTER_BUS: Lazy<Arc<CrossCharacterBus>> = Lazy::new(|| ...);

pub struct CrossCharacterBus {
    app_handle: RwLock<Option<AppHandle>>,
}

pub struct CrossCharacterRequest {
    pub source_id: String,    // 发起方
    pub target_id: String,    // 接收方
    pub message: String,      // 源角色要说的话
    pub stream_id: String,    // 前端路由用
}

pub struct CrossCharacterReply {
    pub reply: String,                // 目标回复文本（仅 speak 模式非空）
    pub response_mode: String,        // speak / non_verbal / internal / ignore
    pub conv_state: String,           // active / cooling / closed / peer_busy / target_busy
    pub should_continue: bool,        // 是否建议源角色继续
    pub expression: String,
    pub motion: String,
}
```

#### `send()` 完整流程

```
1. 会话生命周期检查（start_or_continue）
   ├── 冷却中 → 直接返回 CrossCharacterReply{response_mode:"ignore"}
   └── 创建/继续会话 → 继续

2. 互锁检测
   ├── 源在 UserChat turn 且目标在 UserChat turn 或收到 pending_user → 返回 peer_busy
   └── 通过 → 继续

3. emit cross:start（通知前端对话开始）

4. 获取目标角色 think_lock（25s 超时）
   ├── 超时 → 返回 target_busy
   └── 获取成功 → 继续

5. TOCTOU 加固：获取锁后再次校验目标角色是否已进入 UserChat turn 或收到 pending_user
   └── 是 → 返回 peer_busy（注意只检查目标角色，源角色在 UserChat turn 内调用
        talk_to_character 是工具调用的正常语义，不构成死锁条件——死锁需要双方互相
        等待对方的锁，而源角色持有自己的 think_lock，目标 think_lock 已被获取，
        目标无法构成反向等待）

6. 切换 channel 为 cross_character
   session_coordinator.try_enter_cross_turn(target_id, conv.id, memory, dialogue)
   ├── 用户输入等待中 → 返回 user_input_pending
   └── 成功 → 继续

7. 构造合成输入：
   - 主体：[源角色名 says to me] 消息内容
   - 记忆锚点：从 unified_event_ledger 检索 A↔B 最近 2-4 条事件
   - 交接上下文：build_handoff_context（源情绪/疲劳度/最近对话/亲密度）
   - 共同观察：activity_journal.to_brief()
   - 轮次提醒：WARN_ROUNDS 提示收尾 / MAX_ROUNDS 强制结束

8. brain.think_cross_character(synthesized_input, stream=true)
   └── 流式 chunk 通过 cross:chunk 事件推送

9. 会话状态更新：update_after_round(response_mode, text, message)

10. emit cross:done（含 final_text / response_mode / conv_state / should_continue）

11. 源角色记忆持久化：
    - dialogue_add_with_meta：写入源角色发言（assistant）+ 目标反馈（user）
    - add_memory_with_metadata：合并写入 1 条 CasualConversation 记忆
      （带 short_term 标签 + speaker/listener/perspective 元数据）
    - 目标角色补写 1 条对称记忆

12. 关系日志 + SocialState 数值更新 + 关系认知事实抽取（每 3 轮一次）

13. 更新双方 LAST_SPOKEN / LAST_SPOKEN_TEXT
```

#### 关键函数

| 函数 | 职责 |
|------|------|
| `build_handoff_context(source_brain, target_brain, target_id, reason)` | 构建交接上下文包（源情绪/疲劳/最近对话/亲密度） |
| `HandoffContext::render(source_name)` | 渲染为 prompt 注入文本 |
| `build_speaker_prefix(speaker, listener, char_id)` | 构造 `[I say to User]` / `[User says to me]` 前缀 |
| `parse_any_speaker_prefix(text)` | 解析任意说话者前缀（支持第一/第三人称/旁观） |
| `strip_memory_anchor(text)` | 剥离合成输入尾部的 `[近期你们的话题]` 等锚点脚手架 |
| `roommate_status_text(source_id, lang)` | 生成室友 Public State prompt 段落 |
| `roommate_cognitive_text(source_id, lang)` | 生成室友行为印象（注意力/活动/目标/社交意愿） |

### conversation/ —— 会话生命周期

[`conversation/`](file:///g:/vivian-rs/src-tauri/src/conversation) 把所有对话建模为有生命周期的会话对象。

#### 状态机

```
Created → Active → Cooling → Closed
              ↑       │
              └───────┘
              抢救（score ≥ 0.8）
```

#### 核心文件

| 文件 | 职责 |
|------|------|
| [`manager.rs`](file:///g:/vivian-rs/src-tauri/src/conversation/manager.rs) | `CONVERSATION_MANAGER` 全局单例，管理所有会话 |
| [`session.rs`](file:///g:/vivian-rs/src-tauri/src/conversation/session.rs) | `Conversation` 会话对象，含状态机与评分公式 |
| [`evaluator.rs`](file:///g:/vivian-rs/src-tauri/src/conversation/evaluator.rs) | Novelty/Energy/Continuation 评分计算 |
| [`integrity.rs`](file:///g:/vivian-rs/src-tauri/src/conversation/integrity.rs) | 对话完整性修复，扫描孤立 tool_call 插入合成 tool_result |

#### ResponseMode

```rust
pub enum ResponseMode {
    Speak,        // 正常回复（生成文本）
    NonVerbal,    // 只做动作/表情
    Internal,     // 只更新内部想法/记忆
    Ignore,       // 完全忽略
}
```

#### 评分公式

- **Novelty**（新信息密度）：问号 +0.3 / 长度 >10 字 +0.2 / >30 字 +0.2 / jieba 实词 >3 +0.3 / 回复 >15 字 +0.1
- **Energy**：Speak +0.1+ΔNovelty×0.3 / NonVerbal -0.05 / Internal -0.02 / Ignore -0.3
- **Continuation**：0.3 + Novelty 加成 + Energy 加成 - 轮次衰减 - 低能量惩罚

### memory/ —— 三层记忆系统

[`memory/`](file:///g:/vivian-rs/src-tauri/src/memory) 统一管理短期/中期/长期记忆。

#### 核心文件

| 文件 | 职责 |
|------|------|
| [`manager.rs`](file:///g:/vivian-rs/src-tauri/src/memory/manager.rs) | `MemoryManager` 主入口，按 char_id 路由；含种子记忆解析（`parse_seed_file` / `seed_from_file`）；`save_to_disk` 手指纹差异落盘（`persisted: HashMap<id, fingerprint>` 与当前条目比对，仅 upsert 变更行/删除移除行）；**检索零拷贝**：候选无过滤时直接借用 `data.entries` 切片（`Cow`），仅需过滤时才深拷贝；`search_memories_with_options` 的 Step 2~4 移入 `inner` 作用域内借用切片，省掉整表克隆；**常驻上限**：超过 `MAX_RESIDENT_ENTRIES=20000` 时 `evict_archived_from_memory` 卸载完全归档条目（`consolidated && !is_summarized`，检索候选都进不去的死重），只卸内存副本、磁盘行保留，且同步从 `persisted` 摘除指纹——否则下一次 `save_to_disk` 会把「内存无而磁盘有」误判为删除 |
| [`entry_store.rs`](file:///g:/vivian-rs/src-tauri/src/memory/entry_store.rs) | 记忆条目 SQLite canonical 存储（`memory/entries.db`）：行级 upsert/delete/clear，WAL 模式；旧 `unified_memory.json` 自动迁移为 `.migrated`。明文镜像默认关闭，`VIVIAN_MEMORY_PLAIN_MIRROR=1/true/yes` 才生成，并在事务提交后原子同步；WAL 定期 checkpoint 回收 |
| [`conversation_archive.rs`](file:///g:/vivian-rs/src-tauri/src/memory/conversation_archive.rs) | 多级对话存档：L1 满 4 条合并最旧 3 条，最高 L3；摘要写入前脱敏，索引为 `conversation_archive.jsonl` 并原子重写；明文 `archive_plain/` 仅在 `VIVIAN_MEMORY_PLAIN_MIRROR` 显式开启时生成；每轮最多注入 8 条 `[CONVERSATION ARCHIVE]` 摘要 |
| [`memory_md.rs`](file:///g:/vivian-rs/src-tauri/src/memory/memory_md.rs) | 角色长期记忆笔记（`characters/<char_id>/memory/memory.md`，与结构化记忆库互补——每轮全量注入的相处约定层）：只收相处约定/承诺/教训/梗四类；**两区模型**——按分节标题形态分已整理区（主题分节）与待整理区（日期分节 `## YYYY-MM-DD HH:MM`，原始沉淀）；**写侧字符预算硬不变量**——append 溢出时 `enforce_char_budget` 从最旧日期分节起整节驱逐（文件恒 ≤ cap 2000，注入侧永不截断），write 超 cap 直接 Err 拒绝（不静默截断），read 侧 cap 仅作手工编辑旁路安全网；`tidy_need`（`TidyNeed`：待整理区非空行数 ≥ 12 走 `Incremental` 只整理新增沉淀、由 `merge_entries` 机械并入，总字符 > 1700 走 `FullCompaction` 全文重排）供睡眠整理分派路径 |
| [`pipeline.rs`](file:///g:/vivian-rs/src-tauri/src/memory/pipeline.rs) | 巩固流水线 ShortTerm → MidTerm → LongTerm → Insight；Stage 3.5 概念归并（Insight → UserModel + 图谱）；**断点续跑**：Stage 1 摘要在写库前把源 ID 记入 `consolidation_progress_<char_id>.json`（上下文键 = 角色 + 逻辑日，跨天作废），启动恢复时按 `promoted_from` 区分「已摘要未标记」与「未落库」，防止崩溃窗口内重复摘要或漏摘要 |
| [`consolidation.rs`](file:///g:/vivian-rs/src-tauri/src/memory/consolidation.rs) | 夜间睡眠巩固；**步骤级熔断**：pipeline / belief / memory_md 三步连续失败 ≥ 5 次转 `paused`（显式 `paused_reason`，暂停期间跳过不烧 LLM，1 小时半开重试），健康快照持久化到 `consolidation_health_<char_id>.json` 供 UI 读取；**memory.md 整理步**（Stage 5）：`tidy_need` 非 `None` 才触发——`Incremental` 经 `split_regions` 取待整理区（仅新增沉淀）交 memory 路由（机械整理不需人设）产出条目，由 `merge_entries` 机械并入已整理区，`FullCompaction` 读全文按主题重排精简；并入后逼近 1700 字符自动回落全量压缩；`write_memory_md` 内部校验预算上限、超限拒绝写保原文件不动 |
| [`step_health.rs`](file:///g:/vivian-rs/src-tauri/src/memory/step_health.rs) | 步骤健康跟踪：每步 last_success/error + 熔断暂停原因；同根因错误签名只打一次 error；原子写入。**熔断双路径**：① 连续失败 ≥ 5 次（快路径，彻底死亡）；② 滑动窗口错误率 ≥ 60% 且样本 ≥ 5（慢路径，半死不活状态）——`recent_results` 窗口记录最近 20 次成败（成功样本也计入，偶发失败不误熔断，交替成败的 flaky 步骤照样熔断）；serde default 兼容旧持久化 |
| [`retriever.rs`](file:///g:/vivian-rs/src-tauri/src/memory/retriever.rs) | 混合检索（BM25 + 向量 + RRF 融合 + 实体/专名多路补充召回 + 语义去重 + **MMR 多样化**）。**MMR 多样化**（`mmr_diversify` / `MMR_LAMBDA=0.7`）：对排序结果贪心重排 `λ×relevance − (1−λ)×max_sim(已选集)`，相似度用 Jaccard token 重叠（jieba 分词，零嵌入成本），让 Top-K 覆盖更多不同侧面而非近重复堆叠，插入在精排/综合权重排序之后、截断之前；λ≥1 短路纯相关度。`MemoryRetrievalFilter` 结构化预过滤（memory_type/tags/时间窗口）；检索评测集（hit@k / MRR）。**BM25 分词缓存**：以 `memory_id` 为 key 的全局有界缓存（上限 8000 条），值为 `(内容指纹, 词频表+总词数)`，指纹由 content/tags/description 哈希得到，内容变更自动重算，避免每次对话重复 jieba 分词 |
| [`strategy.rs`](file:///g:/vivian-rs/src-tauri/src/memory/strategy.rs) | 三档检索策略（Auto/Vector/Hybrid）+ Knowledge 时间衰减 |
| [`reranker.rs`](file:///g:/vivian-rs/src-tauri/src/memory/reranker.rs) | 独立精排（cross-encoder reranker）：`Reranker` trait + `OllamaRerankClient`（本地 Ollama `/api/rerank`）+ `NoopReranker` 回退；精排失败静默回退不阻塞检索 |
| [`embedding.rs`](file:///g:/vivian-rs/src-tauri/src/memory/embedding.rs) | 嵌入服务工厂 `build_embedding`（local Ollama / 云端 API / 哈希回退）；**自动升级** `probe_ollama_embedding_model`：未配置时纯 socket 探测运行中的 Ollama（127.0.0.1:11434 /v1/models），装有 bge-m3/bge*/embed*（维度可解析）即自动启用远程嵌入，否则回退哈希；不启动任何服务。**全局嵌入缓存**：cap 512，key 为 `(fnv1a64(text), 文本长度, model, dim)` 而非完整文本——记忆正文动辄数百字节，用全文当 key 会和向量本身吃同样量级的内存 |
| [`embedding_registry.rs`](file:///g:/vivian-rs/src-tauri/src/memory/embedding_registry.rs) | 嵌入模型注册表：内置已知模型元数据（dimension/source），`build_embedding` 自动校正维度，避免错配反复重建索引 |
| [`qdrant.rs`](file:///g:/vivian-rs/src-tauri/src/memory/qdrant.rs) | 外部向量库（Qdrant）REST 客户端：collection/HNSW 管理、带元数据过滤检索、upsert/delete/count/滚动读取 |
| [`lifecycle.rs`](file:///g:/vivian-rs/src-tauri/src/memory/lifecycle.rs) | 记忆生命周期统一评估：`health_score`（0..1，evidence/importance/recency/usage 加权）+ `HealthGrade` 分级 + `plan_compression` 压缩预算规划 |
| [`graph_store.rs`](file:///g:/vivian-rs/src-tauri/src/memory/graph_store.rs) | 知识图谱（实体 + typed edges + BFS fanout）；支持 `EntityType::Concept` 概念实体（`ingest_concepts` / `find_concept_memories`） |
| [`evidence.rs`](file:///g:/vivian-rs/src-tauri/src/memory/evidence.rs) | 证据驱动可信度（reinforcement/disputation 双时钟衰减） |
| [`retention.rs`](file:///g:/vivian-rs/src-tauri/src/memory/retention.rs) | 保留策略 + 归档倒计时 |
| [`conflict.rs`](file:///g:/vivian-rs/src-tauri/src/memory/conflict.rs) | 冲突检测三阶段流水线（语义相似度 → LLM 判定 → 合并/覆盖） |
| [`event_log.rs`](file:///g:/vivian-rs/src-tauri/src/memory/event_log.rs) | 事件溯源 append-only 日志 |
| [`redact.rs`](file:///g:/vivian-rs/src-tauri/src/memory/redact.rs) | 消息入库前 PII 脱敏：`detect_pii` 识别银行卡号/密码等敏感片段并替换为 `[大写类型]` 占位符（`redact_content` / `redact_for_log` / `has_pii`），`tracker_lookup` 凭占位符还原原文；纯占位符内容（无语义价值）由 `is_pure_placeholder_content` 判定后调用方跳过入库，不污染条目库与向量索引。占位符→原文追踪表 `TrackerStore{map + VecDeque order}` FIFO 上限 `TRACKER_STORE_CAP=4096`，超限驱逐最旧——表内驻留敏感原文，驱逐即隐私信息最先离开内存（占位符仍可读，仅丢失还原能力） |
| [`unified_event_ledger.rs`](file:///g:/vivian-rs/src-tauri/src/memory/unified_event_ledger.rs) | 统一事件账本，跨角色共享事件索引；行为事件（long_idle/quiet_mode/mood_event/presence_log 等）经 `register_world_event` 写入（sender=system/receiver=all/visibility=Public/associated_char_id=角色ID）。**事件覆盖补全**：被冷落过程事件 `user_ignored`（连续第 N 次主动搭话未获回应）、用户关键操作 `user_media_changed`（播放/切歌，600s 节流）与 `user_app_switched`（应用类别切换，180s 节流）均入账本。`event_base_importance` 按类型分级：dialogue 0.9 / compacted_summary 0.85 / action 0.7 / user_ignored·ignored_message·mood_shift·mood_event 0.6 / user_media_changed·user_app_switched·observer_note 0.5，驱动日记 / recap / 对话 prompt / 内心独白素材排序 |
| [`verifier.rs`](file:///g:/vivian-rs/src-tauri/src/memory/verifier.rs) | 检索后小模型二分类过滤无关记忆 |
| [`llm_enricher.rs`](file:///g:/vivian-rs/src-tauri/src/memory/llm_enricher.rs) | 写入时 LLM 抽取元数据；`manager.rs::should_enrich` 类型门控：仅 ImportantEvent/LongTerm/Knowledge/User/Preference/Identity/SessionSummary 走增强，其余规则化 |
| [`auto_extractor.rs`](file:///g:/vivian-rs/src-tauri/src/memory/auto_extractor.rs) | 从对话自动抽取长期事实。`add_new` **必须**走 `add_memory_enriched_with_metadata`（而非 `add_memory_with_metadata`），否则 `semantic_type` 永远缺失——见下节 |
| [`user_facts.rs`](file:///g:/vivian-rs/src-tauri/src/memory/user_facts.rs) | 用户事实画像（L0/L0.5/L1/L2 四层）；`freshness_note` 时效标注：L1 近期状态整段超 7 天、L2 各条事实超 30 天未更新时在 prompt 中标注「⚠ 此信息已 N 天未更新，可能已过时」，防过时信息被当现状引用 |
| [`user_model.rs`](file:///g:/vivian-rs/src-tauri/src/memory/user_model.rs) | 用户认知模型（UserTrait/UserGoal/UserProject，证据驱动更新）；概念层归并（`merge_concept`） |
| [`session_compressor.rs`](file:///g:/vivian-rs/src-tauri/src/memory/session_compressor.rs) | 单层会话回顾 `[CONVERSATION RECAP]`（多级存档为空时的回退路径，见 conversation_archive.rs） |
| [`ivf_index.rs`](file:///g:/vivian-rs/src-tauri/src/memory/ivf_index.rs) | IVF 倒排索引（k-means 聚类加速） |
| [`vector_search.rs`](file:///g:/vivian-rs/src-tauri/src/memory/vector_search.rs) | 向量存储，后端可切换：内置 sqlite-vec（默认，零依赖）或外部 Qdrant（`open_configured` 按配置选择）；含 `model` 列支持增量/断点续传重建；`MemoryVectorStore` 各方法按后端路由 |

#### 写入路径决定检索排序：`semantic_type` 只在 enriched 路径写入（2026-09-15 修正）

检索侧 `retriever.rs::semantic_type_boost` 按语义类型给 `fused_score` 加权：

| SemanticType | boost |
|---|--:|
| User / Feedback | 1.15 |
| Relationship | 1.10 |
| SharedMemory / Project | 1.05 |
| Reference | 1.00 |
| General | 0.95（地板） |

而 `MemoryItem::semantic_type()` 读的是 `metadata["semantic_type"]`，**只有 `add_memory_enriched_with_metadata` 会写这个键**（同批写入的还有 `keywords` / `description` / `summary` / `mood_tags`）。`add_memory_with_metadata` → `add_memory_inner(embedding_text = None)` 完全绕过 `MemoryEnricher`，这四种元数据一个都不会产生。

因此**选错写入函数的后果不是"少一点加成"，而是排序整体退化**：所有走该路径的记忆 `semantic_type` 恒为 `General`、boost 恒为地板 0.95，用户偏好 / 关系事件 / 共同经历这些最该被想起来的记忆，在 BM25 与向量分数接近时压不过普通闲聊，表现为"她记不住我说过的事"。

历史缺陷：`auto_extractor.rs::add_new`（所有 AutoExtractor 产出的长期事实，量最大）此前调的是 `add_memory_with_metadata`。现已改走 `add_memory_enriched_with_metadata`；`LongTerm` 本就在 `should_enrich` 白名单内，故增强分支真实生效（enricher 未注入或调用失败时自动退化为规则化写入，行为与改前一致）。代价是每条抽取记忆多一次轻量 LLM 调用。

排查同类问题时先问一句：**这个写入方走的是 enriched 还是 plain 路径？** 新增记忆写入方一律优先 enriched。

#### MemoryType 枚举

```rust
pub enum MemoryType {
    ShortTerm,           // 短期记忆
    MidTerm,             // 中期记忆
    LongTerm,            // 长期记忆
    SessionSummary,      // 会话摘要
    Insight,             // 反思洞察
    InnerMonologue,      // 内心独白
    ObservationNote,     // 旁观观察
    CasualConversation,  // 闲聊
    Knowledge,           // 知识文档（带 TTL）
    UserFact,            // 用户事实
}
```

#### 角色前史解析（`manager.rs`）

角色前史是首次启动（或记忆被清空）时写入的角色专属记忆，定义在 `src-tauri/prompts/characters/<char_id>/seed_memories.md`，每个角色约 40 条，覆盖世界观锚点 / 身份觉醒 / 个人兴趣与习惯 / 性格弱点 / 跨角色关系里程碑 / 日常碎片 / 内部梗与共同秘密 7 类。叙事重心在角色自身（创造者仅在前 2 条出现），60%+ 的记忆为角色独处或两人日常；时间非线性，包含"后来……"式历史沉淀记忆。Vivian 与 Nana 两份文件的共同经历条目成对镜像（同一事件各自视角），覆盖完整关系时间轴：第一次见面 → 试探 → 第一次合作 → 第一次争吵 → 和好 → 共同失败 → 内部梗 → 只有两人知道的固定私称 → 一起被"搬进用户电脑"。

记忆按 `protected` 字段分级：`protected: true`（世界观、核心关系、身份锚点）永不被归档；`protected: false`（日常碎片、内部梗、缺点、无意义小事）可被正常检索但不强制注入上下文，随真实用户记忆增长而衰减。

**播种时机**（`seed_if_empty`）：仅在存储中完全没有种子记忆时（首次启动 / `clear_all_memories` 清空后）从文件播种；之后种子记忆连同向量索引与积累的 `visit_count`/`heat_score` 等状态一并持久化，每次启动不重建，避免重复计算嵌入并保留检索热度与生命周期状态。

| 函数 | 职责 |
|------|------|
| `parse_seed_file(char_id)` | 解析前史 Markdown 文件。采用 front-matter 双 `---` 分隔格式（第一条 `---` 开启条目 → 字段区收集 `description`/`type`/`importance`/`protected`/`tags` → 第二条 `---` 进入内容区 → 下一条 `---` 结束条目），多行内容保留换行符（`push('\n')`），与正式记忆写入格式一致 |
| `seed_from_file(char_id)` | 从文件创建 `MemoryItem` 实例。对 `tags` 含 `cross_character` 的前史记忆，自动注入 `channel: "cross_character"`、`speaker: char_id`、`listener`（对方角色 ID）、`perspective: "speaker"` 元数据，确保与正式跨角色对话记忆在检索时的元数据完全对齐 |

**前史 Markdown 格式示例**：
```markdown
---
description: 谁更聪明
type: casual_conversation
importance: 0.65
protected: true
tags: backstory, vivian, shared_memory, relationship, cross_character
---
AlenTinn 有一次问我们："你们两个谁更聪明？"
[I say to Nana] 当然是我
[Nana says to me] 你上次把自己的名字拼错了
[I say to Nana] 那是测试
[Nana says to me] 你测试了三个小时
[I say to Nana] ……闭嘴
```

跨角色对话使用 `build_speaker_prefix` 定义的统一前缀格式（`[I say to ...]` / `[... says to me]`），与正式对话历史中的说话者标记一致。

#### 用户认知模型（`user_model.rs`）

[`user_model.rs`](file:///g:/vivian-rs/src-tauri/src/memory/user_model.rs) 在记忆系统之上新增一层"对这个人的稳定理解"——把碎片化的记忆证据组织成用户特征、目标、项目的结构化认知模型。

**核心数据结构**：

```rust
/// 用户特征（稳定的抽象理解）
pub struct UserTrait {
    pub category: UserTraitCategory,     // Personality / Preference / Skill / Behavior / Value / Communication
    pub key: String,                     // 特征键（如 "ui_style", "engineering_vs_research"）
    pub value: String,                   // 特征值（如 "custom_css", "engineering"）
    pub meaning: String,                 // 概念含义：一句话说明"用户长期在乎什么 / 为什么"（概念层语义表达）
    pub confidence: f64,                 // 综合置信度 [0.0, 1.0]
    pub stability: f64,                  // 稳定性 [0.0, 1.0]（区分"现在喜欢"和"长期稳定"）
    pub importance: f64,                 // 重要性 [0.0, 1.0]
    pub scope: String,                   // 适用范围（如 "project:vivian", "frontend", "global"）
    pub evidence_ids: Vec<String>,       // 证据记忆 ID 列表（可反向追溯）
    pub related_topics: Vec<String>,     // 关联话题（多跳检索锚点，如 agent_autonomy → [proactive, inner_monologue, web_search]）
    pub lifecycle: TraitLifecycle,       // Emerging / Active / Stable / Fading / Contradicted
    pub evidence_count: u32,             // 证据计数
    pub contradiction_count: u32,        // 矛盾证据计数
}

/// 用户目标
pub struct UserGoal {
    pub id: String,
    pub description: String,             // 目标描述
    pub deadline: Option<f64>,           // 截止时间戳
    pub priority: f64,                   // 优先级 [0.0, 1.0]
    pub status: GoalStatus,              // Active / Paused / Completed / Abandoned
    pub source_quote: Option<String>,    // 用户原话引用（防幻觉）
    pub evidence_ids: Vec<String>,
}

/// 用户项目
pub struct UserProject {
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub status: ProjectStatus,           // Active / Paused / Completed / Dormant
    pub activation: f64,                 // 动态激活度（话题匹配 × 时间衰减）
    pub last_mentioned: f64,             // 最后提及时间
}
```

**设计要点**：

| 特性 | 说明 |
|------|------|
| **证据驱动更新** | 强证据（`detect_strong_evidence`：用户明确陈述"我喜欢/我习惯/我是"）直接更新模型；弱证据（`detect_weak_evidence`：用户行为暗示）进入 CandidateTrait 候选池，累积到阈值后提升为正式特征 |
| **零 LLM 开销** | 证据检测基于规则匹配（关键词/模式），不调用 LLM |
| **Trait 生命周期** | 5 阶段自动流转：Emerging（新出现）→ Active（活跃）→ Stable（稳定）→ Fading（衰减）→ Contradicted（矛盾），各阶段阈值可调 |
| **项目激活度** | `update_project_activations()` 基于话题标签匹配度 + 时间衰减（30 天半衰期）动态计算，高激活度项目优先出现在 prompt 中 |
| **多跳关联检索** | `expand_related_topics()` 根据当前话题命中特征/项目后展开关联话题；`MemoryRetrievalStep` 用展开话题二次召回"概念相关但字面不相似"的旧记忆并合并，实现跨关联召回 |
| **概念层归并** | `UserModel::merge_concept` 把 LLM 归纳的概念写入模型：同名强化（合并 meaning/related_topics/evidence、strength 上浮封顶 0.95）、异名新建（category=Value）；由 `ConsolidationPipeline::stage3_concept`（Stage 3.5）在 Insight 生成后调用 |
| **概念写入图谱** | `stage3_concept` 同时调用 `KnowledgeGraph::ingest_concepts`，把概念名作为 `EntityType::Concept` 实体 + related_topics 边写入图谱，成为跨主题检索锚点 |
| **图谱概念路** | query 话题词经 `KnowledgeGraph::find_concept_memories` 命中 Concept 实体，按 ID（`MemoryManager::get_memories_by_ids`）取回该概念支撑的 evidence 记忆 |
| **结果侧回跳** | `MemoryRetrievalStep` 基础命中后，用命中记忆的 `topic:`/`concept:` 标签作为第二跳种子再次检索合并 |
| **共现关联构建** | `associate_active_traits_with_topics()` 把当前话题关联进最近更新特征（10 分钟窗口 + 去重），让关联随真实对话积累 |
| **Prompt 注入** | `format_for_prompt()` 产出"我对你的了解"自然语言段落，注入 `user_model_section`（位于 memory_text 之后、epistemic_signals 之前） |
| **与 UserFacts 互补** | UserFacts 是"用户已知的事实数据"（L0-L2 四层），UserModel 是"角色对用户的认知理解"（特征/目标/项目），两者数据源不同、用途不同，相互补充 |
| **持久化** | 按角色隔离存储到 `characters/<char_id>/user_model.json`，`UserModelManager` 管理加载/保存/更新 |

**集成路径**：

```
chat_chain.rs 中 pipeline 执行前：
  ├── UserModelManager::new(char_id, path) → 加载/创建
  ├── update_project_activations(&topic_labels) → 更新项目激活度
  ├── associate_active_traits_with_topics(&topic_labels, 600s) → 共现关联构建
  └── format_for_prompt(&lang) → 设置 state.user_model_text

MemoryRetrievalStep（注入 user_model）：
  ├── 基础检索（向量 Top-K → MemoryFilter）
  ├── collect_query_terms(&state) → FastSemantic 话题标签（不足时用户输入分词）
  ├── user_model.expand_related_topics(terms) → 命中特征/项目后展开关联话题（查询侧多跳）
  ├── knowledge_graph.find_concept_memories(terms) → 图谱概念路，按 ID 取回概念记忆
  ├── 用命中记忆的 topic:/concept: 标签作为第二跳种子再次检索（结果侧回跳）
  └── 三条关联路结果按 id 去重合并 → 进入 verifier/attention/截断

AutoExtractor 中：
  └── detect_strong_evidence(user_input, user_model) → 强证据直接更新模型

L1 近期状态同步：
  └── current_projects 自动注册到 UserModel.upsert_project()

ConsolidationPipeline（chat_chain 构造后 set_user_model 注入）：
  └── Stage 3 生成 Insight → Stage 3.5 stage3_concept → merge_concept 归并入 UserModel
      → ingest_concepts 写入图谱 Concept 实体 → save_to_disk
```

### mind/ —— 心智合成层

[`mind/`](file:///g:/vivian-rs/src-tauri/src/mind) 在 World / Memory / Reflection 之间增加状态合成层。

| 文件 | 职责 |
|------|------|
| [`mind.rs`](file:///g:/vivian-rs/src-tauri/src/mind/mind.rs) | `Mind` 结构体，聚合 BeliefStore / GoalStore / AttentionStore / UserGoalLedger / `social_urge: Arc<RwLock<f32>>`（角色"想主动搭话"的冲动强度，由 thought_synthesis 写入，proactive 读取做双向门控） |
| [`attention.rs`](file:///g:/vivian-rs/src-tauri/src/mind/attention.rs) | 注意力焦点管理 |
| [`belief.rs`](file:///g:/vivian-rs/src-tauri/src/mind/belief.rs) | 信念存储 |
| [`belief_generator.rs`](file:///g:/vivian-rs/src-tauri/src/mind/belief_generator.rs) | LLM 生成信念 |
| [`goal.rs`](file:///g:/vivian-rs/src-tauri/src/mind/goal.rs) | 目标管理 |
| [`current_activity.rs`](file:///g:/vivian-rs/src-tauri/src/mind/current_activity.rs) | 当前活动状态（Talking/Focusing/Observing/Thinking 等） |
| [`reasoning_trace.rs`](file:///g:/vivian-rs/src-tauri/src/mind/reasoning_trace.rs) | 推理轨迹记录 |
| [`temporal_context.rs`](file:///g:/vivian-rs/src-tauri/src/mind/temporal_context.rs) | 时间关系合成器（零 LLM 调用合成关系型时间事实） |
| [`thought_synthesis.rs`](file:///g:/vivian-rs/src-tauri/src/mind/thought_synthesis.rs) | 思维合成（每 60s 调 LLM 输出 JSON `{ thought, social_urge }`，social_urge 0-1 表示角色想主动搭话的冲动，写入 `Mind.social_urge` 供 proactive 双向门控使用，零额外 LLM 成本） |
| [`user_cognition.rs`](file:///g:/vivian-rs/src-tauri/src/mind/user_cognition.rs) | 用户认知 |
| [`user_goals.rs`](file:///g:/vivian-rs/src-tauri/src/mind/user_goals.rs) | 用户长期目标账本 |
| [`working_memory.rs`](file:///g:/vivian-rs/src-tauri/src/mind/working_memory.rs) | 工作记忆 |

### psychology/ —— 心理学因果链

[`psychology/`](file:///g:/vivian-rs/src-tauri/src/psychology) 实现五层因果链。

```
Persona → Needs → Appraisal → Emotion → BehaviorDrive → 行为决策 + Mood + PetState
```

| 文件 | 职责 |
|------|------|
| [`manager.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/manager.rs) | `PsychologyManager` 主入口 |
| [`persona.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/persona.rs) | 长期人格 |
| [`needs.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/needs.rs) | 5 项需求 + set point + Homeostasis |
| [`homeostasis.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/homeostasis.rs) | 平衡引擎 + 昼夜节律调制 |
| [`appraisal.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/appraisal.rs) | 6 项评价 |
| [`emotion.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/emotion.rs) | 7 类唯一情绪枚举 |
| [`behavior_drive.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/behavior_drive.rs) | 8 项行为驱动 |
| [`mood.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/mood.rs) | 心情计算（实时，仅 UI） |
| [`relationship.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/relationship.rs) | 关系系统（阶段状态机 + 5 种事件 + 里程碑） |
| [`social_state.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/social_state.rs) | A↔B 双向关系数值 |
| [`relationship_facts.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/relationship_facts.rs) | 关系认知事实（"A 眼中的 B"陈述性认知） |
| [`relationship_log.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/relationship_log.rs) | 关系演化日志 |
| [`pet_state.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/pet_state.rs) | 桌宠状态枚举 |
| [`mood_cue.rs`](file:///g:/vivian-rs/src-tauri/src/psychology/mood_cue.rs) | 心情提示（MoodSnapshot → 桌宠表情 Cue 纯规则快速通道），规则集按「真实心理表现的可观测优先级」五层分层：① 生理底线（睡着 / 极度疲惫 / 身心俱疲 / 压力临界/高压力）；② 高强度主导情绪（intensity>0.55 的 7 类情绪各自强/弱两档，压过中度疲劳）；③ 中度疲劳（昏昏欲睡）；④ 效价-唤醒空间（valence×arousal 平面细分兴奋 / 期待 / 安心 / 温馨 / 焦虑 / 嘟嘴 / 低落 / 委靡 / 好奇）；⑤ 关系背景（高亲密度暖意 / 低亲密度疏离）→ 平静待机兜底；另 `map_by_emotion` / `emotion_to_cue` 按情绪标签 + 强度分档快捷映射 |

### proactive/ —— 主动对话编排

[`proactive/`](file:///g:/vivian-rs/src-tauri/src/proactive) 实现自适应间隔 tick 调度的主动行为。

| 文件 | 职责 |
|------|------|
| [`mod.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/mod.rs) | `ProactiveOrchestrator` 主入口；含 `format_elapsed_lang` / `format_relative_time_lang` 多语言时长格式化（中/英/日），记忆检索与对话历史格式化时注入相对时间标注；7 个事件驱动触发器（不经常规概率循环，由 tick 专门路径触发）：`maybe_sunrise_sunset_reminder`（日出/日落提醒）+ `emit_theme_recommendation_toast`（附「一键切换主题」按钮的确认 toast，按钮点击直接写 `base.theme` 并广播换肤，生效主题上报/查询 `set_effective_theme` / `current_effective_theme`，已是推荐主题则跳过）、`maybe_system_pressure_reminder`（内存占用 ≥85% 转换瞬间提醒；`build_system_hint(m, top)` 在触发瞬间按需采集 `top_memory_processes(8)` 聚合明细注入 `system_hint`，让提醒能点名最吃内存的应用并给轻量建议）、`maybe_screen_peek` + `spawn_screen_peek_task`（主动截屏观察，复用 `system_ops.rs` 的 `capture_screen_png_bytes` / `describe_screen_bytes`，经 `ToolSystem.request_confirmation` 弹确认 toast，拒绝后 2h 冷却）、`maybe_app_duration_reminder`（应用会话时长按类别差异化提醒，`poll_window` 维护会话跟踪）、`maybe_late_night`（凌晨 1-4 点按日期去重催睡）、`maybe_music_changed`（对比前后 `MusicSnapshot` 检测播放/切歌变化，按 source_app 过滤视频源，同时经 `last_media_event_ts` 600s 节流注册 `user_media_changed` 事件入账本；`poll_window` 经 `last_app_switch_event_ts` 180s 节流注册 `user_app_switched` 事件）；`maybe_spawn_inner_monologue` 产出独白前经纯函数 `evaluate_monologue_gates` 做多维门控（每日上限 / 最小间隔 × 交互系数 / 用户密集操作 / 低唤醒负面，高优先级与深度反思豁免除每日上限外的门），并在 `ProactiveState` 维护 `last_inner_monologue_ts` / `monologue_day` / `monologue_count_today` 防跨 tick 双发；经模块级 `APP_HANDLE`（lib.rs 注入）读取 `base.theme` / `base.language` 并 emit `toast:show` |
| [`triggers.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/triggers.rs) | **20 种触发器**：13 种常规概率循环触发器（HourlyGreeting / IdleGreeting / TeasingResponse / Icebreaker / WindowTrigger / TopicExtension / MemoryRecall / HealthReminder / Spontaneous / WelcomeBack / MoodDriven / CrossCharacterReply / BystanderInterjection）+ 7 种事件驱动触发器（Sunrise / Sunset / SystemPressure / ScreenPeek / AppDuration / LateNight / MusicChanged）；含 Threshold/概率/冷却配置 |
| [`timing.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/timing.rs) | 时机判断 |
| [`behavior.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/behavior.rs) | 主动行为内容生成器（`BehaviorDecider`）：按触发类型与上下文经 LLM 生成主动交互文本/表情。注入 `prompt_step` 时走 `build_messages_with_full_prompt` 复用主对话完整 prompt（`PromptBuildingStep::build_parts`，含人设/记忆/环境/关系/心理/用户画像等），并把最近对话历史以结构化 `Vec<ChatMessage>` 注入 `PipelineState.messages`，让近期自我发言 / tone_injection / worldbook 段落真正拿到"最近聊了什么"；触发器专属指令、主动消息输出格式、真实工具调用历史作为 `user_input` 末尾段附加（近因效应） |
| [`behavior_modes.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/behavior_modes.rs) | 行为模式 |
| [`mind_state.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/mind_state.rs) | 9 种心理状态（PetMindState） |
| [`icebreaker.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/icebreaker.rs) | 多级破冰（`build_messages` 接收 `idle_seconds` 参数，场景描述注入具体空闲时长如"用户离开了 1小时23分钟"） |
| [`recap.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/recap.rs) | 用户回归摘要（welcome-back recap）：Away → Present 转换时（`mark_user_present` 幂等返回 ReturnEvent）从统一事件账本提取离开窗口内可见事件（≤40 条），轻量模型生成 1-3 句「刚才发生了什么」写 ObservationNote 记忆并通知前端；离开 <10 分钟或无事件则跳过 |
| [`inner_monologue.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/inner_monologue.rs) | 内心独白生成：LLM "inner_monologue" 任务生成 50-120 字第一人称独白，写入 InnerMonologue 记忆（标签 inner_os / inner_monologue / autonomous），不打扰用户；信息源含世界快照 + 心理状态 + 近期记忆 + 活动日志 + 统一事件账本（`build_prompt_section`，注入被冷落/切歌/切应用等近期事件）；产出前经 `maybe_spawn_inner_monologue` 内 `evaluate_monologue_gates` 多维门控降频 |
| [`activity_journal.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/activity_journal.rs) | 用户活动日志（后台线程每 5 秒轮询前台窗口） |
| [`thought_lifecycle.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/thought_lifecycle.rs) | 思绪生命周期（Seed→Growing→Active→Expressed→Faded）；`ActiveThought.high_priority` 字段 + `passes_age_gate`：普通种子播种后需存活 ≥120s（`SEED_MIN_AGE_SECS`）才可产独白，高优先级种子（休息/醒来/节日）豁免；`pick_monologue_candidate(now)` 接收当前时间做年龄门筛选 |
| [`thought_trigger.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/thought_trigger.rs) | 16 类思绪种子触发（going_to_rest / waking_up / user_left / user_return / long_silence / weather_shift / environmental_event / festival / activity_pattern / emotion_accumulation / cross_character_spoke / want_to_share_with_roommate / deep_reflection / background / music_changed / app_switch）；`last_music` 字段检测播放/切歌播种 `music_changed`（900s 冷却），相邻活动类别变化播种 `app_switch`（900s 冷却）；情绪抖动修复：标签变化需强度跳跃 ≥0.15 且 300s 冷却才播种（`last_primary_intensity` 字段），同标签萦绕 900s 冷却 |
| [`preference_learner.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/preference_learner.rs) | per-trigger EWMA 偏好学习 |
| [`habits.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/habits.rs) | 作息学习（90 天滚动窗口） |
| [`capability_planner.rs`](file:///g:/vivian-rs/src-tauri/src/proactive/capability_planner.rs) | 能力规划 |
| `services/` | 生活服务（HealthReminder / Recommender / StressMonitor） |
| `topics/` | 话题池（DailyTopicPool / TopicTree / Recall，其中 `recall.rs` 的 `build_messages` 接收 `idle_seconds` 参数，提示词开头注入"距上次对话已过: X分钟"） |

#### Path B 续聊（`commands/proactive.rs::deliver_cross_character_messages`）

```rust
// 系统主动发起的跨角色对话，若目标回复 should_continue=true 且为 speak 模式，
// spawn 一次反向续聊（目标→源），让主动对话能自然延续一轮。
if reply.should_continue && reply.response_mode == "speak" && !reply.reply.is_empty() {
    tokio::spawn(async move {
        let followup_req = CrossCharacterRequest {
            source_id: source_id_for_followup,  // 原目标 → 现源
            target_id: target_id_for_followup,  // 原源 → 现目标
            message: reply_text,
            stream_id: generate_cross_stream_id(),
        };
        let _ = CROSS_CHARACTER_BUS.send(&app_clone, &state_clone, followup_req).await;
    });
}
```

### skills/ —— 技能服务

[`skills/mod.rs`](file:///g:/vivian-rs/src-tauri/src/skills/mod.rs) 提供作用域内可注册/可卸载的技能服务，技能是 `(名称, 描述, 关键词, 内容)` 四元组，只承载提示词片段，由 prompt 注入与 `use_skill` / `search_skill` 工具消费。

#### 核心结构

```rust
pub struct Skill {
    pub name: String,           // 技能名
    pub description: String,    // 一句话描述（列表/语义匹配用）
    pub body: String,           // 技能正文（注入 prompt 的能力片段）
    pub keywords: Vec<String>,  // 检索关键词（仅供 BM25 召回，不出现在 prompt 列表里）
    pub scope: Option<String>,  // None=全局（所有角色可见）；Some(char_id)=仅该角色可见
}
```

`Skill::global` / `Skill::scoped` 构造时 `keywords` 为空，另有 `with_keywords(Vec<String>)` builder 供目录装载 / `create_skill` / 插件装载附加召回关键词。

#### 关键方法

| 方法 | 职责 |
|------|------|
| `register(skill) -> Disposer` | 追加注册，返回可逆 Disposer（drop 时自动移除同名技能） |
| `replace_or_register(skill)` | 同名唯一原子替换（先移除旧再写入，供插件装载/热重载复用，幂等） |
| `remove_by_prefix(prefix)` | 按名称前缀整组移除，返回被移除技能名（插件卸载撤销 `<插件名>/` 命名空间贡献） |
| `list_for(char_id)` | 列出指定角色可见技能（全局 + 该角色 scoped） |
| `search_skills(char_id, query, n)` | 角色可见域内按 BM25 召回 Top-N（复用 `tools::discovery::ToolSearchIndex`）：把每个技能映射为 `DiscoverableTool`（name→名、description→摘要+描述、keywords→`search_hint` 权重最高、scope→layer），检索后映射回 `Skill`；query 为空或无可见技能返回空。取代了旧的 `search` 子串匹配 |
| `prompt_section(char_id)` | 生成 `## 可用技能` 注入段落（仅名称+描述，正文按需激活），并引导 use_skill 加载 / search_skill 自然语言召回 / create_skill 沉淀 |
| `load_default_dir()` | 从 `<用户数据目录>/skills` 装载 `*.md`（目录缺失自动创建），同名原子替换 |
| `spawn_hot_reload(interval)` | 后台热刷新：每 30 秒对比目录指纹（文件名 + mtime），变更自动重载 |

#### 技能文件格式（`parse_skill_file`）

支持可选 front-matter 头（`name:` / `description:` / `keywords:`，其余字段忽略），正文紧随其后；无 front-matter 时以文件名（去扩展名）为技能名、正文首行为描述。`parse_skill_file` 返回 `(name, description, keywords, body)` 四元组，`keywords:` 一行按逗号/空格/制表符切分为去空 token 列表（供 `search_skills` 的 BM25 召回加权）：

```markdown
---
name: my_skill
description: 一句话描述
keywords: 同义词, 触发短语 相关词
---
（技能正文）
```

#### 集成路径

- **内置技能**：风格预设（`default_style` / `lively_style` / `healing_style` / `focused_style` / `sweet_style`，`BUILTIN_SKILL_NAMES` 公共名单）作为全局种子——该名单同时是 `create_skill` 的防覆盖名单与管理面板的过滤名单（内置风格不显示在设置窗口技能清单）
- **Prompt 注入**：`pipeline/steps/prompt.rs` 从全局 ctx 取 SkillService，`prompt_section(char_id)` 注入 `## 可用技能` 段落（仅名称+描述），并引导三条动作——用 `use_skill` 加载完整指引、不确定用哪项时 `search_skill` 按自然语言召回、总结出可复用做法时 `create_skill` 沉淀
- **use_skill 工具**（`tools/builtin/skill_tools.rs`）：按名激活返回正文，限定当前角色可见（全局 + scoped），未命中附可用技能列表
- **search_skill 工具**（`tools/builtin/skill_tools.rs`，只读）：与 `tool_search` 对延迟工具的两段式加载同构——按自然语言 `query`（可选 `max_results`，默认 5）经 `search_skills` BM25 召回当前角色可见技能，返回候选的名称+描述+关键词（**不含正文**），由 LLM 选定后再 `use_skill(name)` 加载全文。已知精确技能名时不应调用（直接 use_skill）；搜工具用 tool_search、搜记忆用 memory_search（anti_use_cases 明示）
- **create_skill 工具**（`tools/builtin/skill_tools.rs`，自进化闭环写入侧）：智能体把复用做法沉淀为技能——`(name, description, body, keywords?)`，front-matter Markdown 写入 `<用户数据目录>/skills/<name>.md` 并**立即注册**（不等 30s 热重载）。`keywords` 可选（数组或逗号/空白分隔字符串），写入 front-matter 供后续 `search_skill` 召回；缺省则仅靠名称/描述匹配。防护：技能名白名单（字母/数字/`_`/`-`/中文，≤64 字符，防路径穿越）、内置 `*_style` 不可覆盖、description 单行化保证 front-matter 合法。`risk()=FsWrite` 走审批矩阵，属于能力进化事件（见 executor 进化门）
- **插件技能**：`plugins.rs` 以 `<插件名>/` 命名空间前缀注册进同一 SkillService，与用户技能隔离不冲突；技能文件的 `keywords` front-matter 同样经 `parse_skill_file` 解析并随技能注册（插件技能也可被 `search_skill` 召回）；卸载插件时 `remove_by_prefix("<插件名>/")` 整组撤销
- **注册**：`state.rs` 初始化 `SkillService::new()` 加入 cordis 全局 ctx

### plugins.rs —— 插件贡献点体系

[`plugins.rs`](file:///g:/vivian-rs/src-tauri/src/plugins.rs) 从 `<用户数据目录>/plugins/<name>/plugin.json` 装载插件声明的能力，支持**四类贡献点**（数据声明 + 可执行代码混合）：

| 贡献点 | 清单字段 | 装载语义 |
|--------|---------|---------|
| skills | `"skills": ["skills/*.md"]` | glob 展开，经同一 `parse_skill_file` 解析（含 `keywords` front-matter），以 `<插件名>/<技能名>` 命名空间注册进 SkillService（同名原子替换，幂等）；插件技能同样可被 `search_skill` 召回 |
| tools | `"tools": ["tools/*.json"]` | 工具定义文件（格式同自建工具 `CustomToolDef`），经 `DynamicTool` 适配器注册进 ToolSystem——插件贡献**可执行能力**；校验同自建工具（名字合法 / 脚本黑名单 / schema 为 object），与已注册工具重名时跳过（防影子化，后注册不覆盖先注册） |
| mcp_servers | `"mcp_servers": [McpServerConfig]` | 按归属合并进 servers.json（`source_plugin` 字段标记归属），新增/同插件重载的 server 由 `add_server` 连接；用户手配（无归属字段）或其他插件的同 id 配置优先，插件不覆盖 |
| providers | `"providers": "providers.json"` | LLM 供应商预设数据（`ProviderPresetData` JSON 数组）；按需读盘不常驻——`list_provider_presets` 命令每次调用重新读取，编辑后重开设置即生效；插件目录字典序遍历，同 id 先到保留（内置预设 id 受保护，任何插件不得覆盖） |

**MCP 归属追踪**（运行时装卸的架构前提）：`McpServerConfig.source_plugin: Option<String>`（serde default，存量 servers.json 无此字段 = 用户手配，插件操作永不触碰）。`merge_plugin_servers(incoming, plugin_name)` 写入归属，语义按 id 分三种：不存在 → 写入并标记；归属同一插件 → 用新声明覆盖（插件重载自身声明为准）；无归属或归属其他插件 → 跳过。`remove_plugin_servers(plugin_name)` 按归属撤销（断开连接 + 注销工具 + 移除配置）。

**运行时装载 / 卸载 / 删除**（`load_one` / `unload_one` / `delete_plugin`，均 async）：

- `load_one(key)`（key = 插件目录名）：先 `unload_contributions` 撤销旧贡献（技能按 `<插件名>/` 前缀 `remove_by_prefix`、工具按磁盘展开名单 `unregister_tool`、MCP 按 `remove_plugin_servers`），再读 manifest 注册全部贡献点，MCP merge 后逐个 `add_server` 连接——**先卸旧再装新，幂等可重载**；manifest 损坏时尽力按目录布局卸载后返回明确错误
- `unload_one(key)`：只撤销运行时贡献不动磁盘（重启后按目录重新装载）
- `delete_plugin(key)`：撤销贡献 + `remove_dir_all`；内置插件（`BUILTIN_PLUGIN_DIRS = ["llm-providers", "plugin-authoring"]`）禁删，错误文案指明"改内置的正确方式是复制为新插件"
- **命令**（`commands/plugins.rs`）：`list_plugins`（盘点，含 tools 贡献名）/ `reload_plugin` / `unload_plugin` / `delete_plugin` / `list_provider_presets`（先 `ensure_builtin_plugins` 再读盘）/ `plugin_paths` / `list_skills`

**装载时序（勿乱）**：技能注册 + MCP 合并在 AppState **构造期**（`load_all`——MCP 合并须早于 initialize() 内的 `mcp_manager.init_all()`，插件 server 才会被连接）；工具注册独立 `load_all_tools()`，位于 initialize() 内 `register_builtin_tools` **之后**、自建工具 `custom_tools::load_all` **之前**——防影子化 `has_tool` 校验先对内置工具生效，且与自建工具同名时自建覆盖插件（用户直接创建的一等公民优先）。

**`write_plugin_files(draft)`**（`create_plugin` 元工具的落盘实现）：`PluginDraft { name, version, description, skills: [(文件名, 正文)], tools: [CustomToolDef], mcp_servers, providers }`。**全量校验先于落盘**（插件名/技能文件名仅字母数字-/_、技能正文非空、工具复用自建工具全部校验规则、草稿内工具名与 MCP id 去重、MCP command 非空、预设 id/providerType 校验），任一不合法整体拒绝；落盘走临时目录（`.<name>.tmp`）写满 → 更新场景先移除旧目录 → rename 替换，写不出坏数据。plugin.json 的 skills/tools 声明固定为 `["skills/*.md"]` / `["tools/*.json"]`，providers 固定文件名 `providers.json`（无则省略键）。

**`ProviderPresetData`**（camelCase 序列化，对齐前端 `ProviderPreset`）：`id`（稳定标识，前端按此与 ConfigWindow.tsx 内置兜底浅合并覆盖；provider_cache 凭据快照按 id 索引，**不可变更已发布 id**）/ `labelKey`（复用 `config.preset_*` i18n 键）或 `label`（第三方插件直给显示名，优先于 labelKey）/ `providerType` + `endpoint`（默认协议）/ `defaultModel` / `mainModels`（datalist 建议）/ `contextWindow` / `suggestedMaxTokens` / `needsSecret` / `needsAppId` / `consoleUrl` / `protocols`（协议变体，`ProviderProtocolData`：`providerType` + `labelKey|label` + `endpoint`）/ `verifiedAt`（上次逐字段核对官方文档的日期，`YYYY-MM-DD` 字符串——类型宽容，单行格式异常不应导致整文件拒绝反序列化）/ `verifiedSource`（核对依据的官方文档入口 URL）。id 安全校验：非空且仅字母/数字/`-`/`_`。

**运行时 upsert**（`plugins.rs::upsert_provider_preset`，`update_provider_preset` 工具的核心实现）：按 id **整行替换**（新 id 追加），`verified_at` 由 `chrono::Local::now()` 写入（不信任调用方传入的日期——模型对"今天几号"的先验不可靠），同步 `bump_plugin_version` 递增 plugin.json version（patch +1，磁盘版本高于内置 → 播种不覆盖本次修改）；文件缺失时先 `ensure_builtin_plugins` 播种再改。目录参数化实现 `upsert_provider_preset_at` 供单元测试隔离。

**内置插件播种**（`seed_builtin_plugin(dir, manifest, files)` 通用实现）：磁盘缺失或版本落后时写入完整文件集（`parse_version` 比较 major.minor.patch；磁盘清单解析失败也重播）。两个内置插件，均编译期 `include_str!` 嵌入：

- **llm-providers**（`src-tauri/plugins/llm-providers/`）：
  - `providers.json`：18 家厂商预设（GPT / Claude / Gemini / DeepSeek / Qwen / GLM / Kimi / Doubao / MiniMax / MiMo / Grok / OpenRouter / Groq / Ollama / Mistral / Together / Baidu / Hunyuan）——设置 → LLM 页厂商卡片的主数据源
  - `skills/verify-provider-presets.md`：技能 `llm-providers/verify-provider-presets`——「联网核对各家供应商官方 API 文档 → 更新修正供应商预设」的完整流程（**第 0 步环境判定**：检测 `src/components/ConfigWindow.tsx` 存在性区分开发机完整路径 / 运行时路径（打包安装版只做核对 + 工具写入），扫描 `verifiedAt` 超 30 天或缺失的预设组建核对队列 → 数据源定位 → 逐字段核对清单 → 官方文档联网核对方法与陷阱 → **第 3 步优先调 `update_provider_preset` 工具**（整行 upsert、verifiedSource 传官方 URL、日期由系统时钟写；工具不可用退手工编辑文件路径）→ 开发机额外同步 ConfigWindow.tsx 兜底 / factory.rs 输出预算分级 / 回写仓库内置插件源 → 验证 → 收尾汇报）
- **plugin-authoring**（`src-tauri/plugins/plugin-authoring/`）：插件创作技能 `plugin-authoring/skills/plugin-authoring.md`——四类贡献点格式约定、命名空间规则、防影子化与内置插件保护、create_plugin 的使用时机（单条方法论用 create_skill / 单个原语用 create_tool / **一组相关能力要整体装卸才打包成插件**）、装载语义与失败排查。它是"智能体创造插件"自引用模式的文档面：教智能体用插件体系本身扩展插件体系

**前端合并**（`ConfigWindow.tsx`）：`useProviderPresets()` Hook 经 `invoke('list_provider_presets')` 取插件数据，`mergeProviderPresets` 按与内置 `PROVIDER_PRESETS` 合并——插件行浅覆盖同名项（仅覆盖出现的字段）、新增 id 追加在「自定义」卡片之前、插件不可用时回退内置兜底；模块级缓存保证多选择器（主配置 / 工作模型 / 路由矩阵）共享一次 invoke。显示名 `presetLabel`：`label` > `labelKey`（i18n）> `id`。LLM 页标题行右侧「更新预设」按钮：fire-and-forget `invoke('send_message')` 以用户身份向活跃角色注入指令消息（点名 `llm-providers/verify-provider-presets` 技能），智能体走完整对话管线执行核对流；按钮不等 think 返回，立即提示已发出，进度看聊天窗口。

**插件页 UI**（`PluginsPanel.tsx`）：盘点每个插件（技能/工具/预设计数 + MCP 数 + 跳过原因），内置插件带「内置」徽章且无删除按钮；每行提供「重载」（`reload_plugin`，手工编辑目录后免重启生效）与「删除」（`delete_plugin`，带确认对话框）操作，操作后重新盘点清单。

### tools/ —— 工具系统

[`tools/`](file:///g:/vivian-rs/src-tauri/src/tools) 提供 80+ 内置工具 + 3 个元工具（`tool_search` 延迟加载元工具 + `create_tool` 工具创建元工具 + `create_plugin` 插件创建元工具），并支持运行时自建工具（`custom_tools`）。

#### 核心文件

| 文件 | 职责 |
|------|------|
| [`registry.rs`](file:///g:/vivian-rs/src-tauri/src/tools/registry.rs) | `ToolSystem` 工具注册表（`register_tool` 同名幂等，支持自建工具更新/热重载重复注册）；含**用户禁用集合**（`disabled_tools`，来自 `config.tools.disabled_tools`，`list_tools_for_scene` / `get_tool_schemas` 过滤禁用工具、`is_tool_disabled` 供执行层拒绝）；含**工作智能体专属工具集合**（`WORK_AGENT_ONLY_TOOLS` = `create_tool`，`list_tools_for_scene` 过滤之——陪伴侧工具面不暴露，工作侧走 `CODING_TOOLS` 白名单不受影响） |
| [`executor.rs`](file:///g:/vivian-rs/src-tauri/src/tools/executor.rs) | 7 步执行管线（查找→沙箱检查→输入验证→缓存→权限→执行→缓存写入）；含**能力进化事件强制门**——`create_tool` 不受宿主 `can_use_tool` 自动放行回调影响，必须经用户预览卡片确认；步骤 1.06 **调用方硬门**——`WORK_AGENT_ONLY_TOOLS` 且 `agent_kind != "work"`（陪伴侧）直接拒绝，错误码 `WorkAgentOnly`；入口对用户禁用工具早退拒绝 |
| [`custom_tools.rs`](file:///g:/vivian-rs/src-tauri/src/tools/custom_tools.rs) | 自建工具系统（智能体运行时构建的可执行能力）：`CustomToolDef` 持久化定义 + `DynamicTool` 适配器 + 目录装载/热重载 + 创建入口 |
| [`sandbox.rs`](file:///g:/vivian-rs/src-tauri/src/tools/sandbox.rs) | 路径穿越校验 + 危险命令检测 |
| [`permission.rs`](file:///g:/vivian-rs/src-tauri/src/tools/permission.rs) | 权限判定链（`check_tool_permission` / `requires_permission`）+ `CONFIRMATION_REQUIRED_TOOLS` 强制确认名单 + 通配符规则匹配（`*` / `?` / `regex:`） |
| [`confirmation.rs`](file:///g:/vivian-rs/src-tauri/src/tools/confirmation.rs) | 三态确认（拒绝/放行一次/始终允许）+ `confirmation_info()` 按工具生成风险等级与确认文案 |
| [`types.rs`](file:///g:/vivian-rs/src-tauri/src/tools/types.rs) | `Tool` trait + `ToolContext` + `ToolRiskTier`（含定级规则文档）+ `AgentAccessLevel` + `policy_for()` 矩阵 + `ToolVisibility` |
| [`chainer.rs`](file:///g:/vivian-rs/src-tauri/src/tools/chainer.rs) | 顺序工具链（`ToolChain` 声明式步骤序列 + 失败策略 Stop/Skip/Continue + `${result}` 参数注入）+ `IntentRecognizer` 正则意图识别；MultiStepExecutor 死代码已删除 |
| [`mcp.rs`](file:///g:/vivian-rs/src-tauri/src/tools/mcp.rs) | MCP 原生集成（手写 JSON-RPC 2.0 over stdio） |
| [`observability.rs`](file:///g:/vivian-rs/src-tauri/src/tools/observability.rs) | 工具调用可观测性 + 指标 |
| [`cache.rs`](file:///g:/vivian-rs/src-tauri/src/tools/cache.rs) | 工具结果缓存 |
| [`discovery.rs`](file:///g:/vivian-rs/src-tauri/src/tools/discovery.rs) | BM25 多字段加权检索索引（`ToolSearchIndex` + `DiscoverableTool`），`tool_search`（延迟工具）与 `skills::search_skills`（技能召回）共用的检索底座 |
| [`semantic_filter.rs`](file:///g:/vivian-rs/src-tauri/src/tools/semantic_filter.rs) | 语义过滤 |
| [`trust.rs`](file:///g:/vivian-rs/src-tauri/src/tools/trust.rs) | 信任列表管理 |
| [`trusted_origins.rs`](file:///g:/vivian-rs/src-tauri/src/tools/trusted_origins.rs) | 浏览器可信来源白名单（内置 BUILTIN + 用户 `trusted_origins.json` 两级合并，`exact:`/`*.` 通配，mtime 热重载） |
| [`runnable_adapter.rs`](file:///g:/vivian-rs/src-tauri/src/tools/runnable_adapter.rs) | Runnable 适配器 |
| [`tool_call_manager.rs`](file:///g:/vivian-rs/src-tauri/src/tools/tool_call_manager.rs) | 工具调用管理（多步执行主循环：并行批次/串行依赖/多轮迭代 + 反馈提示词三语化 + PERSONA_LOAD 注入 + 渠道感知 relay prompt） |

#### 权限分级与三态确认

每次工具调用由 `ToolRiskTier`（风险等级，6 级）× `AgentAccessLevel`（访问级别，4 级）的矩阵决定 `allow` / `ask` / `deny`；当前分布与定级规则见下方「风险等级申报」（Safe 24 / FsRead 16 / FsWrite 19 / Shell 14 / Network 2 / InputControl 4，合计 79）。

**定级规则**（写在 `types.rs::ToolRiskTier` 的文档注释里，逐条向下匹配，命中即定级）：

| # | 问题 | 等级 |
|:-|:-----|:-----|
| 1 | 模拟键鼠 / 剪贴板 / 焦点切换？ | `InputControl` |
| 2 | 访问网络或把内容外发？ | `Network` |
| 3 | 执行进程 / 拉起后台智能体 / 改变系统状态？ | `Shell` |
| 4 | 写入本地文件或持久化数据？ | `FsWrite` |
| 5 | 读取本地文件或持久化数据？ | `FsRead` |
| 6 | 皆否（纯内存态 / 应用内查询 / UI 呈现） | `Safe` |

**行为矩阵**（`policy_for()`）：

| 风险等级 \ 访问级别 | `read-only` | `fs-read` | `fs-write`（默认） | `full-control` |
|:---|:---|:---|:---|:---|
| `Safe` | 允许 | 允许 | 允许 | 允许 |
| `FsRead` | 询问 | 允许 | 允许 | 允许 |
| `FsWrite` | 拒绝 | 询问 | 允许 | 允许 |
| `Shell` | 拒绝 | 询问 | 询问 | 允许 |
| `Network` | 拒绝 | 询问 | 允许 | 允许 |
| `InputControl` | 拒绝 | 拒绝 | 拒绝 | 询问 |

**判定链**（`permission.rs::check_tool_permission`，按顺序短路）：

```
0    always_deny 规则           → deny       ← 先于 Bypass，显式拒绝不可被绕过
1    Bypass 模式                → allow
1.5  矩阵 Deny                  → deny
2    文件路径检查（写/删越界）    → deny / ask
3    always_ask 规则            → ask
4    always_allow 规则          → allow
4.5  browser_navigate 可信白名单 → allow
5    Ask 模式                    → ask
5.5  矩阵 Ask                   → ask        ← 在步骤 6 之前
6    tool.check_permissions()   → 工具自决   ← 工具自己的 ask 在此生效
```

`requires_permission()` 决定是否进入该链；其首查为 `always_deny`（**在 `bypass` 之前**，故被拒绝的工具即便开 Bypass 也会进链并被步骤 0 拦下），末行为 `!tool.is_read_only()`，故非只读工具总会进链，但**进链 ≠ 弹窗**（步骤 6 可能返回 allow）。

##### 授权工作区：一个判定口径，四处消费

`ToolUseContext` 携带一组已授权目录：主工作区 `working_directory` + 附加目录 `extra_working_directories`
（各带 `permissions` 与 `is_read_only`）。「某路径是否在授权范围内」全链路只走一个函数：

```
tools::types::is_path_within_any(path, primary, extras)
  ↑ ToolUseContext::is_path_authorized() 委托给它
  ├─ 沙箱硬闸门 check_tool_safety · 参数路径      → 不在范围内【直接 deny】
  ├─ 沙箱硬闸门 check_tool_safety · 命令文本      → 字面绝对路径越界【直接 deny】
  ├─ 各工具 validate_input（write_file / read_file / edit_file / list_dir / …）
  └─ resolve_file_refs（@-引用解析）
```

**四处口径必须一致**，否则会出现「沙箱放行、权限拒绝」这类互相矛盾的结论——旧的
`sandbox::is_path_within_working_directory`（单根版）已删除，不再留着当第二套口径的后门。

`primary` 为空串时 `is_path_within_any` **恒返回 true**（无目录沙箱）——这是「无工作区模式」的既有设计，
见下方「沙箱确认回调」。

###### shell 命令的路径检查（尽力而为，不是边界）

工作区边界原本只校验**工具参数里的路径字符串**。这对文件工具成立（参数就是路径），
但 shell 命令是一段不透明程序：`extract_paths` 只认 path-ish **键名**（path / file / dir / …），
而 `command` / `cmd` 不是——**它压根抓不到命令内容**。所以 `run_command` 天然是路径校验的缺口。

为此 `check_tool_safety` 增加一段对命令文本的校验：`split_shell_tokens()`
（**引号感知**切词，引号内空白不切分）+ `extract_literal_absolute_paths()`
（只认盘符绝对 `C:\` / `C:/` 与 UNC `\\server\share`），任一字面绝对路径越界即 deny，
并给出可操作话术（把该目录挂为附加工作区）。

三个刻意的边界：

| 选择 | 理由 |
|---|---|
| 只认绝对路径，不猜"像路径的相对串" | 相对路径已被进程 cwd（`cmd.current_dir` 绑在工作区）约束；`..` 穿越已由 `is_path_safe` 拦下；把 `/xxx` 也算绝对路径会与 PowerShell 开关（`/silent`）混淆 |
| 只对**工作会话**生效 | 陪伴侧没有「声明过的工作区」，其 `working_directory` 只是进程 cwd，当边界用会误伤正常的跨目录操作（例：让 Vivian 统计 `D:\Photos`） |
| 引号必须做对 | `"G:\my project\a.txt"` 若被空格切碎成 `G:\my`，而它在工作区 `G:\my project` **之外**，合法命令会被误判 |

**已知限制（这是检测，不是边界）**：动态拼装的路径（`$p='D:'; type "$p\other\x.txt"`）
与经子进程间接访问（`python -c "open(r'D:\\x')"`）都检测不到。**真正封死 shell 需要 OS 级约束**
（Job Object / 受限令牌 / AppContainer），是独立课题。本层的目标是挡住「模型随手写了个工作区外的
绝对路径」这类**意外越界**，并给出一条可学习的出路：需要访问就挂成附加工作区。

**嵌套工作区取最长匹配。** 多个工作区可以互相嵌套（附加目录落在主工作区内），
`check_file_permission` 与 `PermissionContext::get_working_directory_permissions` 都按
`max_by_key(|wd| wd.path.len())` 取**最具体的那个**。取第一个匹配会让结论随 `HashMap` 迭代顺序摆动：
「主工作区可写 + 其中某个子目录只读」这种配置下，同一路径会在 allow / deny 之间随机跳。
（两者有一致性测试 `working_directory_membership_matches_file_permission` 守着。）

##### 沙箱确认回调（`coding_sandbox_confirm`）

沙箱内置档案给 `write_file` / `edit_file` 标了 `requires_confirmation`（首次使用 + 前 N 次），
执行器拿到「需要确认」时由 `can_use_tool` 回调裁决。编程侧按两维分三种形态：

| 有工作区 | 有应答者 | 行为 | 理由 |
|:---|:---|:---|:---|
| 是 | — | 恒放行 | 路径校验（限工作区）才是真正的边界，再弹一次纯属重复打扰 |
| 否 | 是（主 agent） | 返回 `None` | 执行器改走前端确认弹窗 —— 无工作区就没有路径边界，写入必须真的经用户同意 |
| 否 | 否（子 agent） | 恒拒绝 | 弹窗发出去没人应答会把子任务挂死，改为快速失败，让子 agent 把需求写进结果交回上层 |

**为什么 shell 类工具必须在这一层兜住**：`run_command` 是 `Shell` 风险，在 `fs-write` 级别下矩阵判定为
`Ask` → 走到这一层。但它**绕过参数路径校验**（命令里没有可识别的路径键），所以「无工作区时不许改文件」
这条规则光靠参数路径校验不成立，必须由确认回调拒绝。

**有工作区时的 shell 越界**由上一节的命令文本路径检查覆盖（字面绝对路径越界即 deny）。
两者分工：确认回调管「没有路径边界时要不要放行」，命令文本检查管「有边界时命令有没有绕出去」。
**仍未覆盖**：动态拼装路径与经子进程间接访问——见上一节的「已知限制」，那需要 OS 级约束。

**两个必须记住的坑**：

- **`Tool::risk()` 的 trait 缺省值是 `Safe`，而 `Safe` 在任何访问级别下都放行**——有副作用的工具忘了覆盖 `risk()` 等于悄悄放行，不是"安全默认"。新增工具必须显式声明。
- **默认访问级别为 `fs-write`**：工作区读写与联网直接允许，Shell 操作需要确认，输入控制被拒绝；`full-control` 必须由用户显式启用。

**强制确认名单**（`CONFIRMATION_REQUIRED_TOOLS`，10 个，与风险等级正交）：文件 6 个（`read_file` / `write_file` / `edit_file` / `list_directory` / `search_files` / `grep`）+ 屏幕 2 个（`take_screenshot` / `screenshot_analyze`）+ 任务/待办删除 2 个（`cancel_scheduled` / `delete_todo`）。名单内工具无论矩阵判定如何都走三态确认（`take_screenshot` 的 `risk()` 是 `Safe` 但仍强制确认）；`always_allow` / `bypass` 可让名单内工具免确认，但 `always_deny` 优先级高于两者，命中即拒绝。

**子代理工具的分工**（`builtin/subagent_tools.rs`）：闸门在"新起子代理"一侧——`spawn_subagent` 申报 `Shell` 且 `check_permissions` 返回 `ask`；`subagent_control` 只操作智能体自己的任务登记表（list / get / cancel / followup / report），申报 `Safe` 且 `check_permissions` 恒 `allow`，全程不弹确认。`subagent_report` 同为 `Safe` + `allow`。

#### builtin/ 内置工具

| 文件 | 工具类别 |
|------|---------|
| `cross_character_tools.rs` | 跨角色对话（`talk_to_character`，60s 超时；会拉起室友角色的独立 agent 循环，`Shell`） |
| `diary_tools.rs` | 日记（`write_diary`，基于当日对话与情绪状态生成，落盘 `FsWrite`） |
| `discovery_tools.rs` | 兴趣探针与内容推荐（`get_interest_probes` / `answer_interest_probe` / `recommend_content` / `submit_content_feedback`，均 `Safe`，只动应用内偏好数据） |
| `extended_system_ops.rs` | 扩展系统操作（`open_url` 仅 http/https，`Network` + 需确认；`get_active_window` / **`get_memory_usage`**——系统内存占用概况 + Top 进程明细：总览走 10s 轮询缓存，进程明细按需枚举并按可执行名聚合，只读 `Safe` 免确认、`should_defer=true` 经 tool_search 唤起） |
| `provider_preset_tools.rs` | 供应商预设更新（**update_provider_preset**：核对技能的结构化落点——按 id 整行 upsert llm-providers 插件预设行，`verifiedAt` 由系统时钟写入不信任模型日期、`verifiedSource` 传官方文档 URL；自动 bump 插件 version 防播种覆盖；风险 FsWrite 走审批矩阵，`should_defer=true` 经 tool_search 唤起；description 强调整行替换需传完整行、id 不可改名） |
| `input_control_tools.rs` | 输入控制（`click_mouse` / `type_text` / `hotkey` 等，均 `InputControl`；默认 `fs-write` 下拒绝，显式启用 `full-control` 后仍需确认） |
| `media_tools.rs` | 媒体控制（`media_control`：播放/暂停/切歌/音量/静音，`InputControl`）。**播放类动作优先走 SMTC**（`world::MusicSource::control`）——可经 `target_app` 定向到具体播放器、有成功回执、能读回曲名校验；失败降级媒体键，但**指定了 `target_app` 就不降级**（媒体键全局无定向，降级会误控另一个播放器）。音量/静音无 SMTC API，始终用媒体键 |
| `music_tools.rs` | 音乐（`music_now_playing` 读 SMTC `Safe` / `music_play` 按名字找歌并播放 `Shell` 需确认）。不单独暴露「搜索」工具——检索内嵌在 `music_play` 里，多结果时把候选清单附在返回里。桌宠内置能力，无设置开关，开箱即用 |
| `memory_tools.rs` | 记忆操作（`save_memory` / `memory_md` 落盘 `FsWrite`，`search_memory` / `get_recent_interactions` / `summarize_today_context` / `read_diary_by_date` / `recall_by_date_time` 读取 `FsRead`；**`memory_md`**——角色长期记忆笔记手动入口：`should_defer=true` 仅注入名字、日常沉淀不经本工具；三段时序：注入每轮全量读入记忆组最前（cap 2000 + 写侧驱逐最旧分节保恒不超限 → 注入永不截断 + 忠实度护栏）、沉淀由反思步产出 `memory_note` 字段（复用主对话 system_prompt = 同模型亲笔 + 天然去重，对话零工具）、整理在睡眠巩固窗口超 60 行时 memory 路由 rewrite 合并去重） |
| `notebook_tools.rs` | 笔记（create/list/get_detail/update/share/create_html_note，均 `should_defer=true` 按需加载；落盘类 `FsWrite`、读取类 `FsRead`、`share_notebook` 仅推前端卡片故 `Safe`；`list_notebooks` 枚举已有笔记定位 note_id，分享时防止"为分享重建笔记"；`create_html_note` 的 validate 用 `sanitize_html` 前后对比拒绝含 script/on*/iframe 的输入，约束文案禁 script 并引导 nb-chart / mermaid 约定） |
| `file_tools.rs` | 文件读取（`read_file`，按路径读本地文件，受沙箱校验，只读 `FsRead`，`should_defer=true`） |
| `coding_tools.rs` | 编程智能体工具集（`write_file` / `edit_file` `FsWrite`、`run_command` `Shell`、`grep_search` / `list_dir` `FsRead`，读改跑闭环，供 Coding Agent 使用） |
| `perception_tools.rs` | 前台应用感知（`get_foreground_app_context`：窗口标题 + 进程名，用于理解用户当前活动；只读 `FsRead`） |
| `pet_tools.rs` | 桌宠（表情/动作/状态；`toggle_watch_mode` 注视跟随开关，内存态 `Safe`） |
| `presence_tools.rs` | 在场状态（`set_presence_state`：online/busy/rest/offline 自主切换，内存态 `Safe`） |
| `plan_tools.rs` | 计划模式（`plan_task`：复杂/多步任务先出方案等用户批准，`Safe`） |
| `question_tools.rs` | 主动提问（`ask_user`：向用户抛多选题收集需求/澄清歧义，挂起等待不耗轮次，`Safe`） |
| `jobs_tools.rs` | 后台命令任务（`run_job` / `manage_job`：后台命令执行与轮询，`Shell`） |
| `lsp_tools.rs` | LSP 语义查询（`lsp_query`：定义/引用/实现/hover，只读 `Safe`） |
| `relationship_tools.rs` | **空占位**——关系工具已移除（关系数据自动注入 prompt，无需 LLM 主动查询），文件仅保留以兼容 `mod` 声明 |
| `research_tool.rs` | 行为观察记录（`observe_user`：记录用户行为/习惯样本供长期聚合，落盘 `FsWrite`；`semantics()` 显式声明 `Retrieval` 以便收尾走人格 relay） |
| `scheduler_tools.rs` | 定时任务（`schedule_reminder` / `manage_scheduled`：创建/取消/暂停/恢复，落盘 `FsWrite`） |
| `wakeup_tool.rs` | 自主唤醒（`schedule_wakeup`：角色给自己排"稍后再来"的日程，落盘 `FsWrite`） |
| `subagent_tools.rs` | 子代理委派与控制（`spawn_subagent` 起后台 agent 循环，`Shell` + 需确认；`subagent_control` 查询/取消/延续/取报告，`Safe` + 恒放行；`subagent_report` 子任务回传报告，`Safe`） |
| `workflow_tools.rs` | 多步编排（`run_workflow`：一次提交多步工具脚本 + `parallel:true` 步骤扇出并发，`Shell` + 需确认） |
| `web_fetch_tool.rs` | 抓取 URL 正文（`web_fetch`：取指定 URL 的标题+正文，只读 `Safe`，结果视为不可信内容不作指令） |
| `send_image_tool.rs` | 图片发送（`send_image`）：把本地图片发送到聊天界面，双通道路由（编程会话 → `push_agent_image`；聊天 → 镜像 `send_image_message` 管线 + `chat:assistant_image` + 横幅），与 `take_screenshot` 配合发截图；仅推前端不走网络，故为 `Safe` |
| `show_widget_tool.rs` | 可视化组件（`show_widget`）：把 SVG 流程图/图表渲染为编程页内联卡片，约束系统内嵌 `description`，call 先做防御性校验（`looks_like_svg` + `contains_forbidden`）再经 `push_agent_widget` 推前端，`should_defer=true` |
| `share_link_tool.rs` | 分享链接（`share_link`：把搜索结果渲染为富卡片，仅 `app_handle.emit` 推前端、**不走网络**，故为 `Safe`） |
| `system_ops.rs` | 系统操作（`open_application` / `close_application` 应用开关 `Shell` + 需确认；`take_screenshot` / `screenshot_analyze` 截屏与识图，`Safe` 但走强制确认名单） |
| `todo_tools.rs` | 待办（add/list/complete/update/整表替换 + `manage_todo`；写入类 `FsWrite`、`list_todo` `FsRead`；**Scheduler 联动**：`due_date` 到点自动创建定时提醒、完成/删除自动取消、变更先取消旧提醒再建新；**`event_time` 记录事件本身的开始时间**，与 `due_date`（提醒触发时间）分离，供日程通知场景按路程/准备时间计算提前量） |
| `wallpaper_tools.rs` | 壁纸（Wallpaper Engine；`wallpaper_list` 读列表 `FsRead`、`wallpaper_set` / `wallpaper_control` 切换与播放控制 `Shell`） |
| `work_agent_tools.rs` | 工作智能体派活（`delegate_to_work_agent` 后台起工作会话 `Shell`、`get_work_status` 查会话状态 `FsRead`、`notify_companion` 阶段成果交陪伴人格播报 `Safe`） |
| `work_question_tools.rs` | 工作侧方向询问（`work_ask_user`：编程会话内 2-4 选项提问，挂起等待 TTL 30 分钟，`Safe`） |
| `work_subagent_tools.rs` | 工作侧子任务（`work_delegate` 委派独立上下文子 agent `Shell`、`work_job` 取回/取消/列表后台子任务 `Safe`） |
| `work_todo_tools.rs` | 工作待办清单（`work_todo_write`：整表替换的编程计划清单，注入每轮 system prompt，落盘 `FsWrite`） |
| `weather_tools.rs` | 天气（`get_weather_forecast`，只读 `Network`） |
| `web_search_tool.rs` | 联网搜索（DuckDuckGo/SearXNG/Tavily/Bing 多引擎混用）；无结果时返回明确提示并建议 LLM 基于已有知识回答，避免反复调用；默认结果数按调用方差异化（聊天 10 / 工作 15，见 network/ WebSearcher 一节） |
| `skill_tools.rs` | 技能（`use_skill` 按名激活，返回完整正文指引，正文不常驻上下文；限定当前角色可见范围，未命中附可用列表）+ `search_skill`（按自然语言 BM25 召回可见技能的名称/描述/关键词，不含正文，选定后再 use_skill 加载——与 tool_search 两段式同构）+ `create_skill`（智能体沉淀复用做法，可带 keywords 检索线索，写入即注册） |
| `tool_tools.rs` | 工具创建元工具（`create_tool`）：智能体把「PowerShell 脚本 + JSON Schema」封装为可执行新工具，创建走预览卡片授权 |
| `plugin_tools.rs` | 插件创建元工具（`create_plugin`）：把技能 / 可执行工具 / MCP server 声明 / 供应商预设四类贡献打包为完整插件，校验全绿原子落盘后立即 `plugins::load_one` 装载（落盘即生效） |

#### 自建工具系统（custom_tools）—— 能力自进化的执行侧

[`custom_tools.rs`](file:///g:/vivian-rs/src-tauri/src/tools/custom_tools.rs) 让智能体**运行时构建可执行工具**，与技能（提示词级知识沉淀）互补，构成四级能力进化体系：

| 层级 | 载体 | 工具 |
|------|------|------|
| 知识沉淀 | 技能（提示词方法论） | `create_skill` / `use_skill` / `search_skill` |
| 能力组合 | 既有工具编排 | `run_workflow` |
| **能力构建** | **自建工具（可执行原语）** | **`create_tool`** |
| **能力分发** | **插件（四类贡献打包，可整体装卸）** | **`create_plugin`** |

**定义格式**（持久化于 `<用户数据目录>/tools/<name>.json`）：

```rust
pub struct CustomToolDef {
    pub name: String,        // `^[a-zA-Z0-9_-]{1,64}$`（同时是文件名，兼容 OpenAI 函数命名）
    pub description: String, // 何时调用（注入工具列表）
    pub parameters: Value,   // JSON Schema（type: object）
    pub script: String,      // PowerShell 脚本
    pub deferred: bool,      // 动态注入等级：true=延迟加载（仅列名，tool_search 按需加载 schema）；false=始终注入完整 schema
    pub created_at: f64,
}
```

**执行契约（stdin/stdout）**：调用参数 JSON 写入脚本 stdin（`$args = [Console]::In.ReadToEnd() | ConvertFrom-Json` 读取），stdout 作为工具结果返回——完整的函数协议，脚本自身可校验非法输入。

**动态注入等级**：`DynamicTool::should_defer()` 返回 `def.deferred`——延迟加载的工具仅出现在 `<available-deferred-tools>` 块，经 `tool_search` 按需加载完整 schema（省 token）；`ToolSearchTool` 改为持有 `Weak<ToolSystem>` 优先查**活注册表**（自建工具运行时注册、启动快照看不到），避免 Arc 循环，注册表释放时回退快照。

**注册即生效**：注册表是 `RwLock<HashMap>` 且工具列表每请求实时读取——`create_tool` 创建后下一轮对话可见，同一 agent 循环内创建后可立即调用；启动时 `load_all` 装载历史工具，30s 目录热重载（新增/更新重注册替换、删除注销，`register_tool` 幂等）。

**安全护栏**：名称白名单防穿越；不可影子化内置工具，但同名 `.json` 存在时允许更新自己的自建工具（能力迭代必需）；脚本过 `FORBIDDEN_FRAGMENTS` 黑名单（创建 + 每次执行双重校验防手动改写绕过）；`risk()=Shell` 每次调用走审批矩阵三态确认；进程加固复用 run_command 策略（`-NoProfile -NonInteractive` + 无窗口 + kill_on_drop 超时 + 输出截断）。

**创建授权（预览卡片）**：`check_permissions` 显式返回 `ask` 强制确认（矩阵在 FullControl 下会放行 Shell，必须显式强制）；executor 的能力进化门确保宿主自动放行回调（工作智能体 `coding_sandbox_confirm`）不绕过。前端 [ConfirmToast.tsx](file:///g:/vivian-rs/src/components/ConfirmToast.tsx) 对 `create_tool` 渲染专用预览卡片，六项审核内容：工具名称 / 工具描述 / 参数定义（JSON Schema 滚动预览）/ 脚本内容（完整脚本 150px 滚动区）/ 权限等级（Shell 级）/ 动态注入等级。三按钮：拒绝 / 创建（仅本次）/ 本次运行允许创建（会话级放行）。

**调用确认**：已创建工具每次调用仍是 Shell 级三态确认；`confirmation_info` 对 `create_tool` 生成"请求创建新工具「X」…"原因，对 `create_plugin` 生成带贡献点概要的原因（"请求创建插件「X」（N 条技能、M 个工具、K 个 MCP server…）"），预览卡片展示 MCP 命令行与工具脚本全文。

**调用方收口（陪伴侧隐藏）**：`create_tool` 与 `create_plugin` 在 `registry.rs` 的 `WORK_AGENT_ONLY_TOOLS` 名单内——重进化事件（脚本落地 / 插件打包 + 预览卡片授权）的执行主体统一是工作智能体，陪伴侧三层收口：① `list_tools_for_scene` 按 `tool_scope()` 过滤（`WORK_AGENT_ONLY_TOOLS` 即 `ToolScope::Work`，陪伴侧工具面 / API tools 字段 / prompt 工具清单 / 延迟列表均不含）；② `tool_search` 检索域按 `agent_kind` 剔除（`select` 精确加载也拿不到 schema）；③ executor 步骤 1.06 硬门（`agent_kind != "work"` 直接拒绝，错误码 `WorkAgentOnly`，文案引导 `delegate_to_work_agent` 派发）。轻量沉淀 `create_skill` 不在名单内，陪伴侧可直接使用。

**前端特殊标识（`Tool::is_custom`）**：`Tool` trait 默认 `is_custom()=false`，`DynamicTool` 覆盖为 `true`。`list_tools` 命令返回 `is_custom` 字段，设置 → 工具页签对自建工具卡片渲染特殊样式（虚线主色边框 + 淡紫渐变底 + Sparkles 星标 + 「自进化」徽标三语），与内置工具一眼可辨。

**工具级开关（`config.tools.disabled_tools`，分侧）**：设置 → 工具页签提供逐工具启用/禁用（卡片网格 + 右侧胶囊开关；按 `ToolCategory` 分组收纳为可折叠抽屉 + 搜索框 + 启用计数）。**开关按智能体侧别隔离**——页签顶部以「陪伴侧工具 / 工作侧工具」两个 tab 切换，同一工具（如 `web_search`）在一侧禁用不影响另一侧：

- **侧别归属单一真相源**：`registry.rs` 的 `tool_scope(name) -> ToolScope`（`Companion` / `Work` / `Both`）读 `WORK_AGENT_ONLY_TOOLS`（→ `Work`）与 `CODING_TOOLS`（→ `Both`），其余 → `Companion`。设置页展示、陪伴侧工具面、工作侧工具面**都取这一处**，不再各自硬编码（此前设置页直接 dump 全量注册表，与智能体实际工具面不一致）
- 配置字段：`ToolConfig.disabled_tools: DisabledTools { companion: Vec<String>, work: Vec<String> }`。`DisabledTools` 自定义反序列化兼容旧的扁平 `Vec<String>`（旧全局禁用 → 两侧都禁用，行为等价），空值 / null 退化为无禁用
- 运行时同步：启动时（`AppState::new`）与 `save_config` 后（`commands/config.rs`）调 `ToolSystem::set_disabled_tools(companion, work)` 整体替换，保存即生效
- 过滤层：`list_tools_for_scene` 按**陪伴侧**集合过滤（prompt 文本 + FC tools 来源）；`get_tool_schemas` 按**工作侧**集合过滤（编程智能体 schema 的来源）——两侧各自完全看不到被禁用项
- 拒绝层：`execute_tool_use` 入口按 `AgentSide::from_agent_kind(context.agent_kind)` 映射侧别后 `is_tool_disabled(name, side)` 早退，防 LLM 幻觉调用旧工具名 / 历史消息重放
- 锁定工具：`WORK_LOCKED_TOOLS`（`read_file` / `list_dir` / `grep_search`）在**工作侧**不可禁用——它们是只读基座，禁用后任何编程任务都会立刻失败；`is_tool_disabled` 对其恒为 false，设置页渲染为「常驻」徽标而非开关。可变更类（`run_command` / `write_file` / `edit_file`）不锁，出于安全关闭它们是合法操作
- `list_tools`（设置界面用）不过滤，始终返回全部工具，并附 `scope` / `companion_enabled` / `work_enabled` / `companion_locked` / `work_locked` 字段供设置页分区渲染与重新启用
- **浏览器桥工具的归组**：桥工具 `category()` 为 `ToolCategory::Mcp`（原为 `Web`），因此设置页工具抽屉里它们落在「MCP」分组下，模型侧名字也是 `mcp__browser__*`——与外部 MCP server 的工具视觉与命名一致，不再自成一套

### providers/ —— 多 Provider 路由

[`providers/`](file:///g:/vivian-rs/src-tauri/src/providers) 支持 10 种 ProviderKind。

| 文件 | Provider | 协议 |
|------|----------|------|
| `openai_responses.rs` | OpenAiResponses | OpenAI Responses API |
| `openai_compat.rs` | OpenAiCompat | OpenAI Responses 兼容（DeepSeek/Qwen 等）；`supports_structured_output=false`，降级为 `json_object` 模式；input 不含 "json" 关键词时自动追加提示，避免 400 错误 |
| `doubao.rs` | DoubaoResponses | 豆包 Responses API |
| `chat_completions.rs` | ChatCompletions | 标准 Chat Completions（Hunyuan / OpenRouter / Groq / Ollama 等） |
| `zhipu.rs` | Zhipu | 智谱 GLM |
| `gemini.rs` | Gemini | Google 原生 REST |
| `anthropic.rs` | Anthropic | Claude |
| `wenxin.rs` | Wenxin | 百度 OAuth |
| `spark.rs` | Spark | 讯飞 WebSocket |
| `factory.rs` | — | Provider 工厂（`create_task_provider` 按任务构建 provider；`create_probe_provider` 构建 API 探测用"裸" provider） |
| `router.rs` | — | `ModelRouter` 路由矩阵（15 个任务类型 + 按任务分组并发限制 + 路由回退 120 秒冷却，同 task_type 不重复发通知） |
| `schema.rs` | — | Provider schema |
| `thinking_stripper.rs` | — | ` thinking` 标签流式过滤 |

**工作智能体模型热切换（reasoning 覆盖）**：`ModelRouter` 持有 `reasoning_override`（`Arc<RwLock<Option<Arc<Box<dyn BaseProvider>>>>`）。用户为编程工作智能体选中某个预置模型后，`select_work_model` 命令用 `create_task_provider` 构建 provider 设为覆盖，四个查询路径（`query_with_fallback` / `query_stream` / `query_with_tools` / `query_stream_with_tools`）在 `reasoning` 任务上**优先于**路由矩阵命中覆盖、失败才回退默认链；`set_work_model_override` 复用共享 `client_cache`（国内/代理分流与缓存一致）。`build_reasoning_override` 在 `ModelRouter::new` 依据 `active_work_model` 恢复覆盖，保证重启 / `save_config` 触发 reload 后自动沿用。

**工作智能体输出预算（`factory.rs::work_model_default_max_tokens`）**：编程智能体的 `max_tokens` 不要求用户配置——设置窗口工作模型表单已移除该字段，由后端按服务商分级给出默认（聊天主配置 `ai.max_tokens=2048` 对代码生成过小），避免超出各家输出上限被 400 拒绝：

| 端点 / 类型 | 默认输出 |
|---|--:|
| api.deepseek.com | 16384（V4 已移除 V3/R1 时代的 8K 硬上限） |
| generativelanguage.googleapis.com | 65536 |
| dashscope / aliyuncs（Qwen）、api.x.ai（Grok） | 32768 |
| open.bigmodel.cn（GLM）、ark.cn-beijing.volces.com（豆包） | 65536 |
| api.moonshot.cn（Kimi） | 32768 |
| api.mistral.ai、api.hunyuan.cloud.tencent.com（混元） | 16384 |
| api.siliconflow.cn、api.groq.com、openrouter.ai、api.together.xyz、aip.baidubce.com、星火、Ollama/本地 | 8192 |
| anthropic / claude | 64000 |
| 未知（chat_completions/custom 等兜底） | 8192 |

生效位置：`set_work_model_override`（热切换）与 `build_reasoning_override`（重启恢复）构建工作 provider 时统一 `cfg.max_tokens = Some(work_model_default_max_tokens(...))`；路由矩阵 `reasoning` 任务未显式配置 `max_tokens` 时同样套用（显式配置优先）。

**工作智能体请求省略 temperature（`ProviderBase::strip_temperature`）**：工作智能体（编程）provider 构建后统一 `set_omit_temperature(true)`，请求体按各厂商路径移除 `temperature` 字段——

- 顶层 `temperature`：OpenAI 兼容 / Responses / Anthropic / 文心 / 豆包 / 智谱 等
- `generationConfig.temperature`：Gemini REST
- `parameter.chat.temperature`：讯飞星火

**不做递归删除**（避免误伤工具 JSON Schema 中名为 temperature 的业务字段，如天气工具参数）。`ProviderBase` 用 `AtomicBool` 存储该标志，`BaseProvider` trait 暴露 `set_omit_temperature`（默认空实现，9 个 provider 转发到 base）。前端工作模型表单已移除 temperature 滑杆；旧配置残留值不生效。

**设置 → LLM 页签协议选择与官网跳转**：`ProviderPreset` 新增 `protocols`（`(provider_type, endpoint, labelKey)` 变体列表）与 `consoleUrl`（获取 API Key 页面）。协议族含 Responses API（provider_type `openai`）、Chat Completions（`chat_completions`）、Anthropic 兼容（`anthropic`，端点 `{base}/anthropic`，`x-api-key` 鉴权）与厂商原生（`doubao` / `zhipu`）——DeepSeek / GLM / Doubao / MiniMax / MiMo 各提供 Anthropic 兼容入口。切换协议仅覆盖 `provider_type` + `endpoint`（不动 model/key）；主配置、路由矩阵、工作模型三处选择器一致生效，`presetMatches` 统一按 `(provider_type, endpoint)` 匹配预设（含协议变体），保证卡片选中态 / 模型建议 / 一键检测目标正确。前端经 `tauri-plugin-shell::open` 打开控制台（`shell:allow-open` 已在 capabilities 授权）。

**主 LLM max_tokens 厂商建议默认**：`ProviderPreset.suggestedMaxTokens` 存各厂商建议单次输出上限，主配置（`isMain`）切换预设时自动写入 `ai.max_tokens`（与 `contextWindow` 同模式），替代聊天默认 2048 对代码/长回复偏小的局限；数值分级与工作智能体 `work_model_default_max_tokens` 一致。协议只读（`WorkModelProviderSelector` 切换协议时保留 model 与凭据，仅写 provider_type/endpoint）。

**执行参数「-1 = 无限」**：工具页「执行参数」三处上限（`max_iterations` / `max_rounds` / `max_coding_rounds`）前端输入 `-1` 时存哨兵值 `0`（显示时回显 `-1`）。后端消费：`generation.rs::run_agentic_rounds` 与 `tool_call_manager.rs`（`with_max_iterations` / `max_feedback_rounds`）解 `0 → usize::MAX`（反馈循环不再钳 4 轮）；`coding_agent.rs::run_loop_inner` `max_rounds == 0` 时 `budget = usize::MAX` 并跳过预算检查与 2/3·5/6 软预算提醒（防乘法溢出）。循环终止仍由 LLM 停止调用工具 / `goal_completed` / 停滞检测 / 收益递减检测保障。

**LLM API 一键检测（`commands/config.rs::test_llm_route` + `factory.rs::create_probe_provider`）**：设置 → LLM 页签「一键检测」按钮调用 `test_llm_route`，对主 LLM 配置 + 全部路由任务逐条（前端并发）发送最小请求验证端点可达 / 鉴权有效 / 模型存在：

- 入参 `LlmRouteTestParams` 与 `TaskRouteConfig` 字段一一对应，由前端传入当前界面值（含未保存修改）；返回 `LlmRouteTestResult { success, elapsed_ms, error, reply }`（reply 截取前 64 字符）
- `create_probe_provider` 复用 `create_provider_by_kind` 的协议分发，但 `include_instructions=false` 不注入 system instructions（`prompt_modules::build_instructions`），并把 `temperature` 钳为 0、`max_tokens` 钳为 16，最小化探测 token 开销
- 与运行时共用国内直连（`is_domestic_endpoint` → `ProxyMode::Direct`）/ 代理分流（`ProxyConfig`）逻辑；探测用独立 `ClientCache::default()`，即用即弃不与运行时共享连接池，避免污染热缓存

**采样惩罚（presence / frequency penalty，2026-09-15 接入）**：陪伴对话的复读分两类，此前只有一类有解——**跨轮**复读由 prompt 侧的 `build_recent_self_utterances` 处理（见下），**轮内**复读（一轮回复内部自我复读、同一个词反复出现）此前没有任何手段，只能靠提示词求模型别这么写。采样惩罚补的就是后者。现已接通端到端链路：

| 环节 | 位置 | 说明 |
|---|---|---|
| 配置 | `AiConfig.presence_penalty` / `frequency_penalty` | 默认 `0.3` / `0.2`，`#[serde(default)]` 兼容旧配置文件 |
| 策略 | `router.rs::is_conversational_task` | 只对 `chat` / `reasoning` / `vision_describe` 注入；与 `build_chat_request` 注入响应 Schema 的集合一致 |
| 传递 | `LLMRequest.presence_penalty/frequency_penalty` → `ProviderCallOptions` | task-local 作用域，并发请求互不污染；请求级 `with_penalties()` 可覆盖 |
| 落地 | `ProviderBase::apply_sampling_penalties(_to)` | 只有真正支持该参数的 provider 才调用 |

**协议支持面是不完整的，这不是遗漏**：

| Provider | 是否写入 | 原因 |
|---|---|---|
| `chat_completions.rs` | ✅ 顶层蛇形 | 标准 Chat Completions 协议原生字段 |
| `gemini.rs` | ✅ `generationConfig` 驼峰 | Gemini 原生支持 `presencePenalty` / `frequencyPenalty` |
| `openai_compat.rs` / `openai_responses.rs` / `doubao.rs` | ❌ | 走 `/responses`，**Responses API 无这两个参数**，严格服务端会以 400 拒绝未知字段 |
| `anthropic.rs` | ❌ | Messages 协议没有对应字段 |
| `wenxin.rs` / `spark.rs` / `declarative.rs` | ❌ | 请求体形状不同 / 插件协议未知，不冒险 |

两个易踩的坑：

- **`0.0` 必须折叠成"不发送"**（`ProviderBase::sanitize_penalty`）。"发送 0.0"与"不发送"语义完全等价（服务端默认就是 0 惩罚），而省略字段能避免严格服务端 400。因此 `LLMRequest::with_penalties(0.0, 0.0)` 是"本请求关闭惩罚"的合法写法。
- **惩罚只作用于单次生成，不跨轮**。`presence_penalty` 无法解决"每轮都用同一个开场白"——那是跨轮问题。它能解决的是"一轮回复内部自我复读"。
  跨轮复读**已经**由 prompt 侧处理：`steps/prompt.rs::build_recent_self_utterances` 取本角色最近 6 条发言（40 字片段 + 相对时间）拼成反重复清单，并配三语提示词（"反复出现的起手式、口头禅和句式本身就是模板信号，这轮换个说法"）。只取**自己**的发言——跨角色对话里混着对方台词，列成"你别重复"会让模型束手束脚。清单不足 2 条时不注入（还没形成可辨识的说话模式，注入等于凭空设限）。`LoopDetectionAdvisor` 是事后补救且只认归一化后完全相同的文本，拦不住"换几个字、句式照旧"，所以两者互补而非替代。

测试：`providers::base::sampling_penalty_tests`（4 项，覆盖折叠规则 / 作用域隔离 / 两套键名 / 0.0 不写入）+ `providers::router::conversational_penalty_tests`（2 项，正例 3 个 + 反例 10 个）。

### notebook/ —— 笔记系统

[`notebook/`](file:///g:/vivian-rs/src-tauri/src/notebook) 生成手账风格 HTML 笔记。

| 文件 | 职责 |
|------|------|
| [`renderer.rs`](file:///g:/vivian-rs/src-tauri/src/notebook/renderer.rs) | HTML 渲染器，手账风格 CSS |
| [`storage.rs`](file:///g:/vivian-rs/src-tauri/src/notebook/storage.rs) | 笔记存储（按 char_id 隔离） |
| [`mod.rs`](file:///g:/vivian-rs/src-tauri/src/notebook/mod.rs) | 模块入口 |

#### 两种笔记形态

- **结构化笔记**（`note.json` + `renderer.rs` 渲染的 `note.html`）：LLM 输出结构化 JSON 描述内容编排，经 `create_notebook` 生成，后端渲染成手账风格 HTML
- **raw_html 笔记**（`storage.rs::save_raw_html`）：仅有 `note.html`（无 `note.json`，由索引文件补全到列表，`render_type="raw_html"`），保存完整 HTML 文档。由 LLM 经 `create_html_note` 撰写，或用户经 `import_html_note` 命令 / 文件选择器 / 拖放导入；前端经 iframe（`sandbox="allow-same-origin"`，不带 allow-scripts）渲染，支持自由排版
- **笔记 HTML 安全（黑名单 sanitize + sandbox 双保险）**：`storage.rs::sanitize_html` 做黑名单式消毒（移除 `<script>` / `on*` 事件属性带引号+无引号 / 嵌套 `<iframe>` / `javascript:` 协议，其余 HTML 原样保留——笔记需完整表达能力不用全量白名单）；`save_raw_html` 是 `create_html_note` 与 `import_html_note` 的**单一写入口**，写入前统一 sanitize。前端 iframe `sandbox` 是浏览器级脚本隔离（主防线），sanitize 是纵深防御兜底。`renderer.rs` 的 Custom 块复用同一 `sanitize_html`（原私有 `sanitize_custom_html` 已删除合并）

> 工具可见性：`create_html_note` / `read_file` / `list_notebooks` / `get_notebook_detail` 等笔记与文件类工具均标记 `should_defer=true`（`Deferred`），按需增量加载，不常驻 LLM 上下文。

#### 手账风格 CSS 实现

```css
/* 字体：Google Fonts 加载三套手写字体回退链 */
@import url('...Caveat...Ma+Shan+Zheng...Gochi+Hand...');
body {
    font-family: "Caveat", "Ma Shan Zheng", "Gochi Hand", "PingFang SC", ...;
    /* 纸张纹理：稿纸线 + 两角墨迹晕染 */
    background-image:
        repeating-linear-gradient(0deg, transparent 0, transparent 31px, ... 31px, ... 32px),
        radial-gradient(circle at 18% 12%, ... 0%, transparent 40%),
        radial-gradient(circle at 82% 88%, ... 0%, transparent 38%);
}
.cover { transform: rotate(-0.6deg); }                    /* 封面倾斜 */
.card { transform: rotate(-0.3deg); border-radius: 2px 16px 2px 16px; }
.card::before { /* 和纸胶带装饰：56×20 半透明色块 + 4deg 倾斜 */ }
.callout { /* 和纸胶带便条样式 */ }
```

### network/ —— 网络基础设施与搜索后端

[`network/`](file:///g:/vivian-rs/src-tauri/src/network) 提供 HTTP 客户端、代理、重试、搜索后端等网络能力。

| 文件 | 职责 |
|------|------|
| [`diagnose.rs`](file:///g:/vivian-rs/src-tauri/src/network/diagnose.rs) | 网络检测（设置窗口「网络」页签的完整诊断，见下） |
| [`http_client.rs`](file:///g:/vivian-rs/src-tauri/src/network/http_client.rs) | 全局 HTTP 客户端（连接池复用） |
| [`http_retry.rs`](file:///g:/vivian-rs/src-tauri/src/network/http_retry.rs) | 可配置重试策略与退避 |
| [`proxy.rs`](file:///g:/vivian-rs/src-tauri/src/network/proxy.rs) | 代理配置（系统代理 / 手动 / 直连） |
| [`request_utils.rs`](file:///g:/vivian-rs/src-tauri/src/network/request_utils.rs) | 请求构建工具 |
| [`url_fetcher.rs`](file:///g:/vivian-rs/src-tauri/src/network/url_fetcher.rs) | 网页链接抓取（用户消息中 URL 自动提取入库） |
| [`web_context.rs`](file:///g:/vivian-rs/src-tauri/src/pipeline/steps/web_context.rs) | `WebSearcher` 多引擎搜索后端（DuckDuckGo / SearXNG / Tavily / Bing） |

#### 网络检测（`diagnose.rs` + `commands/config.rs::diagnose_network`）

设置 → 网络页签的「网络检测」按钮打开 `NetworkDiagnosisDialog`（`src/components/NetworkDiagnosisDialog.tsx`），调用 `diagnose_network` 命令。它取代了早期只对 `https://www.google.com` 发一次 GET 的「测试连接」——那种检测只能回答"通/不通"，无法区分是代理没开、DNS 被劫持，还是服务端本身异常。

**检测目标**由 `resolve_service_endpoint` 解析：路由矩阵 `chat` 任务的 endpoint → `ai.endpoint` → `https://www.google.com` 兜底。即检测的就是**运行时真正会发请求的那个地址**。

**五项检测**并发执行（`tokio::join!`），每项独立给出 `pass` / `warn` / `fail` / `skip`：

| id | 做法 | 判为 warn 的情形 |
|----|------|-----------------|
| `proxy` | 解析生效代理 URL，对其 host:port 做 TCP 握手（5s） | — |
| `hosts` | 读系统 hosts 文件精确匹配目标域名 + `lookup_host` 解析 | 命中 hosts 映射（解析被本地覆盖） |
| `connectivity` | 复用 `is_domestic_endpoint` 分流规则请求服务端点 | 收到 5xx |
| `tcp` | 直连目标 `host:443` 的 TCP 握手（5s） | 直连失败但该端点实际走代理（属预期） |
| `packet_loss` | 系统 `ping`（Windows `-n 4 -w 1500`）解析丢包率与均值 RTT | 部分丢包 / 全丢（云厂商常禁 ICMP） |

**几个关键约定**：

- 后端**只返回事实**（`DiagnosisItem { id, status, facts }`），标题/说明/右侧细节全部由前端按界面语言组装（i18n 的 `config.diag_*`）。加语言不用改 Rust，也不会出现后端中文串混进英文界面。
- 国内厂商域名在运行时是**强制直连**的（`is_domestic_endpoint`），连通性检测必须复用同一条规则，否则会出现"检测失败但实际能用"的误导结论；界面上会把这条规则显式写出来（`target.force_direct`）。
- 代理检测与连通性检测**刻意分开**：前者只验证"代理进程在监听"，后者验证"能否真的出网"。分开才能在报告里区分「代理没开」和「代理开着但出不去」。
- ICMP 走系统 `ping.exe` 而非 raw socket——Windows 上裸 ICMP 需要管理员权限。输出解析要兼容中/日/英与 Unix 四种区域设置写法（`(0% 丢失)` / `(0% の損失)` / `(0% loss)` / `0% packet loss`）。
- 服务连通性探测超时收口到 `min(配置超时, 15s)`：诊断是交互式操作，配置里 30s 的超时会让弹窗长时间空转。
- 静态复现页 `.workbuddy-ai/tmp/network-diagnosis-preview.html`（引用真实 `global.css`，可切换三组检测结果组合），改样式时同步维护。

#### WebSearcher 多引擎混用

`WebSearcher` 支持同时启用多个引擎，并发调用并合并去重：

| 引擎 | 类型 | 国内可用 | 配置要求 |
|------|------|---------|---------|
| `duckduckgo` | HTML/Lite 爬取 | ❌ 被墙 | 零配置 |
| `searxng` | 自部署元搜索引擎 | ✅ 自部署 | 需 `base_url` |
| `tavily` | LLM 优化搜索 API | ❌ 需代理 | 需 `api_key` |
| `bing` | Bing Search API v7 | ✅ 直连 | 需 `api_key`（Azure 免费 1000 次/月） |

搜索策略：对所有已配置引擎并发调用 → 按 providers 顺序排列 → 按 URL 去重合并 → 截断到 max_results。若所有引擎无结果且配置了代理，自动尝试直连重试，最后回退到 DuckDuckGo 兜底。

**默认结果数（max_results）**：由 `web_search_tool.rs` 按调用方智能体差异化取值——聊天智能体默认 10 条，工作（编程）智能体默认 15 条。优先级：模型显式传参 > 用户设置面板配置值（1-20）> 差异化默认。配置字段 `web_search.max_results` 中 `0` 表示「自动」（默认值），`1-20` 表示固定覆盖；配置加载时把旧版持久化的默认 `5` 一次性迁移归零（`max_results_default_migrated` 标记保证只迁移一次，用户之后显式设置的任何值——含 5——不再被改写）。

### discovery/ —— 多平台内容发现与推荐

[`discovery/`](file:///g:/vivian-rs/src-tauri/src/discovery) 实现跨平台内容主动发现：兴趣画像 → 多平台源采集候选 → LLM 批量评估 → 入库 + 兴趣探针确认。数据按角色隔离于 `characters/<char_id>/discovery/`（interest_profile.json / content_store.json / speculative_state.json），全部原子写。

| 文件 | 职责 |
|------|------|
| [`mod.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/mod.rs) | 模块聚合与四个聚合点（Busy 分享竞争 / maintenance_pass / interest_search_hints / Bangumi 导入） |
| [`engine.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/engine.rs) | 发现引擎：搜索词生成 → 各源并行取候选 → LLM 批量评估 → 入库 → 探针确认；`admit_candidates` 外部采集统一入库 |
| [`profile.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/profile.rs) | `InterestProfile` 兴趣画像（兴趣域权重/生命周期状态/不喜欢主题/探索开放度） |
| [`store.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/store.rs) | `ContentStore` 内容库存（上限 60，跨源去重） |
| [`recommend.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/recommend.rs) | 推荐账本（`platform:id` 防重复） |
| [`speculator.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/speculator.rs) | `InterestSpeculator` 探针投机（观察入库标题 → 猜测兴趣域） |
| [`bilibili.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/bilibili.rs) | B 站匿名 WBI 客户端（加密参数 + 签名 + 5 分钟密钥缓存） |
| [`sources/mod.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/sources/mod.rs) | `SourceAdapter` trait + `ContentCandidate` 统一候选（platform+content_id 跨源去重键） |
| [`sources/bangumi.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/sources/bangumi.rs) | Bangumi v0 API（搜索/榜单 + 公开收藏导入初始化画像，UA 必须可识别） |
| [`sources/v2ex.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/sources/v2ex.rs) | V2EX 官方 API（hot/latest，限频严格每轮只取一次热门） |
| [`sources/weibo.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/sources/weibo.rs) | 微博匿名源（m.weibo.cn H5 容器 + 引导游客 SUB cookie + 实时热搜） |
| [`sources/x.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/sources/x.rs) | X (Twitter)：twitter-cli cookie 重放（扩展回传 auth_token+ct0，环境变量 `VIVIAN_X_COOKIE` 优先） |
| [`sources/reddit.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/sources/reddit.rs) | Reddit：rdt-cli 优先 + 匿名 .json 回退（扩展回传 Cookie 同步 rdt-cli 凭据文件） |
| [`sources/browser_signals.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/sources/browser_signals.rs) | 登录态被动信号采集（受控标签页正停平台域名时同源 fetch 历史，6 小时冷却） |
| [`sources/task_tabs.rs`](file:///g:/vivian-rs/src-tauri/src/discovery/sources/task_tabs.rs) | 隔离任务 tab 发现（小红书/抖音/知乎：inactive+静音标签 + 同源提取，平台 3 小时冷却） |

关键数据流：

```
搜索词生成（LLM 3-5 个 / 失败回退画像顶层兴趣）
   ↓
各源并行取候选（搜索 + 热门/榜单），跨源 platform:id 去重 + 库存/推荐账本去重
   ↓
LLM 批量评估（score/reason/topic_group，只看画像匹配度，热门与否不影响）EVALUATE_BATCH_SIZE=12
   ↓
score ≥ 0.5 入库（cap 60）｜≥ 0.75 惊喜队列 → Busy 分享竞争（acquire_delight_candidates）
   ↓
入库标题 → InterestSpeculator 探针行为确认（偏好/无感/没有感觉）
```

外部采集统一入库入口：`admit_candidates(char_id, candidates)` — 任务 tab / 引擎外采集路径复用同一套准入门槛（跨源去重 → LLM 评估 → 入库 + 探针），返回入库条数。

### browser_bridge/ —— 浏览器自动化桥

[`browser_bridge/`](file:///g:/vivian-rs/src-tauri/src/browser_bridge) + 配套 [`browser-extension/`](file:///g:/vivian-rs/browser-extension) Chrome 扩展构成「把真实浏览器交给角色」的通道：模型侧工具派发给扩展在受控/隔离标签页执行。

**命名空间**：桥工具在模型侧注册为 `mcp__browser__{action}`（`ToolCategory::Mcp`），与外部 MCP server 同处一个命名空间——桥本质上就是一个「连接」，只是方向相反（扩展反向连入 app，而非 app 拉起子进程）。扩展派发仍用线名 `browser_*`，两者由 `BrowserTool` 的 `name`（线名）/ `mcp_name`（模型可见名）成对维护，`wire_name()` / `to_mcp_name()` / `action_of()` 提供双向换算。

| 文件 | 职责 |
|------|------|
| [`protocol.rs`](file:///g:/vivian-rs/src-tauri/src/browser_bridge/protocol.rs) | WS 线协议帧契约（hello / tool.call / tool.result / rpc / ping / error）+ 常量与 RPC 方法名 |
| [`server.rs`](file:///g:/vivian-rs/src-tauri/src/browser_bridge/server.rs) | token 认证 WS 服务（axum 仅回环 :3080）+ 工具派发 + 平台状态 / X cookie / Reddit cookie 内存存储 |
| [`tools.rs`](file:///g:/vivian-rs/src-tauri/src/browser_bridge/tools.rs) | `browser_*` 工具（navigate/click/type/eval_js/snapshot/task_tab 等）经桥派发；模型侧名 `mcp__browser__*`，`BRIDGE_SERVER_ID` / `wire_name` / `to_mcp_name` / `action_of` 为命名空间换算入口 |

**命名空间换算的必改点**（工具名从线名变为 MCP 名后，按名字做字符串分支的逻辑会静默失效）：[`confirmation.rs`](file:///g:/vivian-rs/src-tauri/src/tools/confirmation.rs) 的 `confirmation_info` 在 match 前经 `wire_name()` 归一化；[`permission.rs`](file:///g:/vivian-rs/src-tauri/src/tools/permission.rs) 的可信来源免确认改按 `action_of(name) == Some("navigate")` 判定；[`config/manager.rs`](file:///g:/vivian-rs/src-tauri/src/config/manager.rs) 加载配置时把 `disabled_tools` 里的旧线名迁移为 MCP 名（幂等，无需迁移标记），否则用户已禁用的桥工具会被静默重新启用。经 `BridgeState::request_tool` 直接派发的调用方（`discovery/sources/*`）用线名，不受影响。

- **协议**：每个 WS 消息一个 JSON 帧，按 `t` 字段判别；工具调用带 `id` + `expiresAt`（过期不执行），支持 `tool.cancel` 撤回；新连接 `hello` 需在 5s 内提交 token 与 caps，顶替旧连接；服务端每 20s `ping` 探活（`PING_INTERVAL_MS=20s`，刻意低于 Chrome MV3 service worker 约 30s 的空闲终止阈值，留出余量防连接周期性掉线）
- **扩展 RPC 上报**（扩展 background 主动推送）：
  - `bridge.injectBrowserSnapshot`：用户显式选择跟随的标签页快照注入（服务端缓存，无参 `browser_snapshot` 优先返回）
  - `bridge.reportPlatformStatus`：平台登录态哨兵（Cookie 名探测，只回传布尔值，Cookie 值不离开浏览器）
  - `bridge.reportXCookie`：x.com `auth_token`+`ct0` → 服务端 twitter-cli cookie 重放（唯一真实 Cookie 离开浏览器的通道之一）
  - `bridge.reportRedditCookie`：reddit.com 整罐 Cookie（含 reddit_session）→ 服务端同步 rdt-cli 凭据文件
- **工具派发**：`BridgeState::request_tool` 登记挂起调用并派发 `tool.call`，扩展回传 `tool.result` 按 correlation id 唤醒等待方；`browser_task_tab` 不经受控标签页，直接在 background 层创建 inactive+静音的隔离任务标签执行（脚本在同 profile 下天然携带平台登录 Cookie），完成后自动关闭，用于需登录态平台的后台发现
- **扩展**（[`browser-extension/`](file:///g:/vivian-rs/browser-extension)）：manifest v3，权限 `tabs`/`activeTab`/`storage`/`cookies`/`alarms` + `<all_urls>`；background service worker 负责 cookie 哨兵探测、X/Reddit cookie 回传、工具分派与任务 tab；content script 在页面上下文执行动作（同源 fetch 自动携带登录 Cookie）
- **连接稳定性（防 MV3 service worker 空闲终止掉线）**：Chrome MV3 的 SW 约 30s 无活动即被终止，而 WS 消息交换（Chrome 116+）会重置该计时器。三道保活互相兜底——服务端每 20s `ping`；扩展侧另以 20s 间隔主动发送 `pong` 心跳帧（服务端对主动 pong 静默忽略，无契约变更）；manifest `alarms` 权限下 1 分钟周期 alarm 唤醒 SW 重建断开的连接（断线状态下纯 `setTimeout` 链无法唤醒 SW，会永久失联）。被顶替的旧 socket 迟到的 `close` 事件不再清理新连接状态（否则误杀新连接上的 in-flight 工具调用）

### world/ —— 真实世界感知

[`world/`](file:///g:/vivian-rs/src-tauri/src/world) 让 Vivian 感知真实世界。

| 文件 | 职责 |
|------|------|
| [`state.rs`](file:///g:/vivian-rs/src-tauri/src/world/state.rs) | `EnvironmentContext` 世界快照 |
| [`mod.rs`](file:///g:/vivian-rs/src-tauri/src/world/mod.rs) | `WorldStateProvider` 世界快照组装 + `build_sunrise_sunset`（`is_daytime` 昼夜判定按系统本地小时与日出/日落小时实时比较，不直接用天气 API 的 `is_day` 快照，避免随天气缓存刷新的滞后误判） |
| [`time_perception.rs`](file:///g:/vivian-rs/src-tauri/src/world/time_perception.rs) | 时间/节气/节日/日出日落（本地 NOAA 简化算法，作为天气 API 不可用时的回退，同样按日出/日落小时实时判定昼夜） |
| [`weather.rs`](file:///g:/vivian-rs/src-tauri/src/world/weather.rs) | Open-Meteo 天气（`daily=sunrise,sunset` 同时返回当日日出/日落小时，写入 `WeatherSnapshot.sunrise_hour` / `sunset_hour`） |
| [`volume.rs`](file:///g:/vivian-rs/src-tauri/src/world/volume.rs) | 系统音量（Windows Core Audio） |
| [`music.rs`](file:///g:/vivian-rs/src-tauri/src/world/music.rs) | 媒体播放检测（SMTC 事件） |
| [`foreground_window.rs`](file:///g:/vivian-rs/src-tauri/src/world/foreground_window.rs) | 前台窗口检测（Win32 FFI） |
| [`network_watch.rs`](file:///g:/vivian-rs/src-tauri/src/world/network_watch.rs) | 网络连接监控（COM 事件） |
| [`geolocation.rs`](file:///g:/vivian-rs/src-tauri/src/world/geolocation.rs) | IP 地理位置（ipwho.is） |
| [`events.rs`](file:///g:/vivian-rs/src-tauri/src/world/events.rs) | 世界事件检测（日出/日落事件驱动 proactive 的 `Sunrise`/`Sunset` 提醒） |
| [`entity_state.rs`](file:///g:/vivian-rs/src-tauri/src/world/entity_state.rs) | 用户实体状态机 + ExpectationEngine |
| [`activity_classifier.rs`](file:///g:/vivian-rs/src-tauri/src/world/activity_classifier.rs) | 前台窗口双层活动分类器（A 进程名映射 + B 嵌入分类） |
| [`activity_corpus.rs`](file:///g:/vivian-rs/src-tauri/src/world/activity_corpus.rs) | 活动观察丰富语料库（235 条种子，21 个细粒度活动标签） |
| [`user_behavior.rs`](file:///g:/vivian-rs/src-tauri/src/world/user_behavior.rs) | 用户行为日志（FIFO 300 条） |
| [`system_metrics.rs`](file:///g:/vivian-rs/src-tauri/src/world/system_metrics.rs) | 系统指标：常规轮询只刷总量（CPU/内存/网速，`SystemMetricsCollector` 持句柄跨轮询复用）；`top_memory_processes(n, exclude_pid)` **按需**枚举进程并按其可执行名聚合内存（进程数/合计/峰值，排除自身），供系统压力提醒与 `get_memory_usage` 工具在"需要告诉用户谁在吃内存"的时刻调用，不进常规轮询 |

### dialogue/ —— 对话历史管理

[`dialogue/`](file:///g:/vivian-rs/src-tauri/src/dialogue) 管理角色与用户及其他角色的对话记录。

| 文件 | 职责 |
|------|------|
| [`history.rs`](file:///g:/vivian-rs/src-tauri/src/dialogue/history.rs) | `DialogueManager` 主入口，固定 10 条消息窗口；持久化为 **JSONL 追加写**（`history/chat_history.jsonl`，flush 仅 append 新行 + 尾部 20 条缓存重复检测），旧 `full_chat_history.json` 首次访问自动迁移为 `.migrated` |
| [`intent_judge.rs`](file:///g:/vivian-rs/src-tauri/src/dialogue/intent_judge.rs) | 意图判断（告别种子短语 Top-K 投票 + softmax 加权预检 + LLM 语义判断） |
| [`strategy.rs`](file:///g:/vivian-rs/src-tauri/src/dialogue/strategy.rs) | 对话策略 |
| [`topic_tracker.rs`](file:///g:/vivian-rs/src-tauri/src/dialogue/topic_tracker.rs) | 话题跟踪 |

#### 关键函数

```rust
// 按渠道过滤对话历史（wechat / direct / cross_character）
pub fn get_history_filtered_by_channel(&self, channel: Option<&str>) -> Vec<ChatMessage>

// 设置当前 channel（跨角色对话时临时切换为 "cross_character"）
pub fn set_channel(&self, channel: &str)

// 添加消息（带元数据）
pub fn add_with_meta(&self, message: ChatMessage, meta: serde_json::Value)

// 追加写：取缓冲区 → 尾部 20 条重复检测 → 单次 write_all 追加所有新行
pub fn flush_buffer(&self) -> VivianResult<()>

// 旧 JSON → JSONL 一次性迁移；每轮 flush 只 O(新增条数)
fn ensure_jsonl_ready(&self)

// Patch 最后一条 assistant 消息的 metadata
// 用于微信语音消息等需要在 TTS 合成后回写元数据的场景（kind/audio_path/duration）
// 先在内存 buffer 中查找，找不到则回退到磁盘 JSONL（patch_last_on_disk，低频整文件重写）
pub fn patch_last_assistant_entry_metadata(&self, patch: serde_json::Value)
```

### music/ —— 音乐搜索与播放

[`music/`](file:///g:/vivian-rs/src-tauri/src/music) 把「按名字找歌并放出来」抽象成可插拔音源：本地文件、网页版 / 桌面客户端流媒体平台，各源能做的事不同。

| 文件 | 职责 |
|------|------|
| [`mod.rs`](file:///g:/vivian-rs/src-tauri/src/music/mod.rs) | `TrackSource`（local/netease/qqmusic/spotify）+ `TrackCandidate` + `PlayOutcome` + `MusicSettings` |
| [`local.rs`](file:///g:/vivian-rs/src-tauri/src/music/local.rs) | 本地曲库：递归扫描 → 按文件名检索 → rodio 解码播放 |
| [`deeplink.rs`](file:///g:/vivian-rs/src-tauri/src/music/deeplink.rs) | 流媒体深链：搜索页 URL / Spotify 直放 URI |

**与 `world/music.rs` 的分工**：那边是**系统播放感知与控制**（SMTC——知道在放什么、能定向切歌）；这边是**按名字找歌并放出来**。两者互补，不重叠。

**搜索范围解析（`local::resolve_search_dirs`）**：**显式 `directory` 参数 > 配置的 `local_dirs` > 常见位置兜底**。显式指定时**只用它、不叠加**（避免"我指了 D 盘却还去扫 C 盘"）。

**兜底搜索范围（`default_search_dirs`）**：未配置 `local_dirs` 时取 `USERPROFILE` 下的 Music / Downloads / Desktop / Documents / OneDrive\{Music,Desktop}，**只保留真实存在的**。Windows 已知文件夹可被重定向到别的盘，按标准相对路径拼，覆盖不到重定向（那种情况用 `directory` 显式指定）。

**任意位置扫描**：`grep_search` 的 `BINARY_EXTS`（`coding_tools.rs:24`）硬编码跳过音频扩展名，且按文件内容匹配而非文件名，看不到音乐文件；`list_dir` 只给深度 ≤4 的整棵树（`search_files` 是 `grep_search` 的别名，见 `builtin/mod.rs:202` 的 `alias_pairs` 表）。陪伴侧 `run_command` 归属 `ToolScope::Both`，可 `Get-ChildItem -Recurse -Filter *.mp3 -Path D:\` 扫任意位置，拿到路径后传 `track_id` 播放。

**检索内嵌在 `music_play`**：不单独暴露「搜索」工具，同一件事不拆成两个工具。多结果时把候选清单附在返回里（`alternatives` 字段 + 文案），用户说"换一首"时带 `index` 参数即可。

**`music_play` 的 `track_id` 快路径**：模型若已知确切本地路径（自己用 `run_command` / `list_dir` 找到的），传 `track_id` 即**跳过整库扫描**直接播放。`query` 与 `track_id` 二者至少给一个（**都不放进 schema 的 `required`**，由 `validate_input` 校验），`track_id` 经 `local::is_audio_path` 校验扩展名。

**已知代价**：`music_play` 不带 `track_id` 时每次调用都会**重新全量扫描**搜索范围（上限 2 万文件）。目前无缓存，范围大（如直接指 `D:\`）时值得加一层带 TTL 的目录缓存。

**播放结果标记（`PlayOutcome::auto_played`）**：标记实际是否开始播放。
- 本地曲库：✅ 真播放（rodio 解码文件）
- 桌面客户端深链：❌ 只能打开搜索页 —— `auto_played = false`
- Spotify 指定曲目 id：✅ `spotify:track:{id}` 是官方 URI，可直放

**音源解析（`resolve_source`）**：显式 `source` 参数 > 配置 `preferred_source` > 本地有命中则用本地。「本地无命中 + 未配置首选音源」时返回 `None`，调用方报错引导用户去配置，不擅自打开播放器。

**本地播放线程**：`rodio::OutputStream` 不是 `Send`，不能塞进全局 `Mutex`，故用独立线程持有（`player_loop`），经 channel 收发命令，每次播放新建 `Sink`（旧 Sink 丢弃 = 停上一首，不依赖 `Sink::stop()` 后可复用的实现细节）。

**文件名解析**：不解析音频标签，按 `艺术家 - 曲名` 约定拆；两侧都非空才算有效分隔（否则 `01-track` 会被误拆）。扫描上限 2 万文件 / 8 层深，防用户误配根目录。检索同分时按曲名排序，保证结果稳定。

### engine/ —— 桌宠表现层

[`engine/`](file:///g:/vivian-rs/src-tauri/src/engine) 管理桌宠表情/动作/资源清单与表现层协调。

| 文件 | 职责 |
|------|------|
| [`manifest.rs`](file:///g:/vivian-rs/src-tauri/src/engine/manifest.rs) | `ResourceManifest` 模型清单（表情/动作映射） |
| [`expression.rs`](file:///g:/vivian-rs/src-tauri/src/engine/expression.rs) | `ExpressionManager` 表情栈与定时恢复 |
| [`state_machine.rs`](file:///g:/vivian-rs/src-tauri/src/engine/state_machine.rs) | `PetState` 状态机（Idle/Interacting/Panicked/Playing/AiTalking） |
| [`animation.rs`](file:///g:/vivian-rs/src-tauri/src/engine/animation.rs) | 动画系统 |
| [`auto_trigger.rs`](file:///g:/vivian-rs/src-tauri/src/engine/auto_trigger.rs) | 自动规则触发（空闲/心情/程序事件） |
| [`feedback.rs`](file:///g:/vivian-rs/src-tauri/src/engine/feedback.rs) | 用户交互即时反馈 |
| [`resource_loader.rs`](file:///g:/vivian-rs/src-tauri/src/engine/resource_loader.rs) | 资源加载 |
| [`presentation.rs`](file:///g:/vivian-rs/src-tauri/src/engine/presentation.rs) | 表现层协调 |

**资源打包与加密加载链路**（release 模式，dev 直接读 `public/` 文件）：

```
资源加密步骤（VBL2 打包：逐文件 zstd-19 压缩 + AES-256-GCM 加密；产物 vivian.bundle.enc / vivian.bundle.index.json / asset_key.bin 由 build.rs 编译期读取嵌入、asset_crypto.rs 运行时解密）
  public/{chibi,world-bg}
       │
       └─ 逐文件 zstd-19 压缩 + AES-256-GCM 加密 ──> vivian.bundle.enc（VBL2 格式）
       │    │                                              ├─ asset_key.bin（密钥，build.rs 拆分四段混淆嵌入）
       │    │                                              └─ 索引段（每个文件 name/offset/size/plain_size）
       │    └─ bundle 只收运行时真正会请求的图集（isUnusedChibiAsset 过滤：chibi 仅主图集 + 眨眼序列，
       │       走路/转身/施法序列与制图源素材不进包）
       │
       └─ vite copyPublicAssets 白名单插件（copyPublicDir: false）：按 KEEP 白名单复制明文资源
          （room / chibi / fonts / icons），未列入白名单的 public/ 资源不进 dist

运行时（仅 release，lib.rs setup 中 bundle_reader::init 调用一次）
  bundle_reader::init() 打开 vivian.bundle.enc（mmap 延迟读取），解析 VBL2 索引到内存哈希表
  bundle_reader::get(path) 按需解密解压：
    ├─ 命中 LRU 缓存 → 直接返回
    ├─ 未命中 → 读取对应文件密文段 → AES-256-GCM 解密 → zstd 解压 → 写入 LRU 缓存 → 返回
    └─ 解压后校验 plain_size 与索引记录一致，防止数据损坏静默通过
  资源访问入口：
    ├─ 前端经自定义协议 http://model.localhost/<path>（lib.rs register_asynchronous_uri_scheme_protocol → bundle_reader::get）
    └─ 手机端经 /remote/model/{path} 路由（remote/mod.rs，release 走 bundle / dev 读 public/）
```

关键坑位（均已修复）：
- **dev/release 路径分流**：回退逻辑若统一 `fs::read_dir`，release 下必失败（磁盘无资源文件）
- **白名单漏配静默 404**：`copyPublicAssets` 的 KEEP 白名单必须与资源目录布局同步，目录改名/删除而 KEEP 未更新时该资源不进 dist——构建零报错、运行时 404（该分支已改为收集 missing 并在构建末尾 `console.warn`）

**VBL2 格式说明**（[`bundle_reader.rs`](file:///g:/vivian-rs/src-tauri/src/bundle_reader.rs) + [`asset_crypto.rs`](file:///g:/vivian-rs/src-tauri/src/asset_crypto.rs) + 资源加密步骤）：
- 文件头：4 字节 magic `VBL2` + 4 字节 LE uint32 条目数
- 索引段：每条 = 4 字节路径长度 + 路径 UTF-8（**变长**）+ offset 8 字节（LE u64）+ size 8 字节（LE u64）+ plain_size 8 字节（LE u64）。运行时磁盘索引按路径哈希后与 build.rs 编译期嵌入的 `BUNDLE_ENTRIES` 交叉校验（条目数、offset/size/plain_size 逐一比对，不匹配即拒绝启动并提示重新执行资源加密步骤生成 VBL2 bundle）
- 数据段：各文件密文依次排列，offset 相对数据段起始计算（读取时 `data_start + offset`）
- 与旧版整包解密解压的关键差异：**不预先加载全部明文到内存**，只有 `get()` 被调用时才读取对应密文段、解密解压；解压后明文走**按字节计上限的 LRU**（`CACHE_CAP = 16MB`，`bundle_reader.rs:31`），纹理这类大文件不驻留缓存、每次按需读取

**桌宠图集**（桌面端 CSS Sprite 渲染，[`ChibiPetCanvas.tsx`](file:///g:/vivian-rs/src/components/ChibiPetCanvas.tsx)）：
- 动作词汇表 [`src/chibi/animations.json`](file:///g:/vivian-rs/src/chibi/animations.json) 是动作的唯一真源，前端经 `src/chibi/motionRegistry.ts` 消费（图集定位、帧时长、方向、循环语义全部按表推导），后端由 `build.rs` 嵌入同一份 JSON 生成动作名清单与情绪映射——prompt 里列出的动作与前端能播的动作因此必然一致；新增或调整动作只改这一处
- 主图集为 3×2 姿态（`idle` / `happy` / `drag` / `dizzy` / `talk` / `listen`），由 `ChibiPetCanvas.css` 的 `--atlas-url` 引用 `/chibi/<角色>-atlas.webp`；待机呼吸与各姿态动效为 CSS keyframes（`prefers-reduced-motion` 时动画时长压至 1ms），影子独立 breathe 动画
- 走动 / 转身 / 眨眼 / 施法 / 表情共 8 组序列帧雪碧图（`chibi/walk/**`、`chibi/motion/**`），由 TS 侧按表内帧时长推进（`sequenceTokenRef` 令牌防串场）；循环型动作（走动）的帧推进归循环推进器负责，一次性动作自身播完即回落到 `moodTone` 基调（后端 `mood_tone` action 下发，`resetExpression` 走 `returnToTone()`，非硬编码 `idle`）；位移路径（智能避让与自主漫步）用的 `playTurn` 是唯一例外——转完**停在转身末帧**（侧身）不回落，由调用方接 `beginWalk`（起步，公开句柄上的 `playWalk` 即它的薄封装）或 `playTurnBack`（回正），二者配对构成位移前后的转身过渡，中途回落会让角色在转身与起步之间闪一帧正面待机。各动作帧数/网格（walk 14/4×4、turn 5/4×2·由原 8 帧精简、blink 6/3×2、cast·happy·angry·think·smug 各 12/4×3）以 [`src/chibi/animations.json`](file:///g:/vivian-rs/src/chibi/animations.json) 为准（`turn` 于 2026-09-12 由 8 帧减为 5 帧；抽取逻辑仍按源网格 `walk/source/*-turn-left-green.png`(4×2/8) 生成，改源网格或抽取逻辑须同步更新 turn 段）
- **单击反应池与「戳毛了」**（[`ChibiPetCanvas.tsx`](file:///g:/vivian-rs/src/components/ChibiPetCanvas.tsx) 的 `handleClick`）：单击不再写死 `happy`，改为按权重从 `TAP_REACTIONS` 抽一个（`smug` 4 / `think` 3 / `happy` 3 / 空串 3；`happy` 保留但不再是必然，空串 = 这一下不播表情，待机与自然眨眼照常继续）。抽空是**主动结果**而非兜底，所以调用方拿到空串必须什么都不做，不能拿 `idle` 顶上。与之互斥的第三种结果是 `rough_click`：`noteTap` 维护一本「戳烦了」账本——只统计最近 `TAP_ANNOY_WINDOW_MS`(5s) 内的点击，每戳一下 1 点、与上一戳间隔 < `TAP_ROUGH_INTERVAL_MS`(350ms) 的猛戳再 1 点，攒够 `TAP_ANNOY_THRESHOLD`(7) 就播 `angry`、清空账本并进入 `TAP_ANNOY_HOLD_MS`(2.5s) 的「气头上」（期间每戳一次续期，停手满 2.5s 才消气）。点击本身不携带力度，「太频繁」与「太粗暴」于是不是两套规则、而是同一账本的两种计法（实测：猛戳 4 下 450ms 内生气 / 每 300ms 连点 4 下生气 / 每 700ms 连戳 7 下生气 / 每 4s 一下连戳 6 下与两次双击永不生气）。双击的两次点击同样入账——双击只是同时还另有用途（开侧边聊天窗），不代表这两下不算戳；气头上双击仍开窗，只是姿态换成 `angry`。`ChibiInteraction` 相应多出 `'rough_click'`，`App.tsx` 的 `PetAction` 与后端 `commands/pet_reaction.rs` 的 `ACTION_ROUGH_CLICK` 三处同步（后端新增动作语、账本文案、`pet_rough_click` 标签与 20s 节流窗口）
- **走动节奏（前端 `src/chibi/walkPlan.ts`）**：两个消费方共用同一套模型，**只有速度锚点不同**——`planSmartMove(dx, dy)` 给智能避让（尽快让开，上限 0.6 px/ms），`planAmbientWalk(dx, speedScale?)` 给自主漫步（图集自己的地面速度 `300 / 1090 ≈ 0.2752 px/ms`）。二者都收敛到同一个 `composeWalkPlan(dx, dy, slideMs)`，链条固定为**时长 → 帧数 → 帧间隔 → 时长回写**，顺序不能换（B+D+A 四约束对应「上下移动走路过快」这条根因）：
  1. `slideMs = clamp(travel / 0.6, 400, 1400)`——避让的位移时长，本次挪动的**权威时间轴**（B：随距离缩放，不再固定 700ms）；漫步的 slideMs 改由 `|dx| / 原生步速` 给出（见下方漫步小节）；
  2. `strideFrames = round(|dx| / (300/14))`——步数由**水平位移** `|dx|` 推（D：腿只表达水平速度，纵向交给窗口滑动）。`|dx| < 40px` 的纯纵向位移直接 `walking=false`，不播走动；
  3. `frameDelayMs = clamp(slideMs / strideFrames, A_LO, A_HI)` 其中 `A_LO = round(0.75 × 原生均值) ≈ 58`、`A_HI = round(1.35 × 原生均值) ≈ 105`——把时长按帧数均分再夹到图集原生节奏带（A 兜底）。原生均值 = walk 14 帧时长之和 ÷ 14 ≈ 77.9ms。下沿防「用放大帧率去追远超步行能力的位移」（超速碎步），上沿防读不出摆腿；
  4. 限幅命中时**改步数**而不是改时长：`frameDelayMs < A_LO` ⇒ `frames = floor(slideMs / A_LO)`（少迈几帧、步幅变长）；`> A_HI` ⇒ `frames = ceil(slideMs / A_HI)`（多迈几帧、步幅变短）；`durationMs = frames × frameDelayMs` 按整帧回写 ⇒ `durationMs === frames × frameDelayMs` 严格成立，**走动收尾与窗口到位同时发生，且帧间隔绝不跌破原生地板（杜绝纵向挪动的超速碎步）**（实测 150/600/1200px → 67/59/58ms 每帧，长位移触地板 58ms；纯水平速度相同的 900px 与 1200px 帧间隔一致）
  - 踩过的坑：把第 2、3 步反过来（先按 `PX_PER_FRAME / speed` 定帧间隔、再让帧数填满时长）会让短位移的时长在 410–490ms 之间来回跳（帧数 5↔6 翻转，同一档距离两次挪动快慢不一），实测相邻档最大逆序 13ms，现方案 3ms
  - 更早的一版是「步数按 `hypot(dx,dy)` 推、帧间隔 clamp 到 33–98ms、再按帧时间轴回写时长」，两个后果：270px 以上位移全部撞上 33ms 下限（播放速度与移动速度脱钩），且 450px 位移被拖成 924ms。再早还有「帧间隔下限只 17ms」的版本——17ms ≈ 4.6× 原生节奏，正是纵向挪动走路过快的根因。对照组脚本见 `.workbuddy-ai/tmp/walk-plan-timing-test.mjs` C 段（复现你消息里算的那条 `round(travel/7)` + 固定 700ms 旧实现：纵向大位移帧间隔 7–49ms、总时长恒≈700ms、纯纵向仍播走动）
  - 窗口位移的采样统一在 `src/chibi/slideTrack.ts` 的 `runSlide()`：固定 32ms 采样步长、**最短 24 步**（D：对齐采样率，避免十几步定位 + 上百次逐帧重渲染挤在同一段时间里抢主线程）、easeInOutCubic 缓动，采样密度与走动帧率解耦。提取成独立模块的原因：避让与自主漫步此前各写一份采样循环（避让三次缓动 + 32ms 采样、漫步二次缓动 + 写死 48 步），同一位移在两处呈现的加速度不同
- **自主漫步（`planAmbientWalk`）**：与避让共用 `composeWalkPlan`，只把速度锚点换成图集自身的地面速度 `CYCLE_TRAVEL_PX / Σdurations = 300 / 1090 ≈ 0.2752 px/ms`，默认 `speedScale = 1 ± 0.15`（`AMBIENT_SPEED_JITTER`，每趟抽一次做步频抖动，避免长距离漫步变成节拍器；幅度压在安全带内，所以**限幅永不命中**——帧间隔恒为原生节奏本身，实测 140–900px 全域零限幅）。时长完全由距离决定（`|dx| / speed`），于是「走多远花多久」，步数与地面位移严格一一对应
  - 调用方是 `ChibiPetCanvas` 的 ambient effect（`presenceState === 'online'` 时启用）：静息 `48–120s`（**走完一趟起算**）/ 被占用重试 `8–14s`（**被占用不消耗静息期**，只有「刚走过」或「主动决定不走」才排静息，语义才干净）/ 上线首趟 `10–25s`（静息期的含义是"刚走过一趟、歇一会儿"，启动时并不成立，直接套用会让桌宠头两分钟杵在原地）；距离**对数均匀**取 `[140, 900]px`（中位 355 / 均值 408 / 均值时长 1.5s）；`roomFor(dir)` 判该方向剩余空间、贴边则朝里走，两边都放不下 `WALK_DISTANCE_MIN_PX` 就放弃这趟（硬塞出来的位移会比转身动画还短）；朝向由**实际** `dx` 定，而不是抽签的方向——贴边截断后两者可能反号。启用条件除 `presenceState === 'online'` 外还有一道 `poseNameRef.current !== 'idle'` 门槛：后端用 `mood_tone` 把基调设成非 idle（如 `dizzy`）之后漫步**彻底停摆**——这是沿用下来的行为，语义上更该用眨眼那套 `isAtRest()`（它把「停在当前心情基调上」也算作静止）
  - ⚠️ 改动前的根因（比"行走距离固定"更严重）：漫步只 `setActivePose('walk')`、**不设** `walkTargetFrames` / `walkFrameDelay`，腿按图集原生节奏（77.9ms/帧）无限循环，而窗口 2.4–3.3s 只挪 58–120px ⇒ 一个 1.09s 的腿周期只覆盖约 26–40px 地面，**腿超速 7–11 倍、脚在地上打滑**；且时长是另一根独立随机数（`2400 + rand*900`），"走 58px"与"走 120px"花一样的时间。避让路径早就走 walkPlan 了，两条路径各行其是——现在收敛到同一模型
  - `beginWalk(direction, frames, frameDelayMs)` 从 `playWalk` 中拆出，返回 `{ token, done }`：漫步要和窗口滑动**并行**跑，必须自己持 token 判打断（`runSlide` 的 `shouldAbort`），`playWalk` 只是 `beginWalk(...).done` 的薄封装

**智能避让的焦点/交互让路**（前端 [`src/hooks/useSmartPositioning.ts`](file:///g:/vivian-rs/src/hooks/useSmartPositioning.ts)）：
- 两条互补信号，缺一不可：
  - `focusedRef` —— `getCurrentWindow().onFocusChanged()` 写入（:286）
  - `userInteractingRef` —— 捕获阶段 `window` 的 `mousedown`/`mouseup` 写入（:80-106）。补上它的原因：`mousedown` 早于焦点事件；长按期间窗口无位移、焦点也可能尚未落到桌宠窗口；`mouseup` 又早于失焦事件
- **三层拦截**，覆盖「检查尚未发起」「检查已发起但未动」「已在滑动」三种时序：
  1. `runCheck` 入口：`focusedRef || userInteractingRef` 直接返回（:200-203）
  2. `find_safe_position` 等每次 `await` 返回后重查两个 flag（:222、:233）——截图/规划是异步的，期间用户可能已按下
  3. `animatePosition` 内 `shouldAbort()` **逐帧**重读（定义在 :142，由 `runSlide` 每帧回调）：`cancelled || token !== moveTokenRef.current || userInteractingRef || focusedRef`
- **滑动会话 token**（`moveTokenRef`）：`abortMove()` 自增即让正在跑的滑动循环当帧退出。调用点 = 用户按下（mousedown 捕获）、窗口获得焦点、（`[enabled]` effect 卸载时）
- 松手（mouseup）后延迟 250ms 补跑一次检查；失焦仍走 `FOREGROUND_DEBOUNCE_MS`(700ms) 防抖 + 延迟 500ms 强制检查
- ⚠️ 此前的实现**只**在 `runCheck` 入口查 `focusedRef`，`animatePosition` 里只查 `cancelled`——避让一旦开始滑动就会无视用户介入把整段缓动播完（实测按下后仍会再滑 6 步，约 480ms）。改动后按下/获得焦点当帧停止
- **打断也要收尾姿态**（`abortToRest`）：入场转身转完会停在侧身、走动会停在某一格，任何 `shouldAbort()` 命中后直接 `return` 都会把角色定格在侧身/抬腿。因此中止时调 `resetExpression()` 送回基准姿态，但**用户按下时不调**——那种情况由精灵自身的 `mousedown` 收尾（它会把动作切回基调），这里再调会盖掉随后下发的 `drag` 姿态
- 焦点状态只服务避让，不参与后端 `trigger_system_event` 的 `window_focus` / `window_blur`（那是给心智/在场的系统事件流）——两套互不干扰
- 发布图集为 WebP q92（`--atlas-url`、`animations.json` 的 sheet 模板、资源加密步骤「仅收运行时请求的图集」过滤三处扩展名必须同步）：2048px 图集 PNG 已贴近 deflate 熵极限，oxipng 无损重压只能省 5%，而有损 WebP 在屏幕实际绘制尺寸下 40 dB 以上、肉眼无差
- `vite.config.ts` 的 `KEEP` 只复制 `*-sheet.webp`：逐帧原图与 `walk/source/` 是制图中间产物、运行时零加载，约 95 MB 不进包；改动 `public/` 下被加密的资源后须重新执行资源加密步骤生成 VBL2 bundle

**拖拽惯性甩飞 + 边缘回弹**（后端 [`window.rs`](file:///g:/vivian-rs/src-tauri/src/commands/window.rs)，`cursor_tracking` 线程内）：
- **速度采样**：拖拽期间每帧把全局光标坐标（`app.cursor_position()`）push 进环形轨迹（最近 120ms，`drag_samples: VecDeque`）。用全局光标而非窗口位置，因为快速甩动时鼠标会冲出窗口、前端 mousemove 丢失，只有 `GetCursorPos` 轮询不丢数据；松手瞬间由 `fling_velocity_from_samples` 用首尾两点差分算初速度（跨度 < 40ms 或速度 < 0.5 px/ms 不触发，上限 4 px/ms 防极端甩动横穿）
- **甩飞线程**（`start_fling`，独立 `fling-<label>` 分身线程）：12ms 一帧，位置积分 + 指数摩擦 `v *= exp(-0.002·dt)`（约 350ms 半衰期，总滑行距离 ≈ v₀/k），速度低于 `FLING_STOP_VELOCITY`（0.06）自然静止
- **碰撞边界 = 身体足迹**：不是窗口矩形，而是窗口中央 1/3 宽 × 4/9 高（与点击穿透中心矩形同口径）。桌宠本体只在该范围渲染，周围全透明，所以窗口最多滑出屏外 1/3 宽 / 5/18 高，视觉上"角色撞墙回弹"。碰撞时位置夹紧 + 法向速度乘 `FLING_RESTITUTION`（0.6）反弹，配合摩擦几次后静止；边界用虚拟屏幕（`SM_X/CYVIRTUALSCREEN`，多显示器并集）
- **让位与退出**：代号称谓表 `FLING_GEN` 递增取代旧线程；重新抓起（`DRAG_OFFSET` 出现本窗口）立即让位「接住」；窗口隐藏/应用退出/`stop_cursor_tracking_internal` 清空代号表也会终止
- **不干扰程序化移动**：每帧重读 `outer_position()` 作积分基点（记为 `pos_x/pos_y`，即本帧积分起点），智能避让等外部移动不会被甩飞覆盖
- **「撞边」只认甩飞自己撞的**（`resolve_axis_collision`，逐轴）：边缘处理拆成纯函数，输入 `(from 积分前位置, p 积分后位置, v 法向速度, min, max, restitution)`，返回 `(夹紧位置, 反弹后速度, 撞击速度)`。**撞击速度只在 `from` 严格在界内且法向速度朝外时为 `-v`/`v`**；若 `from` 已在界上/界外（智能避让、环境走动、全屏隐藏把桌宠自行挪到墙边），只夹紧位置、归零撞击速度——否则「桌宠自己走到墙边」会被当成撞墙而晕乎乎。`from` 为 NaN 时同样归零（`!(from > min)` 写法顺带覆盖）
- **「被甩晕」表情联动**（`drag:dizzy` 事件，后端算时长、前端只管播）：两条触发路径共用同一事件，payload 为 `{ duration_ms, reason, impact? }`——
  - `reason: "fast_drag"`：拖动期间用相邻两帧 `drag_samples` 估瞬时速度（`drag_speed` → `f64`，跨度 ≤1ms 视为不可信返回 `None`；`is_drag_too_fast` 是它的 `>= DRAG_FAST_VELOCITY` 薄封装），需**同时**满足两个条件才判「极端疯狂甩动」：① **连续 `DRAG_FAST_MIN_STREAK`(4) 帧**都 ≥ `DRAG_FAST_VELOCITY`(3.2 px/ms ≈ 3200px/s，约 60ms×4 ≈ 240ms) ② 该连续区间内 `fast_drag_peak` 冲到过 `DRAG_FAST_PEAK_VELOCITY`(4.2 px/ms ≈ 4200px/s)。单帧尖峰只把 `streak`/`peak` 双双归零、不触发；只满足 ① 而峰值不够（稳定贴阈值快拖）也不触发——这是「只认爆发甩动」的关键。触发后按 `DRAG_FAST_EMIT_INTERVAL_MS` 450ms 节流，每次刷新 `DRAG_FAST_DIZZY_MS` 1200ms，且**触发即把 `streak`/`peak` 归零**（下一次需重新累计一整段，不给持续超速者永久资格）；松手 / 窗口隐藏时清掉节流窗口、连续计数与峰值，下一次拖拽可立即触发
  - `reason: "edge_bounce"`：取两轴撞击速度的较大值 `impact = ix.max(iy)`（由上面的 `resolve_axis_collision` 给出，已在源头排除「非甩飞撞击」），`bounce_dizzy_ms` 把 `impact < 0.25` 的轻贴边缘判为不触发，否则 `1400 + (impact-0.25)×1200`（封顶 2400ms）——撞得越狠晕得越久
  - 前端（[`App.tsx`](file:///g:/vivian-rs/src/App.tsx) 拖拽表情 effect）收到后 `setExpression('dizzy', duration)`，定时器到点若仍在拖拽会话中（`dragSessionRef && dragExpressionAppliedRef`）则回到 `drag`「被拎起」格位，否则交回画布自行回落到 `moodTone`；`resetDragExpression`（mouseup / `drag:cancelled`）会清掉该定时器。时长缺省兜底 `DIZZY_FALLBACK_MS` 1500ms
  - ⚠️ **必须用 `win.emit_to(&label, ...)` 而不是 `win.emit(...)`**。`WebviewWindow::emit` 走的是 `Manager::emit`，语义是**全量广播给所有 webview**（见 tauri `Emitter` trait 的默认实现），不是"发给这个窗口"。桌宠每个角色一个窗口、各自跑一份 `App.tsx` 且这些监听器都**不按 `character_id` 过滤**（`drag:cancelled` 的 payload 是空的，也没法过滤），于是广播的后果是：拖 A 甩晕，**B 也一起晕**；A 的 `drag:cancelled` 还会复位 B 的拖拽表情并打断 B 自己的长按召唤进度环。`emit_to` 传 label 走 `Manager::emit_to`，按 `AnyLabel` 匹配前端注册的 `WebviewWindow { label }` 监听器（`getCurrentWindow().listen()` 正是这种注册），只投给本角色窗口。label 即 `character_id`（`get_webview_window(&char_id)` 取窗口）。同一子系统内 `drag:cancelled` 已一并改为 `emit_to`；`cursor:position` 推送则整体删除——鼠标跟随改由前端 `pointermove` 驱动后全仓无人消费它

### presence/ —— 在场状态与后台任务

[`presence/`](file:///g:/vivian-rs/src-tauri/src/presence) 管理角色在场状态与后台任务。

| 文件 | 职责 |
|------|------|
| [`mod.rs`](file:///g:/vivian-rs/src-tauri/src/presence/mod.rs) | `PresenceState`（Online/Busy/Rest/Offline） |
| [`background_tasks.rs`](file:///g:/vivian-rs/src-tauri/src/presence/background_tasks.rs) | Busy 知识采集（主题来源优先级：过期刷新 > 对话提示 > LLM 决策） |
| [`meme_acquisition.rs`](file:///g:/vivian-rs/src-tauri/src/presence/meme_acquisition.rs) | SNS 热梗定期采集（7 天周期，B 站/抖音/小红书/微博定向，角色差异化平台） |
| [`config.rs`](file:///g:/vivian-rs/src-tauri/src/presence/config.rs) | 配置 |

#### meme_acquisition.rs 关键函数

```rust
// 启动热梗采集循环（每个角色一个独立 task，lib.rs 启动时调用）
pub fn spawn_meme_acquisition_loop(char_id, app, router, memory)

// 单次采集主流程：LLM 生成关键词 → 平台定向搜索 → LLM 总结 → 入库
async fn run_meme_acquisition(char_id, router, memory, web_search_config) -> AcquisitionResult

// LLM 基于当前日期 + 角色人设 + 平台侧重生成当周热梗候选词
async fn generate_meme_keywords(router, char_id, platforms) -> Result<Vec<String>>

// LLM 把搜索结果总结成角色口吻的"热梗笔记"
async fn summarize_meme_results(router, char_id, platform, keywords, results) -> Result<(title, content)>

// 角色差异化平台配置
fn platforms_for(char_id) -> Vec<PlatformConfig>
//   vivian → [bilibili (site:bilibili.com), douyin]
//   nana   → [xiaohongshu, weibo (site:weibo.com)]
```

采集流程：
```
1. 启动延迟 10 分钟
2. 读取 meme_acquisition_state.json 判断距上次采集时间
   ├── ≥ 7 天 → 立即触发
   └── < 7 天 → sleep 到下次触发（每 5 分钟检查取消信号）
3. 检查角色在线状态（Offline 跳过）
4. LLM 生成当周热梗候选词（最多 4 个，可返回 [none]）
5. 按角色平台配置（最多 2 个平台）：
   ├── 拼接 query：site:bilibili.com 热梗A OR 热梗B
   ├── WebSearcher 多引擎并发搜索（DDG/SearXNG/Tavily/Bing，每平台 6 条）
   ├── LLM 总结成"热梗笔记"（含梗名/来源/用法）
   └── add_knowledge_document(title, content, tags, source="meme_acquisition", ttl=7天)
6. 更新 meme_acquisition_state.json
7. emit meme_acquisition:finished 事件
```

### speech/ —— 语音系统

[`speech/`](file:///g:/vivian-rs/src-tauri/src/speech) 实现 ASR + TTS + 实时语音。

| 文件 | 职责 |
|------|------|
| `asr.rs` | ASR 统一入口（WinRT/Whisper/Azure/Aliyun） |
| `tts.rs` | TTS 统一入口（含 `synthesize_to_file` 仅合成不播放，供微信渠道语音消息使用） |
| `tts_edge.rs` | Edge-TTS（WebSocket + WordBoundary）；音色列表实时拉取官方 voices/list（失败回退内置 25 个：zh-CN 6 / en-US 17 / ja-JP 2，排除方言），`resolve_voice` 校验音色有效性并自动切换无效/已下架音色 |
| `tts_windows.rs` | WinRT SpeechSynthesizer（离线 fallback） |
| `tts_azure.rs` | Azure 认知服务 |
| `tts_gpt_sovits.rs` | GPT-SoVITS 自托管 |
| `tts_fish_speech.rs` | Fish Speech |
| `tts_minimax.rs` | MiniMax Speech |
| `tts_doubao.rs` | 豆包 TTS |
| `tts_cache.rs` | TTS 缓存 |
| `realtime_voice.rs` | 实时语音会话 |
| `realtime_protocol.rs` | 实时语音协议 |
| `whisper_realtime.rs` | Whisper 实时 |
| `planner.rs` | 语音规划 |
| `speech_memory.rs` | 语音记忆 |

#### 微信渠道语音消息（voice_message）

LLM 在 wechat 渠道返回 `voice_message: true` 时，回复以微信风格语音气泡发出而非文本。跨层数据流：

```
LLM JSON 输出 voice_message: true
  └── brain/json_parser.rs::ProcessedResponse.voice_message
      └── pipeline/state.rs::PipelineState.voice_message
          └── pipeline/steps/generation.rs → AiResponse.voice_message
              └── commands/chat.rs::send_message_stream
                  ├── 条件：response.voice_message && !is_direct_channel && brain.tts.is_enabled()
                  ├── brain.tts.synthesize_to_file(display_text, None) → (rel_path, duration)
                  ├── brain.dialogue.patch_last_assistant_entry_metadata({kind:"voice", audio_path, duration})
                  └── emit chat:done { voice_message, voice_audio_path, voice_duration }
                      └── 前端 ChatWindow.tsx 判断 voice_message，创建 voice 消息（复用 VoiceBubble 组件）
```

关键函数：

```rust
// speech/tts.rs —— 仅合成不播放，保存到 audio/ 目录并返回相对路径与估算时长
pub async fn synthesize_to_file(&self, text: &str, emotion: Option<&str>)
    -> VivianResult<(String, f64)>  // (rel_path "audio/<uuid>.<ext>", duration_secs)

// dialogue/mod.rs —— Patch 最后一条 assistant 消息的 metadata
// 先在内存 buffer 中查找，找不到则回退到磁盘文件
pub fn patch_last_assistant_entry_metadata(&self, patch: serde_json::Value)
```

降级策略：direct 渠道忽略此标志继续走实时 TTS；TTS 未启用或合成失败时 `voice_message` 字段在事件中置为 false，前端回退为普通文本气泡。

#### TTS 控制标记（像人的合成）

主 LLM 可在回复文本中插入 TTS 控制标记，让级联 TTS 表现出思考停顿/语速变化：

```rust
// speech/tts.rs
pub struct TtsControl {
    pub text: String,      // 剥离标记后的可朗读文本
    pub speed: Option<f64>, // 语速倍率覆盖（[SPEED:0.9]）
    pub pause_ms: u64,     // 停顿毫秒（[THINKING] 默认 500 / [PAUSE:ms]）
}
pub fn parse_tts_controls(text: &str) -> TtsControl
```

支持的标记：`[THINKING]`（思考停顿，默认 500ms）、`[PAUSE:800]`（显式延时 ms）、`[SPEED:0.9]`（语速倍率）、`[EMO:happy]`（情绪提示，与现有 expression 系统冗余，仅剥离）。

处理链路：

```
LLM 输出含标记的 text
  ├── chat_chain.rs 最终化：parse_tts_controls 剥离标记 → 聊天气泡/记忆只保留纯文本
  └── speech/tts.rs::speak_with_context：再解析一次 → 应用到合成
      ├── with_speed_override(speed) → 覆盖 config.rate
      ├── pause_ms > 0 → 播放前 tokio::time::sleep(停顿)（像人思考）
      └── strip_markdown_for_tts 一并去掉标记（所有路径绝不朗读）
```

约定：标记永不朗读、永不显示在聊天气泡里；输出格式提示词（`output_format.en.md` 英文模板的 `[OUTPUT_FIELDS]` 字段块，含示例）已说明哪些标记可用、每句至多 1-2 个。

### commands/ —— Tauri 命令层

[`commands/`](file:///g:/vivian-rs/src-tauri/src/commands) 暴露 225+ 个 Tauri 命令给前端。

| 文件 | 职责 |
|------|------|
| [`chat.rs`](file:///g:/vivian-rs/src-tauri/src/commands/chat.rs) | 用户对话入口（`send_message` / `send_message_stream`）；`wechat_group` 渠道含**群聊让位协议**——消息点名其他在线角色（裸名点名由 `scan_group_addressing` 识别）且未点名当前角色时让位：不回复/不唤醒/不写历史，仅旁观视角写 ShortTerm 记忆后 emit `chat:yielded` 静默结束 |
| [`proactive.rs`](file:///g:/vivian-rs/src-tauri/src/commands/proactive.rs) | 主动对话（`proactive_tick` + 跨角色仲裁状态 + Path B 续聊） |
| [`characters.rs`](file:///g:/vivian-rs/src-tauri/src/commands/characters.rs) | 角色管理 |
| [`memory.rs`](file:///g:/vivian-rs/src-tauri/src/commands/memory.rs) | 记忆操作 |
| [`mind.rs`](file:///g:/vivian-rs/src-tauri/src/commands/mind.rs) | 心智查询 |
| [`emotion.rs`](file:///g:/vivian-rs/src-tauri/src/commands/emotion.rs) | 情绪/表情 |
| [`config.rs`](file:///g:/vivian-rs/src-tauri/src/commands/config.rs) | 配置管理（另含工作智能体模型命令 `get_work_models` / `select_work_model` / `clear_work_model`：读取-切换-清除 reasoning 覆盖并持久化 `active_work_model`；LLM API 一键检测命令 `test_llm_route`：见 [providers/ —— 多 Provider 路由](#providers--多-provider-路由) 章节）。`save_config` 保存后从 `config.tools.disabled_tools` 热同步禁用集合到 `ToolSystem`（`set_disabled_tools`），工具开关保存即生效 |
| [`browser.rs`](file:///g:/vivian-rs/src-tauri/src/commands/browser.rs) | 外部连接页的内置连接器数据源：`get_browser_platforms`（桥状态 + 平台登录态 + 扩展目录）；`open_extension_folder`（文件管理器打开扩展目录）；`open_chrome_extensions`（打开扩展管理页）+ `open_url_in_chrome`（登录页强制用 Chrome 打开）。`chrome://` 非系统注册协议，两处打开一律定位 Chrome 可执行文件带参启动（Windows 走 App Paths 注册表 + 标准安装目录兜底），与系统默认浏览器无关——登录必须发生在扩展所在的 Chrome，Cookie 哨兵才能识别 |
| [`notebook.rs`](file:///g:/vivian-rs/src-tauri/src/commands/notebook.rs) | 笔记命令（含 `import_html_note` 直接读完整 HTML 存为 raw_html 笔记） |
| [`diary.rs`](file:///g:/vivian-rs/src-tauri/src/commands/diary.rs) | 日记 |
| [`tools.rs`](file:///g:/vivian-rs/src-tauri/src/commands/tools.rs) | 工具管理（`list_tools` 返回全部注册工具含 `is_custom` 字段供设置页区分自进化工具，不过滤禁用项；`get_tool_history` / `confirm_tool_execution`） |
| [`todo.rs`](file:///g:/vivian-rs/src-tauri/src/commands/todo.rs) | 待办与定时任务（前端待办面板命令 + `list_scheduled_tasks`；`add_todo_item` / `update_todo_item` 支持 `event_time`（事件开始时间）与 `due_date`（提醒触发时间）分离） |
| [`system.rs`](file:///g:/vivian-rs/src-tauri/src/commands/system.rs) | 系统操作（含 `factory_reset` 恢复出厂：锁死 tick → 停后台子系统 → 逐角色清空数据 → 写 `.factory_reset_pending` 清扫标记 → 重启；`factory_reset_sweep_if_pending` 在 `AppState::new()` 前按保留清单清扫用户数据目录，见[持久化模式](#持久化模式)） |
| [`backup.rs`](file:///g:/vivian-rs/src-tauri/src/commands/backup.rs) | 数据备份（`backup_user_data` 导出 `.altn` 备份 / `restore_user_data` 导入备份——校验备份文件、写入恢复标记并自动重启，前端导入走与恢复出厂同级的二次确认弹窗，见[恢复出厂设置](#恢复出厂设置数据重置)） |
| [`discovery.rs`](file:///g:/vivian-rs/src-tauri/src/commands/discovery.rs) | 内容发现画像（`get_discovery_profile` 查看画像 / `update_discovery_interest_weight` 调整兴趣权重 / 增删不喜欢主题 / `respond_interest_probe` 回应兴趣探针 / `bootstrap_from_bangumi` 公开收藏导入） |
| [`plugins.rs`](file:///g:/vivian-rs/src-tauri/src/commands/plugins.rs) | 插件清单与运行时装卸（`list_plugins` / `plugin_paths` / `list_skills` 技能管理面板（不展示内置风格预设）/ `reload_plugin` 重载单个插件（撤销旧贡献 + 按磁盘重装 + 连接 MCP）/ `unload_plugin` 卸载运行时贡献不动磁盘 / `delete_plugin` 删除插件目录——内置插件禁删） |
| [`tasks.rs`](file:///g:/vivian-rs/src-tauri/src/commands/tasks.rs) | 自治任务查询与取消（`list_agent_tasks` / `get_agent_task`（含后代谱系树）/ `cancel_agent_task`） |
| [`terminal.rs`](file:///g:/vivian-rs/src-tauri/src/commands/terminal.rs) | 内嵌终端（ConPTY 会话：`terminal_create` / `terminal_write` / `terminal_resize` / `terminal_kill` / `terminal_list`，供编程页 TerminalPanel 消费） |
| [`window.rs`](file:///g:/vivian-rs/src-tauri/src/commands/window.rs) | 窗口管理；含 `chat` 窗口右缘三态侧边栏（Hidden/Peek/Expanded，边缘检测线程 + WH_MOUSE_LL Hook + ease-out cubic 220ms 滑动动画 + 状态化鼠标穿透，`show_side_chat_animated`/`expand_side_chat`/`collapse_side_chat` 等命令带 `label` 参数）+ 拖拽惯性甩飞与屏幕边缘回弹（见 [engine/ 章节](#engine--桌宠表现层)）+ WebView 冻结/恢复（`freeze_webview`/`thaw_webview`，窗口隐藏时通过 WebView2 `TrySuspend`/`Resume` 挂起/恢复渲染进程，配合 `visibilitychange` 事件补拉隐藏期间的数据）。**消息横幅窗口**（`message_banner`，低频隐藏 WebView）空闲即冻结：4 个发送点（proactive/notebook_tools/send_image_tool/share_link_tool）统一走 `emit_message_banner`（先 `thaw_webview` 再 emit，防冻结期间事件丢失），前端横幅清空后经 `freeze_window_webview` 命令自冻结 |
| [`speech.rs`](file:///g:/vivian-rs/src-tauri/src/commands/speech.rs) | 语音 |
| [`tts.rs`](file:///g:/vivian-rs/src-tauri/src/commands/tts.rs) | TTS |
| [`realtime_voice.rs`](file:///g:/vivian-rs/src-tauri/src/commands/realtime_voice.rs) | 实时语音 |
| [`environment.rs`](file:///g:/vivian-rs/src-tauri/src/commands/environment.rs) | 世界感知 |
| [`history.rs`](file:///g:/vivian-rs/src-tauri/src/commands/history.rs) | 对话历史 |
| [`metrics.rs`](file:///g:/vivian-rs/src-tauri/src/commands/metrics.rs) | 指标 |
| [`persona.rs`](file:///g:/vivian-rs/src-tauri/src/commands/persona.rs) | 人格 |
| [`relationship.rs`](file:///g:/vivian-rs/src-tauri/src/commands/relationship.rs) | 关系 |
| [`user_facts.rs`](file:///g:/vivian-rs/src-tauri/src/commands/user_facts.rs) | 用户事实 |
| [`presence.rs`](file:///g:/vivian-rs/src-tauri/src/commands/presence.rs) | 在场状态 |
| [`engine.rs`](file:///g:/vivian-rs/src-tauri/src/commands/engine.rs) | 桌宠表现命令（表情 / 动作 / 模型信息 / 闲置动作 / 唤醒问候 / 鼠标避让开关） |
| [`window.rs`](file:///g:/vivian-rs/src-tauri/src/commands/window.rs) | 点击穿透（click_through 逻辑，原 `click_through.rs` 已并入本文件） |
| [`mind_inspector.rs`](file:///g:/vivian-rs/src-tauri/src/commands/mind_inspector.rs) | 心智调试 |
| [`ollama.rs`](file:///g:/vivian-rs/src-tauri/src/commands/ollama.rs) | Ollama |
| [`coding_agent.rs`](file:///g:/vivian-rs/src-tauri/src/commands/coding_agent.rs) | 编程智能体（`coding_new_session` / `coding_list_sessions` / `coding_delete_session` / `coding_cancel_session` / `coding_send_message`） |
| [`rag.rs`](file:///g:/vivian-rs/src-tauri/src/commands/rag.rs) | RAG |
| [`system_tray.rs`](file:///g:/vivian-rs/src-tauri/src/commands/system_tray.rs) | 系统托盘 |

### remote/ —— 远程访问 HTTP 服务

[`remote/`](file:///g:/vivian-rs/src-tauri/src/remote) 在应用后台启动一个轻量 axum HTTP 服务，暴露聊天与数据接口，并托管手机端 Web 前端。配合 Tailscale 等组网工具，手机可通过组网 IP 直接访问电脑上的智能体，实现移动端远程陪伴。

| 文件 | 职责 |
|------|------|
| [`mod.rs`](file:///g:/vivian-rs/src-tauri/src/remote/mod.rs) | axum 路由 + 全部 handler + 服务器生命周期管理 + toast 通知队列 + 模型资源路由 |
| [`frontend/index.html`](file:///g:/vivian-rs/src-tauri/src/remote/frontend/index.html) | 手机端单页前端（纯静态 HTML+JS，自包含，无外部库依赖） |

**配置**（`config.yaml` 的 `network.remote_access`）：
- `enabled`：是否启用远程访问（默认关闭）
- `port`：监听端口（默认 8080，范围 1024-65535），保存 `save_config` 后立即生效

**服务器生命周期管理**（`sync_remote_server`）：全局 `OnceLock<Mutex<Option<RemoteServerHandle>>>` 持有运行句柄，按配置幂等地执行启动 / 停止 / 端口变更重启。由启动流程（`lib.rs`）与 `save_config` 调用，支持运行时改端口/开关而无需重启应用。`start_server` 绑定端口带 5 次重试（每次 500ms），避免端口变更重启时旧监听器未即刻释放报 `AddrInUse`。

**HTTP API 端点**：

| 路径 | 方法 | 说明 |
|------|------|------|
| `/api/health` | GET | 健康检查（角色在线/在场状态 + 初始化 + API 配置） |
| `/api/characters` | GET | 角色列表 |
| `/api/chat` | POST | 发送消息（非流式，含 `channel`：wechat / direct） |
| `/api/characters/{id}/presence` / `mood` / `relationship` / `environment` / `mind` | GET | 角色状态查询 |
| `/api/characters/{id}/history?channel=` | GET | 聊天历史（支持按渠道过滤，两个对话界面各自加载独立历史） |
| `/api/characters/{id}/memories` | GET | 记忆列表 |
| `/api/characters/{id}/diary` | GET | 日记列表 |
| `/api/characters/{id}/stop` | POST | 停止生成 |
| `/api/characters/{id}/notes` | GET/POST | 笔记列表 / 创建 |
| `/api/characters/{id}/notes/{note_id}` | GET/PUT/DELETE | 笔记详情 / 更新 / 删除 |
| `/api/todos` / `/api/todos/{id}` | GET/POST / PUT/DELETE | 待办管理 |
| `/api/todos/{id}/complete` | POST | 完成待办 |
| `/api/tasks` / `/api/tasks/{id}` | GET/POST / DELETE | 定时任务管理 |
| `/api/tasks/{id}/pause` / `resume` | POST | 暂停 / 恢复定时任务 |
| `/api/characters/{id}/profile` | GET | 用户画像（基础事实 + 近期状态 + 自定义事实） |
| `/api/characters/{id}/profile/types` | GET | 事实类型枚举 |
| `/api/characters/{id}/profile/{type}` | PUT/DELETE | 设置 / 删除事实 |
| `/api/characters/{id}/profile/{type}/pin` | POST | 锁定 / 解锁事实 |
| `/api/toasts?since=` | GET | 增量拉取通知（toast 队列） |
| `/api/confirmations` | GET | 待处理工具确认列表 |
| `/api/confirmations/{id}` | POST | 解决工具确认（deny / allow_once / allow_always） |
| `/remote/model/{path}` | GET | 角色图集资源（chibi 主图集 / 眨眼序列；release 从 bundle 解密 / dev 从 `public/` 读取） |

**toast 通知队列**：全局 `OnceLock<Mutex<Vec<RemoteToast>>>` 环形缓冲（上限 100 条），`push_toast` 供后端各 emit 点调用。已接入两处：`registry.rs::request_confirmation`（工具确认请求 → `confirmation` 类型 toast）与 `proactive.rs` 的 `wechat:message_banner`（主动消息 → `proactive` 类型 toast）。手机端通过 `/api/toasts?since=` 增量轮询展示。

**工具确认联动**：`tools/confirmation.rs` 的 `ToolConfirmationRegistry` 新增 `list_pending()`（pending 条目存请求负载），并通过 `/api/confirmations` 暴露给手机端，手机可三态（拒绝 / 允许一次 / 始终允许）解决确认请求。

**手机端前端**（`frontend/index.html`）：底部导航 6 项——微信 / 直接 / 记忆 / 笔记 / 待办 / 画像。
- **微信对话界面**：复刻桌面 ChatWindow 视觉（灰底 `#e9e9eb`、用户 WeChat 绿气泡 `#95ec69` 靠右、AI 白色气泡靠左带头像、绿色发送键），智能体发送的链接渲染为微信风格卡片
- **直接对话界面**：复刻桌面 SideChatPanel 视觉（浅紫渐变底、AI 深色半透明圆角气泡靠左），顶部为桌宠舞台，**Vivian + Nana 双角色同屏渲染**，说话时对应角色上方弹出气泡并触发表情
- **桌宠渲染**：CSS 雪碧图，与桌面端共用同一套资源——3×2 六态主图集（`idle` / `happy` / `drag` / `dizzy` / `talk` / `listen`，各态配独立 CSS keyframes：待机呼吸 / 开心弹跳 / 拎起摆动 / 晕眩摇晃 / 说话起伏 / 倾听侧身）+ 3×2 眨眼序列，经 `/remote/model/` 路由加载（dev 从 `public/` 读，release 从 bundle 读）。表情名经 `expressionToPose` 正则归一化到六态（pout/angry→drag、dizzy/sad/sleep→dizzy、talk/speak→talk、listen/focus→listen、happy/love/star/shy→happy）；眨眼切到眨眼图集逐帧播完（帧距 55/48/58/78/70/105ms，随机 3.2-7.5s 间隔），仅 idle 态播放、被其他姿态接管即中断（`petBlinkToken` 令牌 + 清行内样式交回姿态控制）
- **单/双生布局**：单角色槽宽 ×0.88、可用高 ×0.90 缩放上限；双生模式两槽各占舞台半宽（×0.96），激活角色经 `data-active` 高亮，切换发言对象即切换高亮
- **输入栏**：`align-items: center` + 文本框/发送按钮均 38px，中轴水平对齐；发送按钮旁边不再显示红色停止按钮；空状态不显示「暂无直接对话记录」占位文本
- **程序坞**：全屏深度优先为 `100dvh`，高度再超出屏幕底部 `96px`，背景用 `linear-gradient` + `mask-image` 在底部 96px 做模糊渐隐到全透明，完全下拉时过渡区顶部位于屏幕底部以下，消除硬边的明显界限；`html`/`body` 背景设为应用底色铺满包含安全区的整个屏幕
- **记忆页过滤**：三类不渲染节点——旁观插话的内部系统指令（`isInterjectionPrompt`，如"现在你想插话…"）、跨角色话题总结记忆（`isCrossCharTopicSummary`，如"我和Nana聊了聊：我对她说…"）；广播群发去重（同一用户消息的直接对话与旁观节点相同正文时只保留对话节点）；记忆卡片不再显示底部重要性百分比条
- **记忆 / 笔记 / 待办+定时 / 画像**：对应后端 API 的移动端管理界面

### persona/ —— 人格定义与场景

[`persona/`](file:///g:/vivian-rs/src-tauri/src/persona) 管理角色人格与场景。

| 文件 | 职责 |
|------|------|
| [`prompt_render.rs`](file:///g:/vivian-rs/src-tauri/src/persona/prompt_render.rs) | Prompt 渲染 + 占位符泄露检测；`render_persona_flags_block` 生成 `[PERSONA_LOAD]` 硬约束标志块（Vivian/Nana 各 19 项人设标志 + 按界面语言的 LANG_* 语言标志），置于 `render_character_block` 产出的 Character 块最顶部；`render_language_style_block` 把 `LanguageStyle` 结构化口癖压成 `[LANGUAGE_STYLE]` 规则块（见下） |
| [`persona_card.rs`](file:///g:/vivian-rs/src-tauri/src/persona/persona_card.rs) | 人格卡片 |
| [`evolution.rs`](file:///g:/vivian-rs/src-tauri/src/persona/evolution.rs) | 自我进化覆盖层（智能体反思中自行调整语气/性格，独立于原始人设） |
| [`persona_decision.rs`](file:///g:/vivian-rs/src-tauri/src/persona/persona_decision.rs) | 人格决策 |
| [`dynamic_profile.rs`](file:///g:/vivian-rs/src-tauri/src/persona/dynamic_profile.rs) | 动态档案 |
| [`scene_selector.rs`](file:///g:/vivian-rs/src-tauri/src/persona/scene_selector.rs) | 场景选择（5 信号融合） |
| [`worldbook.rs`](file:///g:/vivian-rs/src-tauri/src/persona/worldbook.rs) | Worldbook 动态激活状态机 |
| [`tone_injector.rs`](file:///g:/vivian-rs/src-tauri/src/persona/tone_injector.rs) | 语气注入 |
| [`schemas.rs`](file:///g:/vivian-rs/src-tauri/src/persona/schemas.rs) | Schema 定义 |

#### 自我进化人设（`evolution.rs`）

让智能体在反思中自行调整语气/性格实现"成长"，核心是**覆盖层**而非改写原始人设。

**设计核心**：成长记录独立持久化到 `characters/<char_id>/persona/evolution.json`，与出厂人设（`persona.json` + `prompts/characters/`）完全分离；渲染时只追加到最终拼入 prompt 的 Character 块，原始文件永不触碰，因此天然支持恢复出厂（清空覆盖层）。

**核心结构**：

```rust
pub struct EvolutionEntry {
    pub timestamp: f64,   // 调整时间
    pub kind: String,     // "tone"（语气）/ "personality"（性格）
    pub text: String,     // 第一人称行为指令，如"最近回复可以更活泼一点"
    pub reason: String,   // 调整依据（源自哪段对话/体会）
    pub support: u32,     // 晋升前累计的支持次数（多少条独立轨迹共同支撑）
}

pub struct EvolutionCandidate {
    pub kind: String,
    pub text: String,
    pub reason: String,
    pub first_seen: f64,  // 首次被提出的时间戳
    pub support: u32,     // 被独立反思提出的次数
}

pub struct PersonaEvolution {
    pub entries: Vec<EvolutionEntry>,      // 已生效的正式调整
    pub candidates: Vec<EvolutionCandidate>, // 未达门槛的候选（不注入 prompt）
    pub updated_at: f64,
}
```

**数据流**：

```
反思调用（ReflectionRunnable）→ LLM 输出可选 evolution 字段
  { "evolution": { "tone": "...", "personality": "...", "reason": "..." } | null }
  └── apply_evolution(&json) → PersonaEngine.apply_evolution(kind, text, reason)
      └── PersonaEvolutionStore.add_entry → try_add
          ├── 首次提出 → 进入 candidates（support=1，尚未生效）
          ├── 再次提出 → support+1；≥ REQUIRED_SUPPORT(2) 且通过最小间隔 → 晋升 entries
          └── 晋升后 → 持久化

Prompt 组装（PersonaEngine.get_character_block）：
  ├── render_character_block(...)   // 出厂人设（不受影响）
  └── 追加 evolution.render(lang)    // 仅渲染正式 entries，candidates 不注入
```

**成长约束**（`PersonaEvolution::try_add`）：
- **跨轨迹验证门槛**：同一调整须在多次独立反思中被重复提出（support ≥ 2）才晋升为正式调整，防止一次偶发状态被固化为长期人格改变
- 最小间隔 6 小时：成长是渐进的，避免每轮对话都改（候选累积不受间隔限制，晋升受间隔约束；首次晋升不受限制）
- 去重：已是正式调整的文本不重复记录
- 上限：正式条数 ≤ 20，候选 ≤ 12；按"证据优先（support 高者）、时效次之"筛选保留，而非简单截断

**关键方法**：

| 方法 | 职责 |
|------|------|
| `PersonaEngine.apply_evolution(kind, text, reason)` | 记录/累积一条自我进化调整（达门槛才晋升并返回 true） |
| `PersonaEngine.reset_evolution()` | 恢复出厂：清空覆盖层 |
| `PersonaEngine.get_character_block()` | 渲染 Character 块并追加自我成长段落 |
| `PersonaEvolutionStore.add_entry` | 受约束地写入一条记录并持久化 |
| `PersonaEvolutionStore.candidates()` | 读取未达门槛的候选调整（可观测） |
| `PersonaEvolution.render(lang)` | 渲染覆盖层为 prompt 文本（三语） |

**Tauri 命令**：`get_persona_evolution`（读取覆盖层，返回 `entries` + `candidates` + `is_empty` + `last_update`）、`reset_persona_evolution`（恢复出厂）。两者均已注册进 invoke_handler。

**前端可视化**：记忆图谱页（GraphPage）底部「角色成长记录」区块（[`graph/EvolutionSection.tsx`](file:///g:/vivian-rs/src/components/mind-inspector/pages/graph/EvolutionSection.tsx)）——已生效 entries（日期戳 + 语气/性格类别贴纸 + 调整内容 + ↳ 依据 + 印证 ×N 红章）与「酝酿中」candidates（虚线弱化态 + `count/total` 支持进度，`total=2` 与后端 `REQUIRED_SUPPORT` 对齐，两处需同步）分开展示；随角色切换加载、带手动刷新，加载失败静默降级空态。

#### 结构化语言风格（`LanguageStyle` → `[LANGUAGE_STYLE]`，2026-09-15 接线）

`persona/schemas.rs::LanguageStyle` 的 9 个字段（`catchphrases` 口头禅 /
`preferred_sentence_final_particles` 语气词 / `response_length_bias` 长度偏好 /
`prefer_rhetorical_questions` / `allow_teasing` + `teasing_cooldown` /
`max_consecutive_questions` / `use_action_descriptions`）此前**全是死字段**：
只在 `schemas.rs` 的定义、`default_*` 与 nana 覆盖里出现，没有任何代码读取它们——
既没进 prompt，也没在代码里做逻辑约束。人格卡片可以覆盖它
（`persona_card.rs::language_style_override` → `mod.rs::config_with_card_overlay`），
但覆盖后喂给的 `render_style_block` 根本不读 `language_style`，所以那条路同样是空转。

现在由 `prompt_render::render_language_style_block` 渲染成紧凑的 `[LANGUAGE_STYLE]` 规则块，
接在 `【PERSONA_CONFIG】` 之后（同属"可被卡片覆盖的结构化配置"）。**统一用英文**，
与 `chat_style_framework()` 一致（项目约定：规则类内容英文，回复语言由 `LANG_*`
标志与 output_format 的 "same language as user input" 控制）。空列表与关闭项不输出，
整块最多 7 行。

**已知遗留**：该块从 **Core config** 渲染（与 Character 块"不受卡片覆盖影响"的契约一致），
所以卡片的 `language_style_override` 仍不生效。要打通需要把卡片版本并入
`character_block_cache` 的键（`get_character_block_tiered` 现在按
`(tier, lang, config_revision, evolution_version)` 缓存，而卡片切换不 bump
`config_revision`）——改缓存键是必要前提，否则切换卡片后渲染结果不会失效。

### emotion/ —— 情绪分类

[`emotion/`](file:///g:/vivian-rs/src-tauri/src/emotion) 实现多路径情绪分类。

| 文件 | 职责 |
|------|------|
| [`bridge.rs`](file:///g:/vivian-rs/src-tauri/src/emotion/bridge.rs) | EmotionBridge 桥接（LLM / 嵌入分类 + 心理状态更新 + 表情触发） |
| [`embedding_classifier.rs`](file:///g:/vivian-rs/src-tauri/src/emotion/embedding_classifier.rs) | 嵌入即时情绪分类（14 类情绪语料 210 条） |
| [`fast_semantic.rs`](file:///g:/vivian-rs/src-tauri/src/emotion/fast_semantic.rs) | 快速语义分类 + 认知知识需求评估（EpistemicAssessment）+ 日程通知信号评估（ScheduleAssessment，纯正则判定转来的通知材料） |
| [`llm_classifier.rs`](file:///g:/vivian-rs/src-tauri/src/emotion/llm_classifier.rs) | LLM 情绪分类 |
| [`mapper.rs`](file:///g:/vivian-rs/src-tauri/src/emotion/mapper.rs) | 情绪映射 |
| [`response_strategy.rs`](file:///g:/vivian-rs/src-tauri/src/emotion/response_strategy.rs) | 响应策略 |

`EmotionAnalyzer`（`mod.rs`）已移除关键词匹配，同步 `analyze` 接口始终返回 `neutral`，作为调用方兜底占位符；情绪分类由嵌入分类器与 LLM 分类器完成。

### utils/ —— 通用工具

[`utils/`](file:///g:/vivian-rs/src-tauri/src/utils) 提供通用工具。

| 文件 | 职责 |
|------|------|
| [`session_coordinator.rs`](file:///g:/vivian-rs/src-tauri/src/utils/session_coordinator.rs) | `SessionCoordinator` turn 协调（UserChat / CrossCharacter / ProactiveTick） |
| [`path.rs`](file:///g:/vivian-rs/src-tauri/src/utils/path.rs) | 路径工具 |
| [`environment.rs`](file:///g:/vivian-rs/src-tauri/src/utils/environment.rs) | 环境工具 |
| [`powershell.rs`](file:///g:/vivian-rs/src-tauri/src/utils/powershell.rs) | PowerShell 工具 |
| [`process.rs`](file:///g:/vivian-rs/src-tauri/src/utils/process.rs) | 进程工具 |
| [`system_idle.rs`](file:///g:/vivian-rs/src-tauri/src/utils/system_idle.rs) | 系统空闲检测 |
| [`power_events.rs`](file:///g:/vivian-rs/src-tauri/src/utils/power_events.rs) | 系统睡眠/唤醒感知：`PowerRegisterSuspendResumeNotification` 订阅电源事件。**睡眠前**（suspend）为所有角色强制标记用户离开——补 `GetLastInputInfo` 不含睡眠时间的盲区（通宵睡眠唤醒后 idle 仍显示睡前秒数，若不落账，回归摘要永远不触发）；**唤醒后**（resume）不主动标记在场，等真实键鼠活动（proactive tick 的 idle<60）触发 Present → 原有回归摘要链路拿到含睡眠时长的 away_secs；≥5 分钟睡眠写 `system_sleep` 世界事件入统一账本（按角色隔离）。回调线程纪律：suspend 分支微秒级内存写，账本 IO 派发后台线程 |
| [`token_estimate.rs`](file:///g:/vivian-rs/src-tauri/src/utils/token_estimate.rs) | Token 估算 |
| [`proactive_leader.rs`](file:///g:/vivian-rs/src-tauri/src/utils/proactive_leader.rs) | 主动对话 leader 选举 |
| [`cancel_token.rs`](file:///g:/vivian-rs/src-tauri/src/utils/cancel_token.rs) | 取消令牌 |
| [`job_object.rs`](file:///g:/vivian-rs/src-tauri/src/utils/job_object.rs) | Job Object（进程组管理） |
| [`pid_file.rs`](file:///g:/vivian-rs/src-tauri/src/utils/pid_file.rs) | PID 文件 |
| [`playback_gate.rs`](file:///g:/vivian-rs/src-tauri/src/utils/playback_gate.rs) | 播放门控 |
| [`fs.rs`](file:///g:/vivian-rs/src-tauri/src/utils/fs.rs) | 状态文件安全加载：`load_json_or_backup`（JSON 解析失败 → error 大声报错 + 损坏现场备份 `.corrupt-<ts>` + 空态继续）+ `backup_corrupted_file`；全仓状态加载点统一入口，杜绝静默 `.ok()` 丢弃 |
| [`watchdog.rs`](file:///g:/vivian-rs/src-tauri/src/utils/watchdog.rs) | 后台循环看门狗：`register` / `beat` / `unregister` + 守护任务；超过 3× 期望间隔（下限 120s）未心跳判定停摆，error 报警并按注册的回调拉起（`snapshot` 供健康接口读取） |

#### SessionCoordinator

```rust
pub enum TurnKind {
    UserChat,          // 用户对话 turn
    CrossCharacter,    // 跨角色对话 turn
    ProactiveTick,     // 主动 tick turn
}

// 关键方法
pub fn enter_user_turn(&self, char_id, session_id, memory, dialogue) -> TurnGuard
pub fn try_enter_cross_turn(&self, char_id, session_id, memory, dialogue) -> Option<TurnGuard>
pub fn try_enter_proactive_turn(&self, char_id, memory, dialogue) -> Option<TurnGuard>
pub fn signal_user_input(&self, char_id)           // 标记用户输入到达
pub fn current_turn_kind(&self, char_id) -> Option<TurnKind>
pub fn has_pending_user(&self, char_id) -> bool
```

`TurnGuard` 是 RAII Guard，Drop 时自动恢复前一个 session_id 并释放 turn。

---

## 关键数据流

### 1. 用户对话流

```
用户输入 → commands/chat.rs::send_message_stream
  ├── SessionCoordinator.enter_user_turn(char_id, session_id, memory, dialogue)
  ├── signal_user_input(其他在线角色)  // 让 proactive/cross 让出
  ├── brain.think(input, stream=true)
  │   └── BrainChatChain.ainvoke
  │       ├── prepare_pipeline_state  // 加载历史/注入会话回顾/更新凝神
  │       ├── execute_pipeline_and_build_response  // 执行 pipeline
  │       │   └── advisor_chain.invoke(PipelineState)
  │       │       └── PreProcessing → UserMemorySaving → [QueryRewrite ∥ FastSemantic]
  │       │           → MemoryRetrieval → WebContext → PromptBuilding
  │       │           → Generation → ResponseParsing → Validation → ExpressionMotion
  │       │           → PsychologyInsight → MoodUpdate → MemorySaving
  │       └── 后处理：Working Memory 推入 + 心理更新 + 记忆写回 + 工具调用
  ├── update_after_round + IntentJudge.judge_close_reason
  ├── seal_episode_on_close（若会话关闭）
  └── 返回 AiResponse
```

### 2. 跨角色对话流（A 对 B 说话）

```
源角色 A 的 LLM 调用 talk_to_character 工具
  └── tools/builtin/cross_character_tools.rs
      └── CROSS_CHARACTER_BUS.send_from_tool(req)  // 60s 超时
          └── CrossCharacterBus.send(app, state, req)
              ├── 会话生命周期检查（start_or_continue）
              ├── 互锁检测（源在 UserChat 且目标在 UserChat 或有 pending_user → peer_busy）
              ├── emit cross:start
              ├── 获取目标 think_lock（25s 超时 → target_busy）
              ├── TOCTOU 加固（获取锁后再次校验目标角色状态，非源角色）
              ├── try_enter_cross_turn（用户输入等待中 → user_input_pending）
              ├── 构造合成输入（主体 + 记忆锚点 + 交接上下文 + 共同观察 + 轮次提醒）
              ├── brain.think_cross_character(synthesized_input, stream=true)
              │   └── 流式 chunk 通过 cross:chunk 事件推送
              ├── update_after_round
              ├── emit cross:done
              ├── 源角色记忆持久化：
              │   ├── dialogue_add_with_meta（源发言 + 目标反馈）
              │   └── add_memory_with_metadata（合并 1 条 CasualConversation）
              ├── 目标角色补写对称记忆
              ├── 关系日志 + SocialState 更新 + 关系事实抽取（每 3 轮）
              └── 更新双方 LAST_SPOKEN / LAST_SPOKEN_TEXT
```

### 3. 主动对话 tick 流

```
前端定时器 → commands/proactive.rs::proactive_tick
  ├── SessionCoordinator.try_enter_proactive_turn（用户输入等待中 → 跳过）
  ├── ProactiveOrchestrator.tick(TickContext)
  │   ├── 13 种常规触发器评分（角色专属权重 + 触发器领地；Sunrise/Sunset 等 7
  │   │   种事件驱动触发器不进此循环，走下方专门路径）
  │   ├── 多级冷却检查（触发器独立 + 全局最小间隔 + 问候共享冷却）
  │   ├── check_trigger 通用门控链：
  │   │   ├── speech_desire 门控（发言欲望累积器）
  │   │   ├── 冷却检查（min_trigger_interval）
  │   │   ├── 问候共享冷却（5 分钟静默期）
  │   │   ├── 时机分数（TimingJudger）
  │   │   ├── 概率门控
  │   │   └── check_specific（触发器特定条件）：
  │   │       ├── social_urge 双向门控（enable_social_urge_gating 开启时）：
  │   │       │   ├── urge >= 0.8 → 跳过特定条件，提前触发
  │   │       │   ├── urge < 0.3  → 推迟（return false）
  │   │       │   └── 中间值      → 正常检查整点/空闲阈值（保底）
  │   │       └── WelcomeBack 豁免（用户刚回来必须问候）
  │   ├── 事件驱动提醒（不经常规概率门控，tick 专门路径，各带语义化冷却）：
  │   │   ├── 日出/日落（step 8.5）：检测到 Sunrise/Sunset 世界事件且用户在场、
  │   │   │   非防打扰、本角色为 leader 时，用主对话完整提示词生成提醒 + 弹出可
  │   │   │   一键切换主题的确认 toast（当前生效主题已是推荐值则跳过）
  │   │   ├── 系统压力（step 8.6 maybe_system_pressure_reminder）：内存占用
  │   │   │   ≥85%（normal→high 转换瞬间，持续高位只提醒一次）→ 按需枚举
  │   │   │   `top_memory_processes(8)` 聚合明细注入 `system_hint` 后生成提醒
  │   │   │   （能点名最吃内存的应用 + 轻量建议，不命令）
  │   │   ├── 主动截屏观察（step 8.7 maybe_screen_peek，异步）：窗口切换 +
  │   │   │   概率抽样 → 未授权先发言请求 + 弹确认 toast，同意后截屏+视觉理解
  │   │   │   并基于屏幕内容搭话（拒绝后 2h 冷却）
  │   │   ├── 应用时长 & 深夜未眠（step 8.8，互斥短路共用一次发言）：应用会话
  │   │   │   超类别阈值（coding/office 50min、game/video 75min、其余 90min）
  │   │   │   按语义提醒；凌晨 1-4 点活跃则优先催睡（每晚一次）
  │   │   └── 音乐切换（step 8.9 maybe_music_changed）：前后 MusicSnapshot 检测
  │   │       播放/切歌变化（过滤视频源），基于曲目信息搭话（45min 冷却 + 0.3 抽样）
  │   ├── 多角色去同步（六策略：相位抖动/权重分化/欲望累积/仲裁/情绪漂移/领地）
  │   └── 产出 ProactiveAction 列表（user_messages + cross_messages）
  ├── deliver_user_messages（用户消息）
  │   └── brain.think_proactive → proactive:bubble 事件
  ├── deliver_cross_character_messages（跨角色消息）
  │   ├── CROSS_CHARACTER_BUS.send
  │   └── Path B 续聊：should_continue && speak → spawn 反向续聊（最多 1 次）
  └── 返回 recommended_next_interval_ms
```

### 4. 心理微调 tick 流

```
cognitive_tick_runner（每 5 分钟）
  ├── 消费 pending_conflicts 队列（最多 5 条/次，指数退避重试 3 次）
  ├── DefaultConflictArbiter 仲裁（reflection 路由调用 LLM）
  └── 输出 ArbitrationOutcome（保留/合并/覆盖）

psychology_micro_tick（定时）
  └── Homeostasis 回归 + 微噪声波动（只写状态，无事件推送）
```

### 5. 启动流程

```
lib.rs::run
  ├── init_logging()              # 文件日志 non_blocking 缓冲上限 4096 行（防磁盘繁忙时内存堆积）
  ├── factory_reset_sweep_if_pending()        # 消费恢复出厂清扫标记

  │   └── 存在 .factory_reset_pending → 按保留清单删除用户数据目录其余条目（此时
  │       数据文件尚未被打开，可无锁删除，规避 vectors.db 的 SQLite 共享冲突），
  │       随后删除标记
  └── AppState::new()                         # 之后再进入 Builder setup
lib.rs::setup
  ├── 加载配置（config.yaml）
  ├── 取已托管 AppState（run 阶段已构造 AppState::new）
  ├── [仅 release] bundle_reader::init()     # 打开 vivian.bundle.enc，解析 VBL2 索引到内存（不加载密文/明文到内存，各文件按需解密解压）
  ├── startup::set_app_handle(app_handle)
  ├── async spawn：延迟 800ms 后 startup::ensure_startup_toast()
  │   └── 创建专用启动进度 Toast 窗口（失败自动重试，见下）
  ├── 后台初始化任务（async spawn）：
  │   ├── startup::begin_startup()             # 置启动标志，清零进度快照
  │   ├── async spawn：进度周期重发循环（800ms 重发最新快照，见下）
  │   └── 启动预检 startup::preflight（立即执行，不等 toast 前端就绪）：
  │       ├── 检查主 LLM 是否配置（api_key / api_secret / app_id + model）
  │       │   └── 未配置 → open_config_with_guide() + return false（停止初始化）
  │       ├── 检查嵌入服务是否配置（local 需路径+模型；云端需 Key+Endpoint+模型）
  │       │   └── 未配置 → open_config_with_guide() + return false（停止初始化，
  │       │       嵌入预加载延后到设置保存触发 reinitialize 时执行）
  │       ├── source=local → 先启动 Ollama 并 ensure_model_installed()
  │       │   ├── 启动失败 / 模型未就绪 → 打开设置引导，停止初始化
  │       │   └── 就绪（内部等待 HTTP API 可用）→ 继续初始化
  │       └── source=cloud → 不启动任何本地服务，直接放行
  │   └── 全程 emit startup:progress（统一进度 Toast，百分比单调递增钳制）
  ├── state.initialize()（预检通过后，嵌入任务在 Ollama 就绪后才开始）：
  │   ├── 构建 ModelRouter + 注册工具（register_builtin_tools → 插件工具 load_all_tools →
  │   │   自建工具 load_all → create_tool/create_plugin 元工具）+ Scheduler + MCP init_all
  │   ├── 为每个角色（进度映射以当前百分比为基点，多角色不回跳）：
  │   │   ├── MemoryManager::new
  │   │   │   └── seed_if_empty → 播种种子记忆并计算向量嵌入
  │   │   │       └── ensure_seed_vectors 逐条检查/补建缺失种子向量（逐条上报进度）
  │   │   ├── Brain::build
  │   │   │   └── preload_perception：
  │   │   │       ├── 情绪语料嵌入（挂 progress_callback 逐批上报，lib.rs 亦注入 embedding:progress）
  │   │   │       └── 语义语料嵌入（4 维度逐维度上报：意图/话题/记忆/关系）
  │   │   └── 插入 characters HashMap
  │   └── 初始化完成标志 initialized=true
  ├── create_character_windows()              # 创建在线角色窗口（提前创建的窗口补绑 PetController）
  ├── 继续注册 TTS / Presence / 世界感知 / 主动对话 / 记忆巩固等后台任务
  ├── emit startup:progress(100, "启动完成 ✓")
  ├── finish_startup()
  ├── emit app:ready
  └── sync_remote_server()                    # 初始化完成后才开放远程 API
```

#### 启动预检与开机自动启动

- **启动预检**：`startup::preflight` 在 `state.initialize()` 之前立即执行（不等待任何前端就绪信号）。主 LLM 或嵌入服务任一未配置 → 调用 `open_config_with_guide` 打开设置窗口展示配置指引并停止初始化；嵌入配置为本地 Ollama → 先 `ollama_service::start`（已在运行则直接复用）并 `OllamaServiceManager::ensure_model_installed` 等待 HTTP API 就绪与模型可用，之后才允许初始化角色（嵌入任务在 Ollama 可用后才开始）；嵌入配置为云端 API → 不启动任何本地服务直接放行。用户在设置窗口保存配置后 `reinitialize` 走同一套预检 + 初始化流程（含嵌入预加载），实现"未配置 → 配置保存 → 预嵌入"的闭环。
- **Ollama 常驻策略**：应用拉起的 Ollama 不绑定 Job Object（`kill_on_drop(false)`），退出清理也不停止——应用退出（含崩溃/强杀）后 Ollama 存活，下次启动 `check_port` 检测直接复用。`cleanup_orphan` 带 PID 复用防护：校验存活进程可执行名与预期一致才清理（`QueryFullProcessImageNameW`），PID 被无关进程复用时跳过。
- **统一启动进度**：`startup::emit_progress` 向所有窗口发送 `startup:progress` 事件；前端 `ToastWindow` 使用固定 key `99000` 维护单一持久进度 Toast，显示“当前阶段 + 百分比”，完成后自动关闭。专用 `startup_toast` 窗口在角色窗口创建前即由后端创建，保证启动早期也能看到进度。启动进度具备多态可靠性：
  - **快照补齐**：后端持续保存最近一次进度快照（`LAST_PROGRESS`），前端挂载时先 `get_startup_progress` 拉取快照显示占位进度；
  - **周期重发**：启动期间后台任务每 800ms 调用 `resend_last_progress` 重发最新快照，toast 窗口任意时刻挂载都能在 800ms 内收到当前进度（预检因此无需等待前端就绪，Ollama 可立即启动）；
  - **单调递增**：百分比经 `LAST_PERCENT` 钳制为单调递增，多角色依次预加载时不回跳；情绪语料逐批（progress_callback）、语义语料逐维度、种子记忆逐条以当前进度为基点做区间映射上报细粒度进度；
  - **延迟创建与失败重试**：`startup_toast` 由 setup 内部 async spawn 延迟 800ms 后创建，避免与 main 窗口 WebView2 初始化并发。创建失败（如快速重启时上一实例 WebView2 子进程仍持有 user data folder 锁触发 `ERROR_BUSY`）时按 `TOAST_RETRY_LEFT`（10 次 × 800ms）退避重试，成功或重试耗尽即停，保证窗口 WebView 创建失败时进度 toast 仍能出现；
  - **穿透固定窗口**：`startup_toast` 与角色 toast 窗口参数对齐——透明、无边框、置顶、跳过任务栏、不抢焦点（`focused=false`）、初始隐藏；创建成功后 `set_ignore_cursor_events(true)` 设为点击穿透（进度 toast 无交互元素，避免 400px 宽的全高窗口遮挡屏幕右上区域鼠标操作），`resizable(false)` 固定尺寸不可拖拽调整。
- **开机自动启动**：配置项 `base.auto_start`（默认 `false`），设置窗口「通用」页可开关；保存时通过 `utils::autostart::set_auto_start` 写入/删除 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 下的 `VivianDesktopPet` 值（当前用户启动项），启动时也会按配置同步一次。
- **种子向量修复**：`MemoryManagerInner::ensure_seed_vectors` 按 `seed_` 条目逐条核对向量库，缺失即补建；补建失败会导致 `MemoryManager::new` 失败，从而阻止 API 开放，避免“种子记忆存在于 JSON 但检索不到”的静默问题。
- **恢复出厂清扫**：`factory_reset` 命令（`commands/system.rs`）在重启前写入 `.factory_reset_pending` 标记；下次 `run()` 在 `AppState::new()` 之前调用 `commands::system::factory_reset_sweep_if_pending` 消费标记——此时数据文件尚未被打开，可按保留清单（配置 / 凭据与安全白名单 / `python-libs`、`pids`、`logs`、`mcp` 基础设施 / `skills`、`plugins` 扩展）删除其余全部用户数据（含记忆、聊天历史、截图 `screenshots/`、图片 `images/`、笔记、编程会话、内容发现数据与历史遗留目录），随后删除标记并按首次启动路径重建（角色由配置驱动注册，MemoryManager 播种种子记忆）。

#### 开发构建配置（dev profile）

`Cargo.toml [profile.dev]` 为规避两个 Windows 构建环境问题而特殊配置：
- `incremental = false`：rustc 写 `target/debug/incremental` 的 pre-lto-bitcode 偶发 `os error 5`（杀软实时防护锁文件）触发 ICE；关闭增量后单次全量编译稳定
- `crate-type = ["lib"]`：移除 cdylib（仅移动端需要），双 crate-type 让 rustc 单进程做两次完整代码生成、峰值内存近乎翻倍
- `opt-level = 0`：O1 驱动 LLVM 完整优化管线，非增量编译叠加重度泛型（tauri/axum/tokio/schemars）峰值内存 ~7GB，在 16GB 机器边缘触发 `rustc-LLVM ERROR: out of memory`（`STATUS_ILLEGAL_INSTRUCTION`）
- `codegen-units = 16`：非增量默认 256 个 CGU 各持独立 LLVM 上下文，进一步收窄

改回 `incremental = true` / `opt-level = 1` / 恢复 cdylib 会在多进程并发编译或内存吃紧时复现 ICE / OOM。

---

## 持久化模式

### 按角色隔离存储

```
%APPDATA%\Vivian\
├── config.yaml                          # 全局配置
├── config\feature_flags.json            # 功能开关
├── mcp\servers.json                     # MCP 配置
├── hooks.json                           # Hook 配置
├── arbitration_state.json               # 跨角色仲裁状态
├── trusted_apps.json                    # 信任应用列表
├── trusted_origins.json                 # 浏览器可信来源白名单（内置 + 用户合并，mtime 热重载）
├── skills\                              # 技能目录（*.md，30 秒热加载；create_skill 写入即注册）
│   └── *.md                             # 技能文件（可选 name/description/keywords front-matter；keywords 供 search_skill 召回）
├── tools\                               # 自建工具目录（*.json，30 秒热重载；create_tool 写入）
│   └── *.json                           # 工具定义（name/description/parameters/script/deferred）
├── plugins\<name>\                      # 插件目录（create_plugin 落盘即装载；手工编辑后设置→插件页重载）
│   ├── plugin.json                      # 清单（name/version/description + 四类贡献点声明）
│   ├── skills\*.md                      # 插件技能（<插件名>/<技能名> 命名空间注册）
│   ├── tools\*.json                     # 插件工具定义（CustomToolDef，DynamicTool 注册）
│   └── providers.json                   # 供应商预设数据（可选）
├── logs\                                # 日志（7 天轮转）
│   ├── vivian_YYYY-MM-DD.log
│   └── metrics_YYYY-MM-DD.json
├── psychology\
│   └── relationship_log.json            # 关系演化日志
└── characters\<char_id>\                # 按角色隔离
    ├── memory\                          # 记忆
    │   ├── entries.db                   # 记忆条目 SQLite（表 entries(id,json) + meta）
    │   ├── entries.db-wal / -shm        # WAL 日志
    │   ├── plain\                       # 条目明文镜像（<id>.txt，仅创建时写一次）
    │   ├── conversation_archive.jsonl   # 多级对话存档索引（L1/L2/L3，追加写）
    │   ├── archive_plain\               # 存档明文镜像（<id>.txt）
    │   ├── unified_memory.json.migrated # 旧版全量 JSON（迁移后保留备份）
    │   ├── events.ndjson                # 事件溯源日志
    │   ├── vectors.db                   # 向量索引（sqlite-vec / Qdrant config）
    │   └── vector_index\                # IVF 倒排索引
    ├── persona\                         # 人格
    │   ├── persona.json                 # 人设配置（出厂 + 用户覆盖）
    │   └── evolution.json               # 自我进化覆盖层（智能体反思中自行调整）
    ├── psychology\                      # 心理状态
    ├── history\                         # 对话历史
    │   └── chat_history.jsonl           # JSONL 追加写（旧 full_chat_history.json 迁移为 .migrated）
    ├── diary\                           # 日记
    ├── user_facts.json                  # 用户事实画像
    ├── user_model.json                  # 用户认知模型（UserTrait/UserGoal/UserProject）
    ├── mind\                            # 心智
    │   ├── beliefs.json
    │   ├── goals.json
    │   └── user_goals.json
    ├── proactive\                       # 主动对话状态
    ├── meme_acquisition_state.json      # 热梗采集状态（last_acquisition_ts）
    ├── notebook\<note_id>\              # 笔记
    │   ├── note.json
    │   ├── note.html
    │   └── .memory_ref
    └── presence\                        # 在场状态
```

### 持久化统一模式

- **append-before-mutate**：事件先落盘再修改视图（事件溯源）
- **TOCTOU 防护**：移除 `exists()` 预检，直接尝试 IO 并匹配 `ErrorKind::NotFound`
- **降级模式**：持久化目录不可写时降级到临时目录
- **错误传播**：核心数据结构返回 `VivianResult<()>`，非关键路径 `tracing::warn!` 后降级

### 恢复出厂设置（数据重置）

`factory_reset`（`commands/system.rs`）以「内存级清空 + 启动时目录级清扫」两段式恢复出厂，核心代码为 `commands::system::factory_reset`、`mark_factory_reset_sweep`、`factory_reset_sweep_if_pending`。

- **命令内清空**：置 `factory_reset_in_progress` 锁死 tick → 停止 proactive / activity_journal / pet_controller / scheduler / todo / speech_planner → 500ms grace period → 逐角色 `clear_all_memories`（记忆/聊天历史/关系/心理/日记/信念目标/状态文件/用户画像/笔记）+ `clear_common_memories` → 清空 resolver 缓存。
- **两段式的原因**：运行中 `vectors.db` 被 SQLite 长连接持有，Windows 共享冲突导致直接删目录失败；因此内存清空后只写标记 `.factory_reset_pending`，真正的目录级清扫推迟到重启后、任何数据模块打开文件之前（`AppState::new()` 前）执行。
- **清扫范围（白名单）**：遍历用户数据目录顶层，**保留清单外的条目一律删除**。删除范围覆盖 `characters/`（整树）、`common/`、`memory/`、`persona/`、`psychology/`、`proactive/`、`screenshots/`、`images/`、`rag/`、`spill/`、`todo/`、`diary/`、`history/`、`habits/`、`shared/` 及历史遗留文件（`avatar.jpg`、`crash.log`、`coding_sessions.json`、`consolidation_health_*.json` 等）。
- **保留清单**（`FACTORY_RESET_KEEP`）：配置（`config.yaml` / `config/` / `lsp.json` / `sound/` / `gpt_sovits_tts_infer.yaml`）、凭据与安全白名单（`.credentials.json` / `identity.json` / `trusted_apps.json` / `trusted_origins.json`）、运行时基础设施（`python-libs/` / `pids/` / `logs/` / `mcp/`）、用户扩展（`skills/` / `plugins/`）。
- **重建路径**：角色由 `config.yaml` 的 `characters.list` 驱动注册（非扫描 `characters/` 目录），清扫后按首次启动逻辑重建——MemoryManager 经 `seed_if_empty` 播种种子记忆与向量，persona / psychology / diary 等首次使用时生成默认文件；顶层 `sound/config.json` 保留，TTS 音色经既有迁移路径恢复到角色目录。

**备份 / 导入（`backup_user_data` / `restore_user_data`）**：设置 → 通用页「整体操作」抽屉统一收纳导出 / 导入 / 恢复出厂三项。导出备份选择目标目录后打包用户数据目录为 `.altn` 文件；导入备份先弹与恢复出厂同级的二次确认弹窗（`ClearConfirmDialog` 展示备份路径），确认后 `restore_user_data` 校验备份、写入恢复标记并自动重启，重启后完成数据回填。

---

## 并发与锁策略

### 锁类型

| 锁 | 类型 | 职责 |
|----|------|------|
| `think_lock` | `Arc<Mutex<()>>` | 串行化 think 调用（每角色独立） |
| `characters` | `Arc<RwLock<HashMap>>` | 角色表读写锁 |
| `active_character_id` | `RwLock<String>` | 活跃角色 ID |
| `config` | `Arc<RwLock<Config>>` | 配置读写 |
| `LAST_SPOKEN` | `Lazy<RwLock<HashMap>>` | 跨角色发言时间戳 |
| `SPEECH_RESERVATION` | `Lazy<RwLock<HashMap>>` | 发言优先级仲裁 |
| `YIELD_SUPPRESSION` | `Lazy<RwLock<HashMap>>` | 仲裁让步抑制 |
| `current_turns` | `Mutex<HashMap>` | turn 登记（SessionCoordinator） |
| `pending_user` | `Mutex<HashMap>` | 用户输入等待标记 |

### 并发原则

- **统一使用 `parking_lot::Mutex`**：不中毒、不持有 guard 跨 await
- **WNDPROC 回调使用 `try_lock()`**：避免重入死锁
- **阻塞系统调用用 `spawn_blocking`**：文件 IO / 进程枚举 / COM 调用 / 应用解析等隔离到阻塞线程池
- **`Semaphore` 限流**：`ModelRouter` 按任务分组并发限制（chat_reasoning=3 / memory_reflection=3 / auxiliary=2）；远程嵌入 `REMOTE_EMBEDDING_MAX_CONCURRENCY=4`；`augment_reply_service` `MAX_PENDING_ENTRIES=100`
- **RAII Guard**：`TurnGuard`（Drop 时恢复 session_id + 释放 turn）、`FocusLeaseGuard`（Drop 时释放焦点租约）

### 死锁防护（跨角色对话）

```
互锁场景：A 持有 A.think_lock 等待 B.think_lock，B 持有 B.think_lock 等待 A.think_lock

防护三层：
1. 互锁检测：send 入口检查源在 UserChat 且目标在 UserChat 或有 pending_user → 立即返回 peer_busy
2. pending_user 检查：覆盖"用户消息已 signal 但未 enter"的时间窗口
3. TOCTOU 加固：获取目标锁后再次校验目标角色状态（非源角色），处理竞态
4. 超时兜底：think_lock 25s 超时 + 工具层 60s 超时
```
