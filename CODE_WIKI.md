<div align="center">

# Vivian Code Wiki

**具备情感、记忆与主动性的 AI 桌面宠物 — 完整代码百科**

Rust + Tauri 2 + React 18 + Live2D Cubism 4

</div>

---

## 目录

- [一、项目概述](#一项目概述)
- [二、技术栈](#二技术栈)
- [三、整体架构](#三整体架构)
- [四、项目结构](#四项目结构)
- [五、后端 Rust 模块详解](#五后端-rust-模块详解)
  - [5.1 入口与编排层](#51-入口与编排层)
  - [5.2 brain/ 大脑核心](#52-brain-大脑核心)
  - [5.3 pipeline/ LangChain 风格流水线](#53-pipeline-langchain-风格流水线)
  - [5.4 memory/ 三层记忆系统](#54-memory-三层记忆系统)
  - [5.5 providers/ 多 Provider 路由](#55-providers-多-provider-路由)
  - [5.6 tools/ 增强工具系统](#56-tools-增强工具系统)
  - [5.7 psychology/ 五层心理架构](#57-psychology-五层心理架构)
  - [5.8 emotion/ 情绪分类](#58-emotion-情绪分类)
  - [5.9 proactive/ 主动对话](#59-proactive-主动对话)
  - [5.10 world/ 真实世界感知](#510-world-真实世界感知)
  - [5.11 persona/ 人格引擎](#511-persona-人格引擎)
  - [5.12 dialogue/ 对话管理](#512-dialogue-对话管理)
  - [5.13 engine/ Live2D 引擎](#513-engine-live2d-引擎)
  - [5.14 speech/ 语音系统](#514-speech-语音系统)
  - [5.15 network/ 网络基础设施](#515-network-网络基础设施)
  - [5.16 diary/ 日记系统](#516-diary-日记系统)
  - [5.17 config/ 配置管理](#517-config-配置管理)
  - [5.18 commands/ Tauri 命令](#518-commands-tauri-命令)
  - [5.19 其他根级模块](#519-其他根级模块)
  - [5.20 cross_character.rs 跨角色通信总线](#520-cross_characterrs-跨角色通信总线)
- [六、前端 React 架构详解](#六前端-react-架构详解)
  - [6.1 入口与组件树](#61-入口与组件树)
  - [6.2 状态管理 Zustand](#62-状态管理-zustand)
  - [6.3 组件清单](#63-组件清单)
  - [6.4 控制器 Controllers](#64-控制器-controllers)
  - [6.5 Hooks](#65-hooks)
  - [6.6 前后端通信机制](#66-前后端通信机制)
- [七、关键数据流](#七关键数据流)
- [八、依赖关系总览](#八依赖关系总览)
- [九、构建与运行](#九构建与运行)
- [十、配置系统](#十配置系统)
- [十一、资源与人格定义](#十一资源与人格定义)
- [十二、关键设计要点](#十二关键设计要点)

---

## 一、项目概述

**Vivian** 是一个常驻 Windows 桌面的多角色 AI 陪伴型宠物系统。系统当前内置两个独立角色,可同时在线:

- **Nana(娜娜)**:温柔大姐姐人设,银白短发狐耳狐尾,治愈系陪伴形象。
- **Vivian(薇薇安)**:weeb 网络少女,傲娇二次元性格,长期泡网的紫色长卷发女孩。

每个角色拥有独立的 Brain / Memory / Psychology / Persona / Live2D 模型与运行时资源,互不干扰;同时通过跨角色通信总线(`CrossCharacterBus`)支持角色之间互相对话,可在私聊窗口中让一个角色向另一个角色发起跨角色对话,或在群聊视图中同时与多个在线角色对话。系统不只是被动响应消息,而是拥有:

- **持续演化的心理状态**:五层心理架构(人格 → 需求 → 评价 → 情绪 → 行为驱动)+ Homeostasis 稳态引擎 + 昼夜节律(每角色独立)
- **跨会话的记忆体系**:三层记忆(短期 / 中期 / 长期)+ 混合检索(BM25 + 向量 + RRF + IVF)+ 写入时 LLM 增强 + 证据驱动可信度 + 事件溯源(每角色独立分桶持久化,`user_facts.json` 按角色隔离存储于 `characters/<char_id>/`)
- **可编排的工具系统**:70+ 内置工具 + 7 步执行管线 + 权限网关 + 沙箱 + MCP 原生集成 + Skills 抽象
- **主动发起对话的能力**:13 种触发器 + 9 种心理状态 + 偏好学习算法 + 内心独白
- **真实世界感知**:时间 / 节气 / 节日 / 天气 / 日出日落 + 系统音量(Core Audio) / 媒体播放(SMTC 事件驱动) / 前台窗口(Win32 FFI) / 网络连接状态(COM 事件) / IP 地理位置(城市级) + 位置注入 LLM 提示词 + 世界事件驱动情绪
- **自主活动**:用户离线时内心独白 + 活动日志观察 + 后台知识采集(Busy 状态下搜索网络→LLM 总结→写入 RAG,含采集/分享双冷却、SessionSummary 话题锚点、share 必须带理由、TTL 分级/时间衰减/过期刷新三层时效管理)
- **流式安全过滤**:思考链泄露过滤 + 工具调用标记泄露过滤 + 提示词占位符泄露检测
- **凝神/专注模式**:漏桶累积器 + 迟滞设计的专注模式状态机,支持 Regular / Focus / TrueName 三种认知模式;激活时注入认知模式 system 指令 + max_tokens 额外余量,proactive_tick 期间 idle 冷却衰减
- **多维度自动表情触发**:四类纯规则触发路径(用户直接交互10种类型/空闲五阶段渐进/心情状态联动/程序事件)零LLM开销即时响应,概率门控+冷却时间避免机械重复,前端17个动作库程序动画(基于模型真实参数驱动,跨模型兼容,Vivian 专属 tail_wag 利用 4 段尾巴参数)
- **多角色架构**:CharacterInstance 抽象 + 多 Tauri WebviewWindow(每角色一个独立窗口,label = character_id)+ 跨角色通信总线 + 角色级持久化分桶 + 命令层 `character_id` 路由

所有计算与持久化均在本地完成,仅在调用 LLM 时访问云端。当前仅支持 Windows(依赖 WinRT 语音识别与 ASR)。

### 主要使用场景

- 日常陪伴对话(流式响应 + 多 Provider 路由 + 联网搜索)
- 桌面自动化(应用控制、文件操作、媒体控制、屏幕感知、输入模拟)
- 主动关怀(基于作息学习与情绪状态的健康提醒、破冰、压力监控)
- 自我演化(人格、关系、需求、情绪的四层心理因果链)
- 真实世界感知(时间/天气/音量/媒体/前台窗口/网络/IP地理位置 + 位置注入提示词 + 世界事件驱动情绪 + 内心独白)

---

## 二、技术栈

| 层级 | 技术 | 版本 |
|------|------|------|
| **后端** | Rust (edition 2021) | 1.75+ stable |
| 桌面框架 | Tauri | 2.1 |
| 异步运行时 | Tokio | 1.41 (full feature) |
| 序列化 | serde / serde_json / serde_yaml | 1.0 / 1.0 / 0.9 |
| HTTP | reqwest (rustls-tls) | 0.12 |
| WebSocket | tokio-tungstenite | 0.24 (Edge-TTS) |
| 数据库 | rusqlite (bundled) + heed (LMDB) | 0.32 / 0.20 |
| 中文分词 | jieba-rs | 0.7 (BM25 检索) |
| Token 计数 | tiktoken-rs | 0.12 |
| Windows API | windows | 0.61 (Win32 + WinRT + Core Audio + SMTC + COM) |
| 音频采集 | cpal | 0.15 (跨平台 ASR) |
| **前端** | React 18 + TypeScript 5.6 | 18.3 / 5.6 |
| 构建工具 | Vite | 5.4 |
| 状态管理 | Zustand | 4.5 |
| 国际化 | i18next + react-i18next | 23 / 15 |
| Live2D | pixi-live2d-display + pixi.js | 0.4 / 6.5 |
| Tauri 前端 API | @tauri-apps/api | 2.1 |

### Tauri 插件(6 个)

- `tauri-plugin-shell`(打开 URL)
- `tauri-plugin-dialog`(文件对话框)
- `tauri-plugin-fs`(文件系统)
- `tauri-plugin-notification`(系统通知)
- `tauri-plugin-global-shortcut`(全局快捷键)
- `tauri-plugin-os`(系统信息)

---

## 三、整体架构

### 3.1 分层架构

```
┌─────────────────────────────────────────────────────────────┐
│ Layer 1: 入口与编排                                          │
│   lib.rs / main.rs / state.rs(AppState:多角色 HashMap)      │
│   cross_character.rs(跨角色通信总线)                        │
│   conversation/(会话生命周期:状态机+CloseReason+ResponseMode)│
│   commands/ (Tauri 命令,大多数带 character_id 参数)          │
└─────────────────────────────────────────────────────────────┘
                              │
┌─────────────────────────────────────────────────────────────┐
│ Layer 2: 核心协调                                            │
│   brain/ (Brain、ChatChain、Scheduler、RateLimiter、Callbacks)│
│   pet_controller.rs (7 种命令类型分发)                        │
│   pipeline/ (Runnable 流水线 + 14 个 Step + 4 个 Advisor)     │
└─────────────────────────────────────────────────────────────┘
                              │
┌─────────────────────────────────────────────────────────────┐
│ Layer 3: 领域子系统                                          │
│   dialogue/  engine/  emotion/  proactive/  persona/  diary/  │
│   psychology/  memory/  tools/  providers/                  │
└─────────────────────────────────────────────────────────────┘
                              │
┌─────────────────────────────────────────────────────────────┐
│ Layer 4: 基础设施                                           │
│   network/  speech/  world/  config/                         │
│   resilience/  metrics/  i18n/  utils/  types/  messages/   │
│   error.rs  feature_flags.rs                                │
└─────────────────────────────────────────────────────────────┘
```

**多角色隔离层**(贯穿 Layer 1~3):

`AppState.characters: Arc<RwLock<HashMap<String, CharacterInstance>>>` 是多角色架构的核心抽象。每个 `CharacterInstance` 持有该角色专属的 `Brain` / `MemoryManager` / `PsychologyManager` / `PersonaManager` / `PetController` / `ResourceManifest` / `RealtimeVoiceManager` / `think_lock` / `online` 状态,角色之间互不共享可变状态。命令层通过 `state.get_character(character_id.as_deref())?` 路由到目标角色实例(`None` 时返回 `active_character_id` 对应的角色)。`utils/path.rs` 提供按 `char_id` 分桶的持久化路径:`get_character_data_dir(char_id)` → `<user_data_dir>/characters/<char_id>/`(每角色独立 memory / persona / psychology / history / user_facts),`get_shared_data_dir()` → `<user_data_dir>/shared/`(跨角色共享数据)。

### 3.2 心理学因果链(五层)

```
Persona(长期人格)→ Needs(5 项需求 + set point)
        ↑                          ↓
   Homeostasis ← 事件 LLM 单次调用 → {appraisal, emotion_update, behavior_drive, reply}
                                    ↓
                               Appraisal(6 项评价)
                                    ↓
                               Emotion(7 项唯一情绪)
                                    ↓
                               Behavior Drive(8 项行为驱动)
                                    ↓
                               行为决策 + Mood + PetState(实时计算,仅 UI)
```

**核心原则**:
- Mood / PetState 不参与决策,仅 UI 展示
- Appraisal 是 Emotion 的前置(事件不直接产生情绪)
- Homeostasis 让所有维度围绕 set point 自动调节
- Behavior Drive 混合模式:对话轮 LLM 决策,主动 tick 规则决策
- LLM 一次调用产出全部心理状态
- `EmotionLabel`(7 项)是系统唯一情绪枚举

### 3.3 对话处理流水线

```
用户输入 → ConversationManager.start_or_continue("user", char_id)
  → Brain.think(message, stream)
  → Pipeline 14 Step 执行（QueryRewrite ∥ FastSemantic 并行）
    PreProcessing → UserMemorySaving → [QueryRewrite ∥ FastSemantic] → MemoryRetrieval
    → PromptBuilding(八层意识模型 + Response Decision)
    → WebContextDecision → Generation(LLM 返回 text + response_mode)
    → ResponseParsing(解析 response_mode,非 speak 清空 text)
    → Validation(空文本/长度截断/空白清理 + 注入 router 时轻量幻觉检测)
    → ExpressionMotion → PsychologyInsight → MoodUpdate → MemorySaving
  → 会话回顾注入(session_compressor 从 TimeStampedMemory 提取摘要到 messages 头部)
  → ConversationManager.update_after_round(更新 Energy/Novelty/Continuation)
  → detect_close_reason(关键词检测命中 → close_with_reason + seal_episode)
  → Provider 路由(LLM 调用,支持原生 Function Calling)
  → 工具执行(权限 + 沙箱 + 并发)
  → PsychologyManager.apply_llm_output(Appraisal→Emotion→Drive)
  → 记忆写入(LLM 增强:写入时抽取分类/元数据,写入后 add_memory_to_session)
  → 前端 emit(chat:assistant_message)
```

### 3.4 会话生命周期状态机(贯穿所有对话路径)

`conversation/` 模块把 User↔Agent 与 Agent↔Agent 统一建模为有生命周期的会话对象,是整个系统的"交通规则":

```
Created → Active → Cooling → Closed
              ↑       │
              └───────┘
              抢救(continuation_score ≥ 0.80)
```

- **Active → Cooling**:Ignore 模式直接进 Cooling;或 `continuation_score < 0.30 || energy < 0.25 || novelty < 0.15`
- **Cooling → Active**:30 秒窗口内收到高分新消息(continuation_score ≥ 0.80)抢救
- **Cooling → Closed**:超过 30 秒自动关闭(`sweep_cooling`,挂在 proactive_tick)
- **Closed → 新会话**:60 秒创建冷却后允许新建;用户主动发消息走 `force_new_session` 绕过冷却

**CloseReason 8 种**:Natural / GoodNight / GoodBye / NoResponse / Interrupted / Timeout / Conflict / SwitchTopic
- 关键词检测(GoodNight/GoodBye/Interrupted)→ 立即 `close_with_reason` + `seal_episode`
- `on_ignored` → `close(NoResponse)`
- 用户 30 分钟无响应 → `sweep_user_session_timeouts` → `close(Timeout)`
- GoodNight/NoResponse/Timeout 时 proactive 跳过主动搭话

**ResponseMode 4 种**:speak / non_verbal / internal / ignore
- LLM 在一次调用里同时返回,避免每条消息都触发完整 LLM 文本回复
- 跨角色路径冷却中返回 `CrossCharacterReply{response_mode:"ignore"}` 不调 LLM
- 主对话路径用户发"嗯/哦"时 LLM 可选 non_verbal(只做动作不说话)

---

## 四、项目结构

```
vivian-rs/
├── src/                          # React 前端
│   ├── components/               # 21 个 UI 组件
│   ├── controllers/              # 6 个控制器(Chat/Bubble/Stream/TtsStreamQueue/Lifecycle)
│   ├── hooks/                    # 5 个 Hooks(useTauriCommands/useHiding/useSmartPositioning 等)
│   ├── stores/                   # Zustand 全局状态
│   ├── i18n/                     # i18next 配置(zh-CN / en / ja)
│   ├── types/                    # 与 Rust 后端对齐的 TS 类型
│   ├── utils/                    # Live2DLipsync 等工具
│   ├── styles/                   # 全局样式
│   ├── App.tsx                   # 主窗口根组件
│   └── main.tsx                  # 应用入口(按 ?view= 参数分发子窗口)
│
├── src-tauri/                    # Rust 后端
│   ├── src/
│   │   ├── brain/                # 大脑核心(18 个子系统编排,含 Focus 模式 + 工具标记过滤 + 活动检测)
│   │   ├── pipeline/             # LangChain 风格 Runnable 流水线(14 Step + 4 Advisor,含 InlineTagScanner + ValidationRunnable + ParallelStep 并行容器) + 死循环检测 + 多级压缩 + 压缩提醒 + Prompt 模板引擎
│   │   ├── conversation/         # 会话生命周期(状态机 + CloseReason + ResponseMode + Episode 联动) + 对话完整性修复
│   │   ├── memory/               # 三层记忆(35 文件,含证据系统 + 事件溯源 + 统一事件账本 + 记忆整合软归档 + 会话压缩)
│   │   ├── providers/            # 9 种 LLM Provider + 路由矩阵
│   │   ├── tools/                # 增强工具系统(37 文件)
│   │   ├── psychology/           # 五层心理架构 + 关系系统 + 人格↔表情双向同步(16 文件)
│   │   ├── emotion/              # LLM 情绪分类 + 映射 + 嵌入即时分类
│   │   ├── proactive/            # 主动对话(20 文件,含多消息/tick + 触发器统一评估)
│   │   ├── world/                # 真实世界感知（时间/天气/音量/媒体/前台窗口/网络/地理位置/世界事件，14 文件）
│   │   ├── persona/              # 人格引擎(8 文件,模块化人设渲染 + worldbook 按需触发 + 场景选择 + 人格↔表情双向同步)
│   │   ├── dialogue/             # 对话历史 + session_id 字段 + 渠道隔离
│   │   ├── engine/               # Live2D 引擎(动画/表情/状态机/auto_trigger 自动表情触发)
│   │   ├── speech/               # ASR(4 引擎) + 多后端 TTS(6 后端)(22 文件)
│   │   ├── network/              # HTTP 客户端 + 代理 + 重试 + 联网搜索
│   │   ├── diary/                # 日记系统 + LLM 智能生成
│   │   ├── config/               # 配置管理(含路由矩阵 + WorldConfig)
│   │   ├── commands/             # 27 个 Tauri 命令文件(219 个命令)
│   │   ├── resilience/           # 熔断器
│   │   ├── i18n/                 # Rust 端 i18n
│   │   ├── types/               # 响应类型 + 多模态消息
│   │   ├── utils/                # 环境感知 + 路径工具 + Token 估算
│   │   ├── hooks/                # PreToolUse / PostToolUse Hook 系统（配置 / 事件 / 执行器）
│   │   ├── feature_flags.rs      # 17 个功能开关
│   │   ├── metrics.rs            # 性能指标(80+ 指标名)
│   │   ├── messages.rs           # 多模态消息系统
│   │   ├── error.rs              # VivianError(15 种变体)
│   │   ├── state.rs              # AppState
│   │   ├── pet_controller.rs     # 桌宠控制器
│   │   ├── character_behavior.rs # 角色行为参数 + 六策略去同步配置(按 char_id 索引)
│   │   ├── lib.rs                # run() 入口
│   │   └── main.rs               # main 入口
│   ├── prompts/                  # 人格与场景定义(模块化结构,双角色独立)
│   │   ├── characters/           # 角色定义(vivian/ 与 nana/ 各 8 个文件)
│   │   │   ├── identity.md       # 核心身份锚点
│   │   │   ├── personality.md    # 场景化人格(触发→反应行为脚本)
│   │   │   ├── speech.md         # 说话节奏/语气/口头禅/禁用模式
│   │   │   ├── examples.md       # 角色专属 few-shot 示例
│   │   │   ├── background.md     # 背景设定
│   │   │   ├── interests.md      # 兴趣爱好
│   │   │   ├── relationships.md  # 关系设定
│   │   │   └── appearance.md     # 外观描述
│   │   ├── framework/            # 通用框架规则(7 个文件,所有角色共享)
│   │   │   ├── chat_style.md / address_rules.md / conversation_rhythm.md
│   │   │   ├── session_rules.md / speaker_prefix.md / output_format.md / safety.md
│   │   ├── styles/               # 5 种说话风格预设
│   │   ├── worldbook/            # 3 类背景知识触发
│   │   └── system_prompt.tera    # Tera 模板入口
│   ├── capabilities/             # Tauri 权限配置
│   ├── resources/                # 运行时资源
│   ├── icons/                    # 应用图标
│   ├── Cargo.toml
│   └── build.rs
│
├── public/                       # 静态资源
│   ├── Nana/                     # Nana 角色资源(nana.model3.json + 11 个表情 + model_manifest.json + nana.vtube.json)
│   └── Vivian/                   # Vivian 角色资源(Live2D 模型 moc3 + physics + 贴图 + 表情)
│
├── package.json
├── README.md
├── CONTRIBUTING.md
└── CODE_WIKI.md                  # 本文档
```

---

## 五、后端 Rust 模块详解

### 5.1 入口与编排层

#### [main.rs](file:///g:/vivian-rs/src-tauri/src/main.rs)

```rust
#![windows_subsystem = "windows"]  // 防止 release 模式弹出控制台
fn main() { vivian_lib::run(); }
```

#### [lib.rs](file:///g:/vivian-rs/src-tauri/src/lib.rs) — `run()` 入口

声明 35 个顶层模块,核心职责:

1. `init_logging()` — 按日生成 `vivian_YYYY-MM-DD.log`,启动时清理保留 7 天
2. 创建 `AppState` 与 `LipsyncRuntime`
3. 注册 6 个 Tauri 插件 + 219 个 `#[tauri::command]` 处理器
4. `setup` 阶段:
   - 初始化系统托盘
   - 启动 Live2D 引擎状态机
   - 注入 AppHandle 到 ToolSystem / todo_tools / pet_tools
   - 加载持久化待办列表
   - 启动 ASR 事件转发器
   - 注册语音输入快捷键
   - 异步初始化 Brain / ModelRouter / MemoryManager(完成后 emit `app:ready`)
   - 启动时自动定位(若世界感知启用且未配置经纬度)
5. `RunEvent::ExitRequested` 时强制 flush 记忆脏数据(带 3 秒超时 + `yield_now`,防止持久化阻塞过久导致系统退出卡死)

#### [state.rs](file:///g:/vivian-rs/src-tauri/src/state.rs) — `AppState` 多角色状态

```rust
pub struct CharacterInstance {
    pub id: String,
    pub name: String,
    pub brain: Brain,
    pub pet_controller: Arc<PetController>,
    pub manifest: Arc<ResourceManifest>,
    pub realtime_voice: Arc<RealtimeVoiceManager>,
    pub online: Arc<RwLock<bool>>,
    pub think_lock: Arc<tokio::sync::Mutex<()>>,  // 串行化该角色的 brain.think
}

pub struct AppState {
    pub config: Arc<RwLock<ConfigManager>>,
    pub characters: Arc<RwLock<HashMap<String, CharacterInstance>>>,
    pub active_character_id: Arc<RwLock<String>>,
    pub model_router: Arc<RwLock<Option<ModelRouter>>>,
    pub tool_system: Arc<RwLock<ToolSystem>>,
    pub generation_cancel: Arc<RwLock<bool>>,
    pub asr: AsrManager,
    pub scheduler: Arc<Scheduler>,
    pub voice_shortcut: parking_lot::Mutex<String>,
    pub mcp_manager: Arc<McpManager>,
}
```

**多角色路由**:`AppState::get_character(character_id: Option<&str>) -> Result<CharacterInstance, String>` — `Some(id)` 返回指定角色,`None` 返回 `active_character_id` 对应的角色;未找到返回错误字符串。命令层统一通过 `state.get_character(character_id.as_deref())?` 获取目标角色实例。

**`initialize()`** 方法异步初始化:
1. 创建 ModelRouter
2. 创建每个角色的 MemoryManager(按 char_id 构造,路径分桶到 `characters/<char_id>/memory/`)
3. 注册内置工具
4. 连接所有 MCP server(非阻塞)
5. 注入 Scheduler 到 todo_tools
6. 启动 Scheduler 后台调度循环
7. 按角色配置(`Nana` / `Vivian`)依次创建 `CharacterInstance`:为每个角色独立加载 `ResourceManifest` / `PetController` / `RealtimeVoiceManager`,构建 Brain(持有 Pipeline / Provider / 角色 MemoryManager / PsychologyManager / PersonaManager,Brain 内置 `char_id` 字段标识所属角色),写入 `characters` HashMap 并注册到 `character_registry`(全局按 char_id 索引的 `HashMap<String, Arc<CharacterResources>>`,供工具层通过 `get_memory_manager(char_id)` / `get_manifest(char_id)` 等查询),默认首角色设为 `active_character_id`
8. 初始化 `CrossCharacterBus::initialize(handle, state)` 注入 AppHandle + AppState(详见 5.20)

---

### 5.2 brain/ 大脑核心

**架构定位**:系统的核心调度层,包含 Brain 主控、BrainChatChain 对话链、Scheduler 定时器、RateLimiter 限流器、Callbacks 回调。

#### 关键结构

- **`Brain`** — 主控器,持有 PetController、Pipeline、Provider、MemoryManager、PsychologyManager 等 Arc 引用
- **`BrainChatChain`** — 对话链,使用 `AIResponseGenerationRunnable + ResponseParsingRunnable`(非 GenerationStep)
- **`Scheduler`** — 定时任务调度器,支持 `Reminder` / `ToolCall` 两类任务,持久化到 `scheduled_tasks.json`。每个任务记录发起角色的 `char_id`;`ToolCall` 任务触发时不再"信任跳过",而是携带 `char_id` 走完整权限管线(`execute_tool_use`),无人值守场景下确认弹窗超时即返回 `PermissionRequired` 而不会挂死。`schedule_tool_call` 创建时按启动期注入的已注册工具名快照(`KNOWN_TOOLS`)预校验 `tool_name`,未注册工具直接拒绝建任务
- **`RateLimiter`** — Token bucket 限流器
- **`InterruptionController`** — 中断控制(用户打断生成)

#### 核心方法

```rust
Brain::new(config, router, memory, manifest: Arc<ResourceManifest>, char_id: &str) -> VivianResult<Self>
Brain::new_with_pet_controller(config, router, memory, pet_controller, manifest: Arc<ResourceManifest>, char_id: &str) -> VivianResult<Self>
Brain::think(&self, user_input: &str) -> impl Stream<...>         // 非流式
Brain::think_stream(&self, user_input: &str) -> impl Stream<...>  // 流式
Brain::generate_startup_greeting(&self) -> Option<String>         // 启动问候(LLM 生成,失败返回 None)
```

> 多角色改动:`Brain::new` / `Brain::new_with_pet_controller` 均新增 `manifest: Arc<ResourceManifest>` 与 `char_id: &str` 参数,以支持角色级资源加载与持久化分桶;`BrainChatChain::new` 同步增加 `manifest` 与 `char_id: &str` 参数(后续又新增 `fast_semantic: Option<Arc<FastSemanticAnalyzer>>` 参数,用于在流水线中与 QueryRewrite 并行执行快速语义感知),char_id 注入 `ToolUseContext` 供工具系统按角色路由,并传入 `PromptBuildingStep::with_char_id` 用于从 `character_registry` 查询当前角色 manifest 生成表情/动作清单。

**`generate_startup_greeting` 启动问候增强**:首次见面判定基于 `non_seed_count() == 0`(排除种子记忆)。生成时注入三类上下文让问候有活人感:
- **当前情绪状态**:读取 `psychology.emotion().dominant()`(主导情绪 + 值),情绪跨会话保留(上次对话结束的情绪带到这次开场),让问候语气与状态衔接
- **天气与时间**:读取 `world_provider.snapshot()` 获取 `local_time` + `weather.description` + `temperature`,提供情境感
- **角色专属心境提示**(首次见面):Vivian 注入"好奇但警惕,不希望被叫主人"的心境;Nana 注入"平静不急,希望相处舒服"的心境;让 LLM 带着具体情绪状态生成开场白而非机械模板

#### 子系统清单(18 文件)

| 文件 | 职责 |
|------|------|
| `brain.rs` | Brain 主控,持有所有 Arc 引用(含 `focus_state`),协调 think 流程;proactive_tick 期间调用 idle_cooldown 让 Focus 电荷衰减 |
| `chat_chain.rs` | BrainChatChain,组合 GenerationRunnable + ParsingRunnable;`ainvoke` 拆分为三个职责明确的方法——`prepare_pipeline_state`(初始化 PipelineState,不含 FastSemantic——已迁移至 `FastSemanticStep` 与 QueryRewrite 并行执行)、`execute_pipeline_and_build_response`(执行流水线+构造 AiResponse)、`ainvoke`(后处理记忆操作);每轮调用 `focus_state.update()` 驱动认知模式三态切换 |
| `scheduler.rs` | 定时任务调度(Reminder / ToolCall) |
| `callbacks.rs` | 流式回调(on_chunk / on_meta / on_done) |
| `command_handler.rs` | 命令处理(解析 LLM 返回的命令意图) |
| `json_parser.rs` | LLM JSON 输出解析(含 user_emotion/ai_emotion/motion 等,集成 ToolLeakFilter 过滤工具标记泄露) |
| `rate_limiter.rs` | Token bucket 限流 |
| `interruption_controller.rs` | 用户中断生成控制 |
| `subagent_context.rs` | 子代理上下文 |
| `augment_reply_service.rs` | 回复增强服务(已启用:Brain::build 初始化注入 memory+router,BrainChatChain::ainvoke 回复后 fire-and-forget 调度;slow 检索 Hybrid 策略 4s 超时;记忆按 importance 升序排序取前 5 条;冷却 120s + pending 上限 2 + 3-gram Jaccard > 0.55 防复读;schedule 方法接收 char_id 参数区分角色人设) |
| `computer_control.rs` | 电脑控制(屏幕感知 + 输入模拟) |
| `control_action_executor.rs` | 控制动作执行器 |
| `smart_app_classifier.rs` | 智能应用分类(用户作息学习) |
| `focus_mode.rs` | 凝神/专注模式状态机(漏桶累积器 + 迟滞设计,Regular/Focus/TrueName 三态;compute_focus_score 信号评分:输入长度 + 问号 + 复杂度关键词 + 用户情绪) |
| `tool_leak_filter.rs` | 工具调用标记泄露过滤(跨 chunk 状态机,过滤 `<tool_call>` / `<seed:tool_call>` / `<function>` 三种泄露形态) |

---

### 5.3 pipeline/ LangChain 风格流水线

**架构定位**:LangChain 风格的可组合 Runnable 流水线 + Advisor 拦截器链。

#### Runnable trait

```rust
pub trait Runnable: Send + Sync {
    async fn ainvoke(&self, input: Value) -> Result<Value>;
    async fn atransform(&self, input_stream) -> Stream<Value>;
    async fn astream_events(&self, input) -> Stream<StreamEvent>;
}
```

#### 14 个 Step(按执行顺序)

```
PreProcessing → UserMemorySaving → [QueryRewrite ∥ FastSemantic] → MemoryRetrieval
→ PromptBuilding(八层意识模型 + Response Decision) → WebContextDecision → Generation
→ ResponseParsing(解析 response_mode,非 speak 清空 text) → Validation(空文本/截断/空白清理 + 注入 router 时轻量幻觉检测)
→ ExpressionMotion → PsychologyInsight → MoodUpdate → MemorySaving
```

> QueryRewrite 与 FastSemantic 通过 `ParallelStep` 容器并行执行（`tokio::join!`），耗时 = max(LLM 改写, 嵌入分类) 而非 sum，缩短用户等待时间。

| Step | 文件 | 职责 |
|------|------|------|
| PreProcessing | `steps/pre_processing.rs` | 输入预处理 |
| UserMemorySaving | `steps/memory.rs` | 用户消息写入记忆 |
| QueryRewrite | `steps/query_rewrite.rs` | 查询重写(LRU 缓存,与 FastSemantic 并行执行)+ FLARE 式按需检索判断(`should_skip_retrieval` 启发式:闲聊/问候/确认词/纯标点表情跳过检索,设置 `metadata.skip_memory_retrieval` 标志) |
| FastSemantic | `steps/fast_semantic_step.rs` | 快速语义感知(嵌入分类情绪/意图/话题,与 QueryRewrite 并行执行) |
| MemoryRetrieval | `steps/memory.rs` | 混合检索记忆 + 检索后 Verifier 过滤(注入 `ModelRouter`,>2 条时用小模型二分类过滤无关记忆)+ 低置信度 `[需验证]` 标记(combined_score/temporal_adjusted_score < 0.3) |
| PromptBuilding | `steps/prompt.rs` | 构建 system prompt(55 字段,不再注入 manifest_context) |
| WebContextDecision | `steps/web_context.rs` | 联网搜索决策 |
| Generation | `steps/generation.rs` | LLM 生成(AIResponseGenerationRunnable,只输出 text/intent/tool/arguments/control_actions) |
| ResponseParsing | `steps/generation.rs` | JSON 解析(ResponseParsingRunnable) |
| Validation | `steps/validation.rs` | 回复验证(空文本检测 + 500 字符句边界截断 + 空白清理;注入 `ModelRouter` 后启用轻量幻觉检测:记忆上下文非空且回复 ≥30 字符时用 `memory` 任务小模型检查是否与记忆矛盾,仅 warning 不修改回复,超时/失败跳过) |
| ExpressionMotion | `steps/expression_motion.rs` | 表情/动作选择(内联标签模式启用时跳过 LLM 调用,否则独立 LLM 推断) |
| PsychologyInsight | `steps/psychology_insight.rs` | 心理洞察提取 |
| MoodUpdate | `steps/mood.rs` | Mood 更新 |
| MemorySaving | `steps/memory.rs` | AI 消息写入记忆 |

#### 生成与提示词拆分原则

生成拆到子 LLM 的内容,对应的 prompt 指导内容也从主对话 prompt 移除:

- **主对话 LLM 输出精简**:`OUTPUT_FORMAT` 只保留 `text / intent / tool / arguments / control_actions`,不再要求 LLM 返回 `user_emotion` / `ai_emotion` / `appraisal` / `emotion_update` / `behavior_drive` 等心理字段(由独立调用推断)
- **manifest_context 移除**:表情/动作可用列表不再注入主对话 prompt。表情/动作有五类触发路径:(1) **LLM内联标签模式**(`config.inline_expression.enabled`)——主 LLM 在文本中嵌入 `<e name="happy" dur="3000"/>` / `<m name="wave"/>` / `<s name="sticker_id"/>` 标签,`InlineTagScanner` 流式扫描剥离标签并 emit `chat:inline_meta` 事件,前端即时驱动 Live2D,`ExpressionMotionRunnable` 跳过二次 LLM;(2) **LLM子调用模式**(默认)——`ExpressionMotionRunnable` 在 text 完成后独立调用 LLM 选择;(3) **嵌入即时反应**——`analyze_emotion_instant` 命令调用 `EmbeddingEmotionClassifier`(基于 `MemoryEmbeddingProvider`,预置 14 类情绪语料 210 条,Top-K=5 余弦相似度投票),在用户消息发送瞬间(Layer 1)与 AI 文本首段完成时(Layer 2)触发即时 FACS 反应,写入 Live2D `instant` 层(优先级 1.5),反思调用完成时由 `manual` 层接管并自动清除 `instant` 层;嵌入失败时弹 toast 报错,不降级到关键词分析;(4) **用户交互即时反馈**——`apply_user_interaction` 命令根据前端检测到的 10 种交互类型(click/drag/pet 等)直接查表 `manifest.interaction_map` 返回反馈,不调 LLM;(5) **自动规则触发**——`auto_expression_tick` 定时检查空闲阶段/心情持续/程序事件(`engine/auto_trigger.rs`),纯规则概率触发
- **control_actions 语义名**:`set_expression(name)` / `play_motion(name)` 的 `name` 接受语义名(happy / shy / wave / nod 等),后端通过 `ResourceManifest::normalize_expression` / `normalize_motion` 映射到实际 model3.json Name;`PetController::play_motion` 调用前先做 manifest 归一化
- **回复验证**:`ValidationRunnable` 在 ResponseParsing 之后、ExpressionMotion 之前执行:空文本检测(should_respond=true 但 text 为空时 warn)、500 字符句边界截断(在 。！？.!?\n 处断开)、连续空行折叠与首尾空白清理。注入 `ModelRouter` 后启用轻量幻觉检测:当记忆上下文非空且回复 ≥30 字符时,用 `memory` 任务小模型检查回复是否包含与记忆矛盾或编造的信息,输出 `OK` 或 `ISSUE: <描述>`,仅记录 warning 写入 `state.metadata["hallucination_check"]` 不修改回复;8 秒超时或 LLM 失败时跳过不阻塞主流程
- **流式期间表情节流**:三层时序隔离保证流式输出期间不触发中间表情抖动——(1) `StreamEmitter` 只推 `TextChunk` 事件,前端 `chat:chunk` 收到的是纯文本片段;(2) 前端 `isStreaming` 守卫在流式期间暂停 `mood_expression_tick`;(3) pipeline 严格串行,`ExpressionMotionRunnable` 在 `Generation` + `ResponseParsing` 完成后一次性调用,不会在流式过程中被多次触发
- **会话回顾注入**:Pipeline 执行前,`session_compressor` 从 `TimeStampedMemory.recent_summary()` 提取摘要,构造 `[CONVERSATION RECAP]` system 消息插入 `state.messages` 头部,让 LLM 感知此前对话概要

#### 4 个 Advisor 拦截器

| Advisor | 职责 |
|---------|------|
| `LoggingAdvisor` | 日志记录 |
| `RateLimitAdvisor` | 令牌桶限流 |
| `Re2Advisor` | 反思推理(Reflection) |
| `LoopDetectionAdvisor` | 循环检测 |

#### 装饰器

- `RunnableBranch` — 条件分支
- `RunnableRetry` — 重试
- `RunnableWithFallbacks` — 降级回退

#### StreamEvent 统一流式协议

`Text` / `Thinking` / `ToolCallDelta` / `Done` / `Error`

#### PipelineState

流水线状态容器,携带 55 个字段贯穿全链,在各 Step 间传递。

#### 关键文件

- [mod.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/mod.rs) — 模块声明
- [base.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/base.rs) — Runnable trait
- [state.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/state.rs) — PipelineState
- [advisor.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/advisor.rs) — Advisor trait + 4 实现
- [decorators.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/decorators.rs) — 装饰器
- [parsers.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/parsers.rs) — 解析器
- [context_compress.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/context_compress.rs) — 多级上下文压缩（Soft Trim → Group Drop → Reminder Inject）
- [doom_loop.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/doom_loop.rs) — 工具调用死循环检测（签名追踪 + 阈值打断）
- [compaction_reminder.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/compaction_reminder.rs) — 压缩后 Reminder 注入（活跃工具名 + 最后话题提取）
- [template_engine.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/template_engine.rs) — Prompt 模板引擎（Section Schema 单一真相源 + 逐 section 元数据）
- [inline_tag_scanner.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/inline_tag_scanner.rs) — 内联表情/动作/贴纸标签流式扫描器
- [steps/validation.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/steps/validation.rs) — 回复格式验证(空文本/截断/空白清理 + 注入 router 后的轻量幻觉检测)
- [prompt_modules.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/prompt_modules.rs) — Prompt 模块化构建(U型注意力布局 + Consciousness Assembler 分层意识模型 + 动态角色支持 + 行为化语音指南 + 中文内心反应 + 记忆块忠实度约束)
- [errors.rs](file:///g:/vivian-rs/src-tauri/src/pipeline/errors.rs) — 错误类型

#### Prompt 模块化构建(`prompt_modules.rs`)

**U 型注意力优化布局**(`PromptBuilder::build_prompt`):

利用 LLM 对 prompt 开头和结尾注意力更强的偏置(U 型注意力曲线),静态区按以下顺序排列:

```
<static>
[CHARACTER - EMBODY THIS]  ← 开头:人格核心最先入脑(首因效应)
Style / Relationship
[EXAMPLES - REFERENCE ONLY] ← 近因效应:生成前最后看到的风格参考
[FRAMEWORK - DO NOT EMBODY, JUST FOLLOW] ← 技术规则,不内化
[FORMAT SPEC - DO NOT EMBODY] ← 结尾:临出口提醒格式要求
</static>

---
## Right now, in this moment...  ← 自然过渡句替代硬编码边界

动态区(Consciousness Assembler 七层 + 尾部注入):
1. Current Mind (Belief/Goal/Attention + Working Memory + Self State + Emotion)
2. World Snapshot (环境上下文 / 用户在场+近期活动+观察 / 室友状态+认知印象 / 环境事件)
3. Social Relationship (关系认知事实 / 共享世界 / 社交状态)
4. Relevant Episode + Relationship Log (经历摘要 / 近期关系信号)
5. Memory (记忆上下文)
6. Tail Context (初见或记忆规则 / 用户事实 / 行为画像 / Worldbook)
7. Tail Guides (Channel Guide / Presence Guide / Inner Reaction[仅无当前念头时注入,近因效应] / Response Decision / Inline Tags / Tone Injection)
8. Available Tools (工具列表放最后,让 LLM 先进入意识状态再看可用工具)
# User Input (Task 层)
```

**核心设计原则**:

- **静态/动态分离**:静态内容(人格/框架/示例)用 `<static>` 标签包裹,提升云端 API 缓存命中率;动态内容(心智/世界/记忆)在后
- **动态边界弱化**:使用自然过渡句("Right now, in this moment...")替代 `__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__` 硬编码标记,减少提示词泄露风险
- **功能提示词动态化**:
  - `build_psychology_insight_prompt(char_name)` 替代硬编码 "Vivian"
  - `build_tool_minimal_identity(char_id)` 工具继续场景注入角色精简单人设
  - `build_cross_character_voice_guide(char_id)` 角色专属行为化语音指南
  - 日记/信念/思维合成/记忆提取等功能模块均使用角色名变量
- **行为化语音指南**:跨角色对话时注入具体行为约束("你说话比她快,句子更短,不要用她那种温温柔柔慢悠悠的语气"),替代非可执行的数值化标签(如 sass=0.65)
- **内心反应中文化+角色化**(`build_inner_reaction`):第一人称内心想法使用中文生成,按角色差异化(Vivian 直率吐槽 / Nana 温柔关心)
- **渠道风格分流**:`build_channel_style_guide(channel)` 根据 "direct"(面对面)/"wechat"(线上聊天)调整回复风格

**PromptParts 结构**(贯穿全链的提示词组装容器):

包含 30+ 可选字段,主要分组:
- 静态块:character_block / examples_block / style_block / relationship_section / style_preset_block
- 心智块:mind_section / working_memory_section / self_state_section / emotion_context / inner_reaction
- 世界块:environment_context / user_entity_section / roommate_status / roommate_cognitive_section / environment_events
- 观察块:world_observations / activity_brief
- 经历块:episode_section / relationship_log_section
- 画像块:user_facts_section / dynamic_behavior_section / relationship_facts_section / shared_world_section / social_state_section
- 其他:memory_text / worldbook_block / tools / inline_tag_section / channel / presence_state / cross_character_mode / char_id

---

### 5.4 memory/ 三层记忆系统

**架构定位**:三层记忆(ShortTerm / MidTerm / LongTerm)+ 混合检索 + 巩固流水线 + LLM 增强 + 多角色按 char_id 物理隔离。并发模型:内部状态统一使用 `parking_lot::Mutex` 保护(不中毒、不持有 guard 跨 await);远程嵌入服务通过 `Semaphore` 限流防止外部 API 过载;核心数据结构(`MemoryVectorStore`)的写操作返回 `VivianResult<()>` 错误向上传播。

#### 多角色隔离机制

- **MemoryManager 按 char_id 构造**:每个角色拥有独立的 `MemoryManager` 实例,存储路径为 `<user_data_dir>/characters/<char_id>/memory/`,物理隔离互不污染
- **工具层路由**:`ToolUseContext.char_id` 由 Brain 在对话链执行时注入,记忆工具(ReadMemoryTool / WriteMemoryTool 等 13 个)与关系工具通过 `get_manager_for_context(context)` 从 `character_registry` 按 char_id 查询对应 MemoryManager;char_id 为空时返回 None 并 warn,不再有全局兜底
- **MemoryService 无状态**:`MemoryService` 删除全局单例,所有业务方法(write_memory / read_memory / delete_memory / log_daily_diary 等)改为接收 `&MemoryManager` + `&str char_id` 参数,由调用方负责路由
- **Tauri 命令层路由**:6 个记忆命令(get_memories / search_memories / clear_all_memories / get_memory_summary / get_recent_interactions / update_memory_importance)均接收 `character_id: Option<String>` 参数,通过 `state.get_character(character_id.as_deref())?` 路由到对应角色实例
- **全局静态变量已移除**:MEMORY_MANAGER / VERIFIER_LLM / PSYCHOLOGY_MANAGER 三个全局静态兜底已删除,state.rs 中第一个角色注入全局变量的兼容逻辑已移除,所有路径强制走 char_id 路由

#### 三层记忆结构

- **ShortTerm**(Turn 级) — 当前对话上下文
- **MidTerm**(Session 级 SessionSummary) — 会话摘要
- **LongTerm**(摘要级) — 巩固流水线产出

#### MemoryManager 核心方法

```rust
async fn add_memory(&self, content, mem_type, importance, tags) -> Result<MemoryItem>
async fn add_memory_enriched(&self, content, mem_type, importance, tags, meta: EnrichedMeta) -> Result<MemoryItem>
async fn add_knowledge_document(&self, title, content, tags, source, ttl_days: Option<i64>) -> Result<MemoryItem>
async fn search(&self, query: &str, strategy: RetrievalStrategy::Auto) -> Vec<MemoryItem>
async fn get_all_memories(&self) -> Result<Vec<MemoryItem>>
async fn delete_memory(&self, id: &str) -> Result<()>
fn flush(&self) -> Result<()>  // 5s 节流落盘
fn set_session_id(&self, session_id: Option<String>)  // Brain 在 think 前设置
fn get_session_id(&self) -> Option<String>
fn push_topic_hint(&self, query: &str)  // 对话中 web_search 搜索关键词记录
fn drain_topic_hints(&self) -> Vec<String>  // 后台知识采集取出并清空(24h 过期)
```

`add_memory` 是 `add_memory_with_embedding_text(content, mem_type, importance, tags, embedding_text=None)` 的 wrapper。`add_memory_enriched` 在 LLM 抽取出 `meta.summary` 时将其作为 embedding 源传入,使长文本(content > 200 字)记忆的向量索引和 MemoryVector.content 字段使用摘要而非原文,避免向量稀释。embedding_text=None 时降级为原文嵌入。所有写入路径最终汇入 `add_memory_inner`,该函数在 metadata 合并后自动注入 `metadata["session_id"]`（`entry().or_insert` 语义,不覆盖调用方显式提供的值）,使前端图谱可按后端真实会话边界分组。

`add_knowledge_document` 以 `MemoryType::Knowledge` 入库,title 与 content 分开存于 metadata,强制建向量索引(不依赖 `should_index` 准入)。`ttl_days` 参数控制知识时效:`Some(7)` → 7 天后过期(short),`Some(30)` → 30 天后过期(mid),`Some(-1)` → 永不过期(long),`None` → 不写入 TTL 字段(兼容手动添加/迁移)。TTL 写入 `metadata.expires_at`(绝对时间戳)和 `metadata.ttl_days`(天数)。

`push_topic_hint` / `drain_topic_hints` 实现对话搜索关键词到后台知识采集的传递:对话中 `web_search` 工具搜索成功后调用 `push_topic_hint(query)` 记录关键词(去重、限 20 条),后台知识采集任务启动时通过 `drain_topic_hints()` 取出并清空(自动过滤超过 24h 的过期提示),作为优先主题供采集使用。

#### 混合检索流程

1. BM25(jieba 中文分词) + 向量搜索(sqlite-vec / Hashing 256 维离线 / OpenAI 兼容在线)并行
2. RRF 融合(K=60)
3. IVF 倒排索引加速(向量数量 > 500 时自动构建 k-means 聚类,查询时只扫描 nprobe 个最近聚类)

#### 五因子检索评分

| 因子 | 默认权重 | 说明 |
|------|---------|------|
| recency | 0.25 | 时间衰减(recency_tau_hours=24.0) |
| relevance | 0.40 | 相关度(BM25 + 向量) |
| importance | 0.15 | 重要性(写入时 LLM 抽取) |
| hook_boost | 0.10 | 钩子加成 |
| need_sim | 0.10 | 需求相似度 |

`min_score = 0.15`,min-max 归一化

#### 知识时效管理(三层)

后台知识采集写入的 Knowledge 类型记忆拥有独立的时效管理机制,分三层:

1. **入库 TTL 分级**(`add_knowledge_document`):LLM 在总结知识时判断时效类别并输出标签——`[short]`(7天,新闻/热搜/赛事) / `[mid]`(30天,技术动态/产品发布) / `[long]`(永不过期,百科/历史/科学原理)。TTL 写入 `metadata.expires_at`(绝对时间戳)和 `metadata.ttl_days`(天数)
2. **检索时间衰减**(`strategy.rs::apply_temporal_decay`):检索结果中 Knowledge 类型记忆的 `combined_score` 乘以时间衰减因子 `recency_factor = exp(-age_days / 30)`(30 天半衰期);已过 `expires_at` 的知识额外乘以 0.3 惩罚系数(降权但不硬删);重新排序后注入 `recency_factor` / `temporal_adjusted_score` / `expired` 到 metadata。三条检索路径(AutoStrategy 档位 1 / VectorStrategy / HybridStrategy)均施加
3. **过期知识刷新**(`background_tasks.rs::collect_expired_knowledge_topics`):后台知识采集任务启动时扫描已过 TTL 的知识文档,删除旧文档(含向量索引)并提取标题作为刷新主题,优先级高于对话搜索提示和 LLM 自主决策

#### 后台知识采集与分享机制(`presence/background_tasks.rs`)

Busy 状态下 `spawn_knowledge_acquisition` 异步执行采集任务,设计上避免机械触发和频繁推送链接:

- **采集冷却**(`proactive/mod.rs::is_knowledge_acquisition_in_cooldown`):距上次采集不足 30 分钟则跳过整个采集任务,避免每次进入 Busy 都触发检索
- **主题来源优先级**:过期知识刷新 > 对话搜索提示(`drain_topic_hints`)> LLM 自主决策(`decide_topics_with_intent`),三者合并去重后截断至 `MAX_TOPICS_PER_ACQUISITION`(3 个)
- **LLM 自主决策主题的锚点**(`decide_topics_with_intent`):不再用固定 query 检索记忆,改用「最近 3 条 SessionSummary 话题总结(`recent_by_type(SessionSummary, 3)`)+ 最近 5 条短期记忆(`recent_by_tags(&["short_term","casual_conversation"], 5)`)」作为 LLM 上下文。SessionSummary 是 Stage 1 提炼过的话题级压缩,比单条对话消息更稳定地代表用户兴趣。LLM 可返回 `[none]` 表示本次无明确兴趣锚点,跳过采集——像人一样没事做时不必硬找事做
- **分享意图克制**(`decide_topics_with_intent`):主题分两类——`[internalize]`(内化为知识,常态)与 `[share:理由]`(分享链接给用户,少数情况)。`[share]` 必须带冒号+理由前缀,无理由自动降级为 internalize;一次最多 1 个 share,多余的降级为 internalize,避免给用户连续推送链接
- **分享冷却**(`proactive/mod.rs::is_knowledge_share_in_cooldown`):距上次链接分享不足 30 分钟则跳过本次分享,避免频繁推送链接给用户
- **分享路径**:`[share:理由]` 主题经搜索 → `prepare_share_payload` 选最佳链接 + 生成 follow_up → `share_link_to_wechat` 通过微信面板立即发送

#### RAG 幻觉抑制(五层防御)

贯穿检索-生成-验证全链路的幻觉抑制机制,降低记忆驱动的幻觉风险。设计原则:所有 LLM 调用均为可选增强,失败时降级为原行为不阻塞主流程。

1. **Prompt 层忠实度约束**(`prompt_modules.rs::build_memory_block`):在记忆块文本末尾追加忠实度指令(中/英/日三语),提示 LLM:(a) 记忆可能包含过时信息,标注"可能已过时"的尤甚;(b) 记忆与用户刚才说的话矛盾时以用户为准;(c) 不要基于记忆编造用户没提过的细节;(d) 标注 `[需验证]` 的记忆置信度较低,谨慎参考
2. **检索结果置信度标记**(`steps/memory.rs`):格式化记忆条目时,读取 `metadata.temporal_adjusted_score` 或 `metadata.combined_score`,低于 0.3 阈值的条目追加 ` [需验证]` 后缀,让 LLM 在生成时对低置信度记忆保持谨慎
3. **主对话路径接入 Verifier**(`steps/memory.rs::MemoryRetrievalStep` + `brain/chat_chain.rs`):`MemoryRetrievalStep` 通过 `with_router(router)` 注入 `ModelRouter`,在 MemoryFilter 过滤后对 `filtered_items` 调用 `memory::verifier::verify_retrieval`。检索结果 >2 条时用 `memory` 任务小模型做二分类(能/不能回答用户问题),过滤掉无关噪声记忆;记忆数 ≤ 2 时自动跳过(开销不值得);LLM 不可用或响应无法解析时降级为全部保留。`chat_chain.rs` 在构建 `MemoryRetrievalStep` 时调用 `.with_router(router.clone())` 注入
4. **生成后幻觉检测**(`steps/validation.rs::ValidationRunnable` + `brain/chat_chain.rs`):`ValidationRunnable` 通过 `with_router(router)` 注入 `ModelRouter`,新增 `check_faithfulness` 方法。在空白清理和长度截断之后,当 `state.memory_text` 非空且 `state.text` ≥30 字符时触发:用 `memory` 任务小模型检查回复是否包含与记忆矛盾或编造的信息,输出 `OK` 或 `ISSUE: <描述>`;8 秒超时。结果写入 `state.metadata["hallucination_check"]`(`status: ok/flagged/skipped` + `issue`),仅记录 warning 不修改回复。`chat_chain.rs` 将 `ValidationRunnable::new()` 改为 `ValidationRunnable::with_router(router.clone())`
5. **按需检索 FLARE 式**(`steps/query_rewrite.rs` + `steps/memory.rs`):`QueryRewriteStep` 内置 `should_skip_retrieval(input)` 启发式函数(零 LLM 调用),对以下输入直接跳过查询重写和记忆检索:(a) 极短输入(≤6 字符)匹配常见闲聊填充词/问候语/确认词(中英日三语,如"嗯"/"你好"/"好的"/"晚安"/"ok"/"bye"等);(b) 纯标点/表情符号(≤10 字符且无字母数字)。命中时设置 `metadata.skip_memory_retrieval=true` + `metadata.skip_retrieval_reason=<reason>` 并提前返回;`MemoryRetrievalStep::ainvoke` 开头读取该标志,为 true 时设置 `metadata.memory_retrieval_skipped=true` 直接返回跳过整个检索步骤,省去向量检索和 Verifier 开销

#### 检索后验证(`verifier.rs`)

`memory::verifier` 模块提供检索后验证能力,用小模型判断检索结果能否回答用户问题,过滤噪声记忆避免污染 prompt 上下文。

**核心抽象**:
- `VerifierLlmClient` trait:`async fn verify(&self, prompt: &str) -> VivianResult<String>`,为 `ModelRouter` 实现该 trait(使用 `memory` 任务类型调用小模型)
- `VerificationResult`:`verified_indices: Vec<usize>`(通过验证的记忆索引,0-based)+ `skipped: bool`(是否跳过 LLM 验证)

**`verify_retrieval(memories, query, llm)` 流程**:
1. 记忆为空 → 返回空结果(skipped=true)
2. 记忆数 ≤ 2 → 全部保留(skipped=true,开销不值得)
3. LLM 客户端为 None → 全部保留(skipped=true,降级)
4. 构造验证 prompt(`build_verify_prompt`):用户问题 + 候选记忆列表(每条带编号/时间/类型/重要性/内容,截断 400 字符,附带 description),要求小模型输出相关记忆编号(如 `1,3,5`)或 `none`
5. 调用 LLM,解析响应(`parse_verify_response`):支持逗号/空格/换行分隔、`[1]` 括号形式、过滤超范围编号;返回空或无法解析时降级为全部保留

**集成点**:`MemoryRetrievalStep::ainvoke` 在 MemoryFilter 过滤后、Attention 重排序前调用,过滤后的 `filtered_items` 进入后续流程

#### 写入时 LLM 增强

`llm_enricher.rs` 在写入时抽取:
- description / keywords / importance / semantic_type
- 分类结果存储在 `memory.metadata["classification"]`
- summary(仅当 content > 200 字时):LLM 顺带输出 ≤100 字摘要,用于向量检索。`build_enrich_prompt` 根据 content 字符数动态决定是否在 prompt 中要求 summary 字段;`EnrichedMeta.summary: Option<String>` 持有结果,空字符串自动归一为 None。短文本(content ≤ 200 字)不需要 summary,直接用原文做 embedding

#### 巩固流水线

- **Stage 1**:摘要生成。筛选 ShortTerm 时排除 `InnerMonologue`(角色主观内心独白)与 `ObservationNote`(旁观记忆,perspective=observer),避免与对话事实混合摘要导致语义失真
- **Stage 2**:六路并行反思(含 relationship signals + L1 近期状态)。新增**证据主动再评估**(`reassess_evidence_for_new_fact`):抽取新事实后,对相似旧记忆调用 `detect_local_contradiction` 检测局部矛盾,命中则对旧记忆应用 `Negates` 证据信号削弱,避免矛盾记忆共存。**触发条件放松**:热度阈值降至 2.5(新摘要 H≈1.05,被检索 3 次即可触发),并增加 24h 兜底触发——SessionSummary 创建满 24h 且 `visit_count=0` 时强制触发,避免低频访问的摘要永远停留在 MidTerm 导致长期记忆稀薄
- **Stage 3**:聚类洞察
- **索引漂移检测**(`check_index_drift_and_rebuild`):每次 `run()` 末尾检测向量索引与可索引记忆条目的数量比,偏离 [0.8, 1.2] 区间时触发全量重建,防止长期增删导致索引与数据脱节

#### 用户事实画像 L0/L0.5/L1/L2

- **L0 稳定身份** — 姓名/年龄/性别/职业/所在地,`UserFact.is_pinned` 锁定保护防止自动覆盖
- **L0.5 结构化偏好** — 生日/作息/常用网站/喜欢的游戏/兴趣爱好,支持手动编辑与锁定,字段定义见 `UserFactType` 枚举
- **L1 近期状态** — goals/projects/preferences,`round_count` 衰减,第六路并行 Stage 2
- **L2 长期事实** — `custom_facts` 去重(包含关系判断)

存储路径:`characters/<char_id>/user_facts.json`(按角色隔离,不同角色对用户的认知可差异化)。前端通过 `commands/user_facts.rs` 暴露的 5 个 Tauri 命令(`get_user_facts` / `set_user_fact` / `pin_user_fact` / `delete_user_fact` / `get_user_fact_types`)提供 CRUD 接口,`UserProfilePage` 组件按角色切换展示四层结构化数据。

#### 记忆冲突检测 3 阶段(`conflict.rs`)

1. 词法矛盾(语义相似度检测)
2. 6 维评分
3. 动作决策:KeepBoth / ReplaceOld / MergeSupersede / QueueLlm

**QueueLlm 队列消费者**:`QueueLlm` 决策不再断层——新记忆写入时若与旧记忆语义相似且判定需 LLM 仲裁,封装为 `PendingConflict` 推入 `MemoryManager::pending_conflicts` 队列持久化。`CognitiveTickRunner::phase_self_update` 每 5 分钟节流调用 `process_pending_conflicts`,批量取出最多 5 条(指数退避重试,最多 3 次),由 `DefaultConflictArbiter`(基于 `ModelRouter` 的 `reflection` 路由)调用 `ConflictLlmArbiter::arbitrate` 仲裁,输出 `ArbitrationOutcome` 决定保留/合并/覆盖,完成后更新证据信号

#### 软归档与反驳宽限期

- **软归档**(soft-archive):达到归档条件的记忆不物理删除,标记 `archived=true` 保留在存储中。检索默认排除软归档记忆,但可通过 `include_archived=true` 参数显式召回;用户可在 MemoryWindow 中手动恢复。设计动机:避免不可逆删除,保留上下文完整性。
- **反驳宽限期**(rebuttal grace period):当用户反驳某条记忆时(`user_rebut` / `user_keyword_rebut` 证据来源),disputation 不立即扣减,而是进入 24 小时宽限期。若用户在宽限期内再次确认反驳,则正式计入 disputation;若仅为一次性口误,宽限期到期后自动衰减。防止单次情绪化反驳导致重要记忆被快速归档。
- **`consolidated` 字段**:每条记忆携带 `consolidated: bool` 标记,记录是否经过夜间巩固流水线整合。未巩固记忆(`consolidated=false`)在归档决策中权重降低,避免刚写入的记忆因尚未经过巩固而被过早归档。

#### 证据主动再评估

证据系统不再仅被动接收信号。Stage 2 抽取新事实后,`reassess_evidence_for_new_fact` 主动调用 `find_similar_persistent_memories(new_fact, 3)` 检索相似旧记忆,对每条调用 `detect_local_contradiction` 检测局部矛盾,命中即对旧记忆应用 `EvidenceSource::UserFact` + `SignalKind::Negates` 信号削弱,让"用户最新说法"自动压制过时旧记忆,避免矛盾记忆共存污染 prompt。

#### 检索语义去重(`retriever.rs::dedup_by_semantic`)

检索结果中语义相同但表述不同的记忆(如"用户喜欢晚上看书" vs "用户偏好夜间阅读")会挤占 token 预算。`dedup_by_semantic` 对候选记忆计算 embedding,使用 Union-Find 聚类(默认阈值 0.85),每簇保留 `evidence_score + importance` 最高的一条。任一 embedding 失败时降级为字面去重 `dedup_by_content`(保守策略,不丢数据)。

#### 容量上限与淘汰策略(`retention.rs`)

`MemoryExpirationRule` 新增 `evict_by_score` 字段区分淘汰策略:

| 类型 | max_age | max_count | 淘汰策略 |
|------|---------|-----------|----------|
| casual_conversation | 24h | 100 | 时间戳升序(最旧优先) |
| temporary_context | 6h | 50 | 时间戳升序 |
| long_term | 720h | — | importance<0.3 才删 |
| knowledge | ∞ | 500 | evidence_score+importance 升序(证据弱、重要性低优先) |
| insight | ∞ | 100 | evidence_score+importance 升序 |
| inner_monologue | 168h(7天) | 200 | 时间戳升序,importance<1.0 才删(确保所有 inner_monologue 7 天后过期) |

Knowledge/Insight 的 `max_age=∞` 因 TTL 分级已处理短期过期,此处仅控制长期累积上限,防止无限增长。

#### 夜间巩固(`consolidation.rs`)

在配置的睡眠窗口内(`sleep_start_hour`..`sleep_end_hour`)+ 6 小时冷却到期时,异步执行完整巩固流水线,模拟人类睡眠时的记忆整理。

#### 记忆类型(`types.rs`)

8 种 `MemoryType`:short_term / mid_term / long_term / user / reference / preference / identity / important_event

外加特殊类型:`SessionSummary`(会话摘要)、`Insight`(反思洞察)、`InnerMonologue`(内心独白)、`ObservationNote`(旁观观察)、`CasualConversation`(闲聊)

#### 种子记忆(`seed_if_empty`)

`MemoryManagerInner::seed_if_empty(char_id)` 在记忆库为空时(首次启动 / 恢复出厂后 `clear_all_memories` 调用)写入角色专属种子记忆,为冷启动提供人设锚点与活人感:

- **ID 前缀** `seed_` + 8 位 UUID;metadata `source: "system_seed"`;UI / 图谱 / `get_memories_all` 均按此过滤不展示
- **Vivian / Nana 各 9 条**,覆盖 6 类:`identity`(protected, 0.95)/ `long_term`(性格/室友关系, 0.80-0.85)/ `preference`(生活习惯/社交边界, 0.65-0.80)/ `important_event`(首次启动里程碑, 0.90)/ `short_term`(当下心境, 0.60)/ `inner_monologue`(内心独白, 0.55)/ `observation_note`(环境观察, 0.50)
- **OpenHook 钩子**:`short_term` 类型携带 `follow_up` 钩子(如"用户透露玩不玩游戏或看不看番"),让角色带着"想了解用户什么"的动机开启对话,未闭环时检索获得 boost
- **时间戳错开**:按 index 递减错开 60s/条,模拟"长期锚点早已存在 → 刚到里程碑 → 当下心境"的自然时序,避免全部同一时间戳的机械感
- **内容约束**:仅写关于自己的既定事实(源自人设卡)与当下的主观感受,不编造未发生的事件;Nana 的"泡茶"动作改为"喜欢茶但没法真泡"的偏好,符合桌面宠物设定
- **向量索引**:写入时同步计算 embedding 并加入向量索引,保证向量检索稳定命中

#### 关键文件(37 文件)

| 文件 | 职责 |
|------|------|
| [manager.rs](file:///g:/vivian-rs/src-tauri/src/memory/manager.rs) | MemoryManager 主控(内部状态用 `parking_lot::Mutex` 保护;调用 `MemoryVectorStore::add` 等方法时通过 `?` 传播错误;`add_knowledge_document` 支持 TTL 分级;`push_topic_hint` / `drain_topic_hints` 主题提示机制;`pending_conflicts` 冲突队列 + `process_pending_conflicts` 批量仲裁 + `save_pending_conflicts` 持久化;`apply_evidence_to_memory` 证据主动应用;`find_similar_persistent_memories` 相似持久记忆检索;`check_index_drift_and_rebuild` 索引漂移检测) |
| [types.rs](file:///g:/vivian-rs/src-tauri/src/memory/types.rs) | 类型定义(含证据字段 reinforcement / disputation / protected / sub_zero_days) |
| [pipeline.rs](file:///g:/vivian-rs/src-tauri/src/memory/pipeline.rs) | 巩固流水线(Stage 1 排除 InnerMonologue/ObservationNote;Stage 2 证据主动再评估 + 24h 兜底触发;Stage 3 聚类洞察;run() 末尾索引漂移检测) |
| [retriever.rs](file:///g:/vivian-rs/src-tauri/src/memory/retriever.rs) | 混合检索 + 语义去重(`dedup_by_semantic` Union-Find 聚类) |
| [embedding.rs](file:///g:/vivian-rs/src-tauri/src/memory/embedding.rs) | 嵌入服务(本地 Hashing 256 维 + OpenAI 兼容在线;远程调用通过 `REMOTE_EMBEDDING_MAX_CONCURRENCY=4` Semaphore 限流防止外部 API 过载) |
| [ivf_index.rs](file:///g:/vivian-rs/src-tauri/src/memory/ivf_index.rs) | IVF 倒排索引 |
| [vector_search.rs](file:///g:/vivian-rs/src-tauri/src/memory/vector_search.rs) | 向量搜索(MemoryVectorStore,内部状态用 `parking_lot::Mutex` 保护避免中毒 panic,`add` / `delete` / `clear` 返回 `VivianResult<()>` 错误向上传播) |
| [filter.rs](file:///g:/vivian-rs/src-tauri/src/memory/filter.rs) | 记忆过滤(读路径零 LLM) |
| [consolidation.rs](file:///g:/vivian-rs/src-tauri/src/memory/consolidation.rs) | 夜间巩固 |
| [retention.rs](file:///g:/vivian-rs/src-tauri/src/memory/retention.rs) | 保留策略 + 证据驱动归档(protected 永不归档;evidence_score <= -2.0 且 sub_zero_days >= 14 触发归档)+ **软归档**(soft-archive):归档不物理删除,标记 `archived=true` 保留在存储中,检索时默认排除但可通过 `include_archived=true` 显式召回,支持手动恢复 + **容量上限**:knowledge 500 / insight 100 / inner_monologue 200,evict_by_score 按证据+重要性淘汰 |
| [llm_enricher.rs](file:///g:/vivian-rs/src-tauri/src/memory/llm_enricher.rs) | LLM 增强(写入路径) |
| [auto_extractor.rs](file:///g:/vivian-rs/src-tauri/src/memory/auto_extractor.rs) | 自动事实提取(跳过 memory_disabled 消息;subject 字段使用 "self" 替代硬编码角色名,build_analysis_prompt 动态引用当前角色;注入已有事实避免重复抽取;对话格式统一使用第一人称说话者标记 `[User says to me]` / `[I say to User]`;三语提示词) |
| [conflict.rs](file:///g:/vivian-rs/src-tauri/src/memory/conflict.rs) | 冲突检测 3 阶段 + QueueLlm 队列消费者(`PendingConflict` 持久化队列 + `ConflictLlmArbiter` trait + `DefaultConflictArbiter` 基于 reflection 路由仲裁 + 指数退避重试) |
| [user_facts.rs](file:///g:/vivian-rs/src-tauri/src/memory/user_facts.rs) | 用户事实画像(L0 稳定身份 5 字段 + L0.5 结构化偏好 5 字段[生日/作息/常用网站/喜欢的游戏/兴趣爱好] + L1 近期状态 goals/projects/preferences + L2 自由事实;`extract_and_upsert` 注入 `format_existing_facts` 已有事实避免重复抽取;`set_fact`/`delete_fact`/`set_pinned` 提供 UI 编辑入口;对话格式统一使用第一人称说话者标记 `[User says to me]` / `[I say to User]`;三语提示词;按角色隔离存储于 `characters/<char_id>/user_facts.json`) |
| [tokenize.rs](file:///g:/vivian-rs/src-tauri/src/memory/tokenize.rs) | jieba 分词 |
| [time_stamped.rs](file:///g:/vivian-rs/src-tauri/src/memory/time_stamped.rs) | 时间戳记忆(全局 `cl100k_base` tokenizer 加载失败时降级到字符数估算:中文 1 字 ≈ 1.5 token,ASCII 4 字符 ≈ 1 token) |
| [session_compressor.rs](file:///g:/vivian-rs/src-tauri/src/memory/session_compressor.rs) | 会话记忆压缩(从 TimeStampedMemory 提取摘要注入对话历史头部) |
| [age.rs](file:///g:/vivian-rs/src-tauri/src/memory/age.rs) | 年龄计算 |
| [hooks.rs](file:///g:/vivian-rs/src-tauri/src/memory/hooks.rs) | 钩子(OpenHook 闭环判定:`judge_and_close` → `judge_item` → `judge_single_hook`,由 BrainChatChain 每轮触发;未闭环 hook 在检索时获得 boost) |
| [strategy.rs](file:///g:/vivian-rs/src-tauri/src/memory/strategy.rs) | 检索策略(VectorStrategy / HybridStrategy / AutoStrategy 三档回退;`apply_temporal_decay` 对 Knowledge 类型施加时间衰减+过期惩罚) |
| [verifier.rs](file:///g:/vivian-rs/src-tauri/src/memory/verifier.rs) | 检索后验证(`VerifierLlmClient` trait + `verify_retrieval` 函数;小模型二分类过滤无关记忆,记忆数 ≤ 2 跳过,LLM 不可用降级全部保留;`build_verify_prompt` 中/英/日三语 + 截断 400 字符 + 附带 description;`parse_verify_response` 支持逗号/空格/括号形式) |
| [evidence.rs](file:///g:/vivian-rs/src-tauri/src/memory/evidence.rs) | 证据驱动记忆可信度(reinforcement / disputation 双时钟半衰期衰减,7 种证据来源,ARCHIVE_THRESHOLD=-2.0,ARCHIVE_DAYS=14)+ **反驳宽限期**(rebuttal grace period):用户反驳后不立即扣减 disputation,给予 24 小时宽限期等用户确认后再正式计入;`consolidated` 字段:标记记忆是否经过巩固流水线整合,未巩固记忆在归档决策中权重降低 |
| [event_log.rs](file:///g:/vivian-rs/src-tauri/src/memory/event_log.rs) | 事件溯源(append-only ndjson,15 种事件类型,Sentinel 游标,Reconciler 幂等重放,10K 行 / 90 天 compaction) |
| [unified_event_ledger.rs](file:///g:/vivian-rs/src-tauri/src/memory/unified_event_ledger.rs) | 统一事件账本(全局共享环境事件索引层) |
| [graph_store.rs](file:///g:/vivian-rs/src-tauri/src/memory/graph_store.rs) | 知识图谱存储(GraphEntity 节点 + GraphEdge typed edge,内存 HashMap + JSON 持久化,BFS fanout 遍历) |
| [entity_extract.rs](file:///g:/vivian-rs/src-tauri/src/memory/entity_extract.rs) | 实体与关系提取(jieba 词性标注零 LLM 抽取实体,regex 链推断关系类型) |
| [relational_recall.rs](file:///g:/vivian-rs/src-tauri/src/memory/relational_recall.rs) | 关系型查询解析(纯 regex 无 IO,将自然语言问句映射为 seed + relation_types + direction) |

#### 统一事件账本(`unified_event_ledger.rs`)

全局共享的环境事件索引层,在保留各角色 MemoryManager 隔离存储的前提下,为多角色系统提供清晰的事件上下文。

**核心结构**:

```rust
pub struct UnifiedEvent {
    pub id: String,
    pub timestamp: f64,           // 创建时间戳(秒)
    pub sender: String,           // "user" / "vivian" / "nana" / "system"
    pub receiver: String,         // "user" / "vivian" / "nana" / "all"(广播)
    pub event_type: String,       // "dialogue" / "action" / "observer_note" / "system" / "cross_character"
    pub content_preview: String,  // 内容预览(前 80 字)
    pub context_tags: Vec<String>,// 上下文标签
    pub visibility: EventVisibility,
    pub associated_char_id: Option<String>, // 关联角色 ID
}

pub enum EventVisibility {
    Public,           // 所有角色可见(跨角色对话、广播)
    Participants,     // 仅参与方可见(用户↔智能体对话)
    Private(String),  // 仅指定角色可见(旁观记忆)
}
```

**关键方法**:

| 方法 | 用途 |
|------|------|
| `append(event)` | 追加事件(带 FIFO 淘汰 + LLM 摘要压缩) |
| `recent_events_visible_to(char_id, n)` | 查询某角色可见的最近 N 条事件(按时间升序) |
| `events_between(a, b, n)` | 实体-实体检索:查询 A↔B 双向事件流 |
| `recent_public_events(n)` | 查询全局公开事件(环境感知用) |
| `clear_all()` | 清空全部事件(清空记忆时同步调用) |

**事件注册入口**:

- `register_event(event)` — 显式注册环境事件(cross_character.rs / chat.rs 调用)
- `register_world_event(event_type, content, tags, ts, associated_char_id)` — 注册世界事件(程序确定的事实,sender=system/receiver=all/visibility=Public)。行为事件(long_idle/quiet_mode/mood_event/presence_log/foreground_app_changed 等)必须走此接口,禁止写入 MemoryManager;MemoryManager 只保留 AI 主观记忆
- `register_event_from_dialogue(char_id, content, metadata, ts)` — 从对话消息自动注册事件(DialogueManager::add_message_with_metadata 中调用)

**压缩机制**:事件数超过 `MAX_EVENTS=500` 时启动 LLM 摘要压缩,取出最旧 `COMPACT_BATCH=100` 条事件压缩为一条摘要事件;LLM 失败时回退 FIFO 淘汰。压缩期间通过 AtomicBool 互斥防止并发压缩。

**前端命令**:`list_unified_events(character_id, limit, offset)` 支持分页查询,无 character_id 时返回 Public 事件。

#### 知识图谱(`graph_store.rs` + `entity_extract.rs` + `relational_recall.rs`)

自布线知识图谱,从记忆内容中零 LLM 抽取实体与关系,形成可多跳查询的结构化关系网络。

**存储结构**:
- `GraphEntity` 节点:name / entity_type(Person/Location/Organization/Other) / salience / memory_ids / created_at / updated_at
- `GraphEdge` typed edge:source / target / relation_type / weight / source_memory_id / context / created_at
- 持久化:`characters/<char_id>/memory/knowledge_graph.json`,内存 HashMap + JSON 落盘

**实体提取**(`entity_extract.rs`):jieba 词性标注(nr/ns/nt/nz/n),零 LLM 调用,salience = count / total。

**关系类型**(`RelationType` 枚举,14 种):

| 类别 | 关系类型 | 中文动词匹配 | 置信度 |
|------|---------|-------------|--------|
| 商业 | WorksAt | 在...工作/就职于 | 0.70 |
| 商业 | InvestedIn | 投资/注资/领投 | 0.80 |
| 商业 | Founded | 创建/创办/联合创始人 | 0.85 |
| 商业 | Advises | 担任...顾问 | 0.75 |
| 社交 | Knows | 认识/介绍/引荐 | 0.60 |
| 社交 | FriendOf | 是...的朋友/好友/闺蜜/哥们(对称) | 0.85 |
| 社交 | FamilyOf | 是...的家人/哥哥/姐姐/爸爸/妈妈...(对称) | 0.90 |
| 偏好 | Likes | 喜欢/喜爱/钟爱/爱好 | 0.75 |
| 偏好 | Dislikes | 不喜欢/讨厌/反感/厌恶 | 0.75 |
| 偏好 | Prefers | 偏好/更倾向 | — |
| 情感 | Trusts | 信任/信赖/相信 | 0.70 |
| 情感 | CaresFor | 关心/在意/照顾/挂念 | 0.65 |
| 情感 | Misses | 想念/思念/惦记/怀念 | 0.65 |
| 兜底 | Mentions | 无明确关系时两两连接 | 0.30 |

`is_symmetric()` 标记对称关系(FriendOf / FamilyOf),A→B 等价于 B→A。

**关系推断优先级**:FamilyOf > FriendOf > Likes/Dislikes > Trusts > CaresFor > Misses > Founded > InvestedIn > Advises > WorksAt > Knows > Mentions 兜底。

**关系型查询解析**(`relational_recall.rs`):纯 regex 无 IO 无 LLM,将自然语言问句映射为 `(seed, relation_types, direction)` 三元组。

| RelationalKind | 问句模式 | direction | 示例 |
|----------------|---------|-----------|------|
| Connects | "X 和 Y 有什么关系" | Both | "小明和小红有什么关系" |
| WhoSocial | "X 的朋友/家人是谁" | Out | "小明的朋友是谁" |
| WhoFeels | "谁喜欢/讨厌/信任 X" | In | "谁喜欢音乐" |
| Intro | "谁介绍我认识 X" | In | "谁介绍我认识小明" |
| WhoAt | "谁在 X 工作" | In | "谁在腾讯工作" |
| WhoRel | "谁投资/创建 X" | In | "谁创建了腾讯" |

匹配优先级:Connects > WhoSocial > WhoFeels > Intro > WhoAt > WhoRel。

---

### 5.5 providers/ 多 Provider 路由

**架构定位**:9 种 Provider 协议适配 + 路由矩阵 + 提示缓存策略 + Strict 熔断 + 视觉能力探测。

#### ProviderKind 枚举

```rust
pub enum ProviderKind {
    OpenAiCompat,      // OpenAI Responses API 兼容(/responses):DeepSeek / Qwen / Moonshot / SiliconFlow / GLM / Grok 等
    OpenAiResponses,   // OpenAI 官方 Responses API(/v1/responses):GPT-4o / o1 / o3 系列,原生 MCP
    DoubaoResponses,   // 火山方舟豆包 Responses API(/api/v3/responses):仅 250615+ 新模型
    ChatCompletions,   // 标准 Chat Completions(/v1/chat/completions):OpenRouter / Groq / Mistral / Together / Ollama / vLLM / LM Studio
    Gemini,            // Google 原生 REST(含 Google Search grounding)
    Anthropic,         // Claude /v1/messages(x-api-key + anthropic-version)
    Wenxin,            // 百度 OAuth + access_token
    Spark,             // 讯飞 WebSocket + HMAC-SHA256
    Custom,            // 自定义(按 Chat Completions 处理)
}
```

`from_str` 支持别名(大小写不敏感):`openai`/`openai_compat` → `OpenAiCompat`;`openai_responses`/`responses_api` → `OpenAiResponses`;`doubao`/`doubao_responses`/`responses` → `DoubaoResponses`;`gemini`/`google` → `Gemini`;`anthropic`/`claude` → `Anthropic`;`wenxin`/`ernie`/`baidu` → `Wenxin`;`spark`/`xfyun`/`iflytek` → `Spark`;`chat_completions`/`openai_chat` → `ChatCompletions`;`custom`/未知 → `Custom`。

#### BaseProvider trait 核心方法

```rust
// 必填(4 个)——最小文本 I/O,所有 provider 必须实现
async fn call_chat(&self, messages: Vec<ChatMessage>) -> VivianResult<String>
async fn call_stream_chat(&self, messages: Vec<ChatMessage>, json_schema: Option<Value>) -> VivianResult<mpsc::Receiver<String>>
fn get_model(&self) -> &str
fn get_circuit_breaker_stats(&self) -> serde_json::Value

// 默认实现(10 个)——支持原生 FC 的 provider 覆盖
fn set_enable_search(&self, _enable: bool)                              // 运行时切换联网搜索(AtomicBool)
fn set_max_tokens_override(&self, _tokens: u32)                         // 凝神模式叠加 max_tokens
fn set_temperature_override(&self, _temp: Option<f64>)                  // emotion→temperature 映射
async fn call_chat_with_search(&self, messages, enable_search, json_schema) -> VivianResult<String>  // 回退到 call_chat
async fn invoke(&self, messages) -> VivianResult<ChatResponse>          // 结构化响应,回退到 call_chat
fn bind_tools(&self, _tools) -> VivianResult<Box<dyn BaseProvider>>     // 绑定工具 schema,默认 NotImplemented
fn supports_native_function_calling(&self) -> bool                      // 默认 false
fn supports_structured_output(&self) -> bool                            // 默认 false
fn supports_json_mode(&self) -> bool                                    // 默认 false
async fn stream_with_tools(&self, _messages, _tools) -> VivianResult<mpsc::Receiver<StreamEvent>>  // 默认 NotImplemented
```

**ProviderBase struct** 字段:`api_key` / `base_url` / `model` / `temperature` / `max_tokens` / `circuit_breaker: Arc<RwLock<CircuitBreaker>>` / `request_cache` / `enable_search: AtomicBool` / `proxy` / `client` / `max_tokens_override: AtomicU32` / `temperature_override: AtomicU64`。`effective_max_tokens` 在默认值上叠加 override。

#### LLMRequest 统一数据结构(9 字段)

```rust
pub struct LLMRequest {
    pub task_type: String,                // 路由 key:chat / reasoning / reflection / consolidation / inner_monologue / activity_extraction / vision_describe 等
    pub messages: Vec<ChatMessage>,       // 含 system / user / assistant / tool 角色
    pub tools: Vec<ToolDefinition>,       // 空 = 不启用原生 FC,走文本路径
    pub stream: bool,                     // 是否流式
    pub enable_search: bool,              // 联网搜索(部分 provider 支持)
    pub temperature_override: Option<f64>,// 请求级 temperature 覆盖
    pub max_tokens_override: Option<u32>, // 请求级 max_tokens 覆盖
    pub json_schema: Option<Value>,       // JSON Schema 结构化输出约束
    pub reasoning: bool,                  // 推理/思维链(部分 provider 支持 extended thinking)
}
```

Builder 方法:`new` / `with_tools` / `with_stream` / `with_search` / `with_temperature` / `with_max_tokens` / `with_json_schema` / `with_reasoning` / `wants_tools` / `wants_json`。

**相关结构体**:`ChatResponse`(content/tool_calls/finish_reason/reasoning/raw) / `StructuredToolCall`(id/name/arguments) / `ToolDefinition`(name/description/parameters) / `StreamEvent` enum(Text/Thinking/ToolCallDelta/Done/Error)。

#### ModelRouter 调用入口与三级 fallback

公开入口统一接收 `LLMRequest`（task_type + messages + tools/stream/search/temperature 等链式选项），内部经三级 fallback 选择 provider：

```rust
pub async fn generate(&self, request: LLMRequest) -> VivianResult<String>
pub async fn generate_stream(&self, request: LLMRequest) -> ...
pub async fn generate_with_tools(&self, request: LLMRequest) -> ...
pub async fn generate_stream_with_tools(&self, request: LLMRequest) -> ...
```

1. **task_providers** — 任务专用(按 chat_reasoning / memory_reflection / auxiliary 分组),成功 emit `chat:route_status: "ok"`
2. **main_provider** — 主 LLM(硬约束:未配置则终止后续流程并 toast)
3. **全部失败** — emit `llm:error` toast(带 60s 冷却,Permanent 类 5 分钟)+ 返回错误

旧版 `query*` 方法已收敛为内部实现（`query_with_fallback` / `query_stream` / `query_with_tools` / `query_stream_with_tools`），不对外暴露。

**路由状态事件**:`chat:route_status`("ok"=绿色/"error"=红色,不受 emit_enabled 限制)、`chat:route_fallback`(任务 provider 失败回退到主 API,受 emit_enabled 限制,仅用户主动发消息时开启)、`llm:error`(LLM 错误 toast,带冷却机制)。

**Strict 熔断**:检测到 400 错误 + 响应含 `json_schema`/`response_format`/`responseSchema`/`strict`/`$ref`/`$defs` 等关键词时置 `strict_broken=true`,持久化到 `strict_broken_model` 文件,下次启动同一模型直接跳过 schema。

#### 任务分组并发信号量

| 分组 | 并发数 | 覆盖任务 |
|------|--------|----------|
| chat_reasoning | 3 | chat / reasoning / vision_describe(兜底:未显式列出的任务也归此组) |
| memory_reflection | 3 | memory / consolidation |
| auxiliary | 2 | emotion_analysis / inner_monologue / diary / activity_extraction / knowledge_acquisition / interest_search / translation |

#### 提示缓存策略 CacheStrategy(4 种)

| 策略 | 说明 | 适用 |
|------|------|------|
| `Auto` | 自动选择 | 默认 |
| `PromptCacheKey` | 自动注入 `prompt_cache_key` | Kimi / Moonshot |
| `CacheControl` | ephemeral cache_control | Anthropic Claude |
| `None` | 不缓存 | — |

#### 兼容推理字段

`reasoning_content` / `thinking` / `reasoning_details` 三种字段统一适配。

#### 联网搜索多模型适配

| Provider | 适配方式 |
|----------|---------|
| DeepSeek | `extra_body` |
| GPT-4o | `web_search_options` |
| Qwen | `enable_search` |
| GLM | `web_search` tool |
| Moonshot | `builtin_function` |
| Doubao | `web_search` |
| Gemini | `google_search` grounding |
| Wenxin | `enable_search` |
| Spark | system prompt |

#### 多模态（图片输入）适配

六家 Provider 协议支持图片输入（文心/星火不支持），统一通过 `ChatMessage::user_with_images` 构造，`MessageImage` 结构包含 `media_type` / `data`(base64) / `url` / `detail` 四字段。

| Provider | 图片字段格式 | 转换位置 |
|----------|------------|---------|
| OpenAiCompat | `input_image: {image_url: data URI}` | `openai_compat.rs` |
| OpenAiResponses | `input_image: {image_url: data URI}` | `openai_responses.rs` |
| DoubaoResponses | `input_image: {image_url: data URI}` | `doubao.rs` |
| ChatCompletions | `image_url: {url, detail}` | `chat_completions.rs` |
| Anthropic | base64 `source` / `url` source | `anthropic.rs` |
| Gemini | `inline_data: {mime_type, data}` (base64) / `file_data: {mime_type, file_uri}` (URL) | `gemini.rs` |

`detail` 字段三档（`auto`/`low`/`high`）由 `ai.image_detail` 配置控制，影响 OpenAI 系的 token 消耗。

#### 原生 Function Calling 与 Structured Outputs 支持

| Provider | 原生 FC | Structured Output | JSON Mode | tools schema 格式 |
|----------|---------|-------------------|-----------|------------------|
| OpenAiCompat | ✅ | ✅ (`text.format`) | ✅ | 扁平(`type`/`name`/`description`/`parameters` 顶层) |
| OpenAiResponses | ✅ | ✅ (`text.format`) | ✅ | 扁平 |
| DoubaoResponses | ✅ | ✅ (`response_format.type=json_schema`) | ✅ | 扁平 |
| ChatCompletions | ✅ | ❌ | ✅ | 嵌套(`{type:function, function:{...}}`) |
| Anthropic | ✅ | ✅ (`emit_response` 伪工具) | ❌ | `input_schema`(非 `parameters`) |
| Gemini | ✅ | ✅ (`generationConfig.responseSchema`) | ✅ | `function_declarations` 双层嵌套 |
| Wenxin | ❌ | ❌ | ❌ | — |
| Spark | ❌ | ❌ | ❌ | — |

**Schema 注入策略**(`schema.rs`):通过 `schemars::schema_for!(ProcessedResponse)` 自动生成 JSON Schema,保证与 Rust 结构体字段同步。主调用 schema 仅约束 `text`/`intent`/`response_mode` 三字段,`tool_calls`/`control_actions` 用 `#[schemars(skip)]` 跳过(走原生 FC 通道)。后端 `validate_vivian_response` 校验必填字段。

#### 视觉能力探测（首次发图自适应）

应用不假设用户填入的 API 支持视觉，而是在**首次发图前用 16×16 透明 PNG 探测**目标模型是否接受图片输入（部分服务商如豆包要求最小 14×14），结果按 model 名缓存。

```rust
pub enum VisionCapability {
    Supported,
    NotSupported(String),
}
```

- **探测时机**：`send_image_message` 命令在 `enable_vision` 开关检查通过后、实际发图前调用 `ModelRouter::check_vision_capability()`
- **探测路径**：与 `vision_describe` 任务实际路由一致（路由矩阵启用时优先任务 provider，否则主 LLM API），绕过 `query_with_fallback` 避免 fallback 掩盖真实结果 + 避免污染路由矩阵 UI 事件
- **探测请求**：16×16 透明 PNG base64 + `detail=low` + 简单 prompt，单次 token 消耗个位数
- **判定逻辑**：调用成功且响应非空 → `Supported`；调用失败或空响应 → `NotSupported(原因)`
- **缓存**：`vision_capability_cache: Arc<RwLock<HashMap<String, VisionCapability>>>`，按 model 名缓存，`save_config` / `reload_config` 时自动清空（用户换模型后下次发图重新探测）
- **不支持时拦截**：emit `chat:error` + 详细 error toast（含原因 + 配置指引，duration 8000ms），不发图

| 场景 | 行为 |
|------|------|
| 首次发图 + 模型支持视觉 | 探测请求 → 缓存 `Supported` → 走原流程发图 |
| 首次发图 + 模型不支持（如 DeepSeek-V3） | 探测失败 → 缓存 `NotSupported` → 拦截 + error toast 提示换模型 |
| 第二次发图（同模型） | 缓存命中 `Supported` → 直接发图，无探测请求 |
| 用户换模型后发图 | `save_config` 清缓存 → 重新探测 |
| 无可用 provider | `NotSupported("未配置视觉模型")` → 拦截 |

#### 关键文件

| 文件 | 职责 |
|------|------|
| [base.rs](file:///g:/vivian-rs/src-tauri/src/providers/base.rs) | BaseProvider trait(4 必填 + 10 默认实现) + LLMRequest(9 字段) + ProviderBase struct(max_tokens_override 运行时覆盖,effective_max_tokens 在默认值上叠加) + ChatResponse/StructuredToolCall/ToolDefinition/StreamEvent |
| [factory.rs](file:///g:/vivian-rs/src-tauri/src/providers/factory.rs) | Provider 工厂(create_task_provider 按 endpoint 域名分流代理,create_provider_by_kind 按 ProviderKind 分发;ClientCache 按 `{endpoint}\|{proxy_url}\|{timeout}` 复用) + ProviderKind 枚举(9 变体) + from_str 别名解析 |
| [router.rs](file:///g:/vivian-rs/src-tauri/src/providers/router.rs) | ModelRouter 路由矩阵(generate/generate_stream/generate_with_tools/generate_stream_with_tools 唯一公开入口,内部 query* 三级 fallback) + set_focus_boost/clear_focus_boost(凝神模式 max_tokens 余量注入) + check_vision_capability(首次发图视觉能力探测 + 缓存) + clear_vision_capability_cache(配置变更清缓存) + Strict 熔断(strict_broken 持久化) + 任务分组信号量(chat_reasoning/memory_reflection/auxiliary) |
| [openai_compat.rs](file:///g:/vivian-rs/src-tauri/src/providers/openai_compat.rs) | OpenAI Responses API 兼容协议(`/responses` 端点,`input` 数组,`max_output_tokens`,`instructions` 顶层参数,`text.format` Structured Outputs,扁平 tools schema,`response.output_text.delta` 流式事件;集成 ThinkingStreamStripper + 提示词占位符泄露检测 + 提示缓存策略 CacheStrategy) — DeepSeek/Qwen/Moonshot/SiliconFlow/GLM/Grok |
| [openai_responses.rs](file:///g:/vivian-rs/src-tauri/src/providers/openai_responses.rs) | OpenAI 官方 Responses API(GPT-4o/o1/o3 系列,`/v1/responses`,原生 MCP,Stateless 模式不使用 previous_response_id,扁平 tools schema,仅支持 GPT-4o 的 `web_search_options` 联网搜索) |
| [doubao.rs](file:///g:/vivian-rs/src-tauri/src/providers/doubao.rs) | 火山方舟豆包 Responses API(`/api/v3/responses`,仅 250615+ 新模型;Structured Outputs 用 `response_format.type=json_schema` 非 `text.format`;不做请求缓存;流式不剥离 thinking;不带 `OpenAI-Beta` 头) |
| [chat_completions.rs](file:///g:/vivian-rs/src-tauri/src/providers/chat_completions.rs) | 标准 Chat Completions API(`/v1/chat/completions`,`messages` 数组,`max_tokens`,`role:system` 消息,嵌套 tools schema `{type:function,function:{...}}`;api_key 为空时不发 Authorization 头支持 Ollama 无鉴权;instructions 作为首条 system 消息注入) — OpenRouter/Groq/Mistral/Together/Ollama/vLLM/LM Studio |
| [anthropic.rs](file:///g:/vivian-rs/src-tauri/src/providers/anthropic.rs) | Claude `/v1/messages` 协议(x-api-key + anthropic-version;`system` 字段可数组+cache_control;Structured Outputs 通过 `emit_response` 伪工具实现;联网搜索注入 `web_search_20250305` 工具;推理回传 assistant 消息携带 `reasoning` 时作为 `thinking` 块原样回传) |
| [gemini.rs](file:///g:/vivian-rs/src-tauri/src/providers/gemini.rs) | Gemini 原生 REST(`:generateContent`/`:streamGenerateContent?alt=sse`;`contents` 数组 + `generationConfig`;强制 `responseMimeType: application/json`;JSON Schema `$ref`/`$defs` 内联解析;联网搜索 `tools=[{google_search:{}}]`;流式中 `functionCall` 完整出现非增量) |
| [wenxin.rs](file:///g:/vivian-rs/src-tauri/src/providers/wenxin.rs) | 文心一言(OAuth + access_token 缓存复用;`messages` 数组 role 仅 user/assistant/function;`system` 单独字段;错误响应 access_token mask 为 `***`;不支持原生 FC/Structured Output/JSON Mode/图片输入) |
| [spark.rs](file:///g:/vivian-rs/src-tauri/src/providers/spark.rs) | 讯飞星火(WebSocket + HMAC-SHA256 签名;按模型版本自动选择端点 v4.0/v3.5/v3.1/v1.1;`{header,parameter,payload}` 帧结构;联网搜索通过 system 消息触发;不支持原生 FC/Structured Output/JSON Mode/图片输入) |
| [schema.rs](file:///g:/vivian-rs/src-tauri/src/providers/schema.rs) | Vivian 通用响应 Schema(`schemars::schema_for!(ProcessedResponse)` 自动生成;`VIVIAN_RESPONSE_SCHEMA` 静态单例;`emit_response_tool_definition` Anthropic 伪工具;`validate_vivian_response` 后端校验必填字段 text/intent/response_mode) |
| [thinking_stripper.rs](file:///g:/vivian-rs/src-tauri/src/providers/thinking_stripper.rs) | CoT 泄露过滤(Qwen3.5/3.6/3.7 混合模型,`leaks_thinking_in_content` 检测;非流式 `strip_thinking_segments` 清理成对/孤立标签;流式 `ThinkingStreamStripper` BUFFERING/PASSTHROUGH 两态状态机,hold content 直到第一个 `</think>` 闭合标签出现再放行) |

---

### 5.6 tools/ 增强工具系统

**架构定位**:工具子系统统一调度层,68 个内置工具 + 2 个元工具,含权限网关、沙箱、MCP、Skills、Observability。安全策略:文件操作强制路径校验 + 敏感目录拦截,Shell 执行禁用,应用名/URL 输入校验,从源头阻断 RCE 与路径穿越风险。

#### Tool trait 核心方法

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult
    async fn check_permissions(&self, input: &Value, ctx: &ToolUseContext) -> PermissionResult
    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult
    fn is_read_only(&self) -> bool
    fn category(&self) -> ToolCategory           // 7 类:File/System/Memory/Pet/Media/Perception/ExtendedSystem
    fn should_defer(&self) -> bool               // true=延迟加载
    fn always_load(&self) -> bool                // true=始终注入 prompt
    fn anti_use_cases(&self) -> &str             // 不适用场景描述
}
```

#### 工具执行 7 步管线(`executor.rs`)

1. 查找工具(find_tool 三级匹配:内置 → MCP → Skills)
2. 沙箱安全检查
3. 输入验证 + **参数归一化**(`normalize_arguments`):先将 key 中所有非字母数字字符过滤并转小写(`normalize_key`)后精确匹配 schema 参数,精确匹配失败时按归一化键长度差异排序选最优候选(优先匹配 required 参数),避免宽松子串匹配导致的错误参数映射
4. 缓存查询
5. 权限检查
6. 执行(带超时,阻塞 IO 通过 spawn_blocking 隔离;依赖 PowerShell 的工具——输入控制 / 媒体键 / 感知 / 截屏——其同步子进程统一经 `spawn_blocking` 投递到阻塞线程池,并在脚本头部注入 `[Console]::OutputEncoding`/`$OutputEncoding = UTF8` 强制 UTF-8 输出,避免中文系统 GBK 乱码)
7. 缓存写入 + 预算截断(max_result_chars=4000)

#### 权限网关矩阵(`permission.rs`)

`AgentAccessLevel`(4:ReadOnly / FsRead / FsWrite / FullControl)× `ToolRiskTier`(6:Safe / FsRead / FsWrite / Shell / Network / InputControl)→ Allow / Ask / Deny

通用规则:Safe 永远 Allow;风险 ≤ 访问级别上限 Allow;高一级 Ask;高两级以上 Deny。Network 与 Shell 单独特判(一维 ordinal 无法表达"FsWrite 包含 Network 但不包含 Shell"的交叉关系):Network 在 ReadOnly→Deny / FsRead→Ask / FsWrite 及以上→Allow;Shell 在 ReadOnly→Deny / FsRead、FsWrite→Ask / FullControl→Allow。

矩阵判定 Deny 时,拒绝消息附带提权提示:InputControl 风险明确提示"请在设置中将访问级别提升至 FullControl(完全控制)后重试",其余风险泛化提示"可在设置中提升访问级别以解锁更高权限的工具",让用户知道如何解锁而非只看到一句"被拒绝"。矩阵判定 Ask 时,`check_tool_permission` 在流程末尾强制返回 `PermissionResult::ask`(不委托给工具自身的 `check_permissions`,避免 Ask 被静默放行)。

显式规则优先级统一为 **always_deny > always_ask > always_allow**(更严格的规则优先),`requires_permission` 与 `check_tool_permission` 两处判定顺序一致;`always_allow` 仍优先于矩阵 Ask 判定(显式放行过的工具不因矩阵 Ask 重复弹窗)。

13 个工具需用户确认:`read_file` / `write_file` / `edit_file` / `list_directory` / `search_files` / `grep` / `capture_screen_region` / `ocr_screen_text` / `get_window_tree` / `take_screenshot` / `delete_memory` / `cancel_scheduled` / `delete_todo`。其中 `delete_memory` 不再接受 LLM 自报的 `confirm` 参数自我批准,实际放行一律由确认弹窗决定;`cancel_scheduled` / `delete_todo` 因不可逆同样强制确认。

#### 沙箱(`sandbox.rs`)

- `ToolRiskLevel`(5)+ `ProtectionMode`(3:Cautious / Permissive / Strict)
- `DANGEROUS_COMMAND_PATTERNS`(13+):增强危险命令正则,覆盖 `rm -rf` / `rm -fr` / `rm -r -f` / `rm -f -r` / `rm --recursive --force` / `mkfs` / `dd if=` / `format c:` / `del /f /s /q` / `rd /s /q` / `:(){ :|:& };:`(fork bomb)等多种变体组合,防止通过参数顺序或格式变形绕过
- `PATH_TRAVERSAL_PATTERNS`:`../` / `..\` / 绝对路径越界检测
- **`extract_paths(args)`** — 递归遍历整个 JSON 参数树(`collect_paths_recursive`)提取所有 `Value::String` 类型的路径值,不再依赖固定参数名(`path`/`file`/`src`/`dest`/`dir`/`directory`/`folder`/`target`/`output`),防止通过非标准参数名注入路径
- **`is_path_safe(path)`** — 路径穿越校验函数,6 个文件工具(read_file / write_file / edit_file / list_directory / search_files / grep)在执行前统一调用,拒绝包含 `../` / `..\` 或越界绝对路径的输入,从源头阻断 LLM 通过工具调用访问预期范围之外的文件
- **`normalize_path(path)`**(`types.rs`) — 权限检查用的路径归一化,真正解析父目录分量(用栈 `pop()` 抵消 `..`,`/a/b/../c` 归一为 `/a/c`),而非简单过滤掉 `..` 分量(那会把 `/a/b/../c` 错算成 `/a/b/c` 导致权限评估错误路径);与 `sandbox.rs` 的 `normalize_path_buf` 解析逻辑对齐
- **`is_sensitive_path(path)`** — 敏感目录写入拦截,write_file / edit_file 在路径校验通过后再调用此函数,拒绝写入 `C:\Windows` / `C:\Program Files` / `C:\Program Files (x86)` / `C:\ProgramData` / `System32` 等系统敏感目录,防止 LLM 误操作导致系统损坏
- **默认放行策略** — 无内置安全档案的工具经通用检查(危险命令 / 路径穿越)后以 `Medium` 风险放行,风险分级交由下游权限系统(access_level × risk 矩阵 + always 规则 + 用户确认)统一管理

#### 文件操作安全策略(`builtin/file_ops.rs`)

6 个文件工具形成统一的安全执行管线,并包含资源限制防止 DoS:

1. **输入路径校验** — 调用 `crate::tools::sandbox::is_path_safe` 拒绝路径穿越
2. **写入敏感目录拦截** — 仅 write_file / edit_file 调用 `is_sensitive_path` 拒绝写入系统目录
3. **递归深度限制** — `list_directory` 递归遍历时强制最大深度 `MAX_RECURSION_DEPTH=10`,超过深度截断并标记 `truncated=true`,防止符号链接环或过深目录导致栈溢出/无限循环
4. **结果条数上限** — grep 最多返回 `MAX_GREP_RESULTS=500` 条、list_directory 最多 `MAX_LIST_ENTRIES=5000` 条、search_files 最多 `MAX_SEARCH_RESULTS=1000` 条,超大结果集会截断并提示
5. **流式读取 + 编码自适应** — `read_file` 使用 `BufReader` 按行读取,支持 `offset`/`limit` 参数跳过不需要的行,不全量加载文件到内存;单次读取上限 `MAX_READ_LIMIT=10000` 字符。读取前先采样文件头部 8KB 猜测编码(`detect_encoding`:合法 UTF-8 直接采用,否则交由 `chardetng` 推断 GBK 等遗留编码),再回到起点用 `read_until(b'\n')` 逐行按检测到的编码解码(`encoding_rs`),非 UTF-8 文件不再报错或乱码;grep 同样逐行解码,不再因首个非 UTF-8 字节就中断整个文件的搜索
6. **正则预编译** — grep 的正则表达式使用 `Lazy<Regex>` 在首次匹配时编译并复用,避免每次调用重复编译
7. **异步隔离** — 所有阻塞文件 IO 操作(文件读写、目录遍历、进程枚举)均通过 `tokio::task::spawn_blocking` 提交到专用线程池,避免阻塞 tokio 异步运行时
8. **排序修正** — `list_directory` 先收集全部条目再排序输出(目录在前、文件在后、名称字典序),而非边收集边截断
9. **错误传播** — 所有校验失败返回 `VivianError::Tool` 错误,不静默吞错

#### Shell 执行禁用与应用启动安全(`brain/computer_control.rs` + `builtin/system_ops.rs`)

`ComputerController::execute_shell` 直接返回 `VivianError::Tool`,从架构层杜绝 LLM 通过 shell 命令实现 RCE 的可能。`Win32ComputerController::open_app` 移除了 lookup 失败时直接将 app_name 作为 shell 命令执行的 fallback 路径,未在白名单 `app_map` 中注册的应用名直接返回错误"仅允许打开白名单中的应用"。

`open_application` 工具的安全机制:

- **危险程序黑名单** — `DANGEROUS_EXES` 列表包含 16 种高危系统程序:cmd.exe / powershell.exe / pwsh.exe / wscript.exe / cscript.exe / mshta.exe / regsvr32.exe / rundll32.exe / regedit.exe / reg.exe / net.exe / net1.exe / schtasks.exe / wmic.exe / bitsadmin.exe / certutil.exe / msiexec.exe,直接以路径形式输入这些程序名时拒绝启动
- **五级解析链** — 非路径输入(纯应用名)通过 where.exe → PATH → Program Files → Start Menu → UWP AUMID 五级链查找可执行目标,避免拼接 shell 命令
- **UWP AppID 白名单** — `is_safe_appid` 函数校验 UWP AppID 仅含字母/数字/`.`/`_`/`-`/`!`,防止通过恶意 AppID 拼接 PowerShell 命令实现注入
- **异步解析** — 应用路径解析(涉及注册表查询、文件系统遍历)通过 `tokio::task::spawn_blocking` 在阻塞线程池中执行,不占用 async 运行时
- **URL 安全** — `open_url` 仅允许 `http://` / `https://` 协议,拒绝 `file://` / `javascript:` / `data:` 等危险协议
- **剪贴板安全** — `set_clipboard_text` 使用 `clip.exe` 通过 stdin 管道写入文本,不再拼接 PowerShell 命令,彻底消除命令注入风险
- **截屏路径校验** — `capture_screen_region` 的 `save_path` 与 `take_screenshot` 的输出路径在拼入 PowerShell 脚本前做字符白名单校验:拒绝路径穿越(`..`),仅允许字母数字与 `_-. \: /`(空格),反引号 / `$` / 引号 / 括号等可触发命令注入的字符一律拒绝
- **跨平台关闭** — `close_application` 使用 `sysinfo` crate 枚举进程并调用 kill() 方法,跨平台兼容且避免硬编码 taskkill 命令

#### ToolChainer 多步编排(`chainer.rs`)

`IntentRecognizer`(5 种意图):单工具执行 / 多步顺序 / 条件分支 / 并行批次 / 工具链

`MultiStepExecutor` 指纹去重:工具名 + 规范化参数哈希,连续 2 次相同指纹终止循环。支持 `${result}` 参数注入。

#### MCP 原生集成(`mcp.rs`)

手写 JSON-RPC 2.0 over stdio 客户端(无外部 SDK):
- 启动时自动连接已配置的 MCP server
- 发现工具后注册到 ToolSystem,与内置工具无差别调度
- 外部工具默认延迟加载 + 权限 `ask`(不可信)
- 配置持久化于 `%APPDATA%\Vivian\mcp\servers.json`
- `new_disabled()` 降级构造方法 — MCP server 初始化失败时返回空实现,保证主流程不阻塞;外部工具调用直接返回错误提示,但内置工具与对话能力不受影响
- **并发安全** — 配置写入使用 `config_lock: Mutex<()>` 互斥锁保护,防止并发 save_configs 导致文件内容竞态覆盖
- **stderr 捕获** — MCP 子进程的 stderr 通过异步任务(`tokio::spawn` + `BufReader::lines()`)捕获并以 `tracing::debug!` 记录日志,便于排查外部 MCP server 的启动错误和运行时问题,不再丢弃 stderr 输出

```rust
pub struct McpServerConfig { command, args, env, cwd }
pub struct McpClient { stdin, stdout, request_id: AtomicU64 }
pub struct McpManager { clients: HashMap<ServerId, Arc<McpClient>>, config_lock: Mutex<()> }
```

`McpClient` 内部状态使用 `parking_lot::Mutex` 保护(不中毒、不持有 guard 跨 await),避免 std Mutex 在 Tokio 异步环境下的中毒 panic 与 Send 边界问题。

#### ToolCache(`cache.rs`)

TTL + LRU 双重淘汰:
- 默认 TTL 300s,max_size 1000
- LRU 优先淘汰 hits=0 的最旧条目
- 缓存键:`DefaultHasher` 对 `"{tool_name}:{args}"` 哈希

#### 异步确认(`confirmation.rs` + `trust.rs`)

三态确认响应 + oneshot channel + 5 分钟 TTL 惰性清理:
```rust
pub enum ConfirmationResponse { Deny, AllowOnce, AllowAlways }
const PENDING_TTL: Duration = 5 * 60s;
pub struct ToolConfirmationRegistry { pending: Mutex<HashMap<u64, PendingEntry>> }
```

`ConfirmationRequest` payload 含 `char_id`(多角色 toast 路由)与 `allow_always_scope`(`persistent` / `session`,决定前端第三按钮文案)。executor 确认分支先走两条快速通道:会话级放行列表(`SESSION_ALLOWED_TOOLS`,内存态,应用重启后重置)与应用信任列表(`trust.rs`,持久化于 `%APPDATA%\vivian\trusted_apps.json`,应用名按小写 + trim + 去 `.exe` 归一化匹配);均未命中才发起 `request_confirmation`。`AllowAlways` 按工具类型路由:`open_application` 写入信任列表,其余工具写入会话放行列表。

前端链路:App.tsx 把 `tool:confirmation_request` 载荷转发给 toast 子窗口(`toast:confirm`,窗口未就绪时缓冲),显示期间 suspend 主窗口点击穿透;ToastWindow 渲染 `ConfirmToast` 三按钮卡片(拒绝 / 放行一次 / 始终允许,30 秒倒计时无操作自动拒绝,期间关闭窗口级点击穿透以接收按钮点击);响应后 emit `toast:confirm_done` 恢复穿透并清理多窗口残留卡片。

#### 工具发现 BM25(`discovery.rs`)

多字段加权 BM25:
```
FIELD_WEIGHTS: name=6.0 / label=4.0 / search_hint=3.0 / summary=2.0 / description=2.0 / schema_key=1.0
k1=1.5, b=0.75
```

#### 原生 Function Calling(`tool_call_manager.rs`)

- `ToolListTool` — 列出可用工具
- `ToolSearchTool` — 延迟加载,支持 `select:A,B,C` 精确加载 / 关键词搜索

#### 工具可见性分层(`types.rs` + `tool_call_manager.rs`)

`ToolVisibility` 枚举控制工具在 LLM 上下文中的展示粒度,减少 token 开销:

```rust
pub enum ToolVisibility {
    Always,   // 完整 schema 注入(核心高频工具)
    Lazy,     // 仅名称 + 一行描述,完整 schema 通过 ToolSearchTool 按需加载
    Deferred, // 仅名称,出现在 <available-deferred-tools> 块中
}
```

`resolve_visibility()` 根据工具属性自动推断层级:

| 条件 | 层级 | 说明 |
|------|------|------|
| `should_defer() == true` | Deferred | 外部 MCP、低频工具 |
| `always_load() == true` | Always | 核心高频工具 |
| `category() == Media` 或 `Mcp` | Lazy | 媒体控制与外部 MCP 工具 |
| 其余 | Always | 默认完整注入 |

渲染层三区分割:Always 工具完整 schema 注入 `<tools>` 块;Lazy 工具在 `<lazy-tools>` 块仅展示名称 + 描述首行;Deferred 工具在 `<available-deferred-tools>` 块仅列名称。`tool_search` 搜索池覆盖 Lazy + Deferred 两层,Native Function Calling 仅注入 Always 层工具。个别工具可通过 `Tool::visibility_tier()` 方法覆盖默认推断。

#### 工具可观测性(`observability.rs`)

```rust
pub struct ToolCallRecord { tool_name, call_id, input_data, duration_ms, success, output_data, error }
pub struct ToolMetrics { total_calls, successful_calls, failed_calls, min/max_duration_ms, total_input/output_chars }
```
默认 max_records=1000

#### 内置工具分类(builtin/ 14 文件,68 工具)

| 类别 | 数量 | 工具示例 |
|------|------|---------|
| File | 6 | read_file / write_file / edit_file / list_directory / search_files / grep |
| System | 4 | get_running_processes / open_application / close_application / take_screenshot |
| ExtendedSystem | 6 | get_clipboard_text / set_clipboard_text / open_url / open_folder / get_active_window / get_system_info |
| Memory | 11 | save_memory / search_memory / clear_memory / read_memory / delete_memory / update_user_preference / log_daily_diary / get_recent_interactions / summarize_today_context / read_diary_by_date / list_recent_diaries |
| Pet basic | 3 | set_expression / play_motion / trigger_idle_action |
| Window control | 6 | get_window_info / set_window_position / set_window_size / get_watch_mode / toggle_watch_mode / set_behavior_mode |
| Todo | 5 | add_todo / list_todo / complete_todo / update_todo / delete_todo |
| Scheduler | 6 | schedule_reminder / schedule_tool_call / list_scheduled / cancel_scheduled / pause_scheduled / resume_scheduled |
| Pet behavior | 5 | set_pet_state / play_animation / speak_bubble / follow_cursor / set_mood |
| Relationship | 3 | get_relationship_status / list_milestones / record_milestone |
| Media | 6 | media_play_pause / next / previous / volume_up / volume_down / mute |
| Perception | 6 | get_cursor_position / get_idle_state / get_foreground_app_context / capture_screen_region / ocr_screen_text / get_window_tree |
| Input control | 12 | move_mouse / click_mouse / double_click_mouse / right_click_mouse / drag_mouse / scroll_mouse / press_key / hotkey / type_text / send_text_to_window / key_down / key_up |
| Wallpaper | 4 | wallpaper_list / wallpaper_set / wallpaper_pause / wallpaper_stop(Wallpaper Engine CLI 集成) |
| Emotional recovery | 4 | detect_emotional_distress / soothe_pet / suggest_recovery_activity / track_emotional_state(4 个工具均通过 `ctx.char_id` 索引 `Lazy<RwLock<HashMap<String, EmotionalState>>>` 按角色读写情绪状态,跨角色完全隔离) |
| Meta tools | 1 | ToolSearchTool |

> **风险等级申报**:`Tool::risk()` 默认返回 `Safe`,未覆盖该方法的工具会绕过权限矩阵直接放行。具有副作用的工具已按实际风险显式申报:输入控制 12 工具(move_mouse / click_mouse / press_key / hotkey / type_text 等)与媒体键 6 工具(media_play_pause / volume_up / mute 等)申报 `InputControl`,`take_screenshot` 申报 `InputControl`(并改为非只读、需用户确认),天气查询申报 `Network`。申报后矩阵才真正生效——例如默认 `fs-read` 访问级别下 InputControl 工具会被 Deny(附"提升至 FullControl"提示),Network 工具会 Ask。

> **工具行为要点**:`get_window_info` 经注入的 `AppHandle` 读取当前角色 `WebviewWindow` 的真实几何(`outer_position` / `outer_size` / `is_visible` / `is_always_on_top`)并返回实际数据,句柄或窗口缺失时返回明确错误,不再返回"已入队"占位结果。`set_pet_state` / `set_mood` 在 `validate_input` 按 schema 枚举校验取值(state:idle/active/sleeping/thinking/listening;mood:happy/calm/sad/excited/angry/neutral),非法值返回带合法列表的错误而非静默接受;`play_animation` 的动画名为模型私有 motion 名,无固定全集,故仅做非空校验。

#### Hook 系统（hooks/）

Hook 系统（hooks/）：PreToolUse / PostToolUse 可扩展拦截点，JSON 配置文件定义匹配规则（Regex）和外部脚本命令，stdin/stdout JSON 协议，fail-open（超时/异常/无效 JSON 默认 allow）；`hooks/runner.rs` 中所有异常路径以 `tracing::warn!` 记录而非静默吞错，便于定位 Hook 配置问题

#### services/ 服务协调层(5 文件)

```rust
pub struct ServiceContext {
    pub memory: RwLock<Option<Arc<MemoryManager>>>,
    pub psychology: RwLock<Option<Arc<PsychologyManager>>>,
    pub proactive: RwLock<Option<Arc<ProactiveOrchestrator>>>,
}
```

- `MemoryService` — 写入 / 读取 / 偏好 / 日记同步 / 今日上下文(无状态,所有方法接收 `&MemoryManager` + `&str char_id` 参数,由调用方按 char_id 路由)
- `PetService` — 桥接 pet_tools 动作队列与前端引擎
- `ProactiveService` — 持有 ProactiveOrchestrator
- `TodoService` — 加载持久化待办列表

> 多角色改动:`RelationshipService` 已删除(原持有的 PsychologyManager 已由 `CharacterInstance` 直接持有,关系系统调用不再经过此服务);`MemoryService` 全局单例已删除,改为无状态工具类。

---

### 5.7 psychology/ 五层心理架构

**架构定位**:五层心理架构(Persona → Needs → Appraisal → Emotion → BehaviorDrive)+ 关系阶段状态机 + Homeostasis 稳态引擎 + 昼夜节律。

#### 第 1 层:persona.rs — 长期人格特质

**AttachmentStyle**(依恋模式,3 维连续):
```rust
pub struct AttachmentStyle {
    pub secure: f64,    // 默认 0.6 - 信任他人、自在亲密
    pub anxious: f64,   // 默认 0.3 - 害怕被抛弃、过度寻求确认
    pub avoidant: f64,  // 默认 0.2 - 回避亲密、偏好独立
}
```

**PersonaTraits**(8 项,0.0-1.0):
| 特质 | 默认 | 说明 |
|------|------|------|
| warmth | 0.75 | 温暖度 |
| playfulness | 0.60 | 顽皮度 |
| sensitivity | 0.55 | 敏感度 |
| resilience | 0.60 | 韧性 |
| curiosity | 0.70 | 好奇心 |
| sociability | 0.55 | 社交性 |
| expressiveness | 0.60 | 表达欲 |
| independence | 0.50 | 独立性 |

**调制系数**:
- `sensitivity_multiplier()` = 0.7 + sensitivity * 0.6(范围 0.7~1.3)
- `resilience_multiplier()` = 0.6 + resilience * 0.9(范围 0.6~1.5)

`from_expression(tsundere, clingy, genki, sass, healing, curiosity)` — 6 维表演参数 → 心理特质映射

`apply_trait_modulation()` — 根据特质调制 set_points 和 recovery_rates

#### 第 2 层:needs.rs — 心理需求(5 项)

| 需求 | 默认 | 语义 |
|------|------|------|
| belonging | 0.40 | 归属感(值越高 = 越缺乏) |
| autonomy | 0.35 | 自主性 |
| security | 0.25 | 安全感 |
| novelty | 0.45 | 新鲜感 |
| expression | 0.35 | 表达欲 |

#### 第 3 层:appraisal.rs — 认知评价(6 项)

| 评价 | 默认 | 说明 |
|------|------|------|
| threat | 0.0 | 威胁感 |
| rejection | 0.0 | 拒绝感 |
| control | 0.5 | 控制感 |
| fairness | 0.5 | 公平感 |
| novelty | 0.3 | 新奇度 |
| significance | 0.5 | 重要性(放大系数 = 0.5 + significance*0.5) |

**Appraisal → Emotion 增量映射**(心理学固定映射):
- Threat↑ → Fear↑(threat * 0.20 * m + (1-control) * 0.10 * m)
- Rejection↑ → Sadness↑(rejection * 0.20 * m)+ Loneliness↑(rejection * 0.12 * m)
- Fairness↓ → Anger↑((1-fairness) * 0.15 * m)
- Novelty↑ → Curiosity↑(novelty * 0.15 * m)

#### 第 4 层:emotion.rs — 情绪状态(7 项)

```rust
pub enum EmotionLabel { Joy, Sadness, Anger, Fear, Closeness, Loneliness, Curiosity }
```

| 情绪 | 默认 |
|------|------|
| joy | 0.35 |
| sadness | 0.05 |
| anger | 0.05 |
| fear | 0.10 |
| closeness | 0.35 |
| loneliness | 0.15 |
| curiosity | 0.45 |

> Trust 已从 EmotionState 移除,仅保留在 RelationshipState。

关键方法:
- `valence()` — 效价(-1.0~1.0)
- `arousal()` — 唤醒度(0.0~1.0)
- `dominant()` — 主导情绪

**Live2D 表情映射**:Joy/Closeness → "shy";Anger/Fear → "cry";其余 → ""

#### 第 5 层:behavior_drive.rs — 行为驱动(8 项)

| 驱动 | 默认 | 来源 |
|------|------|------|
| approach | 0.2 | Llm / Rule |
| avoid | 0.1 | Llm / Rule |
| explore | 0.3 | Llm / Rule |
| express | 0.2 | Llm / Rule |
| rest | 0.3 | Llm / Rule |
| observe | 0.4 | Llm / Rule |
| play | 0.2 | Llm / Rule |
| help | 0.1 | Llm / Rule |

`RuleBasedDriveResolver` — 规则决策路径(不调用 LLM),由需求 + 情绪 + Persona 调制

#### homeostasis.rs — 心理稳态引擎

**核心公式**:

1. **指数回归** `regress(value, set_point, rate, dt)`:
   ```
   value += (set_point - value) * (1 - exp(-rate * dt))
   ```

2. **带噪声回归** `fluctuate(value, set_point, rate, dt, noise_amp)`:
   - 正常回归到 set_point
   - 小幅随机噪声(随 √dt 缩放)
   - 极值回避:> 0.85 向中拉力,< 0.15 向中拉力

3. **需求非对称回归** `need_decay`:
   - 低于 set_point(已满足):缓慢回归(速率 ×0.5)
   - 高于 set_point(未满足):set_point 缓慢上升(饥饿感 hunger_rate=0.005*dt,上限 0.85)

**昼夜节律调制**(4 锚点线性插值):

| 时段 | 中点 | joy | sadness | fear | closeness | loneliness | curiosity | recovery_mult |
|------|------|-----|---------|------|-----------|-----------|----------|---------------|
| 早晨 | 8.5 | -0.05 | 0 | 0 | -0.03 | 0 | +0.08 | 1.0 |
| 下午 | 14.5 | +0.05 | -0.03 | 0 | +0.05 | -0.05 | 0 | 1.2 |
| 傍晚 | 20.5 | 0 | 0 | 0 | +0.08 | 0 | -0.05 | 0.9 |
| 深夜 | 2.5 | -0.08 | +0.05 | +0.03 | +0.05 | +0.10 | -0.08 | 0.8 |

#### mood.rs — UI 展示层(不参与决策)

```rust
pub struct MoodSnapshot {
    pub valence: f64,            // 效价 -1.0~1.0
    pub arousal: f64,            // 唤醒度 0.0~1.0
    pub primary_emotion: EmotionLabel,
    pub secondary_emotion: EmotionLabel,
    pub primary_intensity: f64,
    pub fatigue: f64,            // 疲劳度 0-100
    pub stress: f64,             // 压力 0-100
    pub relationship_score: f64, // 关系综合分 0-100
}
```

- `fatigue = (last_interaction_secs/60 * 0.5 + need_burden*40).clamp(0, 100)`
- `stress = ((fear + anger + sadness)/3 * 60 + security*40).clamp(0, 100)`
- `relationship_score = (trust*30 + intimacy*30 + familiarity*20 + respect*10 + dependency*10).clamp(0, 100)`

#### mood_cue.rs — Live2D 快速通道

纯规则驱动,无 LLM 调用,无 IO。8 条默认规则(按优先级):exhausted / stressed / excited / cozy / anxious / sad / neutral_curious / calm_idle。

#### relationship.rs — 关系状态机

**6 阶段永久态** `RelationshipStage`:

| 阶段 | intimacy 门槛 | interactions 门槛 |
|------|--------------|-------------------|
| Stranger | < 0.10 | 0 |
| Acquainted | >= 0.10 | >= 3 |
| Familiar | >= 0.25 | >= 15 |
| Close | >= 0.45 | >= 50 |
| Intimate | >= 0.65 | >= 150 |
| Soulmate | >= 0.81 | >= 500 |

**3 种临时态** `TemporaryStage`:
- `Soothing` — 安抚态(用户情绪低落)
- `LowActivity` — 低活跃态(缺席 >= 48h && intimacy > 0.10)
- `Reconnecting` — 重新连接态(缺席 >= 72h)

**RelationshipState**(5 维 + 阶段 + 临时态 + 里程碑 + 统计):
- trust(默认 0.30)、intimacy(0.15)、respect(0.40)、dependency(0.20)、familiarity(0.10)

**StageStrategy**(15 字段):tone / formality / enthusiasm / proactivity_level / max_daily_proactive / icebreaker_threshold_hours / memory_recall_depth / personal_question_limit / share_self_disclosure / response_length / empathy_level / humor_frequency / allow_casual_address / allow_physical_reference / privacy_radius

**deltas_from_interaction**(心理学驱动,硬约束:intimacy 正向使用 `(intensity * 2.0).floor() + 1.0` 放慢关系进展):
- `trust = (fairness*0.03*positive - threat*0.03*sig - rejection*0.02*sig) * 2.0`
- `intimacy = closeness*0.02*positive - rejection*0.02*sig`
- `respect = (fairness - 0.5)*0.02*sig`
- `dependency = positive*0.01 - negative*0.005`
- `familiarity = 0.01 + sig*0.005`

#### relationship_log.rs — 关系演化日志

- `RelationshipLogEntry` — 单轮日志(200 条上限,FIFO)
- `RelationshipDailySummary` — 每日摘要(90 天上限)
- 持久化:`{user_data_dir}/psychology/relationship_log.json`

#### pet_state.rs — 桌宠衍生状态(18 种,仅 UI)

按 valence × arousal 分布:Joyful / Excited / Playful / Calm / Content / Affectionate / Anxious / Angry / Frustrated / Worried / Sad / Tired / Sleepy / Bored / Lonely / Curious / Shy / Neutral

#### manager.rs — PsychologyManager 中枢

```rust
pub struct PsychologyManager {
    state: Arc<RwLock<PsychologySnapshot>>,
    persistence_path: PathBuf,
    persist_lock: Mutex<()>,
    micro_tick_count: Mutex<u32>,  // 累积 20 次才持久化
    /// 当前角色的 ResourceManifest（用于交互反馈表情映射）
    manifest: Option<Arc<crate::engine::manifest::ResourceManifest>>,
}
```

关键方法:
- `homeostasis_tick()` — 后台定时(如每 60s)
- `micro_tick()` — 高频(每 3-5 秒)
- `apply_llm_output(output: &PsychologyOutput)` — 应用 LLM 产出
- `apply_external_event(event: &WorldEvent)` — 应用世界事件
- `apply_user_interaction(interaction: &str)` — 用户交互反馈(通过 `self.manifest` 查询当前角色 manifest 的 emotion_to_expression_name / interaction_feedback_names / random_mood_expression,避免跨角色串扰)
- `apply_proactive_feedback(positive: bool, char_id: &str)` — Desire Engine 反馈闭环:用户回应主动发话 → 亲密度 +;冷落 → 亲密度 -。增减幅度按 `CharacterBehavior::get_behavior(char_id)` 差异化(Vivian 正向 +0.002 / 负向 -0.003,Nana 正向 +0.005 / 负向 -0.001),由 `ProactiveOrchestrator::on_user_interacted` / `on_ignored` 调用
- `compute_mood()` — 计算 MoodSnapshot(仅 UI)
- `compute_pet_state()` — 计算 PetState(仅 UI)
- `compute_wake_greeting_probability()` — 唤醒问候概率
- 关系系统方法(`record_interaction` / `record_user_sad` / `record_long_absence` 等)
- `with_manifest(manifest: Arc<ResourceManifest>)` — builder 方法,Brain 构建时注入该角色的 ResourceManifest,后续所有表情/动作映射走该 manifest 而非全局静态

**4 种用户交互反馈**:
- `fast_click`:joy +0.04, closeness +0.02 → 表情 "shy"
- `fast_drag`:joy +0.03, anger +0.01, autonomy +0.03 → motion "umbrella_close"
- `pet`:joy +0.06, closeness +0.08, loneliness -0.05 → "shy"
- `long_press`:curiosity +0.04, closeness +0.01 → "eye_roll"

> 多角色隔离:`PsychologyManager` 持有 `Option<Arc<ResourceManifest>>` 字段,交互反馈与随机心情表情均通过该 manifest 实例查询,而非全局静态函数。Brain::build 在构造 PsychologyManager 时通过 `with_manifest(manifest.clone())` 注入对应角色的 manifest,确保 Nana 与 Vivian 各自的表情映射互不污染。

---

### 5.8 emotion/ 情绪分类

**架构定位**:LLM 情绪分类 + 关键词映射 + 嵌入即时分类 + Live2D 表情映射。

| 文件 | 职责 |
|------|------|
| [bridge.rs](file:///g:/vivian-rs/src-tauri/src/emotion/bridge.rs) | `EmotionBridge`(综合关键词 + LLM 分类 + 即时嵌入分类),持有 `Option<Arc<ResourceManifest>>` 字段与 `Option<Arc<EmbeddingEmotionClassifier>>` 即时分类器字段,`emotion_to_expression` 查询通过该 manifest 实例完成,而非全局静态函数 |
| [embedding_classifier.rs](file:///g:/vivian-rs/src-tauri/src/emotion/embedding_classifier.rs) | `EmbeddingEmotionClassifier` —— 基于 `MemoryEmbeddingProvider` 的即时情绪分类器,预置 14 类情绪语料(每类 ~15 条,共 210 条),首次调用时批量嵌入语料并缓存,输入文本通过 Top-K=5 余弦相似度投票决定情绪(K=5 / 阈值 0.25 / LRU 查询缓存 64 条)。`classify()` 返回 `Result<EmotionResult, String>`:嵌入服务不可用或相似度不足时返回 `Err`,**不降级到关键词分析**,由上层决定如何处理(如弹 toast 报错) |
| [llm_classifier.rs](file:///g:/vivian-rs/src-tauri/src/emotion/llm_classifier.rs) | `LlmEmotionClassifier` |
| [mapper.rs](file:///g:/vivian-rs/src-tauri/src/emotion/mapper.rs) | emotion → expression 映射 |
| [response_strategy.rs](file:///g:/vivian-rs/src-tauri/src/emotion/response_strategy.rs) | `EmotionResult` / `ResponseStrategy` / `recommend_detailed()` |

> 硬约束:`user_emotion` / `ai_emotion` 由 LLM 在 JSON 返回中给出,不再用关键词匹配模块代理。

> 多角色隔离:`EmotionBridge::new(psychology, manifest: Option<Arc<ResourceManifest>>)` 与 `with_dependencies` 同步增加 manifest 参数,Brain::build 时通过 `EmotionBridge::new(psychology.clone(), Some(manifest.clone())).with_instant_classifier(...)` 注入对应角色的 manifest 与即时嵌入分类器(分类器复用 `MemoryManager.embedding()` 的 provider,本地哈希 <1ms / 远程嵌入 50-200ms)。`emotion_to_expression` 通过 `self.manifest.as_ref().map(|m| m.emotion_to_expression_name(...))` 查询当前角色 manifest,而非全局 `crate::engine::manifest::emotion_to_expression` 函数(该全局静态已移除)。

#### 三层反应系统(EmotionBridge + ChatController + useInstantReact)

`classify_instant(text)` 是 `EmotionBridge` 的低延迟入口(不更新心理状态、不写缓存、不触发表情),配合前端 `ChatController` 与 `useInstantReact` hook 实现"用户消息 → AI 文本首段 → 反思后处理"三层渐进式情绪反应:

| 层 | 触发时机 | 实现位置 | 写入的参数层 |
|----|---------|---------|-------------|
| Layer 1 | 用户消息发送瞬间(`ChatController.sendMessage`) | `triggerInstantReact(message, char, 'user')` → `analyze_emotion_instant` 命令 → `classify_instant` | Live2D `instant` 层(优先级 1.5) |
| Layer 2 | AI 文本首段完成时(`chat:chunk` 中检测换行或累积达 40 字符,仅触发一次) | `triggerInstantReact(aiText, char, 'ai')` 覆盖 Layer 1 | Live2D `instant` 层 |
| Layer 3 | 反思调用完成(`chat:meta` / `chat:done`) | `chat:meta` 携带的 expression/motion 由 `manual` 层(优先级 4)接管,同时 `useInstantReact` 自动清除 `instant` 层 | Live2D `manual` 层(优先级 4) |

失败处理:`classify_instant` 返回 `Err` 时,`analyze_emotion_instant` 命令传播错误,前端 `triggerInstantReact` catch 后 `emit('toast:show', { type: 'error' })` 弹 toast,**不降级到关键词分析**。

---

### 5.9 proactive/ 主动对话

**架构定位**:自适应间隔 tick 调度的主动行为系统。Tick 间隔由 `compute_adaptive_tick_ms(idle_seconds, char_id)` 根据用户空闲时间动态计算(活跃 < 5 分钟 10s / 5-15 分钟 30s / 15-60 分钟 120s / > 60 分钟 300s),并叠加角色专属 `TickJitterConfig` 随机乘数(Vivian 0.8~1.2 / Nana 0.9~1.4)使多角色 tick 节拍物理错开。后端通过 `proactive_tick` 返回的 `recommended_next_interval_ms` 字段推荐前端下次调度间隔,减少空转 IPC。用户任何交互重置 idle_seconds 为 0,立即恢复活跃档。

#### 13 种触发器

HourlyGreeting / IdleGreeting / TeasingResponse / Icebreaker / WindowTrigger / TopicExtension / MemoryRecall / HealthReminder / Spontaneous / WelcomeBack / MoodDriven / CrossCharacterReply / BystanderInterjection

#### 9 种 PetMindState

```rust
pub enum PetMindState {
    Curious, Bored, Excited, Sleepy, Caring, Playful, Tired, Content,
    Sleep,  // 深夜真正入睡,区别于 Sleepy 困倦
}
```

#### 关键组件

| 文件 | 职责 |
|------|------|
| [behavior.rs](file:///g:/vivian-rs/src-tauri/src/proactive/behavior.rs) | 行为编排 |
| [behavior_modes.rs](file:///g:/vivian-rs/src-tauri/src/proactive/behavior_modes.rs) | 行为模式 |
| [triggers.rs](file:///g:/vivian-rs/src-tauri/src/proactive/triggers.rs) | 13 种触发器 + TriggerThrottle 阈值表 |
| [timing.rs](file:///g:/vivian-rs/src-tauri/src/proactive/timing.rs) | 多级冷却(独立阈值 + 全局最小间隔) + `score_with_weights` 角色专属权重评分 |
| [habits.rs](file:///g:/vivian-rs/src-tauri/src/proactive/habits.rs) | `HabitTracker`(90 天滚动窗口) |
| [icebreaker.rs](file:///g:/vivian-rs/src-tauri/src/proactive/icebreaker.rs) | `IcebreakerGenerator` 多级破冰 |
| [mind_state.rs](file:///g:/vivian-rs/src-tauri/src/proactive/mind_state.rs) | 9 种心理状态 |
| [inner_monologue.rs](file:///g:/vivian-rs/src-tauri/src/proactive/inner_monologue.rs) | 内心独白(冷却 30 分钟,50-120 字) |
| [activity_journal.rs](file:///g:/vivian-rs/src-tauri/src/proactive/activity_journal.rs) | 用户活动日志(Win32 API 每 5s 轮询,FIFO 上限 100) |
| [preference_learner.rs](file:///g:/vivian-rs/src-tauri/src/proactive/preference_learner.rs) | per-trigger EWMA 偏好学习 |

#### services/(生活服务)

- `HealthReminder` — 健康提醒
- `Recommender` — 推荐
- `StressMonitor` — 压力监控

#### topics/(话题管理)

- `DailyTopicPool` — 每日话题池
- `TopicTree` — 话题树(新鲜度维护)
- `recall` — 话题回忆

#### 安静模式

连续被忽略次数达阈值自动进入 1 小时静默。阈值按角色差异化,读取 `CharacterBehavior::get_behavior(char_id).quiet_mode_threshold`(Vivian 5 次 / Nana 2 次)。

#### 多角色去同步(六策略)

防止多角色同时发声的六层互补机制,所有参数在 `character_behavior.rs` 按角色人设差异化配置:

| 策略 | 配置结构 | 作用层 | Vivian | Nana |
|------|----------|--------|--------|------|
| A. Tick 相位抖动 | `TickJitterConfig` | 物理调度 | 乘数 0.8~1.2 | 乘数 0.9~1.4 |
| B. 权重/阈值分化 | `TimingWeights` + `TriggerModifiers` | 决策评分 | idle=0.35, 阈值×1.2, 冷却×1.5, 概率×0.8 | time=0.35, 阈值×0.8, 冷却×0.7, 概率×1.3 |
| C. 发言欲望累积 | `SpeechDesireConfig` | 门控 | growth=0.08, threshold=0.6 | growth=0.04, threshold=0.4 |
| D. 跨角色仲裁 | `ArbitrationConfig` | 发射仲裁 | priority=1, reluctance=2.0(冷却30s) | priority=2, reluctance=4.0(冷却60s), yield_delay=90s |
| E. 情绪漂移周期 | `MoodDriftConfig` | 冷却调制 | volatility=0.3, recovery_rate=0.02(快周期) | volatility=0.1, recovery_rate=0.05(慢周期) |
| F. 触发器领地 | `TriggerAffinity` | 概率竞争 | mood_driven×1.3, icebreaker×1.2, welcome_back×1.3, hourly×0.4 | hourly×1.3, idle×1.2, health_reminder×1.4, mood_driven×0.5 |

**决策链路**(`check_trigger` 重写后的门控分层):

```
speech_desire ≥ threshold (策略C, 仅问候类触发器)
  → 冷却检查 × cooldown_mult (策略B)
  → 到达问候共享冷却 (问候类触发器在 last_interaction_time 后 min_trigger_interval 内硬拦截)
  → TimingJudger::score_with_weights (策略B)
  → score ≥ threshold × threshold_mult (策略B)
  → roll_probability × probability_mult × affinity (策略B+F)
  → compute_overall_cooling × drift_factor (策略E)
  → check_specific_conditions
```

**到达问候共享冷却**:启动问候与唤醒问候由 Brain 生成(`generate_startup_greeting` / `generate_wake_greeting`),不走 tick 触发循环,但成功后调用 `ProactiveOrchestrator::record_greeting_arrival(kind)` 写入共享冷却状态——全局打扰时间戳 `last_interaction_time`、`last_trigger_times[kind]`(键为 `startup_greeting` / `wake_greeting`)、并重置 `last_user_was_away=false`。此后 `min_trigger_interval`(默认 180s)静默期内,WelcomeBack / HourlyGreeting / IdleGreeting / Icebreaker 四个问候类触发器被硬门控拦截(不依赖 TimingJudger 的软评分——冷却分量仅占 15% 权重,不足以压制连续问候)。

**跨角色仲裁**(策略D, `commands/proactive.rs`):

- `SPEECH_RESERVATION: Lazy<RwLock<HashMap<String, f64>>>` — 全局发言时间戳
- `SPEECH_COLLISION_WINDOW_SECS = 5.0` — 碰撞检测窗口
- 发言成功后写入时间戳; 另一角色在窗口内检测到碰撞时,按 `priority` 数值小者优先,让步方延迟 `yield_delay_secs` 再尝试
- 跨角色冷却 = `CROSS_ROLE_COOLDOWN_SECS(15s) × reluctance`

#### ProactiveOrchestrator 编排器

```rust
pub struct ProactiveOrchestrator {
    state: Arc<RwLock<ProactiveState>>,
    persistence_path: std::path::PathBuf,
    // ... 其他字段 ...
    /// 角色 ID(用于按角色差异化行为参数 + 持久化路径隔离)
    char_id: String,
    /// 发言欲望累积器(策略 C)
    speech_desire: RwLock<f64>,
    /// 情绪漂移相位(策略 E, 0~TAU 循环)
    mood_drift_phase: RwLock<f64>,
}
```

**构造与持久化隔离**:

- `ProactiveOrchestrator::new(char_id: &str)` — 接收 char_id 参数,持久化路径按角色隔离到 `get_character_data_dir(char_id).join("proactive")`(即 `<user_data_dir>/characters/<char_id>/proactive/`),不再使用全局共享的 `<user_data_dir>/proactive/` 目录
- `Brain::build` 中通过 `ProactiveOrchestrator::new(char_id)?` 构造,每个角色拥有独立的编排器实例
- `Default` impl 默认 `char_id = "vivian"`

**CharacterBehavior 参数读取**:

`ProactiveOrchestrator` 在以下决策点读取 `crate::character_behavior::get_behavior(&self.char_id)`:

| 决策点 | 字段 | Vivian | Nana |
|--------|------|--------|------|
| `on_user_interacted` 调用 `apply_proactive_feedback(true, char_id)` | `proactive_feedback_positive` | +0.002 | +0.005 |
| `on_ignored` 调用 `apply_proactive_feedback(false, char_id)` | `proactive_feedback_negative` | -0.003 | -0.001 |
| MoodDriven 需求触发阈值 | `mood_driven_need_threshold` | 0.85 | 0.65 |
| MoodDriven 孤独触发阈值 | `mood_driven_loneliness_threshold` | 0.75 | 0.55 |
| 亲密度冷却系数基础值 | `intimacy_cooldown_multiplier` | ×0.8 | ×1.2 |
| 安静模式触发次数 | `quiet_mode_threshold` | 5 | 2 |
| Tick 间隔随机乘数(策略A) | `tick_jitter` | 0.8~1.2 | 0.9~1.4 |
| 时机评分权重向量(策略B) | `timing_weights` | idle=0.35 偏重 | time=0.35 偏重 |
| 阈值/冷却/概率倍率(策略B) | `trigger_modifiers` | 阈值×1.2 冷却×1.5 概率×0.8 | 阈值×0.8 冷却×0.7 概率×1.3 |
| 发言欲望增长/阈值(策略C) | `speech_desire` | growth=0.08 threshold=0.6 | growth=0.04 threshold=0.4 |
| 仲裁优先级/让步系数(策略D) | `arbitration` | priority=1 reluctance=2.0 | priority=2 reluctance=4.0 yield=90s |
| 情绪漂移振幅/速率(策略E) | `mood_drift` | volatility=0.3 rate=0.02 | volatility=0.1 rate=0.05 |
| 触发器概率领地(策略F) | `trigger_affinity` | mood_driven×1.3 hourly×0.4 | hourly×1.3 mood_driven×0.5 |

**ProactiveState 持久化**:

- `<user_data_dir>/characters/<char_id>/proactive/state.json` — 编排器状态(mind_state / last_interaction_time / last_trigger_times / quiet_mode / ignored_count 等;last_trigger_times 含 `startup_greeting` / `wake_greeting` 键,由 `record_greeting_arrival` 写入;last_interaction_time 驱动问候类触发器硬门控)
- `<user_data_dir>/characters/<char_id>/proactive/topics.json` — 话题冷却
- `<user_data_dir>/characters/<char_id>/proactive/habits.json` — 习惯数据

#### 思绪生命周期（thought_lifecycle）

事件驱动的内心独白与主动表达架构,让角色的"想说点什么"从概率 roll 转为"事件→种子→滋长→阈值表达"的自然积累过程。

**ThoughtSeed（思绪种子）**:由 `ThoughtTriggerEvaluator::detect_seeds` 在每个 tick 产出,共 14 类种子(按 `trigger_kind` 标签区分):

| trigger_kind | thought_key | intensity | 触发条件 |
|---|---|---|---|
| `going_to_rest` | `going_to_rest` | 0.85 | `going_to_rest` 信号(高优先级) |
| `waking_up` | `waking_up` | 0.75 | `waking_up` 信号(高优先级) |
| `user_left` | `user_left` | 0.35×rel | 用户离场(cooldown 180s) |
| `user_return` | `user_return` | miss_factor×rel | 用户回归(cooldown 120s) |
| `long_silence` | `user_miss` | 0.2+factor×0.55 | 无互动>0.5h(cooldown 1800s) |
| `weather_shift` | `weather_rain` / `weather_{...}` | 0.35 / 0.2 | RainStarted / WeatherChanged |
| `environmental_event` | `sunset` / `sunrise` / `season_change` | 0.3~0.4 | 日出/日落/季节变化 |
| `festival` | `festival_{...}` | 0.55 | FestivalArrived(高优先级) |
| `activity_pattern` | `activity_{title}` | 0.25~0.4×rel | 同一活动连续≥2次 |
| `emotion_accumulation` | `emotion_shift` | 0.3~0.45×rel | mood.primary_intensity>0.6 |
| `cross_character_spoke` | `companion_spoke_{id}` | 0.35 | 室友最近发言(cooldown 600s) |
| `want_to_share_with_roommate` | `want_share_roommate_*` | 0.50~0.60 | 分享诱因(cooldown 1800s),起始强度提升使单次诱因即可接近表达阈值 |
| `deep_reflection` | `deep_reflection` | 0.5 | 22-1 点且>24h 未反思 |
| `background` | `background` | 0.15 | 每 7200s 按 0.15 概率随机 |

其中 `rel_weight = relationship_weight(intimacy) = (0.4 + intimacy×0.8).clamp(0.4, 1.2)`,但 `want_to_share_with_roommate` 种子**不乘 rel_weight**——想和室友聊的动机不应被 user↔agent intimacy 抑制。

**分享诱因检测**(`want_to_share_with_roommate` 种子,三类触发源,起始强度均提升使单次诱因即可接近表达阈值):
1. **用户行为类别切换**(起始强度 0.55):从 `activity_snapshot` 最近两条检测 category 变化(如"编程"→"游戏"),过滤掉"系统"/"其他"分类
2. **显著世界事件**(起始强度 0.60):RainStarted / FestivalArrived
3. **情绪累积**(起始强度 0.50):loneliness>0.6 且有在线室友

**ThoughtPhase（5 阶段）**:`Seed → Growing → Active → Expressed → Faded`,通过 intensity 阈值自动推进:
- `intensity ≥ 0.30`(INNER_MONOLOGUE_THRESHOLD)→ Growing,产生内心独白
- `intensity ≥ 0.70`(PROACTIVE_SHARE_THRESHOLD)→ Active,可主动表达
- 表达后强度衰减 0.75,独白后衰减 0.35

**ThoughtLifecycle 管理器**:
- `seed_thought(key, ...)` — 播种或 nourish 已有种子(同 key 合并,增幅 `min(0.15, base×0.5)`)
- `tick(now, user_present)` — 推进所有思绪(NATURAL_DECAY_PER_SEC=0.0008,用户在场时 desire 增长 0.003/s)
- `pick_monologue_candidate()` — 选最强可独白思绪(intensity≥0.30)
- `pick_share_candidate()` — 选最强可表达思绪(intensity≥0.70)
- `MAX_CONCURRENT_THOUGHTS = 4` — 超过时驱逐最弱的

**桥接路径**(`maybe_spawn_inner_monologue` 返回 `Option<(thought_key, context_hint, trigger_kind)>`):

| trigger_kind | 桥接目标 | 门控条件 |
|---|---|---|
| `want_to_share_with_roommate` | `generate_thought_share_to_roommate` → `CrossCharacterReply` trigger → `deliver_cross_character_messages` | 不要求 leader 身份(非 leader 也可主动找室友聊),不要求 user_present |
| 其他 | `generate_thought_share_message` → `Spontaneous` trigger → 对用户说 | `is_speaking_leader && user_present && !lay_low` |

**对室友说 vs 对用户说**:
- `generate_thought_share_message` — "忍不住开口对用户说",20 字以内,prompt 风格为自然开口
- `generate_thought_share_to_roommate` — "想和室友聊聊",20 字以内,prompt 风格为室友间闲聊,接受 `roommate_name` 参数

#### 跨角色聊天真实化（四层架构）

跨角色对话从"被动响应+概率 roll"升级为"事件驱动+思绪桥接+关系差异化+共同情境"的四层架构:

**1. 共同情境注入**(`cross_character.rs::send`):
- `build_handoff_context` 调用处追加 `activity_journal.to_brief()` 作为 `[共同观察]` 段落
- 跨角色对话 prompt 包含"你们都在看着用户做这些事",让对话有共同话题而非空泛闲聊
- 双方观察同一个用户,取发起方的 activity_journal 即可

**2. 关系状态差异化**(`compute_cross_reply_probability`):
- A↔B intimacy 调节:`prob += (intimacy - 0.5) × 0.20`(关系近 +0.10,远 -0.10)
- 近期互动频率:1h 内聊过则 -0.10(防刷屏)
- 基线 0.08 + loneliness×0.30 + sadness×0.10 - joy×0.05 + 用户不在场 +0.15
- **三人共处一室时间衰减**(替代原 5min 硬屏蔽):按用户最后交互时间衰减,让室友对用户说话时本角色可低概率接话——< 2min ×0.0(用户真正在打字,不打断)/ 2-5min ×0.4(用户可能停顿,低概率接话)/ 5-15min 正常概率 / >15min +0.15(用户实际离开)

**3. 事件驱动触发**(`want_to_share_with_roommate` 种子):
- 见上文思绪生命周期章节,30 分钟冷却
- intensity 不乘 rel_weight,避免被 user↔agent intimacy 抑制

**4. 内心独白桥接 talk_to_character**:
- 见上文桥接路径表,`want_to_share_with_roommate` 走独立路径
- 复用 `deliver_cross_character_messages` 投递,标记 `cross_character_reply` trigger

**跨角色死锁解除与三人共处一室互动**(协同修复 + 互动增强):

| 问题根因 | 修复/增强方案 |
|---|---|
| `is_user_chatting` 仅依赖会话活跃状态(默认 30 分钟超时),用户离开后仍误判为"正在聊天",抑制 `CrossCharacterReply` | `is_user_chatting = 会话活跃 && system_idle_seconds < 90.0`,结合系统级空闲时间准确判断用户真实活跃状态;此外 `compute_cross_reply_probability` 引入三人共处一室时间衰减(< 2min ×0.0 / 2-5min ×0.4 / 5-15min 正常 / >15min +0.15)替代原 5min 硬屏蔽,让室友对用户说话时本角色可低概率接话 |
| Leader 选举机制阻止非 leader 角色触发任何触发器,导致"leader 等室友发言,室友等成为 leader" | 新增 `evaluate_cross_character_reply_only` 方法,允许非 leader 角色独立评估 `CrossCharacterReply` 触发器;非 leader 路径调用 `deliver_cross_character_messages` 投递跨角色消息,不与 leader 的用户消息冲突 |
| `LAST_SPOKEN` 仅在角色发"对用户"消息时更新,非 leader 角色只发跨角色消息不发用户消息,其 `LAST_SPOKEN` 永远为空 | `CROSS_CHARACTER_BUS.send` 成功后调用 `record_cross_character_spoken`（speak 模式,更新时间戳+文本）或 `touch_last_spoken`（非 speak 模式,仅更新时间戳不覆盖文本）更新双方的 `LAST_SPOKEN`/`LAST_SPOKEN_TEXT` |
| `CrossCharacterReply` 要求室友 90s 内发言过,冷启动时两角色互相等待 | 冷启动破冰:室友在线但 `last_spoke_secs_ago == None`(从未发言)时以 20% 概率触发 |
| `BystanderInterjection` 触发器在 `try_llm_content` 早返检查列表中被漏掉,实际无法生成 LLM 内容 | 将 `BystanderInterjection` 加入 `try_llm_content` 的 match 列表 |
| 用户与某角色聊天时,其他在线角色无法自然插话加入对话 | 三人共处一室语义:`commands/chat.rs` 写入旁观记忆后以 8% 概率调用其他在线角色的 `ProactiveOrchestrator::seed_roommate_cue`,设置 30s TTL 信号(from_name + topic_brief);被 cue 角色的 `compute_bystander_interjection_probability` +0.35,并在 prompt 中注入"室友刚 cue 了你"提示让插话更自然;用户活跃聊天时(< 5min)旁观插话概率 +0.10(旁听素材丰富) |

**BystanderInterjection 触发器**(`compute_bystander_interjection_probability`):
- 基线 0.10 + curiosity×0.20 + loneliness×0.10 + closeness×0.05 - joy×0.05
- 用户活跃聊天时(< 5min)+0.10
- 被室友 cue 时(30s 内有效信号)+0.35
- 上限 0.85,下限 0.05

**roommate_cue 信号机制**(`ProactiveOrchestrator::seed_roommate_cue` + `check_roommate_cue`):
- `seed_roommate_cue(from_name, topic_brief)` — 由 `commands/chat.rs` 在写入旁观记忆后以 8% 概率调用,设置 `roommate_cue: Arc<Mutex<Option<(String, String, f64)>>>` 信号(30s TTL)
- `check_roommate_cue()` — 检查信号是否在 30s 内有效,返回 `Option<(from_name, topic_brief)>`
- 信号被 `compute_bystander_interjection_probability` 消费(概率提升)和 `try_llm_content` 消费(prompt 注入"室友刚 cue 了你"提示)
- 采用信号机制而非 thought_lifecycle 种子,因为 `new_seed` 会把 intensity clamp 到 0.4 上限,无法达到 PROACTIVE_SHARE_THRESHOLD(0.70);信号机制直接提升 BystanderInterjection 概率,绕过种子强度限制

**关键函数**(`commands/proactive.rs`):
- `record_cross_character_spoken(char_id, text)` — 写入 `LAST_SPOKEN` 时间戳 + `LAST_SPOKEN_TEXT`（截断 80 字符）
- `touch_last_spoken(char_id)` — 仅更新 `LAST_SPOKEN` 时间戳,不覆盖文本（用于非 speak 模式的目标角色）
- `deliver_cross_character_messages(...)` — 公共函数,统一 leader 和非 leader 路径的跨角色消息投递逻辑

---

### 5.10 world/ 真实世界感知

**架构定位**:让 Vivian 在真实世界中"活着"——即使用户不交互也能感知世界。`mod.rs` 作为中央 World 状态提供者,管理所有感知子模块的缓存快照,通过 `refresh_perception()` 异步轮询 + 事件回调双轨更新。

| 文件 | 职责 |
|------|------|
| [mod.rs](file:///g:/vivian-rs/src-tauri/src/world/mod.rs) | 中央 World 状态提供者,管理所有子模块的缓存快照(RwLock),`refresh_perception()` 异步轮询(音量/网络/前台窗口),`start_system_polling` 定时刷新,`get_snapshot()` 聚合快照 |
| [time_perception.rs](file:///g:/vivian-rs/src-tauri/src/world/time_perception.rs) | 本地时间 / 周几 / 周末 / 季节 / 24 节气 / 公历与农历节日 / 日出日落(NOAA 简化算法) |
| [weather.rs](file:///g:/vivian-rs/src-tauri/src/world/weather.rs) | Open-Meteo 免费接口(无需 API Key),WMO 代码到中文描述映射,TTL 缓存 |
| [events.rs](file:///g:/vivian-rs/src-tauri/src/world/events.rs) | 比较前后 `WorldSnapshot` 产出 8 种 `WorldEventKind` |
| [geolocation.rs](file:///g:/vivian-rs/src-tauri/src/world/geolocation.rs) | Windows.Devices.Geolocation 系统定位 + ipwho.is API IP 级城市定位,启动时自动检测 + 30 分钟定期轮询 + 前端手动触发(5s 防抖),`enrich_city_info` 补充城市/省份/国家信息 |
| [volume.rs](file:///g:/vivian-rs/src-tauri/src/world/volume.rs) | Windows Core Audio API (`IAudioEndpointVolume`) 获取主输出设备音量(0-100),通过 `spawn_blocking` 隔离 COM 调用避免 STA/MTA 冲突 |
| [music.rs](file:///g:/vivian-rs/src-tauri/src/world/music.rs) | Windows SMTC (System Media Transport Controls) 事件驱动媒体检测,`PlaybackInfoChanged` 回调实时捕获标题/艺术家/专辑/播放状态,非轮询 |
| [foreground_window.rs](file:///g:/vivian-rs/src-tauri/src/world/foreground_window.rs) | Win32 FFI 获取前台聚焦窗口(标题/进程名/PID),自动跳过应用自身窗口(PID 比较) |
| [network_watch.rs](file:///g:/vivian-rs/src-tauri/src/world/network_watch.rs) | COM `INetworkListManagerEvents::ConnectivityChanged` 事件回调,本机网络适配器连通性变化时即时更新状态 |
| [network_status.rs](file:///g:/vivian-rs/src-tauri/src/world/network_status.rs) | 网络状态查询(已连接/已断开/未知),供 network_watch 和 mod.rs 使用 |
| [state.rs](file:///g:/vivian-rs/src-tauri/src/world/state.rs) | `WorldState` 窄义容器（用户实体 + 行为日志 + 封存回调），与 `WorldStateProvider` 缓存的 environment 字段分离 |
| [entity_state.rs](file:///g:/vivian-rs/src-tauri/src/world/entity_state.rs) | 用户实体状态机（在场/离开/预期回归/持续活动）+ `ExpectationEngine` 从对话抽取预期回归 + 活动意图（Gap 1 快速通道：用户明说"我去上班了"时同时产出 `ExpectedReturn` + `ActivityIntent`，规则抽取直接写入 `current_activity`，不等待 LLM 反思 tick） |
| [system_metrics.rs](file:///g:/vivian-rs/src-tauri/src/world/system_metrics.rs) | 系统指标采集(CPU/内存/网速) |
| [user_behavior.rs](file:///g:/vivian-rs/src-tauri/src/world/user_behavior.rs) | 用户行为日志（已封存的持续状态事件，带 duration，不被 LLM 压缩），供认知引擎整理为习惯 Belief |

**关键设计决策**:

- **COM 线程安全**:音量(Core Audio)和网络(NetworkWatch)模块均依赖 COM API,Tauri/WebView2 初始化 STA 线程,直接调用会触发 `RPC_E_CHANGED_MODE`。解决方案:音量查询通过 `tokio::task::spawn_blocking` 隔离到独立线程池,网络事件通过独立 COM 线程 + 事件回调写入全局状态。
- **前台窗口缓存保留**:`refresh_perception()` 中,当 `get_foreground_window()` 返回 PID=0(应用自身窗口)时,不更新缓存,保留上一次的外部窗口快照,避免前端显示"无活跃窗口"。
- **IP 地理位置三层更新**:①启动时自动检测(条件:缺少坐标或缺少城市名) ②30 分钟定期轮询(补充 NetworkWatch 无法捕获的公网 IP 变化) ③前端点击位置卡片手动触发(`invoke('auto_detect_location')`,5 秒防抖)。
- **位置注入提示词**:城市/省份/国家信息通过 `EnvironmentContext.with_world()` 填入,`build_context_block()` 渲染为三语文本(中:"他在{城市}。" / 英:"They're in {city}." / 日:"彼は今{city}にいます。"),注入 LLM 对话 prompt。

**8 种 WorldEventKind**:天气变化 / 开始下雨 / 节日到来 / 节气切换 / 日出 / 日落 / 季节变化 / 长时间缺席

**WorldSnapshot** 注入对话 prompt,让 Vivian 在对话中"知道"真实世界状态(含时间/天气/音量/媒体/前台窗口/网络/地理位置)。

---

### 5.11 persona/ 人格引擎

**架构定位**:模块化人设渲染引擎 + worldbook 动态激活状态机 + 场景选择 + 人格↔表情双向同步。人设文件采用分层模块化结构,拒绝形容词堆砌,使用场景化行为锚点 + "触发→反应"行为脚本。

| 文件 | 职责 |
|------|------|
| [schemas.rs](file:///g:/vivian-rs/src-tauri/src/persona/schemas.rs) | `PersonaConfig` 配置结构 |
| [dynamic_profile.rs](file:///g:/vivian-rs/src-tauri/src/persona/dynamic_profile.rs) | `DynamicBehaviorProfile` 动态行为画像 |
| [prompt_render.rs](file:///g:/vivian-rs/src-tauri/src/persona/prompt_render.rs) | Prompt 渲染(`render_character_block`/`render_examples_block`) + 提示词占位符泄露检测 |
| [persona_card.rs](file:///g:/vivian-rs/src-tauri/src/persona/persona_card.rs) | 人设卡片渲染 |
| [persona_decision.rs](file:///g:/vivian-rs/src-tauri/src/persona/persona_decision.rs) | 人设决策逻辑 |
| [scene_selector.rs](file:///g:/vivian-rs/src-tauri/src/persona/scene_selector.rs) | `SceneModeSelector`(5 信号融合) |
| [worldbook.rs](file:///g:/vivian-rs/src-tauri/src/persona/worldbook.rs) | `WorldbookEngine` 三态状态机 + constant 常驻层 |
| [mod.rs](file:///g:/vivian-rs/src-tauri/src/persona/mod.rs) | 模块导出 |

**模块化人设文件结构**(`prompts/characters/{char_id}/`):

每个角色拥有 8 个独立 Markdown 文件,职责单一、可独立维护:

| 文件 | 内容 | 设计原则 |
|------|------|---------|
| `identity.md` | 核心身份锚点(你是谁) | 一句话定义核心身份,用具体行为而非形容词 |
| `personality.md` | 场景化人格 | "触发→反应"行为脚本,具体场景替代形容词堆砌(如"被吐槽时→翻白眼但不真生气") |
| `speech.md` | 说话风格 | 节奏/语气/口头禅/自称/句尾/停顿习惯/禁用模式,含正反例 |
| `examples.md` | Few-shot 示例 | 约 5 个角色专属对话示例,避免模型模仿特定句子 |
| `background.md` | 背景设定 | 日常生活/作息/环境细节,让角色落地 |
| `interests.md` | 兴趣爱好 | 具体喜好而非泛泛而谈 |
| `relationships.md` | 关系设定 | 与用户/室友的关系定位 |
| `appearance.md` | 外观描述 | 发色/瞳色/服装/体型等视觉特征 |

**通用框架层**(`prompts/framework/`,所有角色共享,7 个文件):

| 文件 | 内容 |
|------|------|
| `chat_style.md` | 聊天风格通用规则(像发微信不像写作文;短碎片回复/犹豫改口/状态波动/话题偏好触发:感兴趣的话题多说无感的简短带过/情绪化反复:不必每次逻辑自洽) |
| `address_rules.md` | 称呼规则 |
| `conversation_rhythm.md` | 对话节奏 |
| `session_rules.md` | 会话规则(新会话/续聊/首次见面) |
| `speaker_prefix.md` | 说话者前缀标记 |
| `output_format.md` | JSON 输出格式规范 |
| `safety.md` | 安全规则(身份保护/内容边界/工具协议) |

**8 种 SceneMode** + **5 种 StylePreset**

#### WorldbookEngine 三态 + 常驻层

`Archived`(归档)/ `Dormant`(休眠)/ `Active`(激活)

**WorldbookParams**(默认值):
- bu=20.0, bm=8.0, gamma=0.5, lambda=0.3, alpha=1.5, beta=0.3
- active_threshold=30.0, max_active=8

**constant 常驻条目**(`WorldbookEntry.constant: bool`):

用于核心身份、关系里程碑等每轮都必须注入的设定,不参与激活度状态机:

- `update_activation` 跳过 `constant=true` 的条目,激活度始终保持初始值
- `get_injectable_entries` 先全量收集常驻条目排在最前,再处理动态条目过滤/排序/截断
- 常驻条目不占 `max_active` 配额、不受 `active_threshold` 限制
- 通过 `new_constant(id, content)` 构造,或 `add_constant_entry` / `remove_entry` API 管理
- `load_from_disk` 合并时同步 `constant` 字段,存储中独有的常驻条目追加到默认清单

#### 人格↔表情双向同步

`persona.rs` 实现人格状态与 Live2D 表情的双向同步:
- **正向**(人格→表情):`PsychologyManager.apply_llm_output` 驱动表情选择,已有路径
- **反向**(表情→人格):Live2D 表情触发后反向微调人格参数(如长时间 "shy" 表情 → warmth +0.001,expressiveness -0.0005),让人格特质随表演习惯缓慢演化
- 双向闭环每轮对话最多触发一次,避免过度调制

---

### 5.12 dialogue/ 对话管理

**架构定位**:对话历史 + 意图判断 + 话题追踪。并发模型:模块内部所有 `Mutex` 替换为 `parking_lot::Mutex`(约 25 处 `.lock().unwrap()` 简化为 `.lock()`),避免 std Mutex 中毒 panic;guard 不跨 await 持有,符合 Tokio Send 边界要求。

| 文件 | 职责 |
|------|------|
| [history.rs](file:///g:/vivian-rs/src-tauri/src/dialogue/history.rs) | `DialogueManager`(max_history_len / max_buffer_size=10 / flush_interval=2s) |
| [intent_judge.rs](file:///g:/vivian-rs/src-tauri/src/dialogue/intent_judge.rs) | `IntentJudge`(END_CONFIDENCE_THRESHOLD=0.30 / JUDGE_TIMEOUT_SECS=8) |
| [strategy.rs](file:///g:/vivian-rs/src-tauri/src/dialogue/strategy.rs) | `ChatMemoryStrategy` trait + 3 实现(Window / Token / SummaryBuffer) |
| [topic_tracker.rs](file:///g:/vivian-rs/src-tauri/src/dialogue/topic_tracker.rs) | `TopicTracker`(topic_activeness=10 / name_call_cooldown=4) |

`ChatMessageHistory` trait(async,对齐 LangChain)

---

### 5.13 engine/ Live2D 引擎

**架构定位**:Live2D 引擎子系统,动画 / 表情 / 状态机 / **多维度自动表情触发**。

| 文件 | 职责 |
|------|------|
| [animation.rs](file:///g:/vivian-rs/src-tauri/src/engine/animation.rs) | `AnimationManager` |
| [expression.rs](file:///g:/vivian-rs/src-tauri/src/engine/expression.rs) | `ExpressionManager`(DEFAULT_EXPRESSION="neutral" / FALLBACK=["shy","eye_roll","panic"]),持有 `RwLock<Option<Arc<ResourceManifest>>>` 字段,`normalize_expression` 通过该 manifest 实例查询而非全局静态;`set_manifest(manifest)` 在 `AppState::init_pet_controller` 中注入对应角色 manifest |
| [manifest.rs](file:///g:/vivian-rs/src-tauri/src/engine/manifest.rs) | `ResourceManifest` + `ModelManifest`,每角色独立加载(从 `public/<ModelName>/model_manifest.json` 解析),提供 `emotion_to_expression_name` / `interaction_feedback_names` / `random_mood_expression` / `normalize_expression` / `normalize_motion` / `get_idle_trigger` / `get_event_trigger` / `get_mood_idle_expression` / `interaction_feedback` 等实例方法;**已移除全局静态 `MANIFEST: Lazy<RwLock<Option<Arc<ResourceManifest>>>>` 与 8 个便捷函数**,改为由 `Brain::build` 在构造时把 `Arc<ResourceManifest>` 注入到 PsychologyManager / EmotionBridge / ResponseParsingRunnable / ExpressionManager 4 个依赖。`normalize_expression` 兜底语义:别名/原名/回退候选链全部未命中时返回空串(遵循"无匹配时留空,不强制使用"原则),仅当显式请求 `default` / `neutral` / 空字符串时才返回第一个可用表情。`ModelManifest` 新增三类触发映射字段:`idle_triggers`(空闲阶段触发)、`event_triggers`(程序事件触发)、`mood_idle_expressions`(心情持续表情)、`interaction_map`(10 种精细交互类型反馈) |
| [auto_trigger.rs](file:///g:/vivian-rs/src-tauri/src/engine/auto_trigger.rs) | **自动表情/动作触发引擎**(全局单例 `AUTO_TRIGGER`),纯规则驱动(零 LLM 开销)的多维度表情触发系统。核心结构 `AutoExpressionTrigger` 内部按 `char_id` 维护 `HashMap<String, TriggerState>`,每角色独立跟踪交互时间、空闲阶段、情绪标签、冷却表。设计原则:概率门控 + 冷却时间避免机械重复;尊重 ResourceManifest 映射使角色可自定义;通过 PetActionRequest 队列与前端通信统一动作投递 |
| [presentation.rs](file:///g:/vivian-rs/src-tauri/src/engine/presentation.rs) | 表现层辅助,已移除依赖全局 manifest 的 `normalize()` 死代码方法 |
| [motion_player.rs](file:///g:/vivian-rs/src-tauri/src/engine/motion_player.rs) | `MotionPlayer`(motion3.json 解析,线性插值;`MotionCurve::sample_at` 空关键帧场景降级返回 0.0 避免 panic) |
| [resource_loader.rs](file:///g:/vivian-rs/src-tauri/src/engine/resource_loader.rs) | `ResourceLoader` |
| [state_machine.rs](file:///g:/vivian-rs/src-tauri/src/engine/state_machine.rs) | `StateMachine` |
| [mod.rs](file:///g:/vivian-rs/src-tauri/src/engine/mod.rs) | 模块导出(re-export `AUTO_TRIGGER`、`record_user_interaction`、`trigger_event`、`auto_trigger_tick`、`update_mood_state`) |

#### auto_trigger.rs 触发机制详解

**四类触发路径**:

1. **用户交互即时反馈** — `apply_user_interaction` 命令(commands/emotion.rs)被前端调用时:
   - 查表 `manifest.interaction_map` 获取对应表情/动作/前端 action
   - 检测是否从长时空闲(>5 分钟)回来,若是则触发 `user_return` 事件
   - 调用 `record_user_interaction(char_id)` 重置空闲计时器与已触发阶段集合
   - 支持 10 种交互类型:`single_click / double_click / fast_click / drag_start / drag_end / fast_drag / pet / long_press / mouse_enter / mouse_leave`

2. **空闲检测渐进触发** — `auto_trigger_tick(char_id, manifest)`(4 秒间隔由前端 setInterval 驱动):
   - `IdleStage` 五阶段枚举:Active(0-30s) → Short(31-120s) → Medium(121-300s) → Long(301-900s) → Asleep(>900s)
   - 阶段升级时按概率触发(Short 40% / Medium 60% / Long 80% / Asleep 95%),同一阶段只触发一次
   - 查询 `manifest.idle_triggers` 获取对应(expression, motion, action, duration, probability)配置
   - 用户任何交互立即重置到 Active,清空已触发阶段集合

3. **心情状态联动** — `update_mood_state(char_id, manifest, mood_label, intensity)`(由心理系统更新时调用):
   - **情绪变化触发**:主导情绪标签改变且 intensity > 0.4 时,查询 `manifest.event_triggers("mood_change_{label}")` 立即触发(如 joy→happy_bounce / sadness→pout / anger→pout / curiosity→curious / fear→shy / surprised→surprised)
   - **心情持续表情**:空闲 45 秒后,25% 概率查询 `manifest.mood_idle_expressions` 触发当前心情对应表情(3 秒后自动恢复),45 秒冷却避免刷屏

4. **程序事件触发** — `trigger_event(char_id, event_key, manifest)`(由前端或后端主动调用):
   - 支持事件:`morning / afternoon / evening / night / window_focus / window_blur / chat_start / chat_end / music_start / music_stop / battery_low / user_return / mood_change_*`
   - 每个事件有独立冷却时间(时间段事件 3600s / 窗口事件 10s / 对话事件 5s / 电池事件 300s / user_return 10s)
   - 查询 `manifest.event_triggers` 获取配置

**关键类型定义**:

```rust
type TriggerResult = (String, String, String, Option<u64>, f64);  // (expr, motion, action, duration_ms, probability)

pub enum IdleStage { Active=0, Short=1, Medium=2, Long=3, Asleep=4 }

struct TriggerState {
    last_interaction: Instant,
    last_idle_stage: IdleStage,
    triggered_idle_stages: HashSet<IdleStage>,
    current_mood_label: String,
    last_mood_idle_time: Instant,
    event_cooldowns: HashMap<String, Instant>,
}
```

**前端动作库**(17 个程序动画,由 action 字段驱动,基于 Live2D 模型真实参数):
`nod_head / shake_head / tilt_head / look_around / blink_twice / side_glance / bounce_body / body_sway / bow_head / smile / surprised / tail_wag / wink / happy_bounce / shy / pout / curious`

- **通用参数**(两模型共有):`ParamAngleX/Y/Z` / `ParamBodyAngleX/Y/Z` / `ParamEyeLOpen/ROpen` / `ParamEyeBallX/Y` / `ParamMouthOpenY` / `ParamMouthForm` / `ParamEyeLSmile/RSmile` / `ParamBrowLY`
- **跨模型兼容写法**:`JawOpen`(Vivian) / `Jawopen`(Nana) 同设;`CheekPuff` / `CheeckPuff`(拼写差异)同设;`MouthShrug` / `Mouthshrug` 同设。专属参数在其他模型上 `setParameterValueById` 静默无效,不报错
- **Vivian 专属**:`ParamCheek` / `EyeWide` / `Brows` / `ParamBrowRY` + 4 段尾巴参数 `Param_Angle_Rotation_1/3/6/9_ArtMesh321`(`tail_wag` 动作驱动,根部小幅到尖端部大幅的波浪式摆动)
- **Nana 专属**:`ParamEyeSquintL` / `ParamBrowLForm` / `ParamHairBack/Front/Side`
- **动作-情绪映射**:`smile` 用 `ParamMouthForm`+`ParamEyeLSmile/RSmile` 联动笑眼;`surprised` 加 `ParamBrowLY/RY`+`EyeWide` 抬眉睁眼;`wink` 单眼闭合+嘴角上扬;`happy_bounce` 弹跳+笑眼+脸颊红晕;`shy` 歪头+半闭眼+`ParamCheek` 红晕;`pout` 嘴角下压+`MouthShrug`+低头;`curious` 歪头+睁眼+眉毛抬高+`EyeWide`
- **情绪联动动作池**(`selectActionPool`):自主行为调度根据 `mood_label` 加权选择动作池——joy/closeness → `happy_bounce`/`wink`/`smile`;sadness/loneliness → `pout`/`bow_head`;curiosity → `curious`/`tilt_head`;anger → `pout`/`shake_head`;fear → `shy`/`bow_head`;bored → `tail_wag`+`look_around`(仅 Vivian 生效)

**便捷全局函数**(通过 `static AUTO_TRIGGER: LazyLock<AutoExpressionTrigger>` 暴露):
- `record_user_interaction(char_id) -> bool` — 记录交互并返回之前是否长时空闲
- `update_mood_state(char_id, manifest, mood_label, intensity)` — 更新心情并自动 apply 触发结果
- `trigger_event(char_id, event_key, manifest)` — 触发程序事件并自动 apply
- `auto_trigger_tick(char_id, manifest)` — 执行 tick 并自动 apply 所有触发结果

**MotionPriority**(5 级):Idle=0 / Low=10 / Normal=50 / High=100 / Critical=200

**PetState**(5 状态):Idle / Interacting / Panicked / Playing / AiTalking

**DEFAULT_IDLE_INTERVAL**:3000-8000ms

> 多角色隔离:manifest 模块不再持有全局静态,ResourceManifest 实例由 `AppState::init_pet_controller(model_name)` 按角色 `live2d_model` 配置加载,作为 `Arc<ResourceManifest>` 在 `CharacterInstance.manifest` 字段中持有;Brain::build 接收该 Arc 并传播到 PsychologyManager / EmotionBridge / ResponseParsingRunnable / ExpressionManager 4 个依赖;AutoExpressionTrigger 内部 `states: RwLock<HashMap<String, TriggerState>>` 按 char_id 独立维护触发状态。同一进程内 Nana 与 Vivian 的触发状态、冷却表、空闲计时完全隔离。

---

### 5.14 speech/ 语音系统

**架构定位**:ASR 四引擎 + TTS 六后端 + 口型同步。

#### ASR 四引擎

```rust
pub enum AsrBackendType { Winrt, Whisper, Azure, Aliyun }
```

- `Winrt`(默认,Windows 原生)
- `Whisper`(HTTP 后端)
- `Azure`(云端)
- `Aliyun`(阿里云 NLS 流式识别,支持实时 VAD + 中间结果)

`AsrConfig`:sample_rate=16000 / `VadConfig`:energy_threshold=500.0
`AsrManager`:broadcast::channel(64)

#### TTS 六后端

```rust
pub enum TtsEngine {
    None,
    EdgeTts,    // Edge-TTS(WebSocket + WordBoundary,默认在线)
    Azure,      // Azure 认知服务(REST + /voices/list)
    GptSoVits,  // GPT-SoVITS 自托管(兼容 v1/v2)
    FishSpeech, // Fish Speech(fishaudio /v1/tts)
    BertVits2,  // → FishSpeech
    MiniMax,    // MiniMax TTS(REST API,支持多角色音色)
    Windows,    // WinRT SpeechSynthesizer(离线 fallback)
}
```

`TtsConfig`:rate=1.0 / volume=1.0 / retry_count=1 / fallback_engine=Some(Windows)

#### 口型同步

`char_to_mouth_open`:元音→0.7/0.75、辅音→0.35、标点→0.0

事件流:`tts:started` / `tts:word` / `tts:finished` / `tts:error` / `tts:fallback` + `lipsync:start` / `lipsync:update` / `lipsync:stop`

#### 关键文件(22 文件)

| 文件 | 职责 |
|------|------|
| [asr.rs](file:///g:/vivian-rs/src-tauri/src/speech/asr.rs) | ASR 引擎抽象 |
| [tts.rs](file:///g:/vivian-rs/src-tauri/src/speech/tts.rs) | TTS 主控(`Default` 实现在 `SpeechCache::new` 失败时降级到 `SpeechCache::fallback()` 使用系统临时目录,避免 panic) |
| [tts_backend.rs](file:///g:/vivian-rs/src-tauri/src/speech/tts_backend.rs) | `TtsBackend` trait |
| [tts_edge.rs](file:///g:/vivian-rs/src-tauri/src/speech/tts_edge.rs) | Edge-TTS |
| [tts_azure.rs](file:///g:/vivian-rs/src-tauri/src/speech/tts_azure.rs) | Azure TTS |
| [tts_windows.rs](file:///g:/vivian-rs/src-tauri/src/speech/tts_windows.rs) | Windows WinRT |
| [tts_gpt_sovits.rs](file:///g:/vivian-rs/src-tauri/src/speech/tts_gpt_sovits.rs) | GPT-SoVITS |
| [tts_fish_speech.rs](file:///g:/vivian-rs/src-tauri/src/speech/tts_fish_speech.rs) | Fish Speech |
| [tts_minimax.rs](file:///g:/vivian-rs/src-tauri/src/speech/tts_minimax.rs) | MiniMax TTS(REST API,多角色音色) |
| [tts_audio.rs](file:///g:/vivian-rs/src-tauri/src/speech/tts_audio.rs) | 音频播放(MciPlayer) |
| [whisper_backend.rs](file:///g:/vivian-rs/src-tauri/src/speech/whisper_backend.rs) | Whisper |
| [azure_backend.rs](file:///g:/vivian-rs/src-tauri/src/speech/azure_backend.rs) | Azure ASR |
| [aliyun_backend.rs](file:///g:/vivian-rs/src-tauri/src/speech/aliyun_backend.rs) | 阿里云 ASR |
| [winrt_backend.rs](file:///g:/vivian-rs/src-tauri/src/speech/winrt_backend.rs) | WinRT ASR |
| [gpt_sovits_service.rs](file:///g:/vivian-rs/src-tauri/src/speech/gpt_sovits_service.rs) | GPT-SoVITS 子进程管理(单/双实例模式) |

#### GPT-SoVITS 服务管理(`gpt_sovits_service.rs`)

管理 GPT-SoVITS 自托管推理服务的子进程生命周期,支持单实例(同端口同时处理 TTS 和参考音频)与双实例(TTS/参考音频分离端口)两种模式:

- **状态缓存** — `cached_state: Arc<RwLock<ServiceState>>` 缓存最新服务状态,避免 `state()` 方法每次重新计算;状态变更时(启动/停止/健康检查)主动更新缓存
- **HTTP Client 复用** — 使用 `reqwest::Client` 连接池复用,避免在健康检查循环中反复创建 Client 导致 TCP 连接泄漏
- **精确端口杀进程** — Windows 平台杀端口占用进程时,解析 `netstat -ano` 输出的 `Local Address` 字段精确匹配目标端口号,避免仅按行匹配端口数字导致误杀其他端口上的无辜进程
- **子进程监督** — 异步任务监控子进程 stdout/stderr,异常退出时自动清理状态

---

### 5.15 network/ 网络基础设施

| 文件 | 职责 |
|------|------|
| [http_client.rs](file:///g:/vivian-rs/src-tauri/src/network/http_client.rs) | 全局 Client(OnceCell,30s timeout / pool_max_idle_per_host=10 / tcp_keepalive=60s) |
| [http_retry.rs](file:///g:/vivian-rs/src-tauri/src/network/http_retry.rs) | `RetryConfig`(max_retries=3 / base_delay_ms=1000 / max_delay_ms=10000 / RETRYABLE_STATUS_CODES=[429,500,502,503,504]) |
| [proxy.rs](file:///g:/vivian-rs/src-tauri/src/network/proxy.rs) | `ProxyMode`(Direct/System/Custom) |
| [request_utils.rs](file:///g:/vivian-rs/src-tauri/src/network/request_utils.rs) | `SmartRequestBuilder` / detect_format(o1/o3/o4→"responses") |
| [web_context.rs](file:///g:/vivian-rs/src-tauri/src/network/web_context.rs) | `WebSearcher`(DuckDuckGo)+ `WebContextRunnable` + `FreshnessLevel` |
| [builtin/web_search_tool.rs](file:///g:/vivian-rs/src-tauri/src/tools/builtin/web_search_tool.rs) | `web_search` 工具(对话中搜索成功后调用 `push_topic_hint` 记录关键词,留给后台知识采集优先处理;搜索结果本身不写入 RAG) |

代理读取优先级:`HTTPS_PROXY > https_proxy > HTTP_PROXY > http_proxy`

---

### 5.16 diary/ 日记系统

**架构定位**:日记生成与持久化,按 `char_id` 物理隔离存储,每个角色独立写自己的日记。

**多角色隔离**:所有函数(get_entries / add_entry / delete_entry / update_entry / get_config / set_config / get_stats / export_to_markdown 等)均接收 `char_id: &str` 参数,存储路径为 `<user_data_dir>/characters/<char_id>/diary/`(diaries.json + config.json)。`spawn_auto_diary_check` / `should_trigger` / `check_missed_diaries_on_startup` / `catch_up_missed_diaries` 接收 `&Brain`,内部通过 `brain.char_id` 路由。`intelligent_generator` 的 `get_last_diary_summary` / `build_cross_diary_context` 改为接收 `&Brain` 参数,每个角色基于自己的记忆和情绪数据生成日记。

**DiaryEntry**:id / date / start_time / end_time / content / key_events / mood_average / word_count / interaction_count / trigger_type / trigger_score / mood_tag / created_at

**DiaryConfig**:enable_auto_diary=true / auto_diary_time="23:00" / min_interaction_threshold=10 / max_diary_length=500(每个角色独立配置)

**intelligent_generator.rs**:
- `calculate_trigger_score`:交互*10 max50 + 长度/50 max30 + 情绪/10 max20
- `build_prompt`:5 要素叙事框架
- `parse_diary_json`:容错回退
- `build_context`:从 Brain 收集交互/情绪/上一篇摘要/跨日记对比上下文,全部按 `brain.char_id` 路由

---

### 5.17 config/ 配置管理

#### AppConfig 顶层字段

`base` / `window` / `live2d_render` / `ai` / `network` / `providers` / `routing_matrix` / `memory` / `speech_recognition` / `proactive` / `enable_routing_matrix` / `tools` / `world` / `provider_cache`

#### WorldConfig

```rust
enable: bool,
inject_into_prompt: bool,
enable_weather: bool,
weather_cache_ttl_secs: u64 = 3600,
latitude / longitude: Option<f64>,
enable_inner_monologue: bool,
monologue_min_interval_secs: u64 = 1800,
enable_memory_consolidation: bool,
sleep_start_hour: u32 = 1,
sleep_end_hour: u32 = 6,
```

#### ToolConfig(14 字段)

max_iterations=10 / max_rounds=20 / max_result_chars=4000 / cache_ttl_secs=300 / cache_max_size=1000 / cache_strategy="auto" / access_level="fs-read" / compress_threshold_chars=80000 / compress_keep_recent=6 等

#### MemoryConfig

max_short_term_memory=20 / retrieval_strategy="auto"

#### RetrievalWeightsConfig

recency=0.25 / relevance=0.40 / importance=0.15 / hook_boost=0.10 / need_sim=0.10 / recency_tau_hours=24.0 / min_score=0.15

#### EmbeddingConfig

model="BAAI/bge-m3" / dimension=1024

#### ConsolidationConfig

stage1_short_term_threshold=20 / stage1_idle_timeout_sec=1800.0

#### 路由矩阵 8 任务类型

chat / reasoning / diary / memory / embedding / reflection / inner_monologue / consolidation

**ConfigManager**:YAML 持久化、`${VAR}` 环境变量替换、nested get/set、客户端缓存热重载

---

### 5.18 commands/ Tauri 命令

**27 个文件,221 个 `#[tauri::command]`**

**安全策略**:命令层在调用底层子系统前对用户输入做边界校验 — `open_application` 字符白名单(拒绝路径分隔符与 shell 元字符)、`export_diaries_markdown` 调用 `validate_export_path`(拒绝路径穿越与系统敏感目录写入)、`click_through.rs` WNDPROC 回调使用 `try_lock()` 避免重入死锁。

| 文件 | 命令数 | 主要职责 |
|------|--------|----------|
| [window.rs](file:///g:/vivian-rs/src-tauri/src/commands/window.rs) | 23 | 窗口位置/尺寸/透明度/可见性/子窗口/光标跟踪/安全位置 |
| [click_through.rs](file:///g:/vivian-rs/src-tauri/src/commands/click_through.rs) | — | 点击穿透子类化(顶层 Tauri 窗口 + WebView2 后代 HWND 安装 `WM_NCHITTEST` 子类化,按中心 1/3 宽 × 4/9 高矩形判定 `HTCLIENT` / `HTTRANSPARENT`)。并发安全:WNDPROC 回调中所有 `ENTRIES` / `DRAG_OFFSET` 锁操作使用 `try_lock().ok()`,失败时保守视为"拖动中"返回 `HTCLIENT` 避免锁死鼠标;`log_hit_test_transition` 同样 `try_lock`,失败时跳过日志 |
| [diary.rs](file:///g:/vivian-rs/src-tauri/src/commands/diary.rs) | 12 | 日记 CRUD + 智能生成 + 配置 + 统计 + 补记 + Markdown 导出(均带 character_id 参数,按角色路由);`export_diaries_markdown` 调用 `validate_export_path` 校验导出路径(拒绝路径穿越 + 拒绝写入系统敏感目录) |
| [config.rs](file:///g:/vivian-rs/src-tauri/src/commands/config.rs) | 9 | 配置读写 + 网络测试 + 用户头像 |
| [tools.rs](file:///g:/vivian-rs/src-tauri/src/commands/tools.rs) | 10 | 工具列表/执行/历史/确认 + MCP 管理 + Worldbook 参数 |
| [todo.rs](file:///g:/vivian-rs/src-tauri/src/commands/todo.rs) | 10 | 待办 CRUD + 定时任务 |
| [engine.rs](file:///g:/vivian-rs/src-tauri/src/commands/engine.rs) | 9 | Live2D 动作/表情/模型/睡眠/唤醒问候/躲避鼠标 |
| [emotion.rs](file:///g:/vivian-rs/src-tauri/src/commands/emotion.rs) | 11 | 心情/心理状态/交互反馈/微调tick/历史/事件/深度+批量情感分析/**自动表情tick**/**程序事件触发**;`psychology_micro_tick` emit `psychology:state` 事件携带 `character_id` 字段,前端按角色过滤;`mood_expression_tick` 心情表情冷却 `LAST_TRIGGER` 为 `Lazy<RwLock<HashMap<String, i64>>>` 按 `char_id` 索引,冷却时长按角色差异化读取 `CharacterBehavior::get_behavior(char_id).mood_expression_cooldown_secs`(Vivian 30s / Nana 15s);`random_mood_expression` 通过 `instance.manifest.random_mood_expression()` 实例方法查询,而非全局静态函数;`apply_user_interaction` 支持10种精细交互类型并检测长时空闲用户回来触发`user_return`事件;`auto_expression_tick` 每4秒调用驱动空闲阶段/心情持续表情的纯规则概率触发;`trigger_system_event` 供前端触发程序事件(morning/afternoon/evening/night/window_focus/window_blur等) |
| [proactive.rs](file:///g:/vivian-rs/src-tauri/src/commands/proactive.rs) | 9 | 主动交互启停 + tick + 消息消费 + 配置更新 + 自动定位 |
| [system.rs](file:///g:/vivian-rs/src-tauri/src/commands/system.rs) | 6 | 初始化/重初始化 + 系统信息 + 进程列表 + 应用开关;`open_application` 在调用 controller 前做字符白名单校验,拒绝路径分隔符(`/` `\`)与 shell 元字符(`&` `|` `;` `>` `<` `$` `` ` `` `(` `)` 等),防止 LLM 通过应用名注入 shell 命令 |
| [memory.rs](file:///g:/vivian-rs/src-tauri/src/commands/memory.rs) | 6 | 记忆 CRUD + 摘要 + 搜索(过滤 system_seed,均带 character_id 参数按角色路由) |
| [system_tray.rs](file:///g:/vivian-rs/src-tauri/src/commands/system_tray.rs) | 6 | 托盘 tooltip/图标/通知/可见性/销毁 |
| [tts.rs](file:///g:/vivian-rs/src-tauri/src/commands/tts.rs) | 7 | TTS 配置/语音列表/测试/朗读/停止/状态。`speak_text` 在朗读前通过 `strip_action_text()` 过滤括号动作描述(如 `(轻声笑了笑)`),避免 TTS 朗读动作文本;过滤后为空则跳过朗读 |
| [speech.rs](file:///g:/vivian-rs/src-tauri/src/commands/speech.rs) | 5 | ASR 启停 + 状态 + 配置 + 快捷键 + 事件转发器 |
| [rag.rs](file:///g:/vivian-rs/src-tauri/src/commands/rag.rs) | 5 | RAG 文档 CRUD(已合并入 MemoryManager,手动添加时 `ttl_days=None` 不设过期) |
| [user_facts.rs](file:///g:/vivian-rs/src-tauri/src/commands/user_facts.rs) | 5 | 用户事实画像 CRUD:`get_user_facts`(返回 `UserProfileView` 含 basic_facts/recent_state/custom_facts)/ `set_user_fact`(写入指定类型,content 空则跳过)/ `pin_user_fact`(切换 is_pinned 锁定)/ `delete_user_fact`(删除自由事实)/ `get_user_fact_types`(返回枚举列表)。均带 `character_id` 参数按角色路由,存储于 `characters/<char_id>/user_facts.json` |
| [environment.rs](file:///g:/vivian-rs/src-tauri/src/commands/environment.rs) | 5 | 环境信息 + 当前状态 + 用户活动 + 启动问候 |
| [metrics.rs](file:///g:/vivian-rs/src-tauri/src/commands/metrics.rs) | 5 | 指标快照/持久化/重置/计数/Gauge |
| [live2d_lipsync.rs](file:///g:/vivian-rs/src-tauri/src/commands/live2d_lipsync.rs) | 4 | 嘴形联动启停/更新/状态 |
| [relationship.rs](file:///g:/vivian-rs/src-tauri/src/commands/relationship.rs) | 4 | 关系状态/阶段/里程碑/重置 |
| [persona.rs](file:///g:/vivian-rs/src-tauri/src/commands/persona.rs) | 4 | 人格/名称/tagline/style_prompt |
| [chat.rs](file:///g:/vivian-rs/src-tauri/src/commands/chat.rs) | 3 | 消息发送(同步+流式)+ 停止生成 |
| [history.rs](file:///g:/vivian-rs/src-tauri/src/commands/history.rs) | 2 | 聊天历史查询/清空(均带 character_id 参数) |
| [cross_character.rs](file:///g:/vivian-rs/src-tauri/src/commands/cross_character.rs) | 2 | 跨角色对话:`trigger_cross_character_talk` / `list_talkable_characters` |

**多角色路由**:

大多数 Tauri 命令新增 `character_id: Option<String>` 参数,内部通过 `state.get_character(character_id.as_deref())?` 路由到对应 `CharacterInstance`(若 `None` 则使用 `active_character_id`)。涉及命令包括但不限于:`send_message_stream`、`get_chat_history`、`clear_chat_history`、`proactive_tick`、`get_realtime_status`、`start_realtime_call`、`stop_realtime_call`、`send_realtime_text`、`get_memories`、`search_memories`、`clear_all_memories`、`get_memory_summary`、`get_recent_interactions`、`update_memory_importance`、`get_diary_entries`、`generate_diary`、`generate_diary_intelligent`、`get_diary_entry`、`delete_diary_entry`、`get_diary_config`、`set_diary_config`、`get_diary_stats`、`update_diary_entry`、`check_missed_diaries`、`export_diaries_markdown`、`should_trigger_diary` 等。

**关键命令签名**:
- `send_message_stream(state, message: String, stream_id: String, character_id: Option<String>, app)` — 流式 emit `chat:start`/`chunk`/`meta`/`done`/`cancelled`/`error`,按 `character_id` 路由到目标角色 Brain
- `proactive_tick(state, app, context: Value, character_id: Option<String>)` — 10s 调用,注入流式 emitter,睡眠时跳过,按角色路由;内部 `spawn_auto_diary_check(&brain, app)` 通过 `brain.char_id` 路由到对应角色日记存储
- `generate_diary_intelligent(state, character_id: Option<String>, trigger_type: Option<String>, app)` — 主 LLM API 未配置时 emit `llm:not_configured`;按角色路由,生成成功后 emit `diary:written`(携带 character_id)
- `get_memories(state, character_id: Option<String>)` / `clear_all_memories(state, character_id: Option<String>)` — 按 character_id 路由到对应角色 MemoryManager,清空时同步清理对话历史与关系重置
- `trigger_cross_character_talk(state, source_id: String, target_id: String, content: String, app)` — 通过 `CrossCharacterBus` 发起跨角色对话(详见 5.20)
- `list_talkable_characters(state)` — 返回当前在线、可被对话的角色列表
- `apply_user_interaction(interaction: String, character_id: Option<String>, state)` — 前端检测到用户交互后即时调用,查表返回对应表情/动作/action(零LLM开销);同时检测空闲时长,若>5分钟则触发`user_return`事件;重置AUTO_TRIGGER空闲计时
- `auto_expression_tick(character_id: Option<String>, state)` — 前端每4秒调用一次,驱动空闲阶段渐进触发与心情持续表情(纯规则概率触发,不调LLM);通过PetActionRequest队列投递表情/动作/action到前端
- `trigger_system_event(event: String, character_id: Option<String>, state)` — 前端感知到系统事件时调用(如window_focus/blur/morning/afternoon等),查表触发对应表情/动作(带事件独立冷却)

---

### 5.19 其他根级模块

#### 降级模式

系统在关键资源初始化失败时提供降级路径,保证主流程不阻塞:

- `presence::PresenceManager::new_with_temp_dir(char_id)` — 持久化目录不可写时降级到系统临时目录,在场状态仍可运行但无法持久化
- `tools::mcp::McpManager::new_disabled()` — MCP server 初始化失败时返回空实现,外部工具调用直接返回错误提示,内置工具与对话能力不受影响
- `brain::augment_reply_service` — `MAX_PENDING_ENTRIES=100` 硬上限防止回复增强队列无界增长,超限丢弃新条目并 warn
- `speech::tts_cache::SpeechCache::fallback()` — 缓存目录创建失败时降级到系统临时目录(`%TEMP%\vivian-tts-cache`),`TtsManager::default()` 在 `SpeechCache::new` 失败时自动调用此方法避免 panic
- `memory::time_stamped::TOKENIZER` — 全局 `cl100k_base` tokenizer 加载失败时降级到 `None`,token 计数回退到字符数估算(中文 1 字 ≈ 1.5 token,ASCII 4 字符 ≈ 1 token),保证 `TimeStampedMemory` 摘要触发逻辑可用
- `engine::motion_player::MotionCurve::sample_at` — 空关键帧场景降级返回 0.0,避免 `keyframes.last().unwrap()` 在配置异常时 panic
- `commands::config::image_to_data_url` — 文件读取直接 try IO 并匹配 `ErrorKind::NotFound` 原子返回 `Ok(None)`,避免 `exists()` 预检 + 后续 read 的 TOCTOU 竞态

#### [error.rs](file:///g:/vivian-rs/src-tauri/src/error.rs) — VivianError

```rust
pub enum VivianError {
    Config(String), Provider(String), Network(String), Tool(String),
    Permission(String), Memory(String), Sandbox(String), CircuitBreaker(String),
    Timeout(String), Serialization(String), Database(String),
    Io(#[from] std::io::Error), Json(#[from] serde_json::Error),
    Engine(String), Speech(String), NotImplemented(String), Other(String),
}
pub type VivianResult<T> = Result<T, VivianError>;
```

**错误传播策略**:

- **核心数据结构**(`MemoryVectorStore::add/delete/clear` 等)返回 `VivianResult<()>`,调用方通过 `?` 向上传播,避免静默吞错
- **非关键路径**(hooks runner / scheduler / feature flags 持久化等)错误以 `tracing::warn!` 记录后降级继续运行,保证主流程不阻塞
- **降级路径可观测**:嵌入服务失败(`MemoryManager` / `ConsolidationPipeline` / `AutoStrategy`)、主动对话 LLM 查询失败(`BehaviorDecider` / `IceBreaker` / `RecallTopic` / `stream_query_and_parse`)、文件操作失败(`save_user_avatar` / `clear_user_avatar` 删除残留头像)等历史 `.ok()?` / `let _ = ...` 静默吞错路径全部改为 `tracing::warn!` 记录,便于排查"AI 突然变笨"或"清理操作未生效"类问题
- **TOCTOU 防护**:文件/头像相关命令(`image_to_data_url` / `save_user_avatar` / `clear_user_avatar` / `chat.rs` 图片上传)移除 `exists()` 预检,直接尝试 IO 操作并匹配 `ErrorKind::NotFound` 原子返回友好错误,避免"检查后使用"窗口期文件被替换/删除导致的竞态;`std::fs::remove_file` 失败时区分 `NotFound` 与其他错误,仅在非 NotFound 时 warn 记录
- **命令层**使用 `err_str` 统一将错误转字符串返回前端
- **日志安全**:token 等敏感字段在日志中做 URL mask(`providers::wenxin` / `speech::aliyun_backend`);`truncate_for_log` 函数截断长文本避免日志膨胀

#### [feature_flags.rs](file:///g:/vivian-rs/src-tauri/src/feature_flags.rs) — 17 个功能开关

> 注:实际定义 17 个标志,`Integration` 类别下无标志定义。

| 类别 | 标志 | 默认 | 需重启 |
|------|------|------|--------|
| **Core(8)** | voice / proactive / diary / emotion / memory_semantic / relationship | true | false |
| | desktop_control | false | **true** |
| | screen_perception | false | **true** |
| **Experimental(2)** | rag_knowledge_graph / multimodal_output | false | false |
| **Performance(2)** | tool_cache / deferred_tools | true | false |
| **Ui(3)** | wechat_style_chat / advanced_config / memory_visualization | true/false/false | false |
| **Debug(2)** | verbose_logging / tool_observability | false | false |

持久化:`%APPDATA%\Vivian\config\feature_flags.json`(原子写入 tmp+rename,持久化失败以 `tracing::warn!` 记录而非静默吞错)

#### [metrics.rs](file:///g:/vivian-rs/src-tauri/src/metrics.rs) — 性能指标

约 80 个指标名常量,按子系统分组(LLM / Tool / Embedding / Vector / RAG / Memory / Topic 等)。

```rust
pub struct Counter { name, description, value: Mutex<u64> }
pub struct Gauge { name, description, value: Mutex<f64> }
pub struct Histogram { name, description, buckets_ms, inner: Mutex<HistogramInner> }
pub struct TimerGuard { histogram, start, failure_counter, completed }
pub struct MetricsRegistry { counters, histograms, gauges, degradation_total, persist_path }
```

- 默认桶边界 `DEFAULT_BUCKETS_MS`:[1, 5, 10, 25, 50, 100, 250, 500, 1000, 5000]ms
- `record_degradation_attempt()` — 一旦 >0 即严重问题
- 持久化:`%APPDATA%\Vivian\logs\metrics_YYYY-MM-DD.json`(每日轮转)
- 全局单例:`pub static METRICS: OnceCell<Arc<MetricsRegistry>>`

#### [messages.rs](file:///g:/vivian-rs/src-tauri/src/messages.rs) — 多模态消息系统 + 镜像消息来源标记

5 种 `ContentBlock`:`TextContentBlock` / `ImageContentBlock` / `ToolCallBlock` / `ToolResultBlock` / `ReasoningContentBlock`

消息层级:`SystemMessage` / `HumanMessage` / `AIMessage` / `ToolMessage`

系统提示模板 `templates`:`STARTUP_GREETING_FIRST` / `STARTUP_GREETING_RETURN` / `ERROR_*` / `OP_*`

**多模态构造**:`ChatMessage::user_with_images(content, images)` 构造带图 user 消息，`MessageImage` 字段为 `media_type` / `data`(base64) / `url` / `detail`。六家 Provider 协议(OpenAiCompat/OpenAiResponses/DoubaoResponses/ChatCompletions/Anthropic/Gemini)在转换为各自 API 格式时读取该字段（详见 5.5 节多模态适配表），文心与星火不支持图片输入。`send_image_message` 命令在调用前先经 `ModelRouter::check_vision_capability` 探测目标模型是否支持视觉（详见 5.5 节视觉能力探测）。

**镜像消息来源标记**:

`MessageSource` 枚举(5 种):`User` / `Assistant` / `Tool` / `InnerMonologue` / `Mirror`
- `User` / `Assistant`:正常对话路径,进入记忆系统
- `Tool`:工具执行结果,不抽取为用户事实
- `InnerMonologue`:Vivian 的内心独白,不进入对话记忆
- `Mirror`:外部控制器注入的内容(插件/游戏/Agent 回调),默认不进入记忆

`MessageMeta` 结构体:`source` / `is_memory_disabled` / `mirror_kind`
- `is_memory_eligible()` — 是否可进入记忆(仅 User / Assistant)
- 便捷构造:`user()` / `assistant()` / `tool()` / `inner_monologue()` / `mirror(kind)`

#### [pet_controller.rs](file:///g:/vivian-rs/src-tauri/src/pet_controller.rs) — 桌宠控制器

7 种 `ControlCommandType`:MOTION / EXPRESSION / MOUSE_FOLLOW / WINDOW_SIZE / WINDOW_POSITION / OPACITY / SLEEP

常量:PET_MIN_SIZE=100 / PET_MAX_SIZE=2000 / PRIORITY_MAX=200

`play_motion(name, priority, interruptible, loop)` 方法在调用 `AnimationManager::play_motion` 之前,先通过 `character_registry::get_manifest(&char_id)` 查询当前角色 manifest,调用 `normalize_motion(name)` 将语义名(如 `wave` / `nod`)映射到实际 model3.json Name,再传给 AnimationManager 执行,确保 LLM 输出的语义名能正确解析为角色模型实际动作。

#### [character_behavior.rs](file:///g:/vivian-rs/src-tauri/src/character_behavior.rs) — 角色行为参数(按 char_id 索引)

**架构定位**:本地非 LLM 控制参数,按 `char_id` 索引,让不同角色在主动发话反馈幅度、MoodDriven 触发阈值、亲密度冷却、安静模式、表情触发冷却等方面表现出不同节奏感,无需 LLM 决策即可实现角色个性化。

```rust
pub struct CharacterBehavior {
    pub proactive_feedback_positive: f64,
    pub proactive_feedback_negative: f64,
    pub mood_driven_need_threshold: f64,
    pub mood_driven_loneliness_threshold: f64,
    pub intimacy_cooldown_multiplier: f64,
    pub quiet_mode_threshold: u32,
    pub mood_expression_cooldown_secs: i64,
}
```

**两套预设值**:

| 字段 | Vivian(傲娇慢热) | Nana(温柔热情) | DEFAULT |
|------|-------------------|-----------------|---------|
| `proactive_feedback_positive` | 0.002 | 0.005 | 0.002 |
| `proactive_feedback_negative` | 0.003 | 0.001 | 0.003 |
| `mood_driven_need_threshold` | 0.85 | 0.65 | 0.85 |
| `mood_driven_loneliness_threshold` | 0.75 | 0.55 | 0.75 |
| `intimacy_cooldown_multiplier` | 0.8 | 1.2 | 0.8 |
| `quiet_mode_threshold` | 5 | 2 | 5 |
| `mood_expression_cooldown_secs` | 30 | 15 | 30 |

**访问入口**:`pub fn get_behavior(char_id: &str) -> CharacterBehavior` — 通过 `BEHAVIOR_REGISTRY: Lazy<HashMap<&'static str, CharacterBehavior>>` 查询,未注册的 char_id 回退到 `DEFAULT`(等同 Vivian)。

**消费方**:
- `PsychologyManager::apply_proactive_feedback(positive, char_id)` — 读取 `proactive_feedback_positive` / `proactive_feedback_negative`
- `ProactiveOrchestrator::on_user_interacted` / `on_ignored` — 读取上述两字段
- `ProactiveOrchestrator` MoodDriven 触发判断 — 读取 `mood_driven_need_threshold` / `mood_driven_loneliness_threshold`
- `ProactiveOrchestrator` 亲密度冷却系数 — 读取 `intimacy_cooldown_multiplier`
- `ProactiveOrchestrator::on_ignored` 安静模式触发 — 读取 `quiet_mode_threshold`
- `commands/emotion.rs::mood_expression_tick` — 读取 `mood_expression_cooldown_secs`

#### [resilience/](file:///g:/vivian-rs/src-tauri/src/resilience/mod.rs) — 熔断器

```rust
pub enum CircuitState { Closed, Open, HalfOpen }
pub struct CircuitBreaker {
    name, state, failure_count, success_count,
    failure_threshold, failure_rate_threshold, reset_timeout,
    recent_results: VecDeque<bool>,  // 滑动窗口(默认 20)
    window_size: 20, min_samples: 5,
}
```

指数退避:`delay = min(max_delay, base_delay * 2^(attempt-1))`,RateLimit ×2,jitter ±20%

#### [i18n/](file:///g:/vivian-rs/src-tauri/src/i18n/mod.rs) — Rust 端 i18n

```rust
pub struct I18n { locale: String, translations: HashMap<String, Value> }
```
内置 zh-CN + en 字典,支持点号分隔嵌套键。全局静态:`init_i18n(language)` / `set_language(language)` / `tr(key)`

#### [utils/](file:///g:/vivian-rs/src-tauri/src/utils/mod.rs)

- [environment.rs](file:///g:/vivian-rs/src-tauri/src/utils/environment.rs) — `EnvironmentInfo` / `UserActivity` / `CurrentState` / `Environment_Manager`(Windows 电池 PowerShell)
- [path.rs](file:///g:/vivian-rs/src-tauri/src/utils/path.rs) — `get_user_data_dir()`(Windows: `%APPDATA%\Vivian`)/ `get_resource_dir()`(dev 向上查找 public/Vivian,release 查找 resources)/ `get_character_data_dir(char_id)` → `<user_data_dir>/characters/<char_id>/`(每角色独立 memory/persona/psychology/history/user_facts)/ `get_shared_data_dir()` → `<user_data_dir>/shared/`(跨角色共享数据)
- [token_estimate.rs](file:///g:/vivian-rs/src-tauri/src/utils/token_estimate.rs) — Token 估算(本地 tiktoken 近似计数,用于压缩决策和预算控制)
- `mod.rs` 通用工具函数:
  - `fnv1a_64_bytes(data: &[u8]) -> u64` / `fnv1a_64(s: &str) -> u64` — FNV-1a 64 位哈希,用于屏幕变化检测等场景的快速指纹计算
  - `truncate_chars(s: &str, n: usize) -> String` — 按 Unicode 字符截断(非字节),避免中文字符串截断时产生乱码
  - `truncate_chars_with_ellipsis(s: &str, n: usize) -> String` — 截断后追加省略号,用于 UI 展示与日志预览
  - `messages_cache_key(messages: &[ChatMessage]) -> String` — 基于 `role` + `content` + `tool_call_id` 的稳定缓存键,用于 LLM 响应缓存

#### [types/](file:///g:/vivian-rs/src-tauri/src/types)

- [response.rs](file:///g:/vivian-rs/src-tauri/src/types/response.rs) — `AiResponse` / `ChatMessage` / `MessageToolCall` / `MessageImage` / `ToolCall`
  - `ChatMessage` 工厂:`system()` / `user()` / `user_with_images()` / `assistant()` / `assistant_with_tool_calls()` / `tool_result()`
  - `meta` 字段:`Option<MessageMeta>`,标记内容来源与记忆策略。`tool_result()` 默认携带 `MessageMeta::tool()`(不进入记忆)
  - `with_meta(meta)` / `with_source(source)` — 链式设置消息来源
  - `is_memory_disabled()` — 该消息是否应被记忆系统跳过
  - 兼容 `reasoning` 字段(reasoning_content / thinking / reasoning_details)

---

### 5.20 conversation/ 会话生命周期

**架构定位**:把**所有对话**(User↔Agent / Agent A↔Agent B)统一建模为有生命周期的会话对象,是整个多智能体系统的"交通规则"。决定何时调用 LLM、何时结束对话、何时开启新会话。

**全局单例**:

```rust
pub static CONVERSATION_MANAGER: Lazy<Arc<ConversationManager>> = Lazy::new(|| {
    Arc::new(ConversationManager {
        inner: RwLock::new(ManagerInner {
            active: std::collections::HashMap::new(),
            seq: 0,
        }),
    })
});
```

**核心数据模型**:

```rust
pub enum ConversationState { Created, Active, Cooling, Closed }
pub enum CloseReason { Natural, GoodNight, GoodBye, NoResponse, Interrupted, Timeout, Conflict, SwitchTopic }
pub enum ResponseMode { Speak, NonVerbal, Internal, Ignore }

pub struct Conversation {
    pub id: String,
    pub topic: String,
    pub owner: String,
    pub participants: Vec<String>,
    pub state: ConversationState,
    pub energy: f64,           // [0,1] 活跃度
    pub novelty: f64,          // [0,1] 新信息密度
    pub rounds: u32,
    pub continuation_score: f64, // [0,1] 继续得分
    pub created_at: f64,
    pub last_active_at: f64,
    pub cooling_since: Option<f64>,
    pub closed_at: Option<f64>,
    pub close_reason: Option<CloseReason>,
    pub last_user_message_at: Option<f64>,
    pub last_response_mode: ResponseMode,
    pub memory_ids: Vec<String>, // close 时触发 seal_episode
}
```

**关键常量**:

```rust
pub const COOLING_WINDOW_SECS: f64 = 30.0;       // Cooling 窗口
pub const CLOSED_COOLDOWN_SECS: f64 = 60.0;      // 创建冷却
pub const ENERGY_THRESHOLD: f64 = 0.25;
pub const NOVELTY_THRESHOLD: f64 = 0.15;
pub const CONTINUATION_THRESHOLD: f64 = 0.30;
pub const RESCUE_THRESHOLD: f64 = 0.80;          // 抢救阈值
```

**核心方法**:

- `start_or_continue(source, target, first_message) -> Option<Conversation>` — 获取或创建会话。Active/Created → 返回 Some;Cooling + 抢救(score≥0.8) → Some(rescued);Cooling 抢救失败/超时在创建冷却内/Closed 在创建冷却内 → None
- `force_new_session(source, target, first_message) -> Conversation` — 用户主动发消息时绕过创建冷却,旧会话标记 Closed(Natural)
- `update_after_round(conv_id, response_mode, reply_text, user_input) -> Option<Conversation>` — 一轮结束后更新 Energy/Novelty/Continuation,根据得分决定状态转换
- `close_with_reason(conv_id, reason)` / `close_pair_with_reason(a, b, reason)` — 关闭会话并记录原因
- `touch_user_message(char_id)` — 记录用户发言时间戳(用于 NoResponse/Timeout 判定)
- `sweep_cooling() -> Vec<String>` — 清理超时的 Cooling 会话(挂在 proactive_tick)
- `sweep_user_session_timeouts(timeout_secs) -> Vec<(char_id, CloseReason)>` — 用户长时间未发言 → close(Timeout)
- `is_user_session_closed(char_id) -> bool` — proactive 决策用
- `add_memory_to_session(conv_id, memory_id)` / `get_session_memory_ids(conv_id)` — Episode 联动
- `detect_close_reason(text) -> Option<CloseReason>` — 模块级函数,关键词检测(GoodNight/GoodBye/Interrupted)

**评分公式**:

- Novelty = (问号?0.3:0) + (长度>10字?0.2:0) + (长度>30字?0.2:0) + (jieba实词>3?0.3:0) + (回复>15字?0.1:0),上限 1.0
- Energy delta:Speak +0.1+ΔNovelty×0.3 / NonVerbal -0.05 / Internal -0.02 / Ignore -0.3
- Continuation = 0.3 + (Novelty>0.5?0.2) + Novelty×0.3 + Energy×0.2 - min(0.3, rounds×0.02) - (Energy<0.3?0.2)
- 状态转换:Ignore → 直接 Cooling;Continuation<0.30 || Energy<0.25 || Novelty<0.15 → Cooling;否则 Active

**接入点**:

- User↔Agent(`commands/chat.rs`):`send_message` 和 `send_message_stream` 在 `brain.think` 前调 `start_or_continue`(None 时 `force_new_session`)+ `touch_user_message` + `dialogue.set_session_id` + `memory.set_session_id`;think 后调 `update_after_round` + `detect_close_reason` + `seal_episode_on_close` + 清除两个 session_id
- Agent↔Agent(`cross_character.rs::send`):`start_or_continue` 返回 None 时直接返回 `CrossCharacterReply{response_mode:"ignore", conv_state:"cooling"}` 不调 LLM;think 前设 `memory.set_session_id(conv.id)`,think 后清除;think 后调 `update_after_round`
- 主动聊天(`commands/proactive.rs::proactive_tick`):调 `sweep_cooling` + `sweep_user_session_timeouts(1800.0)` + `is_user_session_closed` 检查(GoodNight/NoResponse/Timeout 时跳过主动搭话)
- 被忽略(`proactive/mod.rs::on_ignored`):调 `close_pair_with_reason("user", char_id, NoResponse)`
- Episode 联动(`commands/chat.rs::seal_episode_on_close`):会话 close 时用 `memory_ids` + 会话边界时间戳触发 `EpisodeStore::seal_episode`

**关键文件**:

- [mod.rs](file:///g:/vivian-rs/src-tauri/src/conversation/mod.rs) — 模块导出 + 架构文档
- [session.rs](file:///g:/vivian-rs/src-tauri/src/conversation/session.rs) — Conversation/ConversationState/CloseReason/ResponseMode 定义
- [manager.rs](file:///g:/vivian-rs/src-tauri/src/conversation/manager.rs) — ConversationManager 单例 + 状态机 + 评分公式 + detect_close_reason
- [integrity.rs](file:///g:/vivian-rs/src-tauri/src/conversation/integrity.rs) — 对话完整性修复（消息序列校验 + 缺失补全）

### 5.21 cross_character.rs 跨角色通信总线

**架构定位**:多角色之间的对话桥梁。提供全局单例 `CrossCharacterBus`,允许一个角色(源角色)向另一个角色(目标角色)发起对话,由目标角色的 Brain 独立思考后流式返回。已接入会话生命周期(见 5.20)。

**全局单例**:

```rust
pub static CROSS_CHARACTER_BUS: Lazy<Arc<CrossCharacterBus>> = Lazy::new(|| {
    Arc::new(CrossCharacterBus { app_handle: RwLock::new(None) })
});
```

- `initialize(handle)` — 在 `lib.rs` 的 `setup` 阶段注入 AppHandle
- `send(req) -> VivianResult<CrossCharacterReply>` — 核心方法,流程:
  1. `CONVERSATION_MANAGER.start_or_continue(source, target, message)` → None 时直接返回 `CrossCharacterReply{response_mode:"ignore", conv_state:"cooling"}` 不调 LLM
  2. 通过 `state.get_character(Some(target_id))?` 获取目标角色 `CharacterInstance`
  3. `instance.think_lock.lock()` 串行化该角色的思考
  4. `set_stream_emitter(...)` 注入流式 emitter
  5. `brain.dialogue.set_channel("cross_character")` 切换渠道
  6. 从 UnifiedEventLedger 检索源↔目标最近 2 条共同事件作为记忆锚点
  7. 合成输入:`format!("[{} 对你说] {}{}", source_name, message, memory_anchor)`
  8. `instance.brain.think(&synthesized_input, true)` 启动流式思考(prompt 注入 `CROSS_CHARACTER_RESPONSE_DECISION`)
  9. `CONVERSATION_MANAGER.update_after_round(conv_id, response_mode, reply_text, user_input)`
  10. 流程中依次 emit `cross:start` / `cross:chunk` / `cross:done` / `cross:error`

> `talk_to_character` 工具在调用总线时套了一层 `tokio::time::timeout(60s)`:目标角色长时间不响应(如 LLM 卡死)时不会让发起方角色的 LLM 无限等待,超时后返回明确的 `CrossCharacterTimeout` 错误,发起方可据此自然切换话题。

**CrossCharacterReply 结构**:

```rust
pub struct CrossCharacterReply {
    pub reply: String,           // 目标回复文本(仅 speak 模式非空)
    pub response_mode: String,   // speak / non_verbal / internal / ignore
    pub conv_state: String,      // active / cooling / closed
    pub should_continue: bool,   // false 时源角色应停止/切换话题
    pub expression: String,
    pub motion: String,
}
```

**Public State 暴露**(`roommate_status_text`):只暴露 Public 信息,禁止暴露 Private Mind:
- 在线状态 + 在场状态(Online/Busy/Rest/Offline)+ 持续时间
- 主导情绪(仅标签 + 强度,不暴露 7 维详情)
- 最近发言时间(从全局 `LAST_SPOKEN` 读取)

**认知状态文本**(`roommate_cognitive_text`):在 `roommate_status_text` 基础上追加认知层面的公共摘要,供 LLM 在跨角色对话 prompt 中感知室友当前心理概况:
- 关系阶段标签(Stranger/Acquainted/.../Soulmate)
- 当前需求维度中最高的一项(如"最近比较需要新鲜感")
- 专注模式状态(Regular/Focus/TrueName)
- 数据仍来自 Public State,不暴露具体数值

**记忆持久化**(send 完成后):
- 源角色 dialogue 写 2 条消息(speaker 视角 + listener 视角),均带 `channel="cross_character"` metadata
- 非 speak 模式下,目标反馈转为描述性文本(如"Nana 没有说话,做了一个动作回应")
- 源角色补 1 条记忆(speaker 视角),目标角色补 1 条记忆(speaker 视角,带 response_mode 元数据)
- 写入 AgentAgent 关系日志(`RelationshipDirection::AgentAgent`)
- 更新 A↔B 关系数值(`social_state::apply_delta`)
- 异步抽取关系认知事实(`extract_relationship_facts`)

**共同情境注入**(send 合成输入阶段):
- `build_handoff_context` 调用处追加 `source_instance.brain.proactive.activity_journal().to_brief()` 作为 `[共同观察]` 段落
- 让目标角色在收到跨角色消息时,同时感知"双方都在观察的用户活动",对话有共同话题
- `to_brief()` 是只读安全接口,不清空日志,不影响内心独白的 `drain()` 消费路径

**LAST_SPOKEN 更新**(send 成功后,确保非 leader 角色跨角色对话后 `LAST_SPOKEN` 不为空,避免 `CrossCharacterReply` 触发器死锁):
- 源角色:总是调用 `record_cross_character_spoken(source_id, req.message)` 更新 `LAST_SPOKEN` + `LAST_SPOKEN_TEXT`
- 目标角色:speak 模式调用 `record_cross_character_spoken(target_id, final_text)` 更新两者;非 speak 模式调用 `touch_last_spoken(target_id)` 仅更新时间戳不覆盖文本

**LLM 工具**:

`TalkToCharacterTool`(位于 `tools/builtin/cross_character_tools.rs`)允许角色在思考链中主动调用其他角色。工具返回值通过 `format_reply_for_llm` 转为 LLM 友好文本:
- speak 模式:`{target} 回复:{reply}`
- speak + should_continue=false:`{target} 回复:{reply}\n(对话气氛似乎在冷却...)`
- non_verbal 模式:描述性文本
- ignore 模式:`{target} 没有理你。对话似乎已经结束了,建议换话题或停止。`

---

## 六、前端 React 架构详解

### 6.1 入口与组件树

#### [main.tsx](file:///g:/vivian-rs/src/main.tsx) — URL `?view=` 路由入口

```
<main.tsx>(按 URL ?view= 路由)
├── view=null(主窗口)→ <App>(动态 import 避免子窗口加载 Live2D SDK)
├── view=chat      → <ChatWindow>      (三视图聊天 home/private/group,390×845)
├── view=config    → <ConfigWindow>    (10 Tab 配置,768×624)
├── view=memory    → <MemoryWindow>    (iOS 风格记忆管理,1260×896)
├── view=diary     → <DiaryWindow>     (iOS 风格日记浏览,960×640)
├── view=status    → <StatusWindow>    (心理学状态面板,400×720,Acrylic/Mica/Tabbed)
├── view=bubble    → <BubbleWindow>    (气泡子窗口,340×140 动态 100-420)
├── view=toast     → <ToastWindow>     (Toast 子窗口,400×320)
├── view=todo      → <TodoWindow>      (待办管理)
└── view=scheduler → <SchedulerWindow> (定时任务管理)
```

**多窗口角色绑定**:

- 每个角色一个独立的 Tauri `WebviewWindow`,其 `label = character_id`(如 `"nana"`、`"vivian"`)。该窗口即角色的主桌宠窗口,加载 `App.tsx`。
- `main` 窗口(`label="main"`)是 `tauri.conf.json` 预定义的隐藏控制器,仅用于系统托盘 / 后台调度,不加载 `App.tsx`。`main.tsx` 在启动时检测当前窗口 `label === "main"` 且 URL 无 `?view=` 参数时,渲染空组件直接返回,避免 Live2D SDK 与事件监听器被无谓初始化。
- `main.tsx` 启动时根据 URL `?character_id=` 参数或当前窗口 `label`(若 `label` 是角色 id 之一)调用 `characterContext.setCharacterId(id)` 设置当前窗口的全局角色身份。
- 子窗口 label 按角色区分:`App.tsx` 提供 `charScopedLabel(base)` 函数返回 `<character_id>_<base>`(如 `nana_chat` / `vivian_status`),避免多角色同时存在时子窗口 label 冲突。
- `ChatController.sendMessage` 增加 `characterId` 参数,使每个聊天会话明确归属到某个角色。
- `RealtimeCallWindow.tsx` 中所有 6 处 Tauri `invoke` 调用均注入 `characterId`,确保实时语音会话路由到正确角色。

#### [characterContext.ts](file:///g:/vivian-rs/src/characterContext.ts) — 全局角色身份

模块级单例,提供 `setCharacterId(id)` / `getCharacterId()` 用于在窗口范围内管理当前角色身份。`main.tsx` 在启动时根据 URL `character_id` 参数或窗口 `label` 完成身份设置;其他组件 / 控制器在发起 `invoke` 调用时通过 `getCharacterId()` 读取当前角色 id 注入 `characterId` 参数。

**主窗口特殊性**:
- 仅对子窗口启用 `StrictMode`(主窗口含 Live2D 重型初始化,StrictMode 双执行会拖慢启动)
- 主窗口通过动态 `import App` 避免子窗口加载 Live2D SDK
- 子窗口使用双 RAF + `setTimeout(2000)` 兜底显示

#### [App.tsx](file:///g:/vivian-rs/src/App.tsx) — 主窗口根组件(1925 行)

应用中枢,管理:
- 所有子窗口创建/聚焦/关闭
- 事件监听(app:ready / chat:* / proactive:* / pet:* / tool:* / config:* / tts:* 等)
- 生命周期(启动问候 / 主动对话 / 心理微调 tick)
- 全局快捷键注册(默认 `CommandOrControl+Shift+V`)
- 智能避让 + 隐藏到角落协调
- 拖拽收伞联动

关键常量:
- `DEFAULT_SHORTCUT = 'CommandOrControl+Shift+V'`
- `BUBBLE_WINDOW_WIDTH = 340` / `BUBBLE_WINDOW_HEIGHT = 140`
- `MAIN_WINDOW_REF_HEIGHT = 378`
- `APP_READY_TIMEOUT_MS = 15000`
- `PET_ACTION_DRAIN_INTERVAL_MS = 2500`
- `ENVIRONMENT_UPDATE_INTERVAL_MS = 30_000`
- `IDLE_AWAY_THRESHOLD_SECONDS = 300`

### 6.2 状态管理 Zustand

#### [stores/useAppStore.ts](file:///g:/vivian-rs/src/stores/useAppStore.ts)

**State 字段**:
- `isInitialized` / `isThinking` / `isListening` / `voiceEnabled` / `ttsEnabled`
- `currentBubble` / `isChatOpen` / `isConfigOpen` / `isMemoryOpen` / `isDiaryOpen`
- `isInputDialogOpen` / `autoStartVoice`
- `currentMood` / `userAvatarUrl`

> 注:`messages` / `addMessage` / `setMessages` / `clearMessages` 已从全局 store 移除,聊天消息改为 `ChatWindow` 组件本地 state(单一真相源),避免多窗口共享 store 导致的消息串扰。

**Actions**:
- `setInitialized` / `setThinking` / `setListening` / `setVoiceEnabled` / `setTtsEnabled`
- `showBubble(text, duration)` / `hideBubble()` / `clearBubbleTimer()`(5s 自动关闭)
- `addSettledBubble(bubble)` / `removeSettledBubble(id)` / `clearSettledBubbles()`(已结算气泡段管理)
- `toggleChat` / `toggleConfig` / `toggleMemory` / `toggleDiary`
- `showInputDialog` / `showInputDialogWithVoice` / `hideInputDialog`
- `setMood` / `setUserAvatarUrl`

全局函数:`closeAllPanels()`

**Selector 使用规范**:`App.tsx` 等消费方使用独立 selector 订阅具体字段(如 `useAppStore(s => s.isThinking)`),而非整体订阅 `const store = useAppStore()`,避免无关字段变更触发不必要的重渲染;回调内通过 `useAppStore.getState().xxx` 读取最新值而无需把字段列入依赖。

### 6.3 组件清单

#### components/(21+ 个,含心智观察器子组件)

| 组件 | 文件 | 职责 |
|------|------|------|
| Live2DCanvas | [Live2DCanvas.tsx](file:///g:/vivian-rs/src/components/Live2DCanvas.tsx) | Live2D 主画布,模型加载 + 鼠标跟随 + 10种精细交互检测(single_click/double_click/fast_click/drag_start/drag_end/fast_drag/pet/long_press/mouse_enter/mouse_leave)。双击检测在第二次 pointerdown 时即触发(响应更快),drag_start/fast_drag 检测路径修复(触发后正确 return,确保 handleInteraction 收到事件)。前端动作库执行(17个动作:nod_head/shake_head/tilt_head/look_around/blink_twice/side_glance/bounce_body/body_sway/bow_head/smile/surprised/tail_wag/wink/happy_bounce/shy/pout/curious)。注入 `window.__vivianUpdateCursor`(33ms Rust eval 绕过 IPC)。`useImperativeHandle` 暴露 setExpression/playMotion/focus/executeAction 等;handleInteraction 处理后端返回的 expression/motion/action 三类反馈 |
| ChatWindow | [ChatWindow.tsx](file:///g:/vivian-rs/src/components/ChatWindow.tsx) | 三视图聊天(`view: 'home' \| 'private' \| 'group'`)。**home**:角色选择列表 + 群聊入口;**private**:私聊消息(单角色对话,历史分页 PAGE_SIZE=20 / SCROLL_LOAD_THRESHOLD=100,自定义 Markdown 渲染 + `DOMPurify.sanitize` 防 XSS,`renderMarkdown` 渲染前经 `stripActions()` 过滤括号动作描述,LinkageCard 联动卡片);**group**:群聊消息,群发时遍历在线角色各发一条独立 `stream_id`,跨角色对话通过 `cross:start` / `cross:done` 事件呈现。性能优化:消息列表使用 `@tanstack/react-virtual` 虚拟滚动,仅渲染可视区域 + 缓冲区;`Bubble` 组件用 `React.memo` 包裹,仅在 `text` / `role` 等关键 props 变化时重渲染;聊天消息作为组件本地 state(单一真相源),不进入全局 store |
| ConfigWindow | [ConfigWindow.tsx](file:///g:/vivian-rs/src/components/ConfigWindow.tsx) | 10 Tab 配置(2909 行)。`ProviderSelector` 11 个预设。`ROUTING_TASKS` 8 个路由任务。MCP server 管理。Worldbook 参数调优。保存流程 12 步。日记配置(`get_diary_config` / `set_diary_config`)通过 `getCharacterId()` 传 `characterId` 按角色路由,每个角色的日记配置独立 |
| MemoryWindow | [MemoryWindow.tsx](file:///g:/vivian-rs/src/components/MemoryWindow.tsx) | iOS 风格认知调试窗口(原记忆管理窗口重构)。内部渲染 `<MindInspector />` 作为主内容,包含 7 个顶级页面(Mind/World/Graph/Diary/Profile/Beliefs/Attention/Sessions),其中 Mind 页面内嵌 4 个子视图(Live Mind 实时心智流/Mind Flow 推理轨迹/Context Pipeline 提示词流水线可视化/Reasoning 推理历史详情)。Context Pipeline 视图按 section 层级分组展示(组内按重要性排序、分组抽屉可折叠、自动隐藏 0 字符 section),工具 session 动态加载可用工具完整信息(名称/描述/参数 schema)。标题栏显示角色名紫色胶囊徽章,所有命令通过 `characterId` 按角色路由 |
| DiaryWindow | [DiaryWindow.tsx](file:///g:/vivian-rs/src/components/DiaryWindow.tsx) | iOS 风格日记浏览(1304 行)。`ModernCalendar` 月份日历。7 种心情配置。标题栏显示角色名紫色胶囊徽章(挂载时调用 `list_characters` 匹配 `getCharacterId()` 获取角色名)。`get_diary_entries` / `get_diary_stats` 命令传 `characterId` 按角色路由,每个角色的日记完全独立 |
| StatusWindow | [StatusWindow.tsx](file:///g:/vivian-rs/src/components/StatusWindow.tsx) | 状态面板容器,失焦自动隐藏(不关闭) |
| StatusPanel | [StatusPanel.tsx](file:///g:/vivian-rs/src/components/StatusPanel.tsx) | 心理学状态面板(persona/needs/emotion/relationship/last_appraisal/last_drive/recent_events) |
| BubbleWindow | [BubbleWindow.tsx](file:///g:/vivian-rs/src/components/BubbleWindow.tsx) | 气泡子窗口,监听 `bubble:show`/`update`/`hide`/`settled_add`/`settled_remove`,emit `bubble:ready`。支持多气泡堆叠:流式换行时已结算段落分离为独立气泡,与活跃气泡同时显示在不同位置,各自独立淡出关闭。气泡文本经 `stripActions()` 过滤括号动作描述 |
| ToastWindow | [ToastWindow.tsx](file:///g:/vivian-rs/src/components/ToastWindow.tsx) | Toast 子窗口,listen `toast:show` / `toast:confirm` / `toast:confirm_done` + emit `toast:ready`,有确认卡片时关闭窗口级点击穿透 |
| ConfirmToast | [ConfirmToast.tsx](file:///g:/vivian-rs/src/components/ConfirmToast.tsx) | 工具确认三按钮卡片(拒绝/放行一次/始终允许),30 秒倒计时进度条,无操作自动拒绝,invoke `confirm_tool_execution` 回传 |
| TodoWindow | [TodoWindow.tsx](file:///g:/vivian-rs/src/components/TodoWindow.tsx) | 待办管理,Tab(pending/completed/all) |
| SchedulerWindow | [SchedulerWindow.tsx](file:///g:/vivian-rs/src/components/SchedulerWindow.tsx) | 定时任务,TaskStatus 6 种,TaskType(reminder/tool_call) |
| InputDialog | [InputDialog.tsx](file:///g:/vivian-rs/src/components/InputDialog.tsx) | 底部输入框,支持 autoStartVoice + ASR 事件 |
| ContextMenu | [ContextMenu.tsx](file:///g:/vivian-rs/src/components/ContextMenu.tsx) | 右键菜单 7 项(status/memory/diary/settings/chat/voice/quit) |
| ConfirmDialog | [ConfirmDialog.tsx](file:///g:/vivian-rs/src/components/ConfirmDialog.tsx) | 支持受控 + 命令式 Promise(`ConfirmDialog.confirm(options)`),5 种 iconType |
| ShortcutRecorder | [ShortcutRecorder.tsx](file:///g:/vivian-rs/src/components/ShortcutRecorder.tsx) | 快捷键录制,KeyboardEvent → Tauri accelerator |
| MessageBubble | [MessageBubble.tsx](file:///g:/vivian-rs/src/components/MessageBubble.tsx) | 4 方向尾巴(top/bottom/left/right)。渲染前经 `stripActions()` 过滤括号动作描述,仅显示纯对话文本 |
| Toast | [Toast.tsx](file:///g:/vivian-rs/src/components/Toast.tsx) | ToastType 4 种(info/success/error/warning) |
| LoadingSpinner | [LoadingSpinner.tsx](file:///g:/vivian-rs/src/components/LoadingSpinner.tsx) | SVG 圆形 spinner |
| SystemTray | [SystemTray.tsx](file:///g:/vivian-rs/src/components/SystemTray.tsx) | 不渲染 UI,listen `tray:action` |
| AsrHelpDrawer | [AsrHelpDrawer.tsx](file:///g:/vivian-rs/src/components/AsrHelpDrawer.tsx) | 抽屉式 ASR 后端说明(4 后端) |
| TtsHelpDrawer | [TtsHelpDrawer.tsx](file:///g:/vivian-rs/src/components/TtsHelpDrawer.tsx) | 抽屉式 TTS 后端说明(5 后端) |
| **MindInspector** | [mind-inspector/MindInspector.tsx](file:///g:/vivian-rs/src/components/mind-inspector/MindInspector.tsx) | 心智观察器壳组件,iOS 风格浮动胶囊侧边栏导航 + Large Title + 页面切换动画,管理 7 个顶级页面切换(Mind/World/Graph/Diary/Profile/Beliefs/Attention/Sessions) |
| MindPage | [mind-inspector/pages/MindPage.tsx](file:///g:/vivian-rs/src/components/mind-inspector/pages/MindPage.tsx) | 心智主页面,内嵌 4 个子视图:Live Mind(实时心智流:当前情绪/信念/注意力/工作记忆)、Mind Flow(推理轨迹时间线)、Context Pipeline(提示词流水线可视化:按 section 层级分组展示,组内按重要性排序、分组抽屉可折叠、自动隐藏 0 字符 section,工具卡片显示参数详情)、Reasoning(推理历史:左列表+右详情) |
| WorldPage | [mind-inspector/pages/WorldPage.tsx](file:///g:/vivian-rs/src/components/mind-inspector/pages/WorldPage.tsx) | 世界状态页:时间/天气/节气/在场状态/室友公共状态/统一事件账本 |
| BeliefsPage | [mind-inspector/pages/BeliefsPage.tsx](file:///g:/vivian-rs/src/components/mind-inspector/pages/BeliefsPage.tsx) | 信念系统页:核心信念/关系认知/自我认知 |
| AttentionPage | [mind-inspector/pages/AttentionPage.tsx](file:///g:/vivian-rs/src/components/mind-inspector/pages/AttentionPage.tsx) | 注意力页:当前关注焦点/记忆激活/世界书签激活状态 |
| GraphPage | [mind-inspector/pages/GraphPage.tsx](file:///g:/vivian-rs/src/components/mind-inspector/pages/GraphPage.tsx) | 记忆图谱页:实体关系可视化 |
| DiaryPage | [mind-inspector/pages/DiaryPage.tsx](file:///g:/vivian-rs/src/components/mind-inspector/pages/DiaryPage.tsx) | 日记页:iOS 风格日历视图浏览角色日记,按心情筛选,标题栏显示角色名徽章 |
| UserProfilePage | [mind-inspector/pages/UserProfilePage.tsx](file:///g:/vivian-rs/src/components/mind-inspector/pages/UserProfilePage.tsx) | 用户画像页:展示角色视角下的用户认知。顶部角色切换胶囊 + 副标题"{{char}} 认识的你"明确语义。四层结构化展示——L0 基础身份(姓名/年龄/性别/职业/所在地)/ L0.5 偏好资料(生日/作息/常用网站/喜欢的游戏/兴趣爱好,支持内联编辑和 is_pinned 锁定保护)/ L1 近期状态(最近目标/当前项目/近期偏好,只读自动抽取)/ L2 自由事实(可新增/删除)。数据源:`get_user_facts` / `set_user_fact` / `pin_user_fact` / `delete_user_fact` 命令按 characterId 路由 |
| SessionsPage | [mind-inspector/pages/SessionsPage.tsx](file:///g:/vivian-rs/src/components/mind-inspector/pages/SessionsPage.tsx) | 会话历史页:Conversation 生命周期状态/Episode 边界/关闭原因 |

### 6.4 控制器 Controllers

#### controllers/(6 个,模块级单例)

| 控制器 | 文件 | 职责 |
|--------|------|------|
| BubbleController | [BubbleController.ts](file:///g:/vivian-rs/src/controllers/BubbleController.ts) | 气泡显示控制。`showBubble(text, durationMs=5000)` / `showStreamingBubble(text)` / `settleSegment(completedText, nextText)`(换行分段:将已完成段落分离到 `settledBubbles` 独立显示,活跃气泡继续流式显示下一段,旧气泡不会被顶掉) / `startAutoClose(durationMs)` / `closeAll`。`computeDuration(text)` 按每字 150ms 计算显示时长,最少 4s 无上限 |
| ChatController | [ChatController.ts](file:///g:/vivian-rs/src/controllers/ChatController.ts) | 流式会话管理,`sessions = Map<stream_id, StreamSession>`。监听 `chat:meta`/`inline_meta`/`chunk`/`done`/`error`/`cancelled`。`chat:inline_meta` 接收内联表情标签事件,复用 `onMeta` 回调链即时驱动 Live2D。`sendMessage` 时触发 Layer 1 即时反应(`triggerInstantReact(message, char, 'user')`),`chat:chunk` 中检测首段完成(换行或 40 字符)触发 Layer 2(`triggerInstantReact(aiText, char, 'ai')`),均通过 `analyze_emotion_instant` 命令获取低延迟情绪分类并 `emit('chat:instant_react')` 通知 `useInstantReact` hook 写入 Live2D `instant` 层;失败时 `emit('toast:show', { type: 'error' })` 弹 toast,不降级 |
| LifecycleController | [LifecycleController.ts](file:///g:/vivian-rs/src/controllers/LifecycleController.ts) | 启动问候流程。localStorage `vivian_has_met` + invoke `get_startup_greeting` |
| StreamController | [StreamController.ts](file:///g:/vivian-rs/src/controllers/StreamController.ts) | 流式 JSON 解析状态机(SEARCHING_JSON_START 等 6 状态) |
| TtsStreamQueue | [TtsStreamQueue.ts](file:///g:/vivian-rs/src/controllers/TtsStreamQueue.ts) | TTS 切片队列。MIN_CHUNK_CHARS=15 / MAX_BUFFER_CHARS=100 / MAX_QUEUE_SIZE=8。标点正则 `/[。！？；,。!?;,\n\r]/`。串行播放 |
| index | [index.ts](file:///g:/vivian-rs/src/controllers/index.ts) | 统一导出 |

### 6.5 Hooks

#### hooks/(7 个)

| Hook | 文件 | 职责 |
|------|------|------|
| positioningCoordinator | [positioningCoordinator.ts](file:///g:/vivian-rs/src/hooks/positioningCoordinator.ts) | 模块级单例,协调 useHiding 与 useSmartPositioning 避免并发驱动窗口位移 |
| useHiding | [useHiding.ts](file:///g:/vivian-rs/src/hooks/useHiding.ts) | 隐藏到角落机制。POLL_INTERVAL_MS=1500 / HIDDEN_PEEK_PIXELS=48 / TRANSITION_DURATION_MS=650 / POSITION_STEPS=14。`Corner = 'tl'\|'tr'\|'bl'\|'br'` / `HideReason = 'fullscreen'\|'sleep'` |
| useInstantReact | [useInstantReact.ts](file:///g:/vivian-rs/src/hooks/useInstantReact.ts) | 三层反应系统前端入口。监听 `chat:instant_react` 事件,通过指数平滑器(speed=18)将 FACS 参数写入 Live2D `instant` 层(优先级 1.5,高于 `emotion` 层)。监听 `chat:meta`/`done`/`cancelled`/`error` 自动清除 `instant` 层(由 `manual` 层接管),2500ms 超时自动清除 |
| useLive2DBehavior | [useLive2DBehavior.ts](file:///g:/vivian-rs/src/hooks/useLive2DBehavior.ts) | Live2D 自主行为。17 个动作库(基于模型真实参数,跨模型兼容,Vivian 专属 tail_wag)+ 情绪联动动作池(`selectActionPool` 按 `mood_label` 加权)。`useAutoBehavior`(10 秒 + RAF FPS 采样)+ `useMicroPresence`(呼吸正弦波 + 身体微晃 + 睡眠参数守护)。跨模型兼容工具:`setJawParam` / `setCheekPuff` / `setMouthShrug` 同时写大小写/拼写差异的参数 |
| useSmartPositioning | [useSmartPositioning.ts](file:///g:/vivian-rs/src/hooks/useSmartPositioning.ts) | 智能避让。POLL_INTERVAL_BASE_MS=2500 / MAX=30000 / STEP=5000。invoke `find_safe_position` + 动态间隔。异步 `onFocusChanged` 监听在卸载时正确取消订阅,避免竞态泄漏 |
| useTauriCommands | [useTauriCommands.ts](file:///g:/vivian-rs/src/hooks/useTauriCommands.ts) | 所有 Tauri invoke 命令的 Hook 封装:`useSendMessage` / `useMemories` / `useConfig` / `useMood` / `useTools` / `useTTS` / `useProactive` / `useEnvironment` / `useRelationship` 等 |
| useTauriEvent | [useTauriEvent.ts](file:///g:/vivian-rs/src/hooks/useTauriEvent.ts) | 通用 Tauri 事件订阅 Hook。`useTauriEvent<T>(eventName, handler, deps)` 在 mount 时 `listen`,unmount 时 `unlisten`;处理 unlisten 与组件卸载的竞态(卸载先于 unlisten 完成时丢弃回调),避免对已卸载组件 setState |

#### [utils/Live2DLipsync.ts](file:///g:/vivian-rs/src/utils/Live2DLipsync.ts)

口型同步算法。状态 `'idle'\|'speaking'\|'manual'`。常量 `TARGET_SPEAKING=0.25` / `TARGET_IDLE=0.15` / `SMOOTH_SPEED=0.2`。监听 `lipsync:start`/`update`/`stop`。呼吸偏移叠加 `setBreathOffset(offset)`,动态间隔(空闲稳定 100ms / 活跃 16ms)

#### [utils/ActionText.ts](file:///g:/vivian-rs/src/utils/ActionText.ts)

括号动作描述处理工具。LLM 输出中的括号内容(如 `(轻声笑了笑,对薇薇安那边扬了扬下巴)`)是动作描述,不应在气泡中显示也不应送 TTS 朗读,仅在记忆节点中用紫色渲染。
- `stripActions(text)` — 过滤掉括号内容,返回纯对话文本。用于 `MessageBubble`(气泡显示)和 `ChatWindow`(微信界面气泡)
- `renderTextWithActions(text)` — 将括号内容用紫色斜体 `<span>` 包裹,返回 HTML 字符串。用于 `GraphPage`/`BeliefsPage`(记忆节点渲染)
- Rust 端 `commands/tts.rs` 中的 `strip_action_text()` 实现等效过滤,避免 TTS 朗读动作文本

#### [i18n/index.ts](file:///g:/vivian-rs/src/i18n/index.ts)

`resources = { 'zh-CN': {...} }` / `fallbackLng: 'zh-CN'` / `lng: localStorage.getItem('vivian-lang') || 'zh-CN'`。导出 `changeLanguage(lng)`

#### [types/index.ts](file:///g:/vivian-rs/src/types/index.ts)

与 Rust 后端对齐的 TS 类型:`AiResponse` / `ChatMessage` / `MemoryType`(8 种)/ `MemoryItem` / `EmotionMetrics`(10 维)/ `MoodState` / `ToolInfo` / `AppConfig` / `TtsConfig` / `TtsEngine` / `ProactiveTickContext` / `EnvironmentInfo` 等

### 6.6 前后端通信机制

#### invoke 命令调用(Tauri IPC,219 个命令)

按模块分组:初始化 / 对话流 / 心理学 / 桌宠动作 / 配置 / 窗口位置 / TTS / ASR / 记忆 / 工具 MCP / 心情 / Live2D / 主动对话 / 世界感知 / 关系 / 待办定时 / 日记 / Worldbook / 网络 / 系统

#### emit/listen 事件流

| 事件 | 方向 | 用途 |
|------|------|------|
| `app:ready` | 后端→前端 | 应用初始化完成 |
| `chat:meta` / `chunk` / `done` / `error` / `cancelled` | 后端→前端 | 流式对话(按 stream_id 路由) |
| `chat:inline_meta` | 后端→前端 | 内联标签表情/动作(流式期间即时触发,`InlineTagScanner` 剥离 `<e>/<m>/<s>` 标签) |
| `chat:instant_react` | 前端→前端 | 三层反应系统 Layer 1/2 即时情绪反应(`ChatController.triggerInstantReact` emit,`useInstantReact` 监听。payload: emotion/intensity/facs/layer/character_id) |
| `chat:user_message` / `start` / `assistant_message` / `history-cleared` / `config_error` / `route_fallback` | 主→子窗口 | 聊天窗口同步 |
| `proactive:chunk` / `config-changed` | 后端→前端 | 主动对话 |
| `llm:not_configured` | 后端→前端 | LLM 未配置 |
| `lipsync:start` / `update` / `stop` | 后端→前端 | TTS 口型同步 |
| `bubble:show` / `update` / `hide` / `ready` / `settled_add` / `settled_remove` | 主↔子窗口 | 气泡(`settled_add`/`settled_remove` 用于流式换行分段时已结算气泡的独立显示与关闭) |
| `toast:show` / `ready` / `confirm` / `confirm_done` | 主↔子窗口 | Toast 通知 + 工具确认卡片 |
| `tray:show` / `action` | 后端→前端 | 系统托盘 |
| `config:saved` / `language-changed` / `shortcut-changed` | 配置窗口→其他 | 配置变更同步 |
| `tts:config-changed` | 配置窗口→主窗口 | TTS 开关同步 |
| `diary:written` | 后端→前端 | 日记写入 |
| `todo:changed` / `scheduler:changed` | 后端→前端 | 待办/定时任务变更 |
| `tool:confirmation_request` | 后端→前端 | 工具执行确认(三态:拒绝/放行一次/始终允许,主窗口转发至 toast 子窗口) |
| `pet:action_pending` / `sleep_changed` | 后端→前端 | 桌宠动作队列 + 睡眠状态 |
| `asr:event` | 后端→前端 | ASR 识别事件 |
| `psychology:state` | 后端→前端 | 心理学状态更新 |
| `input:voice_shortcut` | 后端→前端 | 语音输入快捷键 |
| `cross:start` / `cross:chunk` / `cross:done` / `cross:error` | 后端→前端 | 跨角色对话流(payload 携带 `source_id` / `target_id` / `stream_id`,由 `CrossCharacterBus.send` 发出) |

#### 鼠标跟随特殊机制

`Live2DCanvas.tsx` 注入全局函数 `window.__vivianUpdateCursor(cursorX, cursorY, winX, winY, winW, winH)`,由 Rust 后台线程每 33ms 通过 `WebviewWindow::eval()` 调用,**绕过 Tauri IPC 节流**,同时驱动 `ticker.update()` 与 microTick。

---

## 七、关键数据流

### 7.1 用户对话流(核心)

```
1. 用户在 InputDialog 输入消息
2. ChatController.sendMessage(message)
   ├─ store.addMessage(userMsg)
   ├─ store.setThinking(true)
   └─ invoke('send_message_stream', { message, stream_id, channel })
3. 后端 commands::chat::send_message_stream
   ├─ main_api_configured 检查
   ├─ 渠道限制(direct 仅 Online 可用,Rest/Offline 拒绝)
   ├─ brain.presence.wake_on_user_interaction()(从 Rest/Offline 回到 Online)
   ├─ brain_lock.lock()(串行化该角色的 think)
   ├─ 【会话生命周期】
   │   ├─ CONVERSATION_MANAGER.start_or_continue("user", char_id, message)
   │   │   └─ None 时 force_new_session(用户主动绕过创建冷却)
   │   ├─ touch_user_message(char_id)(记录用户发言时间戳)
   │   ├─ brain.dialogue.set_session_id(Some(conv.id))(激活 HistoryEntry.session_id)
   │   └─ brain.memory.set_session_id(Some(conv.id))(激活 metadata.session_id 注入)
   ├─ brain.think(&message, true)
   │   ├─ proactive.on_user_interacted()(重置主动对话冷却)
   │   ├─ boost_attention_from_input(规则驱动注意力聚焦)
   │   ├─ spawn consciousness_update_async(fire-and-forget LLM 后处理)
   │   └─ chat_chain.ainvoke(user_input, stream)
   │       ├─ 构造 PipelineState(current_channel + conversation_id + conv_state)
   │       ├─ 凝神模式状态机更新
   │       ├─ 工具上下文刷新
   │       ├─ 8 维情绪向量计算 temperature 覆盖
   │       └─ advisor_chain.ainvoke → 14 Step 执行
   │           ├─ PreProcessing(输入预处理)
   │           ├─ UserMemorySaving(用户消息写入记忆)
   │           ├─ [QueryRewrite ∥ FastSemantic](ParallelStep 并行:LLM 查询重写 + FLARE 按需检索判断 ‖ 嵌入语义分类)
   │           ├─ MemoryRetrieval(BM25 + 向量 + RRF + IVF + Attention-weighted 重排序 + Verifier 过滤 + 低置信度 [需验证] 标记;按需检索跳过时直接返回空)
   │           ├─ PromptBuilding(八层意识模型 + Response Decision + 工具列表 + 内联标签格式说明 + 记忆块忠实度约束)
   │           ├─ WebContextDecision(联网搜索决策)
   │           ├─ Generation(LLM 返回 text + response_mode + tool + control_actions)
   │           │   ├─ 内联标签模式:InlineTagScanner 流式扫描 <e>/<m>/<s> 标签 → emit chat:inline_meta
   │           │   └─ emit chat:chunk(干净文本,标签已剥离)/ chat:meta
   │           ├─ ResponseParsing(解析 response_mode,非 speak 清空 text)
   │           ├─ Validation(空文本 warn + 500 字符句边界截断 + 空白清理 + 注入 router 时轻量幻觉检测)
   │           ├─ ExpressionMotion(内联模式跳过,否则独立 LLM 选择表情/动作/贴纸)
   │           ├─ PsychologyInsight(心理洞察提取)
   │           ├─ MoodUpdate
   │           └─ MemorySaving(AI 消息写入记忆 + add_memory_to_session)
   ├─ 【会话生命周期:think 完成后】
   │   ├─ update_after_round(conv_id, response_mode, reply_text, user_input)
   │   │   └─ 更新 Energy/Novelty/Continuation,决定状态转换
   │   ├─ detect_close_reason(message)(用户输入优先)
   │   ├─ detect_close_reason(reply_text)(Agent 回复兜底)
   │   └─ 命中 → close_with_reason + seal_episode_on_close
   ├─ 清理:stream_emitter / set_channel("wechat") / dialogue.set_session_id(None) / memory.set_session_id(None) / 释放 think_lock
   ├─ emit chat:meta(表情/动作提前推送)
   └─ emit chat:done
4. PsychologyManager.apply_llm_output(PsychologyOutput)
   ├─ Homeostasis 补偿 tick
   ├─ 应用 Appraisal → Emotion/Need/Relationship 增量
   ├─ 缓存 Behavior Drive
   └─ persist
5. 【第三者旁观记忆】在线的其他角色以 perspective="observer" 写入旁观记忆
6. 前端 ChatController 监听事件
   ├─ chat:meta → Live2D 表情/动作(onMeta 回调) + 清除 instant 层
   ├─ chat:inline_meta → 内联标签表情/动作(流式期间即时触发,复用 onMeta 回调链)
   ├─ chat:instant_react → useInstantReact 写入 FACS 到 instant 层(Layer 1/2 即时反应)
   ├─ chat:chunk → BubbleController.showStreamingBubble + TtsStreamQueue.feed + Layer 2 触发(首段完成时)
   ├─ chat:done → TtsStreamQueue.flush + store.addMessage(assistant) + setThinking(false) + 清除 instant 层
   └─ chat:error → emit toast:show(error)
```

### 7.2 跨角色对话流

```
1. 触发源
   ├─ trigger_cross_character_talk 命令(前端手动)
   ├─ TalkToCharacterTool 工具(LLM 主动调用)
   └─ proactive_tick 自动触发(cross_character_reply 触发器)
2. CrossCharacterBus.send(req)
   ├─ 【会话生命周期】CONVERSATION_MANAGER.start_or_continue(source, target, message)
   │   └─ None → 直接返回 CrossCharacterReply{response_mode:"ignore", conv_state:"cooling"} 不调 LLM
   ├─ 目标角色在线检查
   ├─ emit cross:start(携带 conv_id / conv_state)
   ├─ think_lock.lock()(目标角色串行化)
   ├─ set_channel("cross_character")(切换渠道,影响 dialogue 加载/写入)
   ├─ 从 UnifiedEventLedger 检索源↔目标最近 2 条共同事件作为记忆锚点
   ├─ 合成输入:"[源角色名 对你说] 消息内容 + 记忆锚点"
   ├─ brain.think(synthesized_input, true)
   │   └─ prompt 注入 CROSS_CHARACTER_RESPONSE_DECISION(教 LLM 选择 response_mode)
   ├─ 【会话生命周期】update_after_round(conv_id, response_mode, reply_text, user_input)
   ├─ emit cross:done(含 response_mode / conv_state / should_continue)
   └─ 记忆持久化
       ├─ 源角色 dialogue 写 2 条(speaker + listener 视角,channel="cross_character")
       ├─ 非 speak 模式:目标反馈转为描述性文本
       ├─ 源角色补 1 条记忆(speaker 视角)
       ├─ 目标角色补 1 条记忆(speaker 视角,带 response_mode 元数据)
       ├─ 写入 AgentAgent 关系日志
       └─ 更新 A↔B 关系数值 + 异步抽取关系认知事实
3. 前端监听
   ├─ cross:start → 显示跨角色对话气泡
   ├─ cross:chunk → 流式渲染
   ├─ cross:done → 完成显示
   └─ cross:error → 错误提示
```

### 7.3 主动对话 tick 流

```
1. 前端 setTimeout(proactiveTickIntervalRef.current,动态值,初始 10000ms)
2. 构建 ProactiveTickContext(idle_seconds / away_seconds / user_present / window_changed 等)
3. invoke('proactive_tick', { context })
4. 后端 commands::proactive::proactive_tick
   ├─ 跨角色发言冷却检查(15 秒窗口,避免两角色同时发言)
   ├─ 【会话生命周期】sweep_cooling()(清理超时 Cooling 会话)
   ├─ 【会话生命周期】sweep_user_session_timeouts(1800.0)
   │   └─ User↔Agent 会话用户 30 分钟无响应 → close(Timeout)
   ├─ 【会话生命周期】is_user_session_closed 检查
   │   └─ GoodNight/NoResponse/Timeout → 跳过主动搭话
   ├─ 流式 emitter 注入
   ├─ 在场状态自动触发检查(4 条件:心情/被忽略/两角色协调/想念用户)
   │   └─ 命中 → transition + register_world_event + emit presence:changed
   ├─ 行为日志事件注册(心情显著变化/长时间无互动/被忽略,写 UnifiedEventLedger,1 小时节流)
   ├─ Rest/Offline/Busy 状态不主动发话
   ├─ 室友快照刷新(update_companions_snapshot)
   ├─ brain.proactive_tick(触发器判定 → LLM 生成 → 队列 PendingMessage)
   ├─ 计算 recommended_next_interval_ms = compute_adaptive_tick_ms(idle_seconds)
   ├─ 消息分类
   │   ├─ cross_character_reply 触发器 → 走跨角色路径(7.2)
   │   └─ 其他 → 用户路径
   ├─ LAST_SPOKEN 更新(仅基于用户消息,避免跨角色对话互相冷却)
   ├─ 用户消息处理(写入 dialogue + 异步写入记忆)
   └─ 跨角色消息分发(选第一个在线室友,调 CROSS_CHARACTER_BUS.send)
5. 前端接收返回值
   ├─ proactiveTickIntervalRef.current = recommended_next_interval_ms(动态调整下次调度)
   ├─ proactive:chunk → BubbleController.showStreamingBubble
   └─ proactive:messages → BubbleController.showBubble(8000ms)+ ttsApi.speak
6. 8 秒无互动 → proactiveApi.markIgnored() → on_ignored
   ├─ ignored_count++
   ├─ 达阈值 → 1 小时安静模式
   ├─ intimacy 负向反馈
   └─ 【会话生命周期】close_pair_with_reason("user", char_id, NoResponse)
```

### 7.4 心理微调 tick 流

```
1. 前端 setTimeout(正常 3000ms / 睡眠 30000ms)
2. invoke('psychology_micro_tick', { characterId })
3. 后端 commands::emotion::psychology_micro_tick
   ├─ state.get_character(character_id.as_deref()) 路由到目标角色实例
   ├─ brain.psychology.micro_tick()
   │  ├─ Homeostasis 稳态调节(指数回归 + 噪声 + 极值回避)
   │  ├─ 昼夜节律调制(4 锚点线性插值)
   │  ├─ micro_tick_count++,累积 20 次才 persist
   │  └─ 关系衰减(缺席 > 4h)
   └─ app.emit("psychology:state", { character_id, snapshot, mood })
      └─ payload 携带 character_id 字段,前端按角色过滤
4. 前端 StatusPanel.tsx listen('psychology:state')
   ├─ if (characterId !== event.payload.character_id) return  // 跨角色事件丢弃
   ├─ setSnapshot(snap) / setMood(m)
   └─ mouseFollowMode 随 mood 实时切换
```

### 7.5 桌宠动作队列流

```
1. 工具层(如 pet_behavior_tools)或auto_trigger调用 push_action(expression/motion/action/bubble 等)
2. 后端 emit pet:action_pending
3. 前端 listen pet:action_pending → invoke('drain_pet_actions')
4. 前端根据 kind 分发:
   ├─ expression → live2dRef.setExpression(target, duration_ms)
   ├─ motion/animation → live2dRef.playMotion(target)
   ├─ action → executeAction(model, target as ActionName, params)  (前端动作库:nod_head/shake_head/tilt_head/look_around/blink_twice/side_glance/bounce_body/body_sway/bow_head/smile/surprised/tail_wag/wink/happy_bounce/shy/pout/curious)
   ├─ idle → invoke('trigger_idle_action')
   ├─ bubble → store.showBubble(text)
   ├─ mood → live2dRef.setExpression(expression, 3000)
   ├─ state → live2dRef.setAsleep(true/false)
   └─ window → getCurrentWindow().setPosition/setSize
5. 兜底轮询 PET_ACTION_DRAIN_INTERVAL_MS=2500ms
6. auto_expression_tick: 前端 setInterval 4秒 调用,驱动空闲/心情自动表情触发
7. trigger_system_event: 前端在 window focus/blur、时间段变化等事件时主动调用
```

### 7.6 启动流程

```
1. main.rs → vivian_lib::run()
2. init_logging() — 按日生成日志,清理 7 天
3. AppState::new() — 创建 ConfigManager / PetController / Scheduler / McpManager
4. tauri::Builder::default()
   ├─ 注册 6 个插件
   ├─ manage(AppState) + manage(LipsyncRuntime)
   ├─ generate_handler![219 个命令]
   └─ setup:
       ├─ setup_tray() — 系统托盘
       ├─ pet_controller.start() — Live2D 状态机
       ├─ 注入 AppHandle 到 ToolSystem / todo_tools / pet_tools / CROSS_CHARACTER_BUS
       ├─ TodoService::load() — 加载待办
       ├─ start_asr_event_forwarder() — ASR 事件桥接
       ├─ register_voice_shortcut() — 语音快捷键
       └─ async spawn:
           ├─ state.initialize()
           │   ├─ ModelRouter::new()
           │   ├─ MemoryManager::new()
           │   ├─ register_builtin_tools()
           │   ├─ mcp_manager.init_all() — 连接 MCP server
           │   ├─ Scheduler::run() — 后台调度
           │   └─ Brain::new_with_pet_controller()
           ├─ router.set_app_handle()
           ├─ emit app:ready
           └─ 自动定位(若世界感知启用且未配置经纬度)
5. 前端 App.tsx
   ├─ waitForAppReady() — 监听 app:ready(15s 超时兜底)
   ├─ LifecycleController.initGreeting() — 启动问候
   ├─ proactiveApi.start() — 主动对话
   └─ store.setInitialized(true)
```

---

## 八、依赖关系总览

### 8.1 关键依赖链

```
commands/ → 几乎所有领域模块(通过 AppState)
     │
     ↓
brain/ → dialogue/ / emotion/ / proactive/ / persona/ / memory/ / providers/ / psychology/ / pipeline/ / tools/
     │
     ↓
pipeline/ → providers/ / memory/ / network/web_context / emotion/
     │
     ↓
dialogue/ → providers/(LLM 调用) / memory/ / brain/
proactive/ → world/ / psychology/ / memory/ / providers/ / brain/scheduler
diary/ → brain/ / memory/ / providers/ / psychology/
speech/ → network/ / config/ / resilience/ / engine/(口型同步)
engine/ → pet_controller/ / resource_loader
pet_controller/ → engine/(AnimationManager / ExpressionManager / StateMachine / ResourceLoader)
network/ → config/(代理) / pipeline/(WebContextRunnable)
config/ → utils/path / i18n/
metrics/ → utils/path
feature_flags/ → utils/path
messages/ → types/response(互转) / i18n/(render 协作)
resilience/ → 独立(仅 tokio/parking_lot/once_cell)
i18n/ → 独立(仅 serde_json/once_cell/parking_lot)
```

### 8.2 持久化统一模式

所有需要落盘的模块统一采用 **tmp+rename 原子写入**。多角色架构下,角色相关数据按 `char_id` 分桶存储在 `<user_data_dir>/characters/<char_id>/` 下:

- `feature_flags.rs::persist()` → `feature_flags.json.tmp` → rename
- `metrics.rs::persist()` → `metrics_YYYY-MM-DD.json.tmp` → rename
- `diary/mod.rs::save_diary_file(char_id)` → `characters/<char_id>/diary/diaries.json.tmp` → rename(每个角色独立日记存储)
- `dialogue/mod.rs` 写入缓冲区 → tmp+rename(损坏文件自动备份)
- `psychology/relationship_log.rs` → `.tmp` → rename
- `psychology/manager.rs::persist()` → `.tmp` → rename
- `todo_tools.rs` 待办列表 → `.tmp` → rename
- `scheduler.rs` 定时任务 → `scheduled_tasks.json.tmp` → rename

### 8.3 跨 await 锁安全模式

多个命令文件采用相同模式:先在 await 前 clone `Arc` 引用并释放 `RwLockReadGuard`,避免 parking_lot guard 非 Send 跨 await:
- `commands/diary.rs::generate_diary_intelligent`
- `commands/emotion.rs::analyze_emotion_deep`
- `commands/memory.rs::get_memories`
- `commands/proactive.rs::drain_proactive_messages`
- `commands/config.rs::test_network_connection`

### 8.4 同步互斥锁选型规范

| 场景 | 锁类型 | 理由 |
|------|--------|------|
| 普通同步互斥 | `parking_lot::Mutex` | 不中毒(panic 不污染锁)、API 简洁(`.lock()` 无需 unwrap)、性能优于 std |
| 跨 await 持有 | `tokio::sync::Mutex` | 异步可等待,guard 跨 await 点安全(Tokio Send 边界) |
| WNDPROC 等回调路径 | `parking_lot::Mutex` + `try_lock()` | 回调中不能阻塞,`try_lock()` 失败时保守降级避免死锁 |
| 异步 RwLock | `tokio::sync::RwLock` | 跨 await 的读多写少场景 |

涉及模块:`memory/vector_search` / `memory/ivf_index` / `memory/unified_event_ledger` / `memory/manager` / `dialogue/mod`(约 25 处) / `emotion/llm_classifier` / `commands/click_through`(try_lock) / `tools/mcp`(McpClient 内部状态)。

### 8.5 ExitRequested 退出超时

`lib.rs` 的 `RunEvent::ExitRequested` 钩子执行记忆脏数据 flush 时,通过 `tokio::time::timeout(Duration::from_secs(3), flush_future)` 包装,超时后强制返回并 `tokio::task::yield_now()` 让出执行权,防止持久化阻塞导致系统退出卡死。

### 8.6 全局状态注入模式

`static X: Lazy<RwLock<Option<T>>>` + `set_xxx()` 注入(仅限无角色归属的全局资源):
- `todo_tools::set_scheduler()` / `set_app_handle()`
- `pet_tools::set_app_handle()`
- `ToolSystem::set_app_handle()`
- `ModelRouter::set_app_handle()`

> 注:以下角色相关全局静态注入已全部移除,改为通过 `character_registry` 按 `char_id` 路由或由 `Brain::build` 在构造时注入对应角色的 `Arc<ResourceManifest>`:
> - `memory_tools::set_memory_manager()` / `relationship_tools::set_psychology_manager()` / `MemoryService::install()` — 走 `character_registry` 按 `char_id` 路由(详见 5.4 多角色隔离机制)
> - `engine::manifest::set_manifest()` / `get_manifest()` / `normalize_expression()` / `normalize_motion()` / `emotion_to_expression()` / `interaction_feedback()` / `prompt_expression_names()` / `random_mood_expression()` 共 8 个全局静态便捷函数 — 全部删除,改为 `ResourceManifest` 实例方法,由 Brain::build 注入到 PsychologyManager / EmotionBridge / ResponseParsingRunnable / ExpressionManager 4 个依赖(详见 5.13)
> - `tools::emotional_recovery::EMOTIONAL_STATE: Lazy<Arc<RwLock<EmotionalState>>>` 单一全局状态 — 改为 `Lazy<RwLock<HashMap<String, EmotionalState>>>` 按 `char_id` 索引,`get_emotional_state(char_id)` / `set_emotional_state(char_id, state)` 同步增加 `char_id` 参数,4 个工具从 `ctx.char_id` 读取
> - `commands::emotion::LAST_TRIGGER: AtomicI64` 全局冷却时间戳 — 改为 `Lazy<RwLock<HashMap<String, i64>>>` 按 `char_id` 索引,`mood_expression_tick` 中冷却时长按角色差异化读取 `CharacterBehavior::get_behavior(char_id).mood_expression_cooldown_secs`(Vivian 30s / Nana 15s)

---

## 九、构建与运行

### 9.1 环境要求

| 依赖 | 最低版本 | 说明 |
|------|---------|------|
| Rust | 1.75(stable) | 后端工具链 |
| Node.js | 18 | 前端构建 |
| Windows | 10 / 11 | 当前仅支持 Windows(依赖 WinRT 语音识别与 ASR) |

### 9.2 开发模式

```bash
# 安装前端依赖
npm install

# 同时启动 Vite + Tauri(热重载)
npm run tauri:dev
```

- Vite dev server 启动在 port 1420(strictPort)
- Tauri 加载 `http://localhost:1420`
- HMR:dev host 模式下使用 ws 协议 port 1421
- Rust 改动触发 `src-tauri/` 重新编译(前端 watch 忽略该目录)
- Live2D Cubism Core SDK 通过 CDN 加载(仅主窗口 + status 窗口)
- 开发模式下 Tauri 自动打开 devtools

### 9.3 验证

```bash
# Rust 编译检查
cd src-tauri && cargo check

# TypeScript 类型检查
npx tsc --noEmit
```

### 9.4 构建发布版

```bash
npm run tauri:build
# 产物位于 src-tauri/target/release/bundle/
```

- Vite 构建前端产物到 `dist/`
- Tauri 打包 Rust 后端 + 前端资源为可执行文件
- bundle targets:`nsis` / `msi`
- release profile:`opt-level=3` / `lto="fat"` / `codegen-units=1` / `panic="abort"` / `strip="symbols"`

### 9.5 配置文件路径

| 路径 | 内容 |
|------|------|
| `%APPDATA%\Vivian\config.yaml` | 应用配置(LLM API Key/Endpoint/Model/routing_matrix/providers 等) |
| `%APPDATA%\Vivian\logs\vivian_YYYY-MM-DD.log` | 按日日志(保留 7 天) |
| `%APPDATA%\Vivian\logs\metrics_YYYY-MM-DD.json` | 性能指标(每日轮转) |
| `%APPDATA%\Vivian\config\feature_flags.json` | 功能开关 |
| `%APPDATA%\Vivian\mcp\servers.json` | MCP server 配置 |
| `%APPDATA%\Vivian\psychology\relationship_log.json` | 关系演化日志 |
| `%APPDATA%\Vivian\scheduled_tasks.json` | 定时任务 |
| `%APPDATA%\Vivian\characters\<char_id>\diary\diaries.json` | 角色独立日记(由 `diary::diaries_file(char_id)` 返回) |
| `%APPDATA%\Vivian\characters\<char_id>\diary\config.json` | 角色独立日记配置(enable_auto_diary / auto_diary_time / min_interaction_threshold / max_diary_length) |
| `%APPDATA%\Vivian\characters\<char_id>\memory.json` | 角色独立记忆数据(`<char_id>` 如 `nana` / `vivian`,由 `get_character_data_dir(char_id)` 返回) |
| `%APPDATA%\Vivian\characters\<char_id>\persona.json` | 角色独立人格数据 |
| `%APPDATA%\Vivian\characters\<char_id>\psychology.json` | 角色独立心理学状态 |
| `%APPDATA%\Vivian\characters\<char_id>\history.json` | 角色独立聊天历史 |
| `%APPDATA%\Vivian\characters\<char_id>\user_facts.json` | 角色独立的用户事实画像(L0 稳定身份 + L0.5 结构化偏好 + L1 近期状态 + L2 自由事实,按角色隔离存储,不同角色对用户的认知可差异化) |
| `localStorage['vivian_has_met']` | 启动问候标志(前端) |
| `localStorage['vivian-lang']` | 语言偏好(前端,默认 zh-CN) |

### 9.6 配置即时生效流程

ConfigWindow 保存后:
1. 递归 `setDeep` 写入所有配置项
2. `save_config` 写入磁盘
3. `set_tts_config` + emit `tts:config-changed`
4. `set_diary_config`
5. emit `config:language-changed`(若语言变更)
6. emit `config:saved`(同步非语言配置)
7. `reinitialize`(重新初始化 Brain / ModelRouter)
8. `update_proactive_config` + emit `proactive:config-changed`
9. `update_world_config`(天气/内心独白/记忆巩固开关)
10. `update_asr_config`
11. emit `toast:show`
12. 关闭配置窗口

---

## 十、配置系统

### 10.1 路由矩阵任务类型

| 任务类型 | 用途 |
|---------|------|
| `chat` | 日常闲聊与问答(高频,可用便宜模型) |
| `reasoning` | 长输入(>100 字)的深度推理(低频,需强模型) |
| `diary` | 日记内容生成 |
| `memory` | 写入时抽取关键词/重要性/语义类型(高频,建议便宜模型) |
| `embedding` | 记忆向量索引的嵌入服务 |
| `reflection` | 短期→长期摘要、画像抽取、洞察生成(低频,需强推理模型) |
| `inner_monologue` | 离线内心独白(30 分钟一次,建议廉价快速模型) |
| `consolidation` | 夜间记忆巩固(睡眠时整理记忆,低频,需深度推理模型) |

- 未配置的任务将回退到 LLM 主配置
- 任务 provider 失败后自动 fallback,通过 `chat:route_fallback` 事件通知前端

### 10.2 配置方式

1. **可视化**:右键桌宠 → 设置(ConfigWindow 提供 10 个 Tab:通用 / AI / 工具 / 记忆 / 语音 / 主动对话 / 真实世界 / 网络 / 日记 / 关于)
2. **直接编辑**:关闭 Vivian 后编辑 `config.yaml`,重启生效
3. **Tauri 命令**:`get_config` / `set_config` / `save_config` / `reload_config` / `update_world_config` / `list_mcp_servers` / `add_mcp_server` / `remove_mcp_server` / `get_worldbook_params` / `set_worldbook_params`

### 10.3 错误处理

- `VivianError` 枚举(15 种变体,均带中文错误信息前缀)
- `VivianResult<T> = Result<T, VivianError>`
- 命令层使用 `err_str` 统一将错误转字符串返回前端
- 实现 `From<reqwest::Error>` / `From<io::Error>` / `From<serde_json::Error>` / `From<rusqlite::Error>`
- **错误传播策略**:核心数据结构(`MemoryVectorStore::add/delete/clear` 等)返回 `VivianResult<()>` 向上传播;非关键路径(hooks runner / scheduler / feature flags 持久化等)错误以 `tracing::warn!` 记录后降级
- **降级路径可观测**:嵌入服务失败(`MemoryManager` / `ConsolidationPipeline` / `AutoStrategy`)、主动对话 LLM 查询失败(`BehaviorDecider` / `IceBreaker` / `RecallTopic` / `stream_query_and_parse`)、文件操作失败(`save_user_avatar` / `clear_user_avatar` 删除残留头像)等历史 `.ok()?` / `let _ = ...` 静默吞错路径全部改为 `tracing::warn!` 记录,便于排查"AI 突然变笨"或"清理操作未生效"类问题
- **TOCTOU 防护**:文件/头像相关命令移除 `exists()` 预检,直接尝试 IO 操作并匹配 `ErrorKind::NotFound` 原子返回友好错误,避免"检查后使用"窗口期文件被替换/删除导致的竞态;`std::fs::remove_file` 失败时区分 `NotFound` 与其他错误,仅在非 NotFound 时 warn 记录
- **日志安全**:token 等敏感字段在日志中做 URL mask(`providers::wenxin` / `speech::aliyun_backend`);`truncate_for_log` 函数截断长文本避免日志膨胀

---

## 十一、资源与人格定义

### 11.1 src-tauri/prompts/(模块化人设与框架定义)

采用**双角色独立 + 通用框架共享**的模块化结构,每个角色拥有 8 个独立 Markdown 文件定义人设,通用规则由 framework/ 目录统一提供。Tera 模板引擎(`system_prompt.tera`)负责最终组装。

#### 角色定义层 `characters/{char_id}/`(每角色 8 个文件)

每个角色(vivian/、nana/)拥有独立的人设文件,职责单一可独立维护:

| 文件 | 内容 | 设计原则 |
|------|------|---------|
| `identity.md` | 核心身份锚点(你是谁) | 一句话定义核心身份,用具体行为而非形容词堆砌 |
| `personality.md` | 场景化人格 | "触发→反应"行为脚本,具体场景替代形容词列表(如"被吐槽时→翻白眼但不真生气") |
| `speech.md` | 说话风格 | 节奏/语气/口头禅/自称/句尾/停顿习惯/禁用模式,含正反例 |
| `examples.md` | Few-shot 示例 | 约 5 个角色专属对话示例,避免模型模仿特定句子而非学习风格 |
| `background.md` | 背景设定 | 日常生活/作息/环境细节,让角色落地到真实世界 |
| `interests.md` | 兴趣爱好 | 具体喜好而非泛泛而谈 |
| `relationships.md` | 关系设定 | 与用户/室友的关系定位 |
| `appearance.md` | 外观描述 | 发色/瞳色/服装/体型等视觉特征 |

**Vivian(薇薇安)**:weeb 网络少女,傲娇二次元性格,紫色长卷发、红色双瞳、哥特洛丽塔连衣裙、紫黑小洋伞。说话直接带刺但内心温暖,句尾偶尔带"哼""切",吐槽犀利。

**Nana(娜娜)**:温柔大姐姐人设,银白短发狐耳狐尾,治愈系陪伴形象。说话轻柔缓慢,用"呢""呀"结尾,关心他人感受。

#### 通用框架层 `framework/`(所有角色共享,7 个文件)

| 文件 | 内容 |
|------|------|
| `chat_style.md` | 聊天风格通用规则(像发微信不像写作文,简短自然) |
| `address_rules.md` | 称呼规则(避免客服式称呼,使用自然口语) |
| `conversation_rhythm.md` | 对话节奏(回复长度、停顿、打断模式) |
| `session_rules.md` | 会话规则(新会话/续聊/首次见面的处理逻辑) |
| `speaker_prefix.md` | 说话者前缀标记格式 |
| `output_format.md` | JSON 输出格式规范 |
| `safety.md` | 安全规则(身份保护/内容边界/工具协议) |

#### 风格预设层 `styles/`(5 个文件)

| 文件 | 风格 |
|------|------|
| `01_default.md` | 默认风格:体现角色核心人格 |
| `02_lively.md` | 活泼风格:话多语速快,更易吐槽接梗 |
| `03_healing.md` | 治愈风格:语气更软节奏更慢,陪着大于开导 |
| `04_focused.md` | 专注风格:话更少点到为止,像旁边坐着的安静朋友 |
| `05_sweet.md` | 甜蜜风格:可多一点语气词但不滥用 |

#### 世界知识层 `worldbook/`(3 个文件,触发式注入)

| 文件 | 触发词 |
|------|--------|
| `game_culture.md` | 游戏/王者/吃鸡/原神/星铁/绝区零/肝/氪/抽卡/保底/出货/欧皇/非酋等 |
| `internet_culture.md` | 梗/表情包/贴吧/B站/微博/抖音/小红书/知乎/推特/油管/meme/玩梗/整活/抽象/乐子/吃瓜等 |
| `anime_culture.md` | 番/番剧/动漫/二次元/新番/追番/补番/OVA/剧场版/声优/CV/作画/崩坏/原作/漫画/轻小说等 |

#### 模板入口

- [system_prompt.tera](file:///g:/vivian-rs/src-tauri/prompts/system_prompt.tera) — Tera 模板主入口,由 `prompt_modules.rs` 的 `PromptBuilder` 按 U 型注意力布局填充各 section 后渲染

### 11.2 public/Vivian/(Live2D 模型)

| 文件 | 用途 |
|------|------|
| `Vivian.model3.json` | Live2D 模型主配置 |
| `Vivian.moc3` | 模型数据(二进制) |
| `Vivian.physics3.json` | 物理摆动配置(头发/衣物) |
| `Vivian.cdi3.json` | 参数显示信息 |
| `Vivian.vtube.json` | VTube Studio 配置 |
| `shy.exp3.json` | 害羞表情(对应前端 Param149) |
| `panic.exp3.json` | 惊慌表情(Param132) |
| `eye_roll.exp3.json` | 翻白眼表情(Param135) |
| `cry.exp3.json` | 哭泣表情(Param144) |
| `angry.exp3.json` | 生气表情(Param150) |
| `umbrella_close.exp3.json` | 收伞表情(Param140) |
| `scene1.motion3.json` | 场景动作 |
| `mystery_animation.can3` | 神秘动画(Cubism Animation) |
| `items_pinned_to_model.json` | 模型附加项配置 |
| `Vivian.4096/texture_00.png ~ texture_10.png` | 11 张 4096 纹理贴图 |
| `model_manifest.json` | **角色表情/动作映射配置**(项目自有配置),定义四类触发映射:expressions(表情语义→文件映射)、motion_aliases(动作别名)、interaction_map(10种交互类型→反馈)、idle_triggers(5阶段空闲触发)、event_triggers(程序事件触发)、mood_idle_expressions(心情持续表情)、mood_triggers(心情表情池)。每个角色独立配置,体现不同性格(Vivian偏沉稳内敛,Nana更活泼可爱)。 |

---

## 十二、关键设计要点

1. **五层心理架构 + Homeostasis 稳态引擎**:Persona → Needs → Appraisal → Emotion → BehaviorDrive 完整因果链,所有维度围绕 set point 自动调节,昼夜节律 4 锚点线性插值调制。Mood / PetState 不参与决策,仅 UI 展示。

2. **三层记忆 + 混合检索 + 写入时 LLM 增强**:ShortTerm / MidTerm / LongTerm 三层,BM25(jieba)+ 向量 + RRF + IVF 四重检索,五因子加权评分。写入时即做 LLM 分类与元数据抽取,分类结果存储在 `memory.metadata["classification"]`;读路径的 LLM 调用(检索后 Verifier 过滤、生成后幻觉检测)均为可选增强,失败时降级为原行为不阻塞主流程,详见 [RAG 幻觉抑制](#rag-幻觉抑制五层防御) 章节

3. **LangChain 风格 Runnable 流水线**:14 个独立 Step,可通过 `|` 操作符声明式组合(QueryRewrite ∥ FastSemantic 经 `ParallelStep` 并行执行),4 个 Advisor 拦截器(日志 / 限流 / Re2 / 循环检测),StreamEvent 统一流式协议,PipelineState 55 字段贯穿全链。

4. **多 Provider 路由矩阵 + 任务分组并发**:9 种 ProviderKind(OpenAiCompat/OpenAiResponses/DoubaoResponses/ChatCompletions/Gemini/Anthropic/Wenxin/Spark/Custom),13 种任务类型独立配置模型,三级 fallback(task_providers → main_provider → 全部失败 toast),任务分组信号量(chat_reasoning=3/memory_reflection=3/auxiliary=2)防止后处理挤占主对话资源。6/8 provider 支持原生 FC 与图片输入。4 种缓存策略(Auto/PromptCacheKey/CacheControl/None)。Strict 熔断持久化避免反复触发 schema 错误。视觉能力首次发图 16×16 PNG 探测 + 按 model 缓存。

5. **工具系统 7 步管线 + 权限网关 + 沙箱**:68 内置工具 + 2 元工具,7 步执行(查找 → 沙箱 → 验证 → 缓存 → 权限 → 执行 → 缓存写入),AgentAccessLevel × ToolRiskTier 权限矩阵(Network / Shell 特判),三态用户确认(拒绝 / 放行一次 / 始终允许,oneshot channel + 5 分钟 TTL,应用信任列表持久化 + 会话级放行列表双快速通道,toast 三按钮卡片 30 秒自动拒绝)。MCP 原生集成(手写 JSON-RPC 2.0 over stdio)。Skills 渐进式披露(指令层 vs 执行层分离)。

6. **主动对话 13 触发器 + 9 心理状态 + 偏好学习**:13 种触发器独立冷却 + 全局最小间隔,9 种 PetMindState(含真正入睡 Sleep),per-trigger EWMA 偏好学习自动适应用户偏好,连续被忽略次数达阈值进入 1 小时静默(阈值按角色差异化:Vivian 5 次 / Nana 2 次)。三人共处一室互动:CrossCharacterReply 时间衰减替代 5min 硬屏蔽,BystanderInterjection 旁观插话 + roommate_cue 信号机制让旁观者自然加入对话。

7. **真实世界感知 + 自主活动**:时间/节气/节日/天气/日出日落 + 8 种 WorldEventKind 驱动情绪。内心独白(冷却 30 分钟,50-120 字第一人称)写入记忆不打扰用户。用户活动日志(Win32 API 每 5s 轮询,FIFO 100 条)作为 Vivian 观察用户的信息源。后台知识采集在 Busy 状态下搜索网络→LLM 总结→写入 RAG 向量知识库,采集/分享双 30 分钟冷却避免机械触发,LLM 自主决策主题用「最近 3 条 SessionSummary 话题总结 + 最近 5 条短期记忆」锚定用户兴趣(无锚点可返回 `[none]` 跳过采集),`[share]` 必须带理由前缀避免频繁推送链接,知识文档携带 TTL 分级(short=7天/mid=30天/long=永不过期),检索时施加时间衰减(30天半衰期)+过期降权(0.3倍惩罚),后台采集时自动刷新过期知识。

8. **夜间记忆巩固(睡眠模拟)**:在配置的睡眠窗口内 + 6 小时冷却到期时,异步执行完整巩固流水线(Stage 1/2/3),Stage 2 六路并行反思(含 relationship signals + L1 近期状态)。

9. **Live2D 多窗口 + 智能避让 + 隐藏到角落**:主窗口 + 9 个子窗口(chat/config/memory/diary/status/bubble/toast/todo/scheduler),URL `?view=` 路由。智能避让(2.5-30s 动态轮询 + Win32 GDI 图像分析)与隐藏到角落(全屏/睡眠触发)通过 `positioningCoordinator` 单例协调避免并发。鼠标跟随通过 Rust 33ms `WebviewWindow::eval()` 注入 `window.__vivianUpdateCursor` 绕过 IPC 节流。

10. **流式处理**:LLM 对话按 `stream_id` 路由支持并发会话;TTS 按 15-100 字标点切片串行播放,超 MAX_QUEUE_SIZE=8 合并;流式 JSON 解析状态机兼容后端未解析场景。

11. **启动问候防重复 + 到达问候共享冷却 + LLM Option 返回**:`LifecycleController` 进程内 `greetingShown` + localStorage `vivian_has_met` 持久化。LLM 调用返回 `Option<String>`,失败或空结果不发问候(无模板兜底)。主 LLM API 未配置时终止后续流程并 toast。启动问候与唤醒问候虽不走 tick 触发循环,但成功后经 `record_greeting_arrival` 计入主动问候共享冷却(全局 `last_interaction_time` + `last_trigger_times` 问候键),问候类触发器(WelcomeBack / HourlyGreeting / IdleGreeting / Icebreaker)在 `min_trigger_interval` 静默期内被硬门控拦截,避免刚问候完又触发主动问候。

12. **配置即时生效**:ConfigWindow 保存后 12 步流程,调用 `reinitialize` + `update_proactive_config` + `update_world_config` + `update_asr_config` + emit 多个变更事件,确保新配置无需重启应用即生效。

13. **iOS 风格视觉语言**:MemoryWindow / DiaryWindow / StatusPanel 采用磨砂玻璃 + inset grouped list + 胶囊徽章 + SF Pro 字体层级 + tabular-nums + 弹簧曲线动画 `cubic-bezier(0.16, 1, 0.3, 1)` 统一视觉语言。

14. **模块化人设架构与 U 型注意力调度**:人格定义采用"角色专属层 + 通用框架层"分离架构。每个角色的人设拆分为 8 个职责单一的 Markdown 文件(identity/personality/speech/examples/background/interests/relationships/appearance),拒绝形容词堆砌,使用场景化行为锚点和"触发→反应"行为脚本;通用规则(聊天风格/称呼/节奏/格式/安全)统一放在 framework/ 目录所有角色共享。Prompt 组装采用 **U 型注意力布局**:静态区按 `[CHARACTER]`(首因效应,人格核心最先入脑)→ Style/Relationship → `[EXAMPLES]`(近因效应,生成前最后看到的风格参考)→ `[FRAMEWORK]`(技术规则不内化)→ `[FORMAT SPEC]`(临出口格式提醒)排列,利用 LLM 对 prompt 开头和结尾注意力更强的偏置;动态区使用 Consciousness Assembler 分层意识模型按 Current Mind → World Snapshot(环境/用户在场/室友)→ Social Relationship → Relevant Episode + Relationship Log → Memory → Tail(初见或记忆规则/用户事实/行为画像/Worldbook)→ Tail Guides(渠道/在场/内心反应[无当前念头时注入,近因效应]/响应决策/内联标签/语气注入)→ Tools 顺序组装,工具列表放最后。功能提示词全部动态化(心理洞察、信念生成、思维合成、日记生成、工具身份注入等均使用角色名变量而非硬编码"Vivian"),内心反应中文化并按角色差异化生成(Vivian 直率吐槽/Nana 温柔关心),跨角色语音指南使用行为化约束("你说话比她快,句子更短")替代数值化标签(sass=0.65)。

15. **证据驱动记忆可信度**:每条记忆携带 `reinforcement` / `disputation` 双独立时钟半衰期衰减字段。7 种证据来源(user_fact / user_confirm / user_rebut / user_ignore / user_keyword_rebut / migration_seed / promote_merge)按不同 delta 权重更新评分。`evidence_score = reinforcement - disputation`,`protected` 记忆返回 +∞ 永不归档。分数跌破 `ARCHIVE_THRESHOLD (-2.0)` 时启动 `sub_zero_days` 归档倒计时,累积 14 天后真正归档。保留策略与去重合并均综合 evidence_score 决策。

16. **事件溯源**:append-only `events.ndjson` 日志,15 种事件类型覆盖记忆生命周期。写入契约:append-before-mutate(事件先落盘再修改视图),Sentinel 游标持久化,Reconciler 启动时尾部重放,handler 幂等。10K 行 / 90 天触发 compaction。前向兼容:未知事件类型暂停(非崩溃),保留 sentinel。

17. **流式安全过滤三层管线**:思考链过滤(ThinkingStreamStripper,BUFFERING/PASSTHROUGH 两态状态机,针对 Qwen3.5/3.6/3.7 混合模型)+ 工具调用标记过滤(ToolLeakFilter,跨 chunk 状态机,过滤 `<tool_call>` / `<seed:tool_call>` / `<function>` 三种泄露形态,跟踪代码块避免误伤)+ 提示词占位符泄露检测(正则匹配 `{placeholder}`,排除 `{{name}}` 转义形式,测试 panic / 生产 warn)。

18. **镜像消息系统**:每条消息携带 `MessageMeta`,标记内容来源(User / Assistant / Tool / InnerMonologue / Mirror)。AutoExtractor / UserFactStore 跳过 `memory_disabled` 的消息,避免工具输出 / 内心独白 / 镜像消息被误抽取为用户事实。`tool_result()` 默认携带 `MessageMeta::tool()`。

19. **凝神/专注模式**:漏桶累积器 + 迟滞设计的专注模式状态机,在心理学数值模型之上叠加一层离散认知模式切换。`new_charge = max(0.0, min(charge * retention + score, cap))`。迟滞设计:charge ≥ enter 时时间衰减地板 = enter(不会衰减到零立即退出)。信号评分 `compute_focus_score`:用户输入长度(>150 字 +0.4 / >50 字 +0.2 / <8 字 -0.2)+ 问号(+0.2)+ 复杂度关键词(+0.2)+ 用户情绪(负面 +0.3 / 正面 -0.2)。阈值:retention=0.5 / enter=0.6 / exit=0.3 / cap=1.0 / hard_cap_turns=8。退出原因:Decayed / HardCap / TopicSwitch。副作用:BrainChatChain::ainvoke 每轮调用 focus_state.update() 驱动三态切换;激活时向 messages 追加认知模式 system 指令(放慢节奏、更安静、更有深度);通过 ModelRouter::set_focus_boost 给 provider 注入 thinking_extra_tokens(默认 800)的 max_tokens 额外余量;proactive_tick 期间调用 idle_cooldown 让电荷按 idle retention 衰减。

20. **多角色隔离架构**:从单角色 Vivian 重构为多角色系统(Nana 温柔大姐姐 / Vivian 傲娇二次元),通过 `CharacterInstance` 抽象隔离每个角色的 Brain / Memory / Psychology / Persona / PetController / ResourceManifest / RealtimeVoiceManager / think_lock / online 状态,角色间互不共享可变状态。`AppState.characters: HashMap<String, CharacterInstance>` 是核心容器,`get_character(character_id)` 提供路由(`None` 回退到 `active_character_id`)。持久化按 `char_id` 分桶:`get_character_data_dir(char_id)` 隔离 memory / persona / psychology / history / diary / user_facts,`get_shared_data_dir()` 提供跨角色共享数据目录。记忆系统与日记系统的所有读写函数均接收 `char_id` 参数,存储物理隔离到 `characters/<char_id>/memory/` 与 `characters/<char_id>/diary/`;工具层通过 `ToolUseContext.char_id` 从 `character_registry` 按角色路由到对应 MemoryManager / PsychologyManager,全局静态兜底(MEMORY_MANAGER / VERIFIER_LLM / PSYCHOLOGY_MANAGER)已移除,强制走 char_id 路由。**心情状态完全独立**:`engine::manifest` 模块的全局静态 `MANIFEST: Lazy<RwLock<Option<Arc<ResourceManifest>>>>` 与 8 个便捷函数(set_manifest / get_manifest / normalize_expression / normalize_motion / emotion_to_expression / interaction_feedback / prompt_expression_names / random_mood_expression)已全部移除,改为 `ResourceManifest` 实例方法;`Brain::build` 接收 `Arc<ResourceManifest>` 并传播到 PsychologyManager(`with_manifest`) / EmotionBridge(`new(psychology, Some(manifest))`) / ResponseParsingRunnable(`with_manifest`) / ExpressionManager(`set_manifest`) 4 个依赖;`commands/emotion.rs::mood_expression_tick` 的 `LAST_TRIGGER` 改为 `Lazy<RwLock<HashMap<String, i64>>>` 按 `char_id` 索引,冷却时长按角色差异化读取 `CharacterBehavior::get_behavior(char_id).mood_expression_cooldown_secs`(Vivian 30s / Nana 15s);`psychology_micro_tick` emit `psychology:state` 事件 payload 携带 `character_id` 字段,前端按角色过滤;`tools/emotional_recovery.rs` 的 `EMOTIONAL_STATE` 改为 `Lazy<RwLock<HashMap<String, EmotionalState>>>` 按 `char_id` 索引,4 个工具(detect_emotional_distress / soothe_pet / suggest_recovery_activity / track_emotional_state)从 `ctx.char_id` 读取。多窗口架构中每个角色一个独立 `WebviewWindow`(label = character_id),`main` 控制器窗口(label="main")不加载 App,子窗口 label 通过 `charScopedLabel(base)` 生成 `<character_id>_<base>` 防冲突,子窗口 URL 携带 `character_id` 参数由前端 `characterContext` 注入全局角色上下文。跨角色通信由 `CrossCharacterBus` 单例统一调度,合成输入 `"[源角色名 对你说] 消息内容"` 后调用目标角色 Brain.think,通过 `cross:start` / `cross:chunk` / `cross:done` / `cross:error` 事件回传前端。命令层统一通过 `character_id: Option<String>` 参数路由。前端记忆/日记窗口标题栏显示角色名紫色胶囊徽章,让用户直观区分当前查看的是哪个角色的数据;StatusPanel 子窗口通过 `getCharacterId()` 读取当前窗口角色身份,所有 invoke(`get_recent_events` / `get_psychology_state` / `get_current_mood`)均传 `characterId` 参数,`psychology:state` 事件监听按 `character_id` 字段过滤,确保 Nana 与 Vivian 各自的心情面板只显示自己的状态。

21. **会话生命周期(Conversation Lifecycle)**:`conversation/` 模块把**所有对话**(User↔Agent / Agent A↔Agent B)统一建模为有生命周期的会话对象,是整个多智能体系统的"交通规则"。状态机 `Created → Active → Cooling → Closed` 由 Energy/Novelty/Continuation Score 综合判定,不靠轮数概率结束。**Cooling 窗口**(30 秒)允许高分新消息(continuation_score ≥ 0.80)抢救回 Active,超时则自动 Close。**创建冷却**(60 秒)防止 A 的 LLM 在 B 总 ignore 时反复创建新会话导致无限调用;用户主动发消息走 `force_new_session` 绕过此冷却。**CloseReason 8 种**(Natural/GoodNight/GoodBye/NoResponse/Interrupted/Timeout/Conflict/SwitchTopic)由关键词检测(规则)+ LLM 兜底判定共同决定,触发不同后续行为(GoodNight → 睡眠时段不主动搭话;NoResponse → 不主动搭话直到新 Trigger;Timeout → 同 NoResponse)。**ResponseMode 4 种**(speak/non_verbal/internal/ignore)由 LLM 在一次调用里同时返回,避免每条消息都触发完整 LLM 文本回复——用户发"嗯/哦"时 Agent 可选 non_verbal 只做动作,跨角色路径冷却中直接返回 ignore 不调 LLM。**Episode 联动**:会话 close 时用 `memory_ids` + 会话边界时间戳触发 `EpisodeStore::seal_episode`,让经历边界对齐会话边界。**MemoryFilter 替换**:`is_new_session` 改为查询 `CONVERSATION_MANAGER` 状态机(单一真相源),旧的 1 小时阈值/问候语/短输入启发式逻辑全部移除。**session_id 双通道激活**:`HistoryEntry.session_id` 写入当前 `Conversation.id` 实现对话历史按会话切分;`MemoryManager.set_session_id` 使 `add_memory_inner` 自动向 `metadata["session_id"]` 注入会话 ID（`entry().or_insert` 不覆盖调用方显式值）,前端图谱据此按真实会话边界绘制分组圈。接入点:`commands/chat.rs`(User↔Agent)、`cross_character.rs`(Agent↔Agent)、`commands/proactive.rs`(主动聊天 sweep + is_user_session_closed 检查)、`proactive/mod.rs::on_ignored`(close NoResponse)、`commands/chat.rs::seal_episode_on_close`(Episode 联动)。

22. **共享世界,不共享大脑(多智能体架构)**:智能体仅共享世界状态(World)和事件总线(Event Bus),私有心智(Thought/Belief/Memory/Attention/Goal)完全独立。角色间通过 `CrossCharacterBus` 发布/订阅通信,无直接 RPC 调用;跨角色对话统一为 Communication Event,与用户对话处理逻辑一致(共用 ConversationManager 状态机)。智能体仅能感知其他角色的 Public State(在线状态/在场状态+持续时间/主导情绪+强度/最近发言时间),通过 `roommate_status_text` 暴露,禁止暴露 Private Mind。分布式仲裁:通过 `LAST_SPOKEN` 时间戳(15 秒冷却)、前端 tick 错峰(Vivian 延迟 10s / Nana 延迟 5s)、`PET_MOVE_INTENTS` 位置避让实现无智能仲裁,避免角色冲突。统一事件流架构:全局事件账本(UnifiedEventLedger)作为底层存储,事件结构包含 timestamp/sender/receiver/event_type/content_preview/context_tags/visibility/source_memory;可见性分级为 Public/Participants/Private(observer_id)。

23. **安全加固(纵深防御)**:从输入边界到执行层多重防御,防止 LLM 通过工具调用造成系统损害。
    - **Shell 执行禁用**:`brain::computer_control::execute_shell` 直接返回错误,从架构层杜绝 RCE;`computer_control::open_app` 移除 lookup 失败时的 shell fallback,仅允许白名单 `app_map` 中注册的应用启动
    - **open_application 危险程序黑名单**:`builtin/system_ops.rs` 内置 16 种高危系统程序黑名单(cmd.exe / powershell.exe / wscript.exe / rundll32.exe / regedit.exe 等),路径形式输入直接拒绝;纯应用名通过 where.exe/PATH/Program Files/Start Menu/UWP 五级解析链查找,整个解析过程通过 `spawn_blocking` 异步执行;UWP 路径对 AppID 实施 `is_safe_appid` 白名单校验(仅允许字母/数字/`.`/`_`/`-`/`!`),防止通过恶意 AppID 拼接 PowerShell 命令实现注入
    - **剪贴板操作无 PowerShell**:`set_clipboard_text` 使用 `clip.exe` stdin 管道写入,不拼接 PowerShell 命令,消除命令注入
    - **文件操作沙箱**:6 个文件工具(read_file / write_file / edit_file / list_directory / search_files / grep)调用 `tools::sandbox::is_path_safe` 拒绝路径穿越;write_file / edit_file 调用 `is_sensitive_path` 拒绝写入 Windows / Program Files / System32 等系统敏感目录;`commands::diary::export_diaries_markdown` 调用 `validate_export_path` 校验导出路径
    - **沙箱纵深增强**:路径参数递归遍历整个 JSON 参数树提取所有字符串路径值(不再依赖固定参数名);危险命令正则覆盖 `rm -rf`/`rm -fr`/`rm -r -f`/`--recursive --force`/`format c:`/`del /f/s/q`/fork bomb 等多种变体;无内置安全档案的工具经通用检查后放行,风险分级交由下游权限系统统一管理
    - **MCP 安全**:MCP 配置写入使用 `Mutex<()>` 防止并发竞态覆盖;MCP 子进程 stderr 通过异步任务捕获记录日志,避免静默失败
    - **文件资源限制**:递归遍历最大深度 10 层;grep 结果 ≤500 条、list_directory ≤5000 条、search_files ≤1000 条;`read_file` 使用 `BufReader` 按行读取(支持 offset/limit 跳过),不全量加载文件;grep 正则 `Lazy<Regex>` 预编译复用;所有阻塞 IO 经 `spawn_blocking` 隔离到线程池
    - **GPT-SoVITS 精确端口杀进程**:`kill_port_occupant` 解析 netstat 输出 Local Address 字段精确匹配目标端口号,避免误杀其他端口进程
    - **错误传播而非静默吞错**:核心数据结构(`MemoryVectorStore::add/delete/clear`)返回 `VivianResult<()>` 向上传播;非关键路径错误以 `tracing::warn!` 记录后降级(hooks runner / scheduler / feature flags 持久化);嵌入服务失败(`MemoryManager` / `ConsolidationPipeline` / `AutoStrategy`)与主动对话 LLM 查询失败(`BehaviorDecider` / `IceBreaker` / `RecallTopic` / `stream_query_and_parse`)均改为 warn 记录后降级到模板回退,避免静默吞错导致"AI 突然变笨"类问题难以排查
    - **TOCTOU 防护**:文件/头像相关命令(`image_to_data_url` / `save_user_avatar` / `clear_user_avatar` / `chat.rs` 图片上传)移除 `exists()` 预检,直接尝试 IO 操作并匹配 `ErrorKind::NotFound` 原子返回友好错误;`std::fs::remove_file` 失败时区分 `NotFound` 与其他错误,仅在非 NotFound 时 warn 记录,避免静默吞错留下脏数据
    - **日志安全**:token 等敏感字段做 URL mask(`providers::wenxin` / `speech::aliyun_backend`);`truncate_for_log` 截断长文本避免日志膨胀
    - **URL 协议白名单**:`open_url` 仅允许 http/https,拒绝 file:///javascript:/data: 等危险协议
    - **参数归一化严格化**:工具参数名归一化先过滤非字母数字并转小写后精确匹配,再按长度差异排序选最优候选,避免子串匹配导致参数错误映射

24. **并发模型(parking_lot + try_lock + Semaphore + spawn_blocking)**:
    - **同步互斥锁统一为 `parking_lot::Mutex`**:不中毒(panic 不污染锁)、API 简洁(`.lock()` 无需 unwrap)、性能优于 std;`memory/vector_search` / `memory/ivf_index` / `memory/unified_event_ledger` / `memory/manager` / `dialogue/mod`(约 25 处) / `emotion/llm_classifier` / `tools/mcp` 等模块全部替换
    - **WNDPROC 回调使用 `try_lock()`**:`commands/click_through` 在 `WM_NCHITTEST` 子类化回调中所有 `ENTRIES` / `DRAG_OFFSET` 锁操作用 `try_lock().ok()`,失败时保守视为"拖动中"返回 `HTCLIENT` 避免锁死鼠标;`log_hit_test_transition` 同样 `try_lock`,失败时跳过日志
    - **远程 API 限流**:`memory::embedding` 通过 `REMOTE_EMBEDDING_MAX_CONCURRENCY=4` Semaphore 限流防止外部嵌入服务过载
    - **队列硬上限**:`brain::augment_reply_service::MAX_PENDING_ENTRIES=100` 防止回复增强队列无界增长,超限丢弃新条目并 warn
    - **阻塞 IO 隔离**:所有阻塞系统调用(文件读写、目录递归遍历、进程枚举 `sysinfo::System::new_all()`、应用路径解析、系统信息采集、剪贴板操作)均通过 `tokio::task::spawn_blocking` 提交到专用阻塞线程池执行,避免阻塞 tokio 异步运行时;严禁在 async 函数中直接使用同步 sleep(`std::thread::sleep`)或同步文件 IO
    - **HTTP Client 复用**:GPT-SoVITS 服务管理器持有单个 `reqwest::Client` 实例复用连接池,避免健康检查循环中反复创建 Client 导致 TCP 连接泄漏
    - **退出超时**:`lib.rs::ExitRequested` 钩子带 3 秒超时 + `yield_now`,防止持久化阻塞导致系统退出卡死

25. **降级模式(关键资源初始化失败容错)**:
    - `presence::PresenceManager::new_with_temp_dir(char_id)` — 持久化目录不可写时降级到系统临时目录,在场状态仍可运行但无法持久化
    - `tools::mcp::McpManager::new_disabled()` — MCP server 初始化失败时返回空实现,外部工具调用直接返回错误提示,内置工具与对话能力不受影响
    - `speech::tts_cache::SpeechCache::fallback()` — 缓存目录创建失败时降级到系统临时目录(`%TEMP%\vivian-tts-cache`),`TtsManager::default()` 在 `SpeechCache::new` 失败时自动调用此方法避免 panic
    - `memory::time_stamped::TOKENIZER` — 全局 `cl100k_base` tokenizer 加载失败时降级到 `None`,token 计数回退到字符数估算(中文 1 字 ≈ 1.5 token,ASCII 4 字符 ≈ 1 token),保证 `TimeStampedMemory` 摘要触发逻辑可用
    - `engine::motion_player::MotionCurve::sample_at` — 空关键帧场景降级返回 0.0,避免 `keyframes.last().unwrap()` 在配置异常时 panic
    - 跨 await 锁安全模式:命令文件在 await 前 clone `Arc` 引用并释放 `RwLockReadGuard`,避免 parking_lot guard 非 Send 跨 await(`commands/diary.rs::generate_diary_intelligent` / `commands/emotion.rs::analyze_emotion_deep` / `commands/memory.rs::get_memories` / `commands/proactive.rs::drain_proactive_messages` / `commands/config.rs::test_network_connection`)

26. **前端性能优化**:
    - **Zustand selector 精细订阅**:`App.tsx` 使用 7 个独立 selector(`useAppStore(s => s.xxx)`)替代整体订阅 `const store = useAppStore()`,避免无关字段变更触发不必要的重渲染;回调内通过 `useAppStore.getState().xxx` 读取最新值而无需把字段列入依赖
    - **聊天消息本地化**:`messages` / `addMessage` / `setMessages` / `clearMessages` 从全局 store 移除,聊天消息改为 `ChatWindow` 组件本地 state(单一真相源),避免多窗口共享 store 导致的消息串扰
    - **虚拟列表**:`ChatWindow` 使用 `@tanstack/react-virtual` 虚拟滚动,仅渲染可视区域 + 缓冲区,长对话不卡顿
    - **React.memo**:消息气泡 `Bubble` 组件用 `React.memo` 包裹,仅在 `text` / `role` 等关键 props 变化时重渲染
    - **XSS 防护**:`renderMarkdown` 返回前调用 `DOMPurify.sanitize` 清理 HTML,防止 LLM 输出注入恶意脚本
    - **通用事件订阅 Hook**:`useTauriEvent(eventName, handler, deps)` 在 mount 时 `listen`、unmount 时 `unlisten`,处理 unlisten 与组件卸载的竞态(卸载先于 unlisten 完成时丢弃回调),避免对已卸载组件 setState
    - **异步监听清理**:`useSmartPositioning` 的 `onFocusChanged` 异步监听在卸载时正确取消订阅,避免竞态泄漏

27. **多维度自动表情/动作触发**(`engine/auto_trigger.rs`):表情/动作触发不再依赖 LLM 单一来源,形成四类触发路径互补的多维度系统,全部纯规则驱动零 LLM 开销。**(1) 用户直接交互触发**:`Live2DCanvas` 前端实时检测 10 种精细交互类型(single_click/double_click/fast_click/drag_start/drag_end/fast_drag/pet/long_press/mouse_enter/mouse_leave),通过 `apply_user_interaction` 命令即时查表 `manifest.interaction_map` 返回表情/动作/前端 action;同时检测空闲时长,若 >5 分钟则触发 `user_return` 事件惊喜反馈。双击检测在第二次 pointerdown 时即触发(无需等 pointerup),响应更快;drag_start/fast_drag 检测路径修复(触发后正确 return,确保 handleInteraction 收到事件,伸手表情/动作能正确播放)。**(2) 空闲检测渐进触发**:前端 4 秒间隔调用 `auto_expression_tick`,后端 `IdleStage` 五阶段枚举(Active→Short→Medium→Long→Asleep)按时间阈值(30s/2min/5min/15min)升级,阶段升级时按递增概率(40%/60%/80%/95%)触发对应表情/动作,同一阶段只触发一次避免刷屏;用户任何交互立即重置到 Active。**(3) 心情状态联动**:主导情绪标签改变且 intensity > 0.4 时立即触发 `mood_change_*` 事件表情;空闲 45 秒后 25% 概率触发当前心情持续表情(3 秒自动恢复,45 秒冷却)。**(4) 程序事件触发**:`trigger_system_event` 命令供前端在 window focus/blur、时间段变化(morning/afternoon/evening/night)、对话开始结束等时机调用,每个事件有独立冷却时间防止重复。所有触发统一通过 `PetActionRequest` 队列投递,支持三种目标类型:expression(Live2D exp3 表情)、motion(motion3 动作文件)、action(前端 17 个程序动画动作库:nod_head/shake_head/tilt_head/look_around/blink_twice/side_glance/bounce_body/body_sway/bow_head/smile/surprised/tail_wag/wink/happy_bounce/shy/pout/curious,基于模型真实参数驱动,跨模型兼容,Vivian 专属 tail_wag 利用 4 段尾巴参数 Param_Angle_Rotation_1/3/6/9_ArtMesh321)。`ModelManifest` 扩展四类映射字段:`interaction_map`(交互反馈)、`idle_triggers`(空闲阶段)、`event_triggers`(程序事件)、`mood_idle_expressions`(心情持续表情),每个角色可独立配置体现不同性格(Vivian 沉稳内敛 / Nana 活泼可爱)。全局单例 `AUTO_TRIGGER: LazyLock<AutoExpressionTrigger>` 内部按 char_id 维护独立 TriggerState(last_interaction/last_idle_stage/triggered_idle_stages/current_mood_label/event_cooldowns),多角色完全隔离。概率门控 + 冷却时间双保险避免机械重复。

---

## 附录:架构约束(23 条,与项目 memory 一致)

1. LLM 分类用于记忆类型必须在记忆写入操作时执行,而非读取操作
2. MemoryFilter 不能在读取路径调用 LLM;分类结果必须从 `memory.metadata["classification"]` 读取
3. 中文关键词提取在记忆过滤中必须使用 jieba 而非 split() 以避免失效
4. 关系情感分析必须使用用户输入情感而非 AI 响应情感
5. `record_interaction` 的 text 参数必须从函数签名中移除
6. 亲密度增量计算必须使用 `(intensity * 2.0).floor() + 1.0` 为正向情感以放慢关系进展
7. BrainChatChain 必须使用 `AIResponseGenerationRunnable + ResponseParsingRunnable`,而非 GenerationStep(简化版本不解析 JSON)
8. `user_emotion` / `ai_emotion` 由 LLM 在 JSON 返回中给出,不再用关键词匹配模块代理
9. 沉默标记 `intent="no_reply"` 由 ResponseParsingRunnable 识别并清空 text
10. 当 LLM 输出 `motion="umbrella_close"` 时,ResponseParsingRunnable 必须映射为 `expression="umbrella_close"` 并设置 `motion="idle"`
11. 主 LLM API(api_key / endpoint / model)必须完全配置;否则,终止后续流程并显示 toast
12. 多角色命令层:涉及角色状态的 Tauri 命令必须接收 `character_id: Option<String>` 参数并通过 `state.get_character(character_id.as_deref())?` 路由到目标 `CharacterInstance`(None 时回退到 `active_character_id`)
13. 多窗口子窗口 label 必须按角色区分:由 `App.tsx` 的 `charScopedLabel(base)` 函数生成 `<character_id>_<base>`(如 `nana_chat` / `vivian_status`),严禁跨角色复用同一 base label,避免多角色同时存在时子窗口冲突
14. `main` 控制器窗口(`label="main"`,在 `tauri.conf.json` 预定义为隐藏窗口)不得加载 `App.tsx`;`main.tsx` 检测到 `label === "main"` 且 URL 无 `?view=` 参数时必须渲染空组件,避免 Live2D SDK 与事件监听器被无谓初始化
15. 多角色心情状态独立:`engine::manifest` 模块禁止持有任何全局静态 `MANIFEST` 或便捷函数,所有表情/动作映射必须是 `ResourceManifest` 实例方法,由 `Brain::build` 在构造时注入对应角色的 `Arc<ResourceManifest>` 到 PsychologyManager / EmotionBridge / ResponseParsingRunnable / ExpressionManager 4 个依赖;`commands/emotion.rs::mood_expression_tick` 的 `LAST_TRIGGER` 必须按 `char_id` 索引(`Lazy<RwLock<HashMap<String, i64>>>`),冷却时长按角色差异化读取 `CharacterBehavior::get_behavior(char_id).mood_expression_cooldown_secs`(Vivian 30s / Nana 15s),严禁全局共享冷却时间戳;`psychology_micro_tick` emit `psychology:state` 事件 payload 必须携带 `character_id` 字段;`tools/emotional_recovery.rs` 的 `EMOTIONAL_STATE` 必须按 `char_id` 索引(`Lazy<RwLock<HashMap<String, EmotionalState>>>`),4 个工具从 `ctx.char_id` 读取,严禁全局共享单一情绪状态
16. 前端 StatusPanel 子窗口必须通过 `getCharacterId()` 读取当前窗口角色身份,所有 invoke(`get_recent_events` / `get_psychology_state` / `get_current_mood`)必须传 `characterId` 参数,`psychology:state` 事件监听必须按 payload 中的 `character_id` 字段过滤,严禁跨角色接收事件
17. 主对话 LLM 输出精简:`OUTPUT_FORMAT` 只保留 `text / intent / tool / arguments / control_actions`,不再要求主对话 LLM 返回 `user_emotion` / `ai_emotion` / `appraisal` / `emotion_update` / `behavior_drive` 等心理字段(由独立调用推断);表情/动作可用列表(`manifest_context`)不再注入主对话 prompt,由独立的 `ExpressionMotionRunnable` 在 text 完成后调用 LLM 选择;`control_actions` 中的 `set_expression` / `play_motion` 接受语义名(happy / shy / wave / nod 等),后端通过 `ResourceManifest::normalize_expression` / `normalize_motion` 映射到实际 model3.json Name;`PetController::play_motion` 调用前必须先做 manifest 归一化
18. 角色个性化行为参数(`character_behavior.rs`):本地非 LLM 控制参数必须按 `char_id` 索引(`get_behavior(char_id)` 方法),禁止全局共享单一参数集;`ProactiveOrchestrator::new(char_id)` 持久化路径必须按角色隔离到 `characters/<char_id>/proactive/`,禁止使用全局共享的 `<user_data_dir>/proactive/` 目录;`PsychologyManager::apply_proactive_feedback` 必须接受 `char_id` 参数,增减幅度按 `CharacterBehavior` 差异化;MoodDriven 触发阈值、亲密度冷却系数、安静模式阈值、心情表情冷却时长均读取 `CharacterBehavior` 对应字段

19. 会话生命周期(`conversation/`):所有对话路径(User↔Agent 与 Agent↔Agent)必须通过 `CONVERSATION_MANAGER.start_or_continue` 获取或创建会话,禁止绕过状态机直接调 `brain.think`。`start_or_continue` 返回 None 时:User↔Agent 路径必须调 `force_new_session` 绕过创建冷却(用户主动行为是最高优先级信号);Agent↔Agent 路径必须直接返回 `CrossCharacterReply{response_mode:"ignore"}` 不调 LLM。会话关闭必须记录 `CloseReason`(Natural/GoodNight/GoodBye/NoResponse/Interrupted/Timeout/Conflict/SwitchTopic),由关键词检测(规则)+ LLM 兜底判定共同决定。`on_ignored` 必须调 `close_pair_with_reason("user", char_id, NoResponse)`。`proactive_tick` 必须调 `sweep_cooling` + `sweep_user_session_timeouts(1800.0)` + `is_user_session_closed` 检查,GoodNight/NoResponse/Timeout 时跳过主动搭话。会话 close 时必须触发 `seal_episode_on_close` 让经历边界对齐会话边界。`MemoryFilter::is_new_session` 必须查询 `CONVERSATION_MANAGER` 状态机作为单一真相源,禁止使用旧的 1 小时阈值/问候语/短输入启发式逻辑。`HistoryEntry.session_id` 必须写入当前 `Conversation.id`,禁止永远写 None

20. 共享世界,不共享大脑(多智能体架构):智能体仅共享世界状态(World)和事件总线(Event Bus),私有心智(Thought/Belief/Memory/Attention/Goal)完全独立。角色间通信必须通过 `CrossCharacterBus` 发布/订阅事件,禁止直接 RPC 调用;跨角色对话统一为 Communication Event,与用户对话处理逻辑一致(共用 ConversationManager 状态机)。公共状态暴露:智能体仅能感知其他角色的 Public State(在线状态/在场状态+持续时间/主导情绪+强度/最近发言时间),通过 `roommate_status_text` 暴露,禁止暴露 Private Mind。分布式仲裁:通过 `LAST_SPOKEN` 时间戳(15 秒冷却)、前端 tick 错峰(Vivian 延迟 10s/Nana 延迟 5s)、`PET_MOVE_INTENTS` 位置避让实现无智能仲裁,避免角色冲突。统一事件流架构:全局事件账本(UnifiedEventLedger)作为底层存储,事件结构包含 timestamp/sender/receiver/event_type/content_preview/context_tags/visibility/source_memory;可见性分级为 Public/Participants/Private(observer_id)。四层认知架构(Facts/Episodes/Memories/Beliefs):行为事件(long_idle/quiet_mode/mood_event/presence_log)必须走 `register_world_event` 写入 UnifiedEventLedger(sender=system/receiver=all/visibility=Public),禁止写入 MemoryManager;MemoryManager 只保留 AI 主观记忆

21. RAG 幻觉抑制(五层防御,读路径 LLM 调用均为可选增强):(1) `prompt_modules.rs::build_memory_block` 必须在记忆块末尾追加忠实度约束指令(中/英/日三语),提示记忆可能过时、与用户矛盾时以用户为准、不编造细节、注意 `[需验证]` 标记;(2) `steps/memory.rs` 格式化记忆时必须对 `combined_score` 或 `temporal_adjusted_score` < 0.3 的条目追加 `[需验证]` 标记;(3) `MemoryRetrievalStep` 通过 `with_router` 注入 `ModelRouter` 后,检索结果 >2 条时必须调用 `memory::verifier::verify_retrieval` 过滤无关记忆,LLM 不可用/响应无法解析时降级为全部保留,禁止阻塞主流程;(4) `ValidationRunnable` 通过 `with_router` 注入 `ModelRouter` 后,当记忆上下文非空且回复 ≥30 字符时必须触发轻量幻觉检测,结果仅记录 warning 写入 `state.metadata["hallucination_check"]` 禁止修改回复,超时/失败时跳过;(5) `QueryRewriteStep::should_skip_retrieval` 对闲聊/问候/确认词及纯标点表情输入设置 `metadata.skip_memory_retrieval=true`,`MemoryRetrievalStep::ainvoke` 开头必须读取该标志,为 true 时跳过整个检索步骤

22. 多模态视觉能力自适应(首次发图探测 + 缓存):应用不假设用户填入的 API 支持视觉能力,`send_image_message` 命令在 `ai.enable_vision` 开关检查通过后、实际发图前必须调用 `ModelRouter::check_vision_capability()` 探测目标模型是否接受图片输入。探测用 16×16 透明 PNG + `detail=low`(部分服务商如豆包要求最小 14×14),探测路径与 `vision_describe` 任务实际路由一致(路由矩阵启用时优先任务 provider,否则主 LLM API),绕过 `query_with_fallback` 避免 fallback 掩盖真实结果 + 避免污染路由矩阵 UI 事件。结果按 model 名缓存到 `vision_capability_cache: Arc<RwLock<HashMap<String, VisionCapability>>>`,`save_config` / `reload_config` 时必须调 `clear_vision_capability_cache()` 清空缓存(用户换模型后下次发图重新探测)。`NotSupported` 时必须 emit `chat:error` + 详细 error toast(含原因 + 配置指引,duration 8000ms)拦截发图,禁止把不支持视觉的请求发给 API 导致静默失败。六家 Provider 协议(OpenAiCompat / OpenAiResponses / DoubaoResponses / ChatCompletions / Anthropic / Gemini)必须完整实现图片输入字段转换(`input_image` / `image_url` / base64 `source` / `inline_data`+`file_data`),禁止任何 provider 静默丢弃 `m.images` 字段;文心与星火不支持图片输入

23. 后台知识采集去机械化(`presence/background_tasks.rs`):Busy 状态知识采集必须带 30 分钟采集冷却(`is_knowledge_acquisition_in_cooldown`),避免每次进入 Busy 都触发检索;LLM 自主决策主题(`decide_topics_with_intent`)必须以「最近 3 条 SessionSummary 话题总结(`recent_by_type(SessionSummary, 3)`)+ 最近 5 条短期记忆(`recent_by_tags(&["short_term","casual_conversation"], 5)`)」作为上下文锚点,禁止用固定 query 检索记忆或拼接单条对话消息作为 query;LLM 可返回 `[none]` 表示本次无明确兴趣锚点,直接跳过采集;主题意图分两类——`[internalize]`(内化为知识,常态)与 `[share:理由]`(分享链接给用户,少数情况),`[share]` 必须带冒号+理由前缀,无理由自动降级为 internalize,一次最多 1 个 share 多余的降级为 internalize;链接分享必须带 30 分钟分享冷却(`is_knowledge_share_in_cooldown`),避免频繁推送链接给用户

---

<div align="center">

**Vivian Code Wiki** — 由深度递归遍历代码生成

</div>
