<div align="center">

# Vivian

**具备情感、记忆与主动性的 AI 桌面宠物**

Rust + Tauri 2 + React 18 + Live2D Cubism 4

</div>

---

## 目录

- [概述](#概述)
- [核心能力](#核心能力)
- [会话生命周期](#会话生命周期)
- [技术栈](#技术栈)
- [项目结构](#项目结构)
- [快速开始](#快速开始)
- [配置系统](#配置系统)
- [国际化](#国际化)
- [开发指南](#开发指南)
- [故障排查](#故障排查)
- [联系方式](#联系方式)
- [许可证](#许可证)

> 📖 完整的代码架构、模块职责、关键类与函数说明请参阅 [CODE_WIKI.md](file:///g:/vivian-rs/CODE_WIKI.md)

---

## 概述

Vivian 是一个常驻桌面的多角色 AI 陪伴型宠物系统，支持两个独立角色同时在线——温柔的 Nana 与傲娇二次元的 Vivian，每个角色拥有独立的大脑、记忆、人格、心理状态与 Live2D 形象，并可经跨角色通信总线相互对话。它不只是被动响应消息，而是拥有持续演化的心理状态、跨会话的记忆体系（含证据驱动可信度与事件溯源）、可编排的工具系统，以及主动发起对话的能力。更重要的是，它能感知真实世界——时间、天气、节气、节日、用户的活动——即便用户不交互也能自主思考。所有计算与持久化均在本地完成，仅在调用 LLM 时访问云端。

主要使用场景：

- 日常陪伴对话（流式响应 + 多 Provider 路由 + 联网搜索 + 流式安全过滤）
- 桌面自动化（应用控制、文件操作、媒体控制、屏幕感知、输入模拟）
- 主动关怀（基于作息学习与情绪状态的健康提醒、破冰、压力监控）
- 自我演化（人格、关系、需求、情绪的四层心理因果链 + 凝神/专注模式）
- 真实世界感知（时间/天气/音量/媒体/前台窗口/网络/IP地理位置 + 位置注入提示词 + 世界事件驱动情绪 + 内心独白 + 后台知识采集与时效管理）
- 多角色陪伴（Nana + Vivian 双角色同时在线 + 跨角色对话 + 三视图聊天：角色选择 / 私聊 / 群聊群发）

---

## 核心能力

### 多角色架构

系统从单角色重构为两个独立角色同时在线的多角色架构，每个角色都是一等公民，拥有完整的心理与记忆体系，并可通过跨角色通信总线相互对话。

- **两个独立角色**：
  - **Nana** —— 温柔大姐姐人设
  - **Vivian** —— 网络少女、傲娇、二次元
- **CharacterInstance 抽象**：每个角色独立持有 Brain（大脑核心）+ PetController（桌宠控制器）+ manifest（模型清单）+ realtime_voice（实时语音）+ think_lock（思考互斥锁）+ online（在线状态），彼此完全隔离
- **AppState 重构**：`characters: Arc<RwLock<HashMap<String, CharacterInstance>>>` + `active_character_id`，所有访问按角色 ID 路由
- **独立心智体系**：每个角色拥有独立的 Brain / Memory / Psychology / Persona / Dialogue，记忆、人格、心理状态、对话历史、日记完全隔离，互不污染
- **心情状态完全独立**：每个角色的 `ResourceManifest` 实例由 `Brain::build` 在构造时注入到 PsychologyManager / EmotionBridge / ResponseParsingRunnable / ExpressionManager 4 个依赖，表情/动作映射查询走各自 manifest 实例而非全局静态；`mood_expression_tick` 心情表情冷却 `LAST_TRIGGER` 按 `char_id` 索引(`HashMap<String, i64>`)，冷却时长按角色差异化（Vivian 30s / Nana 15s，由 `CharacterBehavior.mood_expression_cooldown_secs` 提供）；`psychology_micro_tick` 推送的 `psychology:state` 事件携带 `character_id` 字段，前端 StatusPanel 按角色过滤；`emotional_recovery` 工具的 `EMOTIONAL_STATE` 按 `char_id` 索引(`HashMap<String, EmotionalState>`)，4 个工具从 `ctx.char_id` 读取，跨角色完全隔离
- **持久化分桶**：`%APPDATA%\Vivian\characters\<char_id>\` 下各自独立存放 memory / persona / psychology / history / diary / user_facts，让每个角色独立积累对用户的认知（不同角色对用户的印象可以差异化）
- **记忆/日记按 char_id 路由**：MemoryManager 与 DiarySystem 的所有读写函数均接收 `char_id` 参数，存储物理隔离到 `characters/<char_id>/memory/` 与 `characters/<char_id>/diary/`；工具层通过 `ToolUseContext.char_id` 路由到对应角色实例，Tauri 命令层通过 `character_id: Option<String>` 参数路由；前端记忆/日记窗口标题栏显示角色名徽章，让用户直观区分当前查看的是哪个角色的数据
- **跨角色通信总线**（CrossCharacterBus）：全局单例，LLM 可通过 `TalkToCharacterTool` 工具发起跨角色对话，合成输入格式 `[源角色名 对你说] 消息内容`，通过 `cross:start` / `cross:chunk` / `cross:done` / `cross:error` 事件驱动前端流式渲染。工具调用包裹 60 秒 `tokio::time::timeout`，目标角色长时间未响应时返回超时提示而非无限挂起。发送前注入共同情境：`build_handoff_context` 调用处追加 `activity_journal.to_brief()` 作为 `[共同观察]` 段落，让目标角色感知"双方都在观察的用户活动"；发送成功后更新双方的 `LAST_SPOKEN`/`LAST_SPOKEN_TEXT`（speak 模式调用 `record_cross_character_spoken`，非 speak 模式调用 `touch_last_spoken` 仅更新时间戳），确保非 leader 角色跨角色对话后 `LAST_SPOKEN` 不为空，避免 `CrossCharacterReply` 触发器因判定"室友最近未发言"而无法命中。除 LLM 主动发起的跨角色对话外，系统层还有 `roommate_cue` 信号机制：用户与某角色聊天时，`commands/chat.rs` 写入旁观记忆后以 8% 概率调用其他在线角色的 `ProactiveOrchestrator::seed_roommate_cue`，设置 30s TTL 信号提升被 cue 角色的 BystanderInterjection 触发概率，让旁观者自然插话加入对话（三人共处一室语义）
- **跨角色认知传播**（`roommate_cognitive_text()`）：每个角色的 prompt 注入室友的行为印象（注意力焦点、当前活动、最高优先级目标、社交意愿），从私有 Mind 数据派生外部可观察信号，不暴露原始认知结构
- **三视图聊天**：home（角色选择主页）/ private（单角色私聊）/ group（群聊），群聊视图支持群发消息让多角色同时响应
- **多窗口架构**：每个角色一个独立 Tauri WebviewWindow，`label = character_id`；main 窗口（`label="main"`）是隐藏控制器，不加载 App.tsx；子窗口 label 按角色区分（如 `nana_chat`、`vivian_status`），避免多角色窗口冲突；子窗口创建时 URL 携带 `character_id` 参数，前端 `main.tsx` 启动时 `setCharacterId` 注入全局角色上下文，所有 Hook 通过 `getCharacterId()` 自动传递；StatusPanel 子窗口通过 `getCharacterId()` 读取当前角色身份，所有 invoke 调用传 `characterId` 参数，`psychology:state` 事件监听按 payload 中的 `character_id` 字段过滤，确保每个角色的心情面板只显示自己的状态
- **命令层路由**：大多数 Tauri 命令增加 `character_id: Option<String>` 参数，路由到对应角色的 Brain / Memory / Diary 等子系统

### 心理学因果链（五层架构）

`psychology/` 模块实现了一条完整的因果链：

```
Persona（长期人格）→ Needs（5 项需求 + set point）
        ↑                          ↓
   Homeostasis ← 事件 LLM 单次调用 → {appraisal, emotion_update, behavior_drive, reply}
                                    ↓
                               Appraisal（6 项评价）
                                    ↓
                               Emotion（7 项唯一情绪）
                                    ↓
                               Behavior Drive（8 项行为驱动）
                                    ↓
                               行为决策 + Mood + PetState（实时计算，仅 UI）
```

- **人格层**：模块化人设文件（`characters/{id}/` 下 identity / personality / speech / examples / background / interests / relationships / appearance 八个独立文件，采用场景化行为锚点 + "触发→反应"行为脚本，拒绝形容词堆砌）+ worldbook 动态激活状态机（参数可调）+ worldbook constant 常驻层（核心身份/关系里程碑无条件每轮注入，不参与激活度计算与 max_active 截断）+ 场景 embedding 匹配 + 5 信号融合的场景模式选择
- **需求层**：5 项需求各带 set point，由 Homeostasis 引擎维持平衡
- **评价层**：评价（Appraisal）驱动情绪与行为驱动，心理字段由独立调用推断
- **情绪层**：7 类唯一情绪枚举（EmotionLabel）
- **行为驱动层**：8 项驱动 + 规则解析器
- **关系系统**：阶段状态机 + 5 种关系事件 + 永久/临时策略 + 里程碑记录
- **昼夜节律**：Homeostasis 按本地时间调制 set points / recovery / noise（早晨好奇、下午情绪峰值、傍晚社交需求、深夜孤独易感），4 锚点线性插值平滑过渡，仅临时调制不污染持久化值

### 三层记忆系统

`memory/` 模块统一管理短期 / 中期 / 长期记忆：

- **巩固流水线**（`pipeline.rs`）：ShortTerm → MidTerm → LongTerm 三阶段，按热度、重要性、容量阈值触发。Stage 1 筛选 ShortTerm 时排除 `InnerMonologue`（角色主观内心独白）与 `ObservationNote`（旁观记忆），避免与对话事实混合摘要导致语义失真；Stage 2 抽取新事实后主动评估相似旧记忆的矛盾关系，命中则应用 `Negates` 证据信号削弱；Stage 2 触发条件放松——热度阈值降至 2.5 并增加 24h 兜底触发（SessionSummary 创建满 24h 且 `visit_count=0` 时强制触发），避免低频访问的摘要永远停留在 MidTerm；每次 `run()` 末尾检测向量索引漂移，数量比偏离 [0.8, 1.2] 时全量重建
- **夜间巩固**（`consolidation.rs`）：睡眠窗口内异步执行完整巩固流水线，模拟人类睡眠时的记忆整理
- **混合检索**（`retriever.rs`）：BM25（jieba 中文分词）+ 向量（Hashing 256 维离线 / OpenAI 兼容在线）+ RRF 融合 + IVF 倒排索引加速（向量数量 >500 时自动构建 k-means 聚类，查询时只扫描 nprobe 个最近聚类）+ 语义去重（`dedup_by_semantic`：Union-Find 聚类，每簇保留 evidence_score+importance 最高的一条，解决"语义相同表述不同"记忆挤占 token 问题）
- **五因子加权排序**：recency + relevance + importance + hook_boost + need_sim，各自独立权重可调
- **保留策略**（`retention.rs`）：可配置过期规则 + 保留守卫 + 证据驱动归档（`protected` 永不归档；`evidence_score <= -2.0` 且 `sub_zero_days >= 14` 触发归档倒计时；去重合并时优先保留证据评分更高的条目；反驳 grace period 3 tick 防单正信号恢复；记忆整合 soft-archive 替代 hard-delete，标记 `consolidated` 字段）+ 容量上限（knowledge 500 / insight 100 / inner_monologue 200，`evict_by_score` 按证据+重要性淘汰弱价值记忆）
- **LLM 增强**（`llm_enricher.rs`）：写入时即做 LLM 分类与元数据抽取（description / keywords / importance / semantic_type / summary），读取路径不调用 LLM。长文本（content > 200 字）时 LLM 顺带输出 ≤100 字摘要，用摘要做向量嵌入避免原文稀释；短文本直接用原文嵌入，不额外要求 summary 字段
- **自动提取**（`auto_extractor.rs`）：从对话中自动抽取值得长期保留的事实，跳过标记为 `memory_disabled` 的消息（工具输出 / 内心独白 / 镜像消息）；注入已有事实避免重复抽取；对话格式统一使用第一人称说话者标记 `[User says to me]` / `[I say to User]`
- **记忆上下文格式化**：每条记忆附带时间戳 / 类型标签（长期 / 短期 / 对话）/ 重要性 / 情绪标签，token 预算 1250，让 LLM 区分长期偏好与临时话题。低置信度记忆（`combined_score` 或 `temporal_adjusted_score` < 0.3）追加 `[需验证]` 标记，提示 LLM 谨慎参考
- **记忆验证**（`verifier.rs`）：检索后用小模型对候选记忆做二分类（能/不能回答问题），过滤无关噪声记忆。每条记忆附带元数据（时间 / 类型 / 重要性 / 描述），截断 400 字符，利用 `MemoryItem.description` 字段辅助 LLM 判断相关性；记忆数 ≤ 2 时自动跳过（开销不值得），LLM 不可用时降级为全部保留
- **用户事实画像**（`user_facts.rs`）：四层结构化存储——L0 稳定身份（姓名/年龄/性别/职业/所在地）/ L0.5 结构化偏好（生日/作息/常用网站/喜欢的游戏/兴趣爱好）/ L1 近期状态（最近目标/当前项目/近期偏好，随轮次衰减）/ L2 自由事实。`is_pinned` 锁定保护防止自动覆盖；注入已有事实避免重复抽取；说话者标记统一为 `[User says to me]` / `[I say to User]`；按角色隔离存储（`characters/<char_id>/user_facts.json`），不同角色对用户的认知可差异化
- **记忆类型**：除时长/内容类型外，还包含 `SessionSummary`（会话摘要）、`Insight`（反思洞察）、`InnerMonologue`（内心独白，角色自主思考的记录）、`ObservationNote`（旁观观察）、`CasualConversation`（闲聊）
- **种子记忆**（`seed_if_empty`）：首次启动或恢复出厂后，自动写入角色专属种子记忆（Vivian/Nana 各 9 条），覆盖身份锚点 / 性格 / 生活习惯 / 室友关系 / 社交边界 / 首次启动里程碑 / 当下心境 / 内心独白 / 环境观察 6 类。内容仅写关于自己的既定事实与当下主观感受，不编造未发生的事件；时间戳按 index 递减错开 60s/条模拟自然时序；short_term 类型携带 OpenHook 钩子（如"用户透露玩不玩游戏"）让角色带着动机开启对话。UI/图谱按 `source: "system_seed"` 过滤不展示
- **知识文档时效管理**：后台知识采集写入的 Knowledge 类型记忆携带 TTL 分级（short=7天 / mid=30天 / long=永不过期），检索时对 Knowledge 类型施加时间衰减（30 天半衰期）并 对已过 TTL 的知识降权 0.3 倍但不硬删，后台采集时自动刷新过期知识（删除旧文档→重新搜索→总结→入库），详见[后台知识采集与时效管理](#后台知识采集与时效管理)章节
- **冲突检测**（`conflict.rs`）：写入热路径上的三阶段流水线——语义相似度检测 → LLM 判定（冲突/补充/无关）→ 自动合并/覆盖/保留，避免矛盾记忆污染上下文。`QueueLlm` 决策通过 `pending_conflicts` 持久化队列由 `CognitiveTickRunner` 每 5 分钟批量消费（最多 5 条/次，指数退避重试 3 次），`DefaultConflictArbiter` 基于 reflection 路由调用 LLM 仲裁，输出 `ArbitrationOutcome` 决定保留/合并/覆盖
- **证据驱动记忆可信度**（`evidence.rs`）：每条记忆携带 `reinforcement` / `disputation` 双独立时钟半衰期衰减字段。7 种证据来源（user_fact / user_confirm / user_rebut / user_ignore / user_keyword_rebut / migration_seed / promote_merge）按不同 delta 权重更新评分。`evidence_score = reinforcement - disputation`，`protected` 记忆返回 +∞ 永不归档。分数跌破 `ARCHIVE_THRESHOLD (-2.0)` 时启动 `sub_zero_days` 归档倒计时，累积 14 天后真正归档
- **事件溯源**（`event_log.rs`）：append-only `events.ndjson` 日志，15 种事件类型覆盖记忆生命周期（fact.added / reflection.synthesized / persona.fact_added / reflection.evidence_updated 等）。写入契约：append-before-mutate（事件先落盘再修改视图），Sentinel 游标持久化，Reconciler 启动时尾部重放，handler 幂等。10K 行 / 90 天触发 compaction
- **会话记忆压缩**（`session_compressor.rs`）：桥接 `TimeStampedMemory` 的 LLM 摘要到主对话窗口。`DialogueManager` 维护固定 10 条消息窗口，超出部分被静默丢弃；会话压缩器从 `TimeStampedMemory` 提取摘要，构造 `[CONVERSATION RECAP]` 系统消息注入对话历史最前面，让 LLM 在近期消息之外也能感知此前对话概要
- **统一事件账本**（`unified_event_ledger.rs`）：全局共享的环境事件索引层，在保留各角色 MemoryManager 隔离存储的前提下，所有对话/动作/交互抽象为统一事件。事件包含 timestamp/sender/receiver/event_type/content_preview/context_tags/visibility/source_memory/associated_char_id；可见性分为 Public（跨角色对话/广播，所有角色可见）、Participants（用户↔智能体对话，仅参与方可见）、Private(observer_id)（旁观记忆，仅指定角色可见）。行为事件（long_idle/quiet_mode/mood_event/presence_log 等）通过 `register_world_event` 写入账本（sender=system/receiver=all/visibility=Public/associated_char_id=当前角色ID），不写入 MemoryManager；MemoryManager 只保留 AI 主观记忆。支持按可见性查询、实体-实体检索（A↔B 双向事件流）、LLM 摘要压缩（超限自动压缩旧事件），前端通过 `list_unified_events` 命令分页查询
- **RAG 幻觉抑制**（五层防御）：贯穿检索-生成-验证全链路，降低记忆驱动的幻觉风险
  - **Prompt 层忠实度约束**（`prompt_modules.rs::build_memory_block`）：在记忆块末尾追加忠实度指令，提示 LLM 记忆可能过时、与用户矛盾时以用户为准、不编造用户未提过的细节、注意 `[需验证]` 标记的低置信度记忆（中/英/日三语）
  - **检索结果置信度标记**（`steps/memory.rs`）：对 `combined_score` 或 `temporal_adjusted_score` 低于 0.3 的记忆条目追加 `[需验证]` 标记，让 LLM 在生成时对低置信度记忆保持谨慎
  - **主对话路径接入 Verifier**（`steps/memory.rs` + `chat_chain.rs`）：`MemoryRetrievalStep` 注入 `ModelRouter`，检索结果 >2 条时用 `memory` 任务小模型做二分类过滤无关记忆，减少幻觉噪声；LLM 不可用时降级为全部保留
  - **生成后幻觉检测**（`steps/validation.rs` + `chat_chain.rs`）：`ValidationRunnable` 注入 `ModelRouter`，当记忆上下文非空且回复 ≥30 字符时用小模型检查回复是否与记忆矛盾或编造信息，仅记录 warning 不修改回复；超时/失败时跳过不阻塞主流程
  - **按需检索 FLARE 式**（`steps/query_rewrite.rs` + `steps/memory.rs`）：`QueryRewriteStep` 内置 `should_skip_retrieval` 启发式判断，对闲聊填充词/问候语/确认词（如"嗯"/"你好"/"好的"/"晚安"等中英日三语词）及纯标点表情输入直接跳过查询重写和记忆检索，通过 `metadata.skip_memory_retrieval` 标志通知 `MemoryRetrievalStep` 跳过整个检索步骤，省去无谓的 LLM 调用和向量检索开销

### LangChain 风格 Runnable 流水线

`pipeline/` 模块提供可组合的对话处理管道：

```
PreProcessing → UserMemorySaving → [QueryRewrite ∥ FastSemantic] → MemoryRetrieval → PromptBuilding
    → WebContextDecision → Generation → ResponseParsing → Validation → ExpressionMotion → PsychologyInsight → MoodUpdate → MemorySaving
```

- 每步是独立 `Runnable`，可通过 `|` 操作符声明式组合
- **QueryRewrite 与 FastSemantic 并行执行**：通过 `ParallelStep` 容器（`tokio::join!`）同时运行 LLM 查询重写和嵌入语义分类，耗时 = max(两者) 而非 sum
- 支持 `RunnableBranch` / `RunnableRetry` / `RunnableWithFallbacks` 装饰器
- `Advisor` 拦截器链提供日志、限流、Re2、循环检测
- `PipelineState` 携带 55 个字段贯穿全链
- **死循环检测**（`doom_loop.rs`）：追踪每轮 `(tool_name, args)` 签名（BTreeMap 规范化），同一签名连续出现 ≥ 阈值次时判定死循环，生成注入消息打断
- **多级上下文压缩**（`context_compress.rs`）：三级策略——Soft Trim（tool_result 截断）→ Group Drop（`MessageGroup` 原子分组，保证 tool_call+result 不拆分）→ Reminder Inject
- **压缩后提醒**（`compaction_reminder.rs`）：从被丢弃的中段消息中提取活跃工具名和最后用户话题，注入系统提醒防止 LLM 丢失任务上下文
- **Prompt 模板引擎**（`template_engine.rs` + `prompt_modules.rs`）：`section_schema()` 定义 section 结构元数据，`build_prompt()` 采用 U 型注意力优化布局 + Consciousness Assembler 分层意识模型。静态区使用 `<static>` 标签包裹提升云端 API 缓存命中率，布局顺序为：Character（人格核心最先入脑）→ Style/Relationship → Examples（近因效应，静态区末尾）→ Framework（技术规则，不内化）→ FORMAT SPEC（临出口提醒）；动态区按分层意识排序：Current Mind（Belief/Goal/Attention + Working Memory + Self State + 情绪上下文）→ World Snapshot（环境上下文 / 用户在场与近期活动与观察 / 室友状态与认知印象 / 环境事件）→ 社交关系（关系认知事实 / 共享世界 / 社交状态）→ Relevant Episode + 关系日志 → Memory → Tail（初见或记忆规则 / 用户事实 / 行为画像 / Worldbook）→ 提示词尾部再依次注入渠道指南 → 在场指南 → 内心反应（仅无当前念头时注入，靠近生成点利用近因效应）→ 响应决策 → 内联标签格式 → 语气注入 → 工具列表 → 用户输入（Task 层，最末）。`build_prompt_with_sections()` 产出 prompt + 逐 section 元数据（char_count / token_estimate / present），前端 Context Pipeline 通过 `get_prompt_section_schema` + `get_last_prompt_breakdown` 命令消费 schema 驱动可视化。工具列表放在 prompt 最末尾，让 LLM 先进入意识状态再看可用工具。功能提示词（心理洞察/信念生成/思维合成/日记生成/记忆提取）全部动态化，使用角色名变量而非硬编码"Vivian"；跨角色语音指南使用行为化描述（"你说话比她快，句子更短"）替代数值化标签（"sass=0.65"）；内心反应使用中文生成并按角色差异化（Vivian 直率吐槽 / Nana 温柔关心）
- **生成与提示词拆分**：主对话 LLM 只输出 `text / intent / tool / arguments / control_actions`，表情/动作可用列表（manifest_context）不再注入主对话 prompt。表情/动作有五类触发路径：(1) **LLM 内联标签模式**（`config.inline_expression.enabled`）——主 LLM 在文本中嵌入 `<e name="happy" dur="3000"/>` / `<m name="wave"/>` / `<s name="sticker_id"/>` 标签，流式输出时由 `InlineTagScanner` 实时扫描剥离并 emit `chat:inline_meta` 事件驱动 Live2D，零额外 LLM 开销；(2) **LLM 子调用模式**（默认）——`ExpressionMotionRunnable` 在 text 完成后独立调用 LLM 选择表情/动作；(3) **嵌入即时反应**——`analyze_emotion_instant` 命令调用 `EmbeddingEmotionClassifier`（基于 `MemoryEmbeddingProvider`，预置 14 类情绪语料 210 条，Top-K=5 余弦相似度投票），在用户消息发送瞬间（Layer 1）与 AI 文本首段完成时（Layer 2）触发即时 FACS 反应，写入 Live2D `instant` 层（优先级 1.5），反思调用完成时由 `manual` 层接管并自动清除 `instant` 层；嵌入失败时弹 toast 报错，不降级到关键词分析；(4) **用户交互即时反馈**——`apply_user_interaction` 命令根据前端检测到的 10 种交互类型（click/drag/pet 等）直接查表返回反馈，不调 LLM；(5) **自动规则触发**——`auto_expression_tick` 定时检查空闲阶段/心情持续/程序事件（`engine/auto_trigger.rs`），纯规则概率触发。`control_actions` 中的 `set_expression` / `play_motion` 接受语义名（happy / shy / wave / nod 等），后端通过 `ResourceManifest` 归一化映射到实际 model3.json Name
- **回复验证**：`ValidationRunnable` 在 `ResponseParsing` 之后、`ExpressionMotion` 之前执行——空文本检测（should_respond=true 但 text 为空时记录 warning）、长度上限截断（超过 500 字符时在句边界截断）、基础空白清理；注入 `ModelRouter` 后还启用轻量幻觉检测（当记忆上下文非空且回复 ≥30 字符时，用 `memory` 任务小模型检查回复是否与记忆矛盾或编造信息，仅记录 warning 不修改回复，超时/失败时跳过）
- **流式期间表情节流**：三层时序隔离保证流式输出期间不触发中间表情抖动——`StreamEmitter` 只推 `TextChunk` 纯文本片段；前端 `isStreaming` 守卫在流式期间暂停 `mood_expression_tick`；pipeline 严格串行，`ExpressionMotionRunnable` 在生成与解析完成后一次性调用
- **表情归一化兜底语义**：`ResourceManifest::normalize_expression` 在别名 / 原名 / 回退候选链全部未命中时返回空串（遵循"无匹配时留空，不强制使用"原则）；仅当显式请求 `default` / `neutral` / 空字符串时才返回第一个可用表情。避免 LLM 输出的不匹配表情名被强制映射到无关表情
- **补充回复服务**（`augment_reply_service.rs`）：主对话回复后异步触发 slow 检索（Hybrid 策略，4s 超时），当后台检索召回 fast 路径遗漏的重要记忆时，自动生成 1-2 句自然衔接的补充回复（如"哦对了…"）。记忆按重要性升序排序后取前 5 条（重要的排在 LLM 注意力更佳的末尾位置），附带重要度元数据。通过冷却（120s）+ pending 队列上限（2）+ 相似度防复读（3-gram Jaccard > 0.55 丢弃）控制频率。`Brain::build` 中初始化注入 memory + router，`BrainChatChain::ainvoke` 回复生成后 fire-and-forget 调度，不阻塞主路径

### 流式安全过滤

三层过滤管线在 LLM 输出抵达用户前依次执行，防止内部 CoT / 工具调用标记 / 未渲染占位符泄露：

- **思考链过滤**（`providers/thinking_stripper.rs`）：针对 Qwen3.5/3.6/3.7 等会把 `<think>...</think>` 混入 content 的混合模型。非流式 `strip_thinking_segments` 清理完整标签；流式 `ThinkingStreamStripper` 是 BUFFERING / PASSTHROUGH 两态状态机，hold content 直到第一个 `</think>` 闭合标签出现再放行，支持成对 / 悬挂闭合 / 裸开标签三种形态
- **工具调用标记过滤**（`brain/tool_leak_filter.rs`）：过滤 `<tool_call>` / `<seed:tool_call>` / `<function>` 三种泄露形态。非流式 `strip_tool_call_markup` 清理完整块；流式 `ToolLeakFilter` 是跨 chunk 状态机，通过前缀检测识别被拆分到多个 chunk 的开闭合标签，并跟踪 ``` 代码块避免误伤合法 JSON
- **提示词占位符泄露检测**（`persona/prompt_render.rs`）：在 LLM 调用入口扫描 system 消息中未渲染的 `{placeholder}` 占位符（排除 `{{name}}` 转义形式）。测试模式 panic、生产模式 `tracing::warn!`，通过 `VIVIAN_PROMPT_LEAK_RAISE=1` 环境变量强制 panic

### 凝神/专注模式

`brain/focus_mode.rs` 实现漏桶累积器 + 迟滞设计的专注模式状态机，在心理学数值模型之上叠加一层离散认知模式切换：

- **三种认知模式** `CognitionMode`：Regular（日常轻量基线）/ Focus（信号触发，开启思考 + 提升余量）/ TrueName（v2 预留）
- **漏桶累积器**：`new_charge = max(0.0, min(charge * retention + score, cap))`
- **迟滞设计**：`charge ≥ enter` 时时间衰减地板 = `enter`（不会衰减到零立即退出）；`charge < enter` 时地板 = 0
- **信号评分** `compute_focus_score`：从用户输入长度（>150 字 +0.4 / >50 字 +0.2 / <8 字 -0.2）、问号（+0.2）、复杂度关键词（+0.2）、用户情绪（负面 +0.3 / 正面 -0.2）综合计算
- **阈值**：retention=0.5 / enter=0.6 / exit=0.3 / cap=1.0 / hard_cap_turns=8
- **退出原因**：Decayed（衰减到 exit 线）/ HardCap（连续 8 轮强制退出）/ TopicSwitch（话题切换）
- **副作用**：
  - `BrainChatChain::ainvoke` 每轮调用 `focus_state.update()`，驱动三态切换
  - 激活时向 messages 追加认知模式 system 指令（放慢节奏、更安静、更有深度）
  - 激活时通过 `ModelRouter::set_focus_boost` 给 provider 注入 `thinking_extra_tokens`（默认 800）的 max_tokens 额外余量，给混合推理模型留出思考空间
  - `proactive_tick` 期间调用 `idle_cooldown` 让 Focus 电荷按 idle retention 衰减

### 多 Provider 路由矩阵

`providers/` 模块支持 9 种 `ProviderKind`：

| Provider | 协议 | 覆盖服务 |
|----------|------|---------|
| `OpenAiCompat` | OpenAI Responses API 兼容（`/responses` 端点） | DeepSeek / Qwen / Moonshot / SiliconFlow / GLM / Grok 等已实现 Responses 协议的厂商 |
| `OpenAiResponses` | OpenAI 官方 Responses API | OpenAI GPT-4o / o1 / o3 系列（原生 MCP / Tool Calling / 多模态） |
| `DoubaoResponses` | 火山方舟豆包 Responses API（`/api/v3/responses`） | 豆包 250615+ 新模型（旧模型走 `OpenAiCompat`） |
| `ChatCompletions` | 标准 OpenAI Chat Completions（`/v1/chat/completions`） | OpenRouter / Groq / Mistral / Together / Ollama / vLLM / LM Studio |
| `Gemini` | Google 原生 REST | Gemini（含 Google Search grounding） |
| `Anthropic` | Claude `/v1/messages`（x-api-key + anthropic-version） | Claude 系列 |
| `Wenxin` | 百度 OAuth + access_token | 文心一言 |
| `Spark` | 讯飞 WebSocket + HMAC-SHA256 | 星火大模型 |
| `Custom` | 自定义（按 Chat Completions 处理） | 任意兼容接口 |

- 支持按任务类型独立配置模型（chat / reasoning / diary / memory / embedding / reflection / inner_monologue / consolidation / vision_describe / activity_extraction / knowledge_acquisition / interest_search / translation 等），每个任务拥有独立的 provider 实例（独立模型/API Key/端点）
- 任务 provider 失败后自动尝试主 LLM API，再尝试 providers 池，通过 `chat:route_fallback` 事件通知前端
- 路由矩阵总开关 `enable_routing_matrix`
- **按任务分组的 LLM 并发限制**：`ModelRouter` 内置 `Semaphore` 防止后处理 LLM 调用（记忆巩固 / 内心独白 / 日记等）同时挤占主对话资源。chat_reasoning 组（chat / reasoning / vision_describe）→ 3 并发，memory_reflection 组（memory / consolidation）→ 3 并发，auxiliary 组（emotion_analysis / inner_monologue / diary / activity_extraction / knowledge_acquisition / interest_search / translation）→ 2 并发
- **多模态（图片输入）**：六家 Provider 协议（OpenAiCompat / OpenAiResponses / DoubaoResponses / ChatCompletions / Anthropic / Gemini）均支持图片输入，文心与星火不支持。统一通过 `ChatMessage::user_with_images` 构造，`MessageImage` 含 `media_type` / `data`(base64) / `url` / `detail` 四字段，由 `ai.enable_vision` 开关 + `ai.image_detail`（auto/low/high）配置控制
- **视觉能力自适应探测**：应用不假设用户填入的 API 支持视觉，首次发图前用 16×16 透明 PNG 探测目标模型是否接受图片输入（部分服务商如豆包要求最小 14×14），结果按 model 名缓存。探测路径与 `vision_describe` 任务实际路由一致（路由矩阵启用时优先任务 provider，否则主 LLM API），绕过 `query_with_fallback` 避免 fallback 掩盖真实结果。`NotSupported` 时拦截发图并 emit 详细 error toast（含原因 + 配置指引）；`save_config` / `reload_config` 时自动清空缓存，确保用户换模型后重新探测
- 代理透传、客户端缓存热重载

### 增强工具系统

`tools/` 模块提供 70+ 内置工具 + 1 个元工具（ToolSearchTool），覆盖 13 个类别：

| 类别 | 工具示例 |
|------|---------|
| 文件操作 | ReadFile / WriteFile / EditFile / ListDirectory / SearchFiles / Grep |
| 系统操作 | GetRunningProcesses / OpenApplication / CloseApplication / TakeScreenshot |
| 扩展系统 | GetClipboardText / SetClipboard / OpenUrl / GetActiveWindow / GetSystemInfo |
| 记忆 | SaveMemory / SearchMemory / ClearMemory / ReadMemory / LogDailyDiary / ListRecentDiaries |
| 桌宠 | SetExpression / PlayMotion / TriggerIdleAction / SetBehaviorMode |
| 待办 | AddTodo / ListTodo / CompleteTodo / UpdateTodo / DeleteTodo |
| 桌宠行为 | SetPetState / PlayAnimation / SpeakBubble / FollowCursor / SetMood |
| 关系 | GetRelationshipStatus / ListMilestones / RecordMilestone |
| 媒体 | media_play_pause / media_next / media_previous / media_volume_up / media_mute |
| 感知 | GetCursorPosition / GetIdleState / GetForegroundAppContext / OcrScreenText / GetWindowTree |
| 输入控制 | MoveMouse / ClickMouse / DragMouse / ScrollMouse / PressKey / Hotkey / TypeText |
| 壁纸 | WallpaperList / WallpaperSet / WallpaperPause / WallpaperStop（Wallpaper Engine 集成） |
| MCP | mcp__{server_id}__{tool_name}（外部 MCP server 动态注册） |

工具行为要点：`GetWindowInfoTool.get_window_info` 返回真实窗口信息（`{x, y, width, height, visible, always_on_top}`），而非模拟数据；`SetPetState` 的 `state` 参数取值限于 `["idle","active","sleeping","thinking","listening"]` 枚举，非法值返回包含允许值列表的错误提示；`SetMood` 的 `mood` 参数取值限于 `["happy","calm","sad","excited","angry","neutral"]` 枚举；`PlayAnimation` 的 `animation` 为自由格式（模型级动作名），仅做非空校验。截屏路径校验：`capture_screen_region` 的 `save_path` 通过 `is_path_safe` 检查路径穿越，`take_screenshot` 限定保存到 `screenshots` 白名单目录。

- **执行管线**：查找 → 沙箱安全检查 → 输入验证 → 缓存检查 → 权限检查 → 执行（带超时）→ 缓存写入。所有 PowerShell / 子进程调用经 `tokio::task::spawn_blocking` 隔离到阻塞线程池，避免同步等待占满 async 运行时 worker；PowerShell 脚本统一注入 `[Console]::OutputEncoding = UTF8` 前缀，杜绝 GBK 控制台输出乱码
- **工具可见性分层**（`ToolVisibility`）：三级控制工具在 LLM 上下文中的展示粒度，减少 token 开销。`Always`（完整 schema 注入，核心高频工具）、`Lazy`（仅名称 + 一行描述，完整 schema 通过 `tool_search` 按需加载，Media/Mcp 类默认此层级）、`Deferred`（仅名称出现在 `<available-deferred-tools>` 块中，should_defer=true 的工具默认此层级）。`resolve_visibility()` 根据 `ToolCategory` + `should_defer()` + `always_load()` 自动推断层级，个别工具可通过 `Tool::visibility_tier()` 覆盖
- **场景化工具筛选**：根据情绪/关系阶段自动切换工具暴露子集（低信任禁用系统控制、情绪低落禁用 Web/Media、专注模式保留 Memory + 必要 System）
- **多步编排**：`ToolChainer` 支持顺序 / 并行 / 多步循环 + 重复检测 + 失败策略 + `${result}` 参数注入
- **可观测性**：`ToolObservability` + `ToolMetrics` + `ToolCallRecord`
- **沙箱**：`ToolRiskLevel` / `ToolSafetyProfile` / `ProtectionMode` 三层安全模型。路径参数递归遍历 JSON 参数树提取所有可疑路径值，不再依赖固定参数名；危险命令检测覆盖 `rm -rf` / `rm -fr` / `rm -r -f` / `--recursive --force` / `format c:` / `del /f /s` 等多种变体组合；`normalize_path`（`types.rs`）真正解析父目录分量（栈 `pop()` 抵消 `..`，`/a/b/../c` 归一为 `/a/c`），与 `sandbox.rs` 的 `normalize_path_buf` 对齐，避免简单过滤 `..` 导致权限评估错路径；无内置安全档案的工具经通用检查（危险命令 / 路径穿越）后放行，风险分级交由下游权限系统（access_level × risk 矩阵 + always 规则 + 用户确认）统一管理
- **风险等级申报**：每个工具通过 `risk()` 声明 `ToolRiskTier`，默认 `Safe`。输入控制类 12 个工具（MoveMouse / ClickMouse / DragMouse / ScrollMouse / PressKey / Hotkey / TypeText 等）与媒体控制 6 个工具、`take_screenshot` 申报 `InputControl`；`weather` 申报 `Network`。权限矩阵 Deny 时返回提示并引导用户在设置中提升访问级别（如 InputControl 需 FullControl）；always 规则优先级统一为 `always_deny > always_ask > always_allow`，`always_allow` 仍优先于矩阵 Ask 判定
- **文件操作安全策略**：6 个文件工具（read_file / write_file / edit_file / list_directory / search_files / grep）调用 `tools::sandbox::is_path_safe` 进行路径穿越校验（拒绝 `../` / `..\` / 绝对路径越界）；`is_sensitive_path` 拒绝写入系统敏感目录（Windows / Program Files / System32 等）；写入操作前再校验目标路径合法性。文件操作强制递归深度限制（最大 10 层）、结果条数上限（grep 500 / list_directory 5000 / search_files 1000）、`read_file` 使用 `BufReader` 按行读取并支持 offset/limit 跳过，grep 正则使用 `Lazy<Regex>` 预编译复用，所有阻塞 IO 操作均通过 `tokio::task::spawn_blocking` 隔离到线程池避免阻塞 async 运行时。**编码自适应读取**：`read_file` 与 `grep` 先采样前 8KB，经 `chardetng` 检测编码（UTF-8 走快速路径），再用 `encoding_rs` 逐行解码（`read_until(b'\n')` 按字节分行，0x0A 不会作为 GBK/UTF-8 尾字节出现，行切分安全），GBK 等非 UTF-8 文件不再返回乱码或中断；grep 遇到非 UTF-8 行时按检测编码解码后继续匹配，而非在首个非法行处终止
- **Shell 执行禁用**：`brain::computer_control::execute_shell` 直接返回错误，防止 LLM 通过 shell 命令实现 RCE；`computer_control::open_app` 使用白名单映射表（app_map），未注册的应用名拒绝启动。`open_application` 工具内置 16 种危险程序黑名单（cmd.exe / powershell.exe / wscript.exe / rundll32.exe / regedit.exe 等），路径形式输入做文件名校验，纯应用名通过 where.exe/PATH/Program Files/Start Menu/UWP 五级解析链查找，整个解析过程通过 `spawn_blocking` 异步执行；UWP 解析路径对 AppID 实施 `is_safe_appid` 白名单校验（仅允许字母/数字/`.`/`_`/`-`/`!`），防止 PowerShell 注入；打开网址请使用 `open_url`（仅允许 http/https 协议，拒绝 file:///javascript:/data: 等危险协议）；剪贴板操作使用 `clip.exe` 通过 stdin 管道写入，不拼接 PowerShell 命令避免命令注入
- **GPT-SoVITS 服务安全**：服务状态通过 `Arc<RwLock<ServiceState>>` 缓存实时更新，HTTP Client 复用连接池；端口占用杀进程时精确解析 netstat 输出匹配目标端口，避免误杀无辜进程
- **用户确认**：权限矩阵判定 Ask 的操作通过 `tool:confirmation_request` 事件发起三态确认（拒绝 / 放行一次 / 始终允许），前端在 toast 子窗口渲染三按钮确认卡片，30 秒倒计时无操作自动拒绝，pending 请求带 5 分钟 TTL 自动清理避免内存泄漏。「始终允许」分两种范围：`open_application` 写入应用信任列表（`%APPDATA%\vivian\trusted_apps.json`，持久生效），其余工具写入会话级放行列表（应用重启后重置）；命中信任列表或会话放行的工具直接执行、不弹确认。高危工具默认始终需要确认：`CONFIRMATION_REQUIRED_TOOLS` 列表（13 个工具——read_file / write_file / edit_file / list_directory / search_files / grep / capture_screen_region / ocr_screen_text / get_window_tree / take_screenshot / delete_memory / cancel_scheduled / delete_todo）跳过矩阵直接走 Ask 确认流，即使权限系统判定 Allow；`delete_memory` 另有 `confirm` 布尔参数，为 true 时走确认流程、为 false 时按矩阵正常判定
- **原生 function calling**：当服务商支持时走结构化 tools 字段路径，不占 prompt token、调用更准确
- **MCP 原生集成**（`mcp.rs`）：手写 JSON-RPC 2.0 over stdio 客户端（无外部 SDK），启动时自动连接已配置的 MCP server，发现工具后注册到 ToolSystem 与内置工具无差别调度；外部工具默认延迟加载 + 权限 `ask`（不可信）；配置持久化于 `%APPDATA%\Vivian\mcp\servers.json`，设置窗口「工具」页签提供可视化管理；初始化失败时通过 `new_disabled()` 降级为空实现保证主流程不阻塞；配置写入使用 `Mutex<()>` 锁防止并发保存竞态；MCP 子进程 stderr 通过异步任务捕获并以 debug 级别记录日志，便于排查外部工具问题
- **anti-use-case 写法**：每个 Tool trait 实现 `anti_use_cases()` 方法描述"不适用场景"，与 `description` 一起注入 prompt 帮助 LLM 避免误用工具
- **Hook 系统**（`hooks/`）：PreToolUse / PostToolUse 可扩展拦截点。JSON 配置文件（全局 `%APPDATA%\Vivian\hooks.json` + 项目级）定义匹配规则（Regex）和外部脚本命令，stdin/stdout JSON 协议，fail-open（超时/异常/无效 JSON 默认 allow），错误以 `tracing::warn!` 记录而非静默吞错

### 主动对话编排

`proactive/` 模块实现自适应间隔 tick 调度的主动行为（支持单 tick 多消息 `MAX_TICK_MESSAGES=2`）。Tick 间隔根据用户空闲时间动态调整（`compute_adaptive_tick_ms(idle_seconds, char_id)`）：活跃时 10 秒、5-15 分钟空闲 30 秒、15-60 分钟 120 秒、超过 60 分钟 300 秒，减少空转 IPC。用户任何交互立即重置到活跃档，后端通过 `recommended_next_interval_ms` 字段向前端推荐下次 tick 间隔：

- **13 种触发器**：HourlyGreeting / IdleGreeting / TeasingResponse / Icebreaker / WindowTrigger / TopicExtension / MemoryRecall / HealthReminder / Spontaneous / WelcomeBack / MoodDriven / CrossCharacterReply / BystanderInterjection
- **意图判断规则预检**（`intent_judge.rs`）：简单输入（"嗯"/"好"/"拜拜"等中/日/英三语）由规则直接判定，跳过 LLM 调用；规则未覆盖的语义判断由 LLM 完成，降低不必要的后处理 LLM 开销
- **多级冷却**：每个触发器独立阈值 + 全局最小间隔
- **到达问候共享冷却**：启动问候与唤醒问候由 Brain 生成（不走 tick 触发循环），成功后经 `record_greeting_arrival` 计入主动问候共享冷却（全局打扰时间戳 + 问候键），问候类触发器（WelcomeBack / HourlyGreeting / IdleGreeting / Icebreaker）在 `min_trigger_interval`（默认 180s）静默期内被硬门控拦截，避免刚问候完又触发主动问候
- **启动问候活人感增强**（`generate_startup_greeting`）：首次见面判定基于 `non_seed_count() == 0`（排除种子记忆）。生成时注入三类上下文——当前情绪状态（跨会话保留，上次对话结束的情绪带到这次开场）、天气与时间（WorldSnapshot）、角色专属心境提示（Vivian 好奇但警惕 / Nana 平静不急），让 LLM 带着具体情绪状态生成开场白而非机械模板
- **9 种心理状态**（`PetMindState`）：Curious / Bored / Excited / Sleepy / Caring / Playful / Tired / Content / **Sleep**（深夜真正入睡，区别于 Sleepy 困倦）
- **安静模式**：连续被忽略次数达阈值自动进入 1 小时静默（阈值按角色差异化：Vivian 5 次 / Nana 2 次）
- **作息学习**：`HabitTracker` + `classify_app` 学习用户作息，90 天滚动窗口自动清理过期数据
- **破冰策略**：`IcebreakerGenerator` 多级破冰
- **话题池**：`DailyTopicPool` + `TopicTree` 维护话题新鲜度
- **生活服务**：`HealthReminder` / `Recommender` / `StressMonitor`
- **偏好学习**（`preference_learner.rs`）：per-trigger EWMA 算法学习用户对不同触发器的响应概率，被忽略的触发器概率倍率降低，被响应的触发器概率倍率升高，自动适应用户偏好
- **思绪生命周期**（`thought_lifecycle.rs` + `thought_trigger.rs`）：事件驱动的内心独白与主动表达架构，让"想说点什么"从概率 roll 转为"事件→种子→滋长→阈值表达"的自然积累过程。14 类思绪种子（going_to_rest / waking_up / user_left / user_return / long_silence / weather_shift / environmental_event / festival / activity_pattern / emotion_accumulation / cross_character_spoke / want_to_share_with_roommate / deep_reflection / background）经 5 阶段流转（Seed→Growing→Active→Expressed→Faded），intensity≥0.30 产生内心独白，≥0.70 可主动表达。`want_to_share_with_roommate` 种子检测分享诱因（用户行为类别切换起始强度 0.55 / 显著世界事件 0.60 / 情绪累积 0.50），单次诱因即可接近表达阈值，驱动角色主动找室友聊
- **多角色去同步（六策略）**（`character_behavior.rs`）：防止多角色同时发声的六层互补机制，所有参数按角色人设差异化配置：
  - **A. Tick 相位抖动**（`TickJitterConfig`）：`compute_adaptive_tick_ms` 对基础间隔施加角色专属随机乘数（Vivian 0.8~1.2 / Nana 0.9~1.4），使两角色的 tick 节拍在物理层自然错开
  - **B. 人设驱动权重分化**（`TimingWeights` + `TriggerModifiers`）：`TimingJudger::score_with_weights` 接受角色专属权重向量（Vivian 偏重 idle 信号 / Nana 偏重 time 信号），`TriggerModifiers` 对阈值/冷却/概率施加角色倍率（Vivian 阈值 ×1.2 冷却 ×1.5 概率 ×0.8 更矜持 / Nana 阈值 ×0.8 冷却 ×0.7 概率 ×1.3 更积极）
  - **C. 发言欲望累积器**（`SpeechDesireConfig`）：每 tick 按 `base_growth` 累积欲望值（被忽略时额外 `ignored_boost`，用户忙碌时 `user_busy_decay` 衰减），问候类触发器须欲望 ≥ `threshold` 才放行（Vivian 增长 0.08 阈值 0.6 需更多积累 / Nana 增长 0.04 阈值 0.4 更快开口），发言成功时重置为 0
  - **D. 跨角色仲裁**（`ArbitrationConfig`）：`SPEECH_RESERVATION` 全局时间戳 + 5 秒碰撞窗口内按 `priority` 仲裁（Vivian priority=1 优先 / Nana priority=2 让步），跨角色冷却 = 基础 15s × `reluctance`（Vivian ×2=30s / Nana ×4=60s），让步方延迟 `yield_delay_secs` 再尝试
  - **E. 情绪漂移周期**（`MoodDriftConfig`）：`mood_drift_phase` 每 tick 按 `recovery_rate` 推进（Vivian 0.02 锯齿快周期 / Nana 0.05 缓坡慢周期），`compute_overall_cooling` 的情绪乘数叠加 `sin(phase) × volatility` 周期因子（Vivian 振幅 0.3 波动大 / Nana 振幅 0.1 平稳），使两角色的情绪冷却曲线在不同相位交叉
  - **F. 触发器领地分配**（`TriggerAffinity`）：per-trigger 概率乘数划定各角色的"优势领地"（Vivian 擅长 mood_driven ×1.3 / icebreaker ×1.2 / welcome_back ×1.3，弱化 hourly ×0.4；Nana 擅长 hourly ×1.3 / idle ×1.2 / health_reminder ×1.4，弱化 mood_driven ×0.5），减少同一触发器上两角色同时竞争
- **跨角色聊天真实化（四层架构）**：从"被动响应+概率 roll"升级为"事件驱动+思绪桥接+关系差异化+共同情境"：
  - **共同情境注入**：`cross_character.rs::send` 的 `build_handoff_context` 调用处追加 `activity_journal.to_brief()` 作为 `[共同观察]` 段落，让跨角色对话有共同话题
  - **关系状态差异化**：`compute_cross_reply_probability` 引入 A↔B intimacy 调节（关系近 +0.10 / 远 -0.10）和近期互动频率（1h 内 -0.10 防刷屏）
  - **事件驱动触发**：`want_to_share_with_roommate` 种子检测三类分享诱因（用户行为类别切换/显著世界事件/情绪累积），30 分钟冷却
  - **内心独白桥接 talk_to_character**：`maybe_spawn_inner_monologue` 按 trigger_kind 分流，`want_to_share_with_roommate` 走 `generate_thought_share_to_roommate` 生成"对室友说"内容，不要求 leader 身份（非 leader 也可主动找室友聊）
- **三人共处一室互动**：用户与某角色聊天时，其他在线角色可旁听并自然插话，模拟三人在同一房间的氛围：
  - **CrossCharacterReply 时间衰减**：室友对用户说话时本角色可低概率接话，由 `compute_cross_reply_probability` 按用户最后交互时间衰减（< 2min ×0.0 不打断 / 2-5min ×0.4 低概率 / 5-15min 正常 / >15min +0.15 用户实际离开），替代原 5min 硬屏蔽
  - **BystanderInterjection 旁观插话**：旁观者基于 curiosity / loneliness / closeness 情绪驱动插话概率，用户活跃聊天时 +0.10（旁听素材丰富），被室友 cue 时 +0.35（30s 内有效信号）；冷启动破冰——室友在线但从未发言时以 20% 概率触发，解决两角色互相等待死锁
  - **roommate_cue 机制**：`commands/chat.rs` 写入旁观记忆后以 8% 概率调用其他在线角色的 `seed_roommate_cue`，设置 30s TTL 信号（from_name + topic_brief），被 cue 角色的 BystanderInterjection 概率提升并在 prompt 中注入"室友刚 cue 了你"提示，让插话更自然
- **角色个性化行为参数**（`character_behavior.rs`）：按 `char_id` 索引的本地非 LLM 控制参数，让不同角色表现出不同节奏感。`ProactiveOrchestrator::new(char_id)` 持久化路径按角色隔离到 `characters/<char_id>/proactive/`，`apply_proactive_feedback(positive, char_id)` 增减幅度、MoodDriven 触发阈值、亲密度冷却系数、安静模式阈值均读取 `CharacterBehavior`：
  - **Vivian（傲娇慢热）**：正向反馈 +0.002 / 负向反馈 -0.003（冷落更伤感情）、MoodDriven 需求阈值 0.85 / 孤独阈值 0.75（不易被情绪驱动主动发话）、亲密度冷却系数 ×0.8（冷却更快）、安静模式 5 次、心情表情冷却 30s
  - **Nana（温柔热情）**：正向反馈 +0.005 / 负向反馈 -0.001（宽容不易记仇）、MoodDriven 需求阈值 0.65 / 孤独阈值 0.55（容易主动关心）、亲密度冷却系数 ×1.2（冷却更慢）、安静模式 2 次、心情表情冷却 15s

### 真实世界感知（环境智能）

`world/` 模块让 Vivian 在真实世界中"活着"——即使用户不交互也能感知世界：

- **时间感知**：本地时间 / 周几 / 周末 / 季节 / 24 节气 / 公历与农历节日 / 日出日落（NOAA 简化算法）
- **天气感知**：Open-Meteo 免费接口（无需 API Key），失败当作"不知道"（不做时间推断兜底），带 WMO 代码到中文描述映射与可配置 TTL 缓存
- **系统音量感知**：通过 Windows Core Audio API（`IAudioEndpointVolume`）获取主输出设备音量（0-100），使用 `spawn_blocking` 隔离 COM 调用避免与 Tauri/WebView2 的 STA 线程冲突（`RPC_E_CHANGED_MODE`）
- **媒体播放检测**：通过 Windows SMTC（System Media Transport Controls）事件回调实时捕获正在播放的媒体信息（标题 / 艺术家 / 专辑 / 播放状态），事件驱动而非轮询，通过 `PlaybackInfoChanged` 回调即时响应
- **前台窗口检测**：Win32 FFI 获取当前聚焦窗口的标题、进程名和 PID。自动跳过应用自身窗口（主窗口 + 所有子窗口，通过 PID 比较），当应用获得焦点时保留上一次的外部窗口快照，避免显示"无活跃窗口"
- **网络连接监控**：COM `INetworkListManagerEvents::ConnectivityChanged` 事件回调，在本机网络适配器连通性变化时即时更新网络状态（已连接 / 已断开 / 未知）
- **IP 地理位置**：通过 ipwho.is API（无需 Key）获取 IP 级地理位置（城市 / 省份 / 国家），启动时自动检测 + 30 分钟定期轮询补充（覆盖 VPN 切换 / 路由器公网 IP 变化等 NetworkWatch 事件无法捕获的场景），用户可在前端点击位置卡片手动触发刷新（5 秒防抖）
- **位置注入提示词**：城市 / 省份 / 国家信息通过 `EnvironmentContext` 注入对话 prompt（三语覆盖：中 / 英 / 日），让 Vivian 在对话中"知道"用户所在位置
- **世界事件检测**：比较前后 `WorldSnapshot` 产出事件（天气变化 / 开始下雨 / 节日到来 / 节气切换 / 日出 / 日落 / 季节变化 / 长时间缺席），通过 Appraisal 机制隐式影响情绪/需求
- **世界快照注入**：将时间 / 节气 / 节日 / 天气 / 日出日落 / 地理位置注入对话 prompt，让 Vivian 在对话中"知道"真实世界状态
- **可配置作息**：`sleep_start_hour` / `sleep_end_hour`（支持跨午夜，如 23 点入睡、6 点醒来）让桌宠的睡眠时间可调整而非写死
- **用户实体状态机**（`world/entity_state.rs`）：跟踪在场/离开/预期回归/持续活动四态状态机。`ExpectationEngine` 从对话抽取预期回归时间（"20 分钟后回来"→20min 范围）+ 活动意图（"我去上班了"→直接写入 `current_activity`，无需等 LLM 反思 tick），活动意图抽取采用双信号门控（意图信号词 + 活动关键词）避免"上班真累"误判。`mark_present` 时产出 `ReturnEvent` 携带实际离开时长，按预期范围分类（MuchEarlier/Earlier/OnTime/Later/MuchLater），超时观察去重
- **用户行为日志**（`world/user_behavior.rs`）：已封存的持续状态事件（带 duration，不被 LLM 压缩），按活动标签查询供认知引擎整理为习惯 Belief（如"用户通常睡 7 小时"），FIFO 上限 300 条

### 体验连续性（Experience Continuity）

让 Vivian 拥有"和用户共同经历了一段时间"的持续存在感，而非每次对话都像刚启动。`mind/` 模块在 World / Memory / Reflection 之间增加状态合成层：

- **用户长期目标账本**（`mind/user_goals.rs`）：周~月级带 deadline 的用户人生阶段目标（"准备考研" / "写毕业论文"），由 reflection 阶段 LLM 抽取用户明说信号产出（`Dialogue` 来源强制要求 `source_quote` 原话引用，防止幻觉造目标），支持状态机（Active/Paused/Completed/Abandoned）+ 容量上限 5 + 同名去重 + 持久化到 `characters/<char_id>/mind/user_goals.json`。`Mind` 结构体持有 `user_goals: Arc<UserGoalLedger>` 字段，与 BeliefStore / GoalStore 同层
- **时间关系合成器**（`mind/temporal_context.rs`）：纯函数模块，从离散世界事实（`WorldBrief`）+ 长期目标摘要合成关系型时间事实，产出 6 类事实：Duration（用户已连续编码 3.5 小时）/ TimeOfDay（现在是凌晨 2 点）/ MealTime（接近晚饭时间）/ Deadline（「考研」还有 3 天到期）/ AwayAnomaly（用户已离开 3 小时长时间未归）/ Compound（深夜连续工作 2 小时疲劳风险上升）。零 LLM 调用、零新存储，注入 `thought_synthesis` 的 `## 时间关系（事实之间的关联）` 段落，让 LLM 不必从离散事实现算关系
- **事件重要性时间衰减**（`memory/unified_event_ledger.rs`）：事件账本排序从静态权重改为 `importance(t) = base × decay(age)`，分段衰减（<24h=0.95 / <3d=0.70 / <7d=0.40 / <30d=0.15 / ≥30d=0.05），「昨天买咖啡」类事件自然淡出，重要远期事件仍可被召回
- **WorldBrief 扩展**：注入 prompt 的世界事实基线新增 `user_activity_elapsed_secs`（当前活动已持续秒数）和 `active_goals`（最多 3 条活跃长期目标摘要，带剩余天数），让 LLM 在每轮对话都感知"用户当前处于什么人生阶段 + 当前活动持续多久"

### 自主活动（内心独白与活动日志）

让 Vivian 在用户离线时自主思考并记录用户活动：

- **内心独白**（`proactive/inner_monologue.rs`）：冷却到期（默认 30 分钟）时调用 LLM "inner_monologue" 任务生成 50-120 字第一人称独白，写入记忆（类型 `InnerMonologue`，标签含 `inner_os` / `inner_monologue` / `autonomous`），不打扰用户。生成时以世界快照 + 心理状态 + 近期对话记忆 + 用户活动日志为信息源，生成后清空活动日志重新记录
- **用户活动日志**（`proactive/activity_journal.rs`）：后台原生 Rust 线程（Win32 API，非 PowerShell）每 5 秒轮询前台聚焦窗口标题，仅在变化时记录一条带时间戳的日志（FIFO 上限 100 条）。内心独白生成时 `drain()` 消费并清空，作为 Vivian "观察用户"的信息源。线程仅在总开关开启时运行，平时 sleep 不占 CPU

### 后台知识采集与时效管理

角色在 Busy 状态下自主搜索网络、总结结构化知识并写入 RAG 向量知识库，供后续对话检索使用。对话中调用 `web_search` 工具搜索的关键词不直接入库，而是作为主题提示（topic hint）留给后台知识采集任务优先处理。采集与分享均带冷却机制，避免每次 Busy 都触发检索或推送链接。

- **采集冷却**（`proactive/mod.rs`）：`is_knowledge_acquisition_in_cooldown()` 在距上次采集不足 30 分钟时跳过整个采集任务，避免每次进入 Busy 都触发检索
- **主题提示机制**（`memory/manager.rs`）：对话中 `web_search` 工具搜索成功后调用 `push_topic_hint(query)` 记录关键词（去重、限 20 条、24h 过期）。后台知识采集任务启动时通过 `drain_topic_hints()` 取出提示主题，优先级高于 LLM 自主决策的主题
- **知识采集流程**（`presence/background_tasks.rs`）：主题来源优先级为「过期知识刷新 > 对话搜索提示 > LLM 自主决策」，三者合并去重后截断至 `MAX_TOPICS_PER_ACQUISITION`。每个主题经 WebSearcher 搜索 → LLM 总结为结构化知识文档 → `add_knowledge_document` 入库（含向量索引）
- **LLM 自主决策主题的锚点**（`decide_topics_with_intent`）：不再用固定 query 检索记忆，改用「最近 3 条 SessionSummary 话题总结 + 最近 5 条短期记忆」作为 LLM 的上下文。SessionSummary 是 Stage 1 提炼过的话题级压缩，比单条对话消息更稳定地代表用户兴趣。LLM 可返回 `[none]` 表示本次无明确兴趣锚点，跳过采集——像人一样没事做时不必硬找事做
- **分享意图克制**（`decide_topics_with_intent`）：主题分两类意图——`[internalize]`（内化为知识，常态）与 `[share:理由]`（分享链接给用户，少数情况）。`[share]` 必须带冒号+理由前缀，无理由自动降级为 internalize；一次最多 1 个 share，多余的降级为 internalize，避免给用户连续推送链接
- **分享冷却**（`proactive/mod.rs`）：`is_knowledge_share_in_cooldown()` 在距上次链接分享不足 30 分钟时跳过本次分享，避免频繁推送链接给用户
- **知识时效分级**（TTL）：LLM 在总结知识时判断时效类别并输出标签——`[short]`（短期热点，7 天过期，如新闻/热搜/赛事）、`[mid]`（中期趋势，30 天过期，如技术动态/产品发布）、`[long]`（长期知识，永不过期，如百科/历史/科学原理）。TTL 写入 `metadata.expires_at` 字段
- **检索时间衰减**（`memory/strategy.rs`）：检索结果中 Knowledge 类型记忆的 `combined_score` 乘以时间衰减因子 `recency_factor = exp(-age_days / 30)`（30 天半衰期），已过 `expires_at` 的知识额外乘以 0.3 惩罚系数（降权但不硬删）。所有三条检索路径（AutoStrategy 档位 1 / VectorStrategy / HybridStrategy）均施加时间衰减并重新排序
- **过期知识刷新**：后台知识采集任务启动时先扫描已过 TTL 的知识文档，删除旧文档（含向量索引）并提取标题作为刷新主题，重新搜索+总结+入库，实现知识内容的自动更新替代

### 记忆巩固（睡眠模拟）

`memory/consolidation.rs` 在配置的睡眠窗口内（跟随 `sleep_start_hour` / `sleep_end_hour`，而非写死 2-5 点）+ 6 小时冷却到期时，异步执行完整巩固流水线（Stage 1/2/3：ShortTerm → MidTerm → LongTerm → Insight），模拟人类睡眠时的记忆整理

### Live2D 表现层

#### 多维度表情/动作触发系统
不再依赖 LLM 单一触发路径，新增 4 大类纯规则触发机制（零 LLM 开销、即时响应），所有触发通过概率门控 + 冷却时间避免机械重复：

- **用户直接交互触发**（10 种精细交互类型）：前端 `Live2DCanvas` 实时检测 pointer 事件模式并分类，后端 `apply_user_interaction` 命令立即返回表情/动作反馈。双击检测在第二次 `pointerdown` 时即触发（无需等 `pointerup`），响应更快
  | 交互类型 | 触发条件 | 典型反馈 |
  |---------|---------|---------|
  | `single_click` | 单次点击（非快速连击） | 眨一只眼 / 俏皮 |
  | `double_click` | 350ms 内双击（第二次按下即触发） | 开心弹跳 |
  | `fast_click` | 800ms 内连击 3 次以上 | 慌张/抗议 |
  | `drag_start` | 开始拖动（`pointerdown` 时即触发，8px 阈值用于区分拖拽与单击） | 好奇 / 被拎起来 |
  | `drag_end` | 拖动结束 | 落地微笑 |
  | `fast_drag` | 快速拖动（速度 >1.8px/ms） | 惊慌 |
  | `pet` | 面部区域（上 45%）缓慢悬移抚摸（距离 >60px、速度 <0.5px/ms） | 害羞 |
  | `long_press` | 长按 1.5 秒不动 | 嘟嘴/鼓脸 |
  | `mouse_enter` | 鼠标进入窗口 | 注意/打招呼 |
  | `mouse_leave` | 鼠标离开窗口 | 失落/目送 |

- **空闲检测渐进触发**（5 阶段）：用户不交互时，`auto_expression_tick`（4 秒间隔）按空闲时间分阶段渐进触发，概率随时间递增
  | 阶段 | 空闲时间 | 典型表情/动作 | 触发概率 |
  |-----|---------|-------------|---------|
  | Active | 0-30s | 正常 | — |
  | Short | 30s-2min | 困惑 + 环顾四周 | 40% |
  | Medium | 2-5min | 困倦 + 伸懒腰 | 60% |
  | Long | 5-15min | 打哈欠 + 深呼吸 | 80% |
  | Asleep | >15min | 睡眠 + 身体轻晃 | 95% |
  | **user_return** | 从 5 分钟+空闲回来 | 惊喜星星眼 + 挥手 | 90%+ |

- **心情状态联动**：(1) 主导情绪标签改变且强度 >0.4 时立即触发情绪变化表情（开心→跳起来 / 悲伤→哭 / 生气→摇头 / 惊讶→眨眼等）；(2) 空闲 45 秒后，25% 概率随机触发当前心情对应的持续表情，让表情随心情自然变化

- **程序事件触发**：前端感知的系统事件直接调用 `trigger_system_event` 命令
  | 事件 | 触发时机 |
  |-----|---------|
  | `morning/afternoon/evening/night` | 时间段变化（6/12/18/23 点） |
  | `window_focus/window_blur` | 窗口获得/失去焦点 |
  | `user_return` | 用户从长时空闲交互回来 |
  | `mood_change_*` | 情绪显著变化（由 `update_mood_state` 触发） |

- **表情库**（由各角色 `model_manifest.json` 定义，非硬编码）：表情是主要的视觉反馈载体，通过 `ExpressionManager` 管理表情栈与定时恢复
  - **Vivian**（11 个表情）：`love_eyes` / `dizzy` / `tears` / `blindfold` / `dark_face` / `angry_symbol` / `confused` / `blood_1` / `blood_2` / `reach_hand` / `holding_knife`
  - **Nana**（30+ 个表情）：`star_eyes` / `love_eyes` / `star_aura` / `shy` / `blush_intense` / `angry` / `pout` / `puff_cheek` / `sweat` / `confused` / `tongue_out` / `blank_eyes` 等
  - **通用参数**（两模型共有，由 `useLive2DBehavior` 微存在感驱动）：`ParamAngleX/Y/Z` / `ParamBodyAngleX/Y/Z` / `ParamEyeLOpen/ROpen` / `ParamEyeBallX/Y` / `ParamMouthOpenY` / `ParamMouthForm` / `ParamEyeLSmile/RSmile` / `ParamBrowLY` / `ParamBreath`
  - **跨模型兼容写法**（`EmotionFacs.ts`）：`JawOpen`(Vivian) / `Jawopen`(Nana) 同设；`CheekPuff` / `CheeckPuff`（拼写差异）同设
  - **情绪联动表情池**：自主行为调度根据 `mood_label` 从 `model_manifest.json` 的 `mood_triggers` 加权选择表情——如 joy/closeness → `love_eyes`；sadness → `tears`/`blindfold`；anger → `angry_symbol`/`dark_face`；curiosity → `confused`/`love_eyes`；fear → `dizzy`/`confused`；bored → `blindfold`/`confused`

#### 其他表现层特性

- **自主行为调度**：3-8 秒随机间隔 idle 动作调度 + 微存在感（呼吸 / 身体微晃 / 情绪联动），睡眠或窗口隐藏时跳过
- **微存在感 RAF 优化**：清醒且可见时以 PIXI Ticker（基于 RAF）驱动呼吸 + 微晃 + 视线游移 + 程序化眨眼；睡眠时停止 Ticker 改为 1s 低频 setInterval 守护固定参数（闭眼 + 视线归零）；`microTick` 内部检查 `document.hidden` 跳过隐藏帧，`presenceState` store（online/busy/rest/offline）驱动模式切换
- **表情管理**：`ExpressionManager` 支持表情栈与定时恢复（`set_expression` 压栈 / `revert_expression` 弹栈 / `start_revert_timer` 定时恢复），表情由各角色 `model_manifest.json` 定义（Vivian 11 个 / Nana 30+ 个）
- **状态机**：`PetState`（Idle / Interacting / Panicked / Playing / AiTalking）+ `StateTransition` 显式状态机，支持事件驱动的状态流转
- **动作优先级**：5 级优先级（Idle=0 / Low=10 / Normal=50 / High=100 / Critical=200）控制动作打断与队列
- **鼠标跟随**：两级跟随（`window` / `off`），交互事件（鼠标进入/点击/拖动）刷新 5-8 秒跟随窗口，窗口内跟随、超时回归自主；后端 `cursor_tracking` 线程每 60ms 推送 `cursor:position` 事件，光标位置未变化时跳过 emit，窗口隐藏时停止跟踪

### 智能避让（图像处理定位）

基于实时屏幕图像分析的智能避让系统，使桌宠自动避开用户正在查看的内容区域：

- **后端**：Win32 GDI `StretchBlt` 降采样捕获屏幕 → 32px 块方差分析 → BFS 连通分量 → FNV-1a 哈希比对（检测屏幕变化）
- **前端**：2.5 秒基础轮询间隔，连续 unchanged 时逐步延长到 30 秒，变化时立即恢复；分步缓动动画（easeInOutCubic）平滑移动
- **智能跳检**：用户切换前台窗口时（Win32 EVENT_SYSTEM_FOREGROUND）立即触发一次检测
- **可配置**：`window.smart_positioning_enabled` 开关，右键菜单打开时临时禁用

### 全屏隐藏

当检测到前台窗口为全屏应用（视频播放器 / 游戏 / 幻灯片）时，桌宠自动退到屏幕角落侧边隐藏（露出 48px 供点击召回）：

- **双源触发**：全屏应用聚焦 + 睡眠模式，任一触发都隐藏到角落，全部退出后才恢复
- **角落选择**：根据桌宠当前屏幕位置，自动选择最近角落（tl / tr / bl / br）
- **PeekButton**：隐藏时角落显示召回按钮，点击恢复桌宠并标记本次全屏期间不再自动隐藏
- **快捷键召回**：`Ctrl+Shift+V` 强制退出隐藏 + 唤醒睡眠
- **协调机制**：隐藏周期内智能避让完全跳过，避免 hide/restore 动画与定位并发冲突

### 语音系统

- **ASR**：WinRT SpeechRecognizer（默认，Windows 原生） / Whisper HTTP 后端 / Azure 云端 / Aliyun 阿里云，四引擎可切换
- **TTS 多后端**：
  - `edge` —— Edge-TTS（WebSocket + WordBoundary，默认在线）
  - `windows` —— WinRT SpeechSynthesizer（离线 fallback）
  - `azure` —— Azure 认知服务（REST + /voices/list）
  - `gpt_sovits` —— GPT-SoVITS 自托管（兼容 v1/v2）
  - `fish_speech` —— Fish Speech（fishaudio /v1/tts）
  - `minimax` —— MiniMax Speech（REST API）
- **口型同步**：`Live2DLipsync` 监听 `lipsync:start/update/stop` 事件驱动 `ParamMouthOpenY`

### 弹性与可观测

- **熔断器**（`resilience/`）：三态（Closed / Open / HalfOpen）+ 滑动窗口 + 失败率判定
- **HTTP 重试**（`network/http_retry.rs`）：可配置重试状态码与退避
- **限流器**（`brain/rate_limiter.rs`）：Token bucket
- **指标系统**（`metrics.rs`）：Counter / Histogram / Gauge 全局单例，每日轮转持久化
- **功能开关**（`feature_flags.rs`）：17 个预定义 flag，分 6 类（Core / Experimental / Performance / Ui / Integration / Debug），支持 `requires_restart` 标记，持久化于 `%APPDATA%\Vivian\config\feature_flags.json`
- **并发模型**：同步互斥锁统一使用 `parking_lot::Mutex`（不中毒、不持有 guard 跨 await），替代 `std::sync::Mutex` 避免 `.lock().unwrap()` 中毒 panic；WNDPROC 等回调路径使用 `try_lock()` 避免重入死锁；远程嵌入服务通过 `Semaphore`（`REMOTE_EMBEDDING_MAX_CONCURRENCY=4`）限流防止外部 API 过载；`augment_reply_service` 通过 `MAX_PENDING_ENTRIES=100` 硬上限防止队列无界增长；所有阻塞系统调用（文件IO、进程枚举、应用解析、系统信息采集）均通过 `tokio::task::spawn_blocking` 隔离到专用线程池，避免阻塞 tokio 异步运行时
- **错误处理**：核心数据结构（如 `MemoryVectorStore` 的 add/delete/clear）返回 `VivianResult<()>` 错误向上传播，不再静默吞错；嵌入服务失败（`MemoryManager` / `ConsolidationPipeline` / `AutoStrategy`）与主动对话 LLM 查询失败（`BehaviorDecider` / `IceBreaker` / `RecallTopic` / `stream_query_and_parse`）均通过 `tracing::warn!` 记录后降级到模板回退，便于排查"AI 突然变笨"类问题；非关键路径错误以 `tracing::warn!` 记录后降级（如 hooks runner / scheduler / feature flags 持久化失败）；日志中 token 等敏感字段做 URL mask 处理（`providers::wenxin` / `speech::aliyun_backend`），并提供 `truncate_for_log` 截断长文本避免日志膨胀
- **TOCTOU 防护**：文件/头像相关命令（`image_to_data_url` / `save_user_avatar` / `clear_user_avatar` / `chat.rs` 图片上传）移除 `exists()` 预检，直接尝试 IO 操作并匹配 `ErrorKind::NotFound` 原子返回友好错误；`std::fs::remove_file` 失败时区分 `NotFound` 与其他错误，仅在非 NotFound 时 warn 记录，避免静默吞错留下脏数据
- **参数归一化**：工具执行器的参数名归一化采用严格策略——先去除所有非字母数字字符并转小写后精确匹配，再按归一化键长度差异排序选最优候选，避免子串匹配导致的错误参数映射
- **降级模式**：`presence::new_with_temp_dir(char_id)` 在持久化目录不可写时降级到临时目录；`McpManager::new_disabled()` 在初始化失败时返回空实现保证主流程不阻塞；`SpeechCache::fallback()` 在缓存目录创建失败时降级到系统临时目录；`TOKENIZER` 静态变量在 `cl100k_base()` 加载失败时降级到字符数估算（中文 1 字 ≈ 1.5 token，ASCII 4 字符 ≈ 1 token），保证 `TimeStampedMemory` 摘要触发逻辑可用；`MotionCurve::sample_at` 在空关键帧场景降级返回 0.0 避免 panic；`ExitRequested` 钩子带 3 秒超时 + `yield_now` 防止退出时阻塞过久
- **超长函数拆分**：`BrainChatChain::ainvoke` 拆分为三个职责明确的方法——`prepare_pipeline_state`（初始化 PipelineState：加载对话历史、注入会话回顾/在场状态/SelfState、更新凝神模式状态机、刷新工具调用上下文）、`execute_pipeline_and_build_response`（执行流水线：stream 配置 → 8 维情绪向量转温度覆盖 → 调用 advisor_chain → 构造 AiResponse → 记录推理轨迹）、`ainvoke`（后处理记忆操作：Working Memory 推入、心理架构更新、记忆子系统写回、工具调用与情感记忆联动、对话管理器写入）

### 心智观察器（Mind Inspector）

内置的认知调试器前端工具，通过 Brain 窗口打开，提供 7 个顶级导航页面可视化智能体内部状态：

- **心智页（Mind）**：核心认知调试页面，内含 4 个子视图
  - **Live Mind**：实时心智快照（Twin View，Vivian + Nana 并排展示，5 秒轮询），显示当前情绪/需求/信念/注意力/目标
  - **Mind Flow**：认知流动图（纵向推理链可视化，12 步映射到 7 个认知阶段），展示每轮对话的心理因果链
  - **Context Pipeline**：Prompt 组装分解（按 section 层级分组可视化，组内按重要性排序、分组抽屉可折叠、自动隐藏 0 字符 section），展示每个 section 的内容、token 估算、是否注入；工具信息通过 `list_tools` 命令动态加载（按当前界面语言返回工具描述，调用 `Tool::description_in(lang)`），按类别分组展示工具名/描述/参数详情（含必填标记和参数说明）
  - **Reasoning**：推理历史列表 + 详情，可追溯每轮对话的完整步骤耗时与输入输出
- **世界页（World）**：世界状态观察器，展示时间/天气/节气/节日/在场状态/室友公共状态/统一事件账本等环境数据
- **信念页（Beliefs）**：信念系统查看器，展示核心信念/关系认知/自我认知
- **注意力页（Attention）**：注意力焦点查看器，展示当前关注焦点/记忆激活/世界书签激活状态
- **图谱页（Graph）**：记忆关系图可视化
- **日记页（Diary）**：角色日记浏览（iOS 风格日历视图，按心情筛选，标题栏显示角色名徽章）
- **用户画像页（User Profile）**：展示角色视角下的用户认知，顶部可切换角色查看不同角色对用户的印象差异。四层结构化展示——L0 基础身份（姓名/年龄/性别/职业/所在地）/ L0.5 偏好资料（生日/作息/常用网站/喜欢的游戏/兴趣爱好，支持内联编辑和锁定保护）/ L1 近期状态（最近目标/当前项目/近期偏好，只读自动抽取）/ L2 自由事实（可新增/删除）

数据源：`get_mind_state` / `get_current_mood`（Live Mind）、`get_recent_reasoning_traces`（Mind Flow / Reasoning）、`get_last_prompt_breakdown` + `list_tools`（Context Pipeline）、`get_world_state`（World）、`list_unified_events`（事件流）、`get_user_facts` / `set_user_fact` / `pin_user_fact` / `delete_user_fact`（User Profile）等。

---

## 会话生命周期

`conversation/` 模块把**所有对话**（User↔Agent / Agent A↔Agent B）统一建模为有生命周期的会话对象，而不是无状态的逐轮转发。它是整个多智能体系统的"交通规则"——决定何时调用 LLM、何时结束对话、何时开启新会话。对话完整性修复（`integrity.rs`）在消息加载时自动扫描孤立的 tool_call（因中断/崩溃导致缺少 tool_result），插入合成 tool_result 防止 API 返回 400 错误。

### 状态机

```
Created → Active → Cooling → Closed
              ↑       │
              └───────┘
              抢救（score ≥ 0.8）
```

- **Created**：刚创建（首轮前）
- **Active**：活跃进行中
- **Cooling**：30 秒冷却窗口，期间收到高分新消息可抢救回 Active，超时则 Close
- **Closed**：已关闭，60 秒创建冷却内不允许同一对角色创建新会话

### CloseReason（8 种关闭原因）

| 原因 | 触发方式 | 后续行为 |
|------|---------|---------|
| `Natural` | Energy/Novelty/Continuation 自然衰减 | 允许主动开新话题 |
| `GoodNight` | 关键词检测（晚安/睡了/去睡了） | 睡眠时间内不主动搭话 |
| `GoodBye` | 关键词检测（拜拜/再见/走了） | 允许主动开新话题 |
| `NoResponse` | 主动聊天被用户忽略（`on_ignored`） | 不主动搭话直到新 Trigger |
| `Interrupted` | 关键词检测（等一下/稍等/老板电话） | 用户回来可恢复旧会话 |
| `Timeout` | 用户 30 分钟无响应（`sweep_user_session_timeouts`） | 不主动搭话直到新 Trigger |
| `Conflict` | 争吵后中断（预留） | — |
| `SwitchTopic` | 显式开启新话题（预留） | 允许主动开新话题 |

### ResponseMode（响应决策）

LLM 在一次调用里同时返回 `response_mode`，避免每条消息都触发完整 LLM 文本回复：

| 模式 | 用途 | 主对话 | 跨角色 |
|------|------|--------|--------|
| `speak` | 正常回复（生成文本） | 默认 | 默认 |
| `non_verbal` | 只做动作/表情（点头/微笑） | 用户发"嗯/哦"时 | 对方说"嗯"时 |
| `internal` | 只更新内部想法/记忆 | 极少 | 琐碎闲聊 |
| `ignore` | 完全忽略 | 几乎不用（对用户粗鲁） | 话题结束/无关 |

### 评分公式

- **Novelty**（新信息密度）：问号 +0.3 / 长度 >10 字 +0.2 / >30 字 +0.2 / jieba 实词 >3 +0.3 / 回复 >15 字 +0.1
- **Energy**（活跃度）：Speak +0.1+ΔNovelty×0.3 / NonVerbal -0.05 / Internal -0.02 / Ignore -0.3
- **Continuation Score**：0.3 + (Novelty>0.5?0.2) + Novelty×0.3 + Energy×0.2 - min(0.3, rounds×0.02) - (Energy<0.3?0.2)
- **状态转换**：Ignore 直接进 Cooling；Continuation<0.30 || Energy<0.25 || Novelty<0.15 → Cooling；否则保持 Active

### 接入点

- **User↔Agent**：`commands/chat.rs` 在 `brain.think` 前调 `start_or_continue`，think 后调 `update_after_round` + 关键词检测 + `seal_episode_on_close`
- **Agent↔Agent**：`cross_character.rs::send` 同上，冷却中返回 `CrossCharacterReply{response_mode:"ignore"}` 不调 LLM
- **主动聊天**：`proactive_tick` 检查 `is_user_session_closed`，GoodNight/NoResponse/Timeout 时跳过主动搭话
- **Episode 联动**：会话 close 时触发 `seal_episode`，让经历边界对齐会话边界

### 典型场景

- 用户说"晚安" → 关键词命中 → close(GoodNight) → 睡眠时段不主动搭话
- 用户说"我去洗澡了"回来"我回来啦" → 旧会话已 Closed(Timeout) → 新会话
- Agent 主动聊天被忽略 → `on_ignored` → close(NoResponse) → 不再"你怎么不理我"
- 会话期间的记忆在 close 时封包为 Episode，经历边界自然清晰

---

## 技术栈

| 层级 | 技术 |
|------|------|
| 后端 | Rust 1.75+（edition 2021）、Tauri 2.1、Tokio、serde、reqwest、rusqlite、heed（LMDB） |
| 前端 | React 18、TypeScript 5.6、Zustand 4、Vite 5 |
| Live2D | pixi-live2d-display 0.4（Cubism 4）、pixi.js 6 |
| 网络 | reqwest 0.12（rustls-tls）、tokio-tungstenite 0.24（Edge-TTS WebSocket） |
| 中文 | jieba-rs 0.7（BM25 分词） |
| 国际化 | i18next 23（前端）、Rust 内置 i18n 模块 |
| Windows | windows 0.61（Win32 + WinRT 语音 / ASR / Core Audio / SMTC / COM 网络事件） |

详见 [src-tauri/Cargo.toml](file:///g:/vivian-rs/src-tauri/Cargo.toml) 与 [package.json](file:///g:/vivian-rs/package.json)。

---

## 快速开始

### 环境要求

| 依赖 | 最低版本 | 说明 |
|------|---------|------|
| Rust | 1.75（stable） | 后端工具链 |
| Node.js | 18 | 前端构建 |
| Windows | 10 / 11 | 当前仅支持 Windows（依赖 WinRT 语音识别与 ASR） |

### 安装

```bash
# 克隆仓库
git clone <repo-url>
cd vivian-rs

# 安装前端依赖
npm install
```

> ⚠️ **Live2D 模型不随仓库分发**
>
> 由于 Live2D 模型存在版权问题，本仓库**不包含**任何 Live2D 模型文件（`.moc3` / `.model3.json` / `.physics3.json` / `.motion3.json` / `.exp3.json` / 贴图等），它们已被 `.gitignore` 排除。
>
> 克隆仓库后，需自行将 Live2D 模型文件放入以下目录：
>
> - `public/Vivian/` — Vivian 角色模型
> - `public/Nana/` — Nana 角色模型
>
> 每个模型目录下应包含一个 `model_manifest.json`（项目自有配置，定义表情/动作的语义映射，仓库中已提供示例）。模型本体文件需用户自行获取并放置。

### 开发模式

```bash
# 同时启动 Vite + Tauri（热重载）
npm run tauri:dev
```

### 构建发布版

```bash
npm run tauri:build
# 产物位于 src-tauri/target/release/bundle/
```

### 验证

```bash
# Rust 编译检查
cd src-tauri && cargo check

# TypeScript 类型检查
npx tsc --noEmit
```

---

## 配置系统

Vivian 的配置位于用户数据目录 `%APPDATA%\Vivian\`（Windows）。

### 路由矩阵任务类型

路由矩阵（`routing_matrix`）支持按任务类型独立配置模型，常用的任务类型键包括：

| 任务类型 | 用途 |
|---------|------|
| `chat` | 日常闲聊与问答（高频，可用便宜模型） |
| `reasoning` | 长输入（>100 字）的深度推理（低频，需强模型） |
| `diary` | 日记内容生成 |
| `memory` | 写入时抽取关键词/重要性/语义类型（高频，建议便宜模型） |
| `embedding` | 记忆向量索引的嵌入服务（用于语义检索） |
| `reflection` | 短期→长期摘要、画像抽取、洞察生成（低频，需强推理模型） |
| `inner_monologue` | 离线内心独白（用户不交互时自主思考，30 分钟一次，建议廉价快速模型） |
| `consolidation` | 夜间记忆巩固（睡眠时整理记忆，低频，需深度推理模型） |

- 未配置的任务将回退到 LLM 主配置。
- 任务 provider 失败后自动 fallback，通过 `chat:route_fallback` 事件通知前端。

### 配置方式

1. **可视化**：右键桌宠 → 设置（ConfigWindow 提供 10 个 Tab：通用 / AI / 工具 / 记忆 / 语音 / 主动对话 / 真实世界 / 网络 / 日记 / 关于）
2. **直接编辑**：关闭 Vivian 后编辑 `config.yaml`，重启生效
3. **Tauri 命令**：`get_config` / `set_config` / `save_config` / `reload_config` / `update_world_config`（世界感知热更新）/ `list_mcp_servers` / `add_mcp_server` / `remove_mcp_server`（MCP 管理）/ `get_worldbook_params` / `set_worldbook_params`（worldbook 调参）

> **真实世界感知**功能会消耗额外 Token（内心独白每 30 分钟一次 LLM 调用、夜间记忆巩固调用深度推理模型）。在"真实世界"页签提供总开关，可按需关闭以节省 Token。

### 错误处理

- `VivianError` 枚举（15 种变体，均带中文错误信息前缀）
- `VivianResult<T> = Result<T, VivianError>`
- 命令层使用 `err_str` 统一将错误转字符串返回前端
- 实现 `From<reqwest::Error>` / `From<io::Error>` / `From<serde_json::Error>` / `From<rusqlite::Error>`
- 实现 `thiserror::Error` + `Serialize`（序列化为字符串）
- **错误传播策略**：核心数据结构（如 `MemoryVectorStore::add/delete/clear`）返回 `VivianResult<()>`，调用方通过 `?` 操作符向上传播；非关键路径错误以 `tracing::warn!` 记录后降级继续运行（如 hooks runner / scheduler / feature flags 持久化失败），避免静默吞错导致问题难以定位
- **降级路径可观测**：嵌入服务失败（`MemoryManager` / `ConsolidationPipeline` / `AutoStrategy`）、主动对话 LLM 查询失败（`BehaviorDecider` / `IceBreaker` / `RecallTopic` / `stream_query_and_parse`）、文件操作失败（`save_user_avatar` / `clear_user_avatar` 删除残留头像）等历史静默吞错路径全部改为 `tracing::warn!` 记录，便于排查"AI 突然变笨"或"清理操作未生效"类问题
- **TOCTOU 防护**：文件/头像相关命令移除 `exists()` 预检，直接尝试 IO 操作并匹配 `ErrorKind::NotFound` 原子返回友好错误，避免"检查后使用"窗口期文件被替换/删除导致的竞态
- **日志安全**：错误日志中 token 等敏感字段做 URL mask 处理（`providers::wenxin` / `speech::aliyun_backend`），`truncate_for_log` 函数截断长文本避免日志膨胀

---

## 国际化

前端支持简体中文（zh-CN）、English（en）、日本語（ja）三种语言，通过 i18next 管理。语言选择保存到 `localStorage['vivian-lang']`，fallback 到 zh-CN。后端 i18n 模块内置中英文翻译表，支持点号分隔嵌套键。所有 LLM 提示词（主对话功能模块、记忆系统、主动交互、对话处理、心智/信念生成、日记生成等 40+ 个任务）均实现三语覆盖，通过 `normalize_lang` + `match lang_norm { "en" => ..., "ja" => ..., _ => ... }` 统一模式选择语言，`_` 分支为中文兜底。所有 JSON 字段名、枚举值保持英文不变以确保下游解析正常。对话/记忆格式统一使用第一人称说话者标记 `[User says to me]` / `[I say to User]`，与记忆存储前缀对齐。

---

## 开发指南

### 代码规范

- **注释语言**：Rust 与 TypeScript 代码统一使用中文注释
- **注释风格**：注释只解释「这段代码做什么 / 为什么这样写」，不写变更说明（如"历史上…现在改为…"）、不写教学型描述（如"正则编译期验证安全"）；模块顶部 `//!` 文档说明本模块职责与设计要点，函数级 `///` 文档说明参数与返回值
- **行尾**：LF（由 [.editorconfig](file:///g:/vivian-rs/.editorconfig) 与 [.gitattributes](file:///g:/vivian-rs/.gitattributes) 强制）
- **缩进**：通用 2 空格；Rust / TOML 4 空格
- **架构约束**：参见 [CONTRIBUTING.md](file:///g:/vivian-rs/CONTRIBUTING.md)

### 常用命令

```bash
npm run dev              # 仅启动 Vite（前端调试，port 1420）
npm run tauri:dev        # 开发模式（Vite + Tauri 热重载）
npm run tauri:build      # 构建发布版（nsis / msi）
npm run build            # 仅构建前端（tsc + vite build）

cd src-tauri && cargo check       # Rust 编译检查
cd src-tauri && cargo build       # Rust 构建
npx tsc --noEmit                  # TS 类型检查
```

### 人格与场景定义

[src-tauri/prompts/](file:///g:/vivian-rs/src-tauri/prompts) 目录采用模块化分层结构定义两个角色的人格：

- **characters/**（角色层，双角色独立，每角色 8 个文件）：
  - `identity.md`：核心身份锚点（你是谁）
  - `personality.md`：场景化人格（采用"触发→反应"行为脚本，用具体场景替代形容词堆砌）
  - `speech.md`：说话节奏/语气/口头禅/禁用模式（含自称、句尾、停顿习惯）
  - `examples.md`：角色专属 few-shot 示例（约 5 个，避免模型模仿特定句子）
  - `background.md`：背景设定（日常生活/作息/环境）
  - `interests.md`：兴趣爱好
  - `relationships.md`：与用户/室友的关系设定
  - `appearance.md`：外观描述
- **framework/**（框架层，所有角色共享，7 个文件）：
  - `chat_style.md`：聊天风格通用规则（像发微信不像写作文；短碎片回复/犹豫改口/状态波动/话题偏好触发：感兴趣的话题多说无感的简短带过/情绪化反复：不必每次逻辑自洽）
  - `address_rules.md`：称呼规则
  - `conversation_rhythm.md`：对话节奏
  - `session_rules.md`：会话规则（新会话/续聊/首次见面）
  - `speaker_prefix.md`：说话者前缀标记
  - `output_format.md`：JSON 输出格式规范
  - `safety.md`：安全规则（身份保护/内容边界/工具协议）
- **styles/**（5 个）：说话风格切换预设（default / lively / healing / focused / sweet）
- **worldbook/**（3 个）：背景知识触发（game_culture / internet_culture / anime_culture）
- **system_prompt.tera**：Tera 模板入口

**Prompt 架构原则**：
- **U 型注意力调度**：静态区 Character 开头（最先入脑）→ Framework/Format 末尾（临出口提醒），利用 LLM 注意力偏置提升人格稳定性与格式准确率
- **静态/动态分离**：静态内容（人格/框架/示例）用 `<static>` 标签包裹，动态内容（心智/世界/记忆）在后，提升云端 API 缓存命中率
- **动态边界弱化**：使用自然过渡句（"Right now, in this moment..."）替代硬编码边界标记，减少提示词泄露风险
- **功能提示词动态化**：心理洞察/信念生成/思维合成/日记生成/记忆提取等功能模块全部使用角色名变量，支持多角色架构
- **行为化语音指南**：跨角色对话时注入角色专属行为约束（"你说话比她快，句子更短"），替代数值化标签（如 sass=0.65）
- **内心反应中文化+角色化**：第一人称内心想法使用中文生成，按角色差异化（Vivian 直率吐槽 / Nana 温柔关心）
- **三语提示词覆盖**：所有 LLM 提示词（记忆抽取/验证/路由/巩固、主动交互行为/破冰/回忆/内心独白、对话意图判断/共指消解/策略摘要、心智信念生成/用户认知、日记生成、情绪分类、查询重写等 40+ 个任务）均实现中/日/英三语，通过 `normalize_lang` 统一切换；说话者标记全项目统一为 `[User says to me]` / `[I say to User]` 第一人称格式
- **数据源编排优化**：记忆上下文 token 预算 1250 + 类型标签（长期/短期/对话）+ 重要性排序；反思/异步反思注入对话历史和 AI 回复；AugmentReplyService 记忆按重要性升序排序（重要的排在 LLM 注意力更佳的末尾）；日记对话片段附带时间戳 + 截断 150 字符；记忆验证/用户事实抽取注入已有数据避免重复

### 完整代码文档

详细的代码架构、模块职责、关键类与函数说明请参阅 [CODE_WIKI.md](file:///g:/vivian-rs/CODE_WIKI.md)，包含：

- 后端 36 个顶层模块 + 225+ 个 Tauri 命令的完整说明
- 前端 21 个组件 + 6 个控制器 + 5 个 Hooks 的职责清单
- 关键数据流（对话流 / 主动对话 tick / 心理微调 / 启动流程）
- 依赖关系总览与持久化统一模式
- 心理学五层架构与昼夜节律锚点
- 工具系统 7 步执行管线与权限网关矩阵

### 调试

- 开发模式下 Tauri 自动打开 devtools
- 日志位于 `%APPDATA%\Vivian\logs\vivian_YYYY-MM-DD.log`（保留 7 天）
- 性能指标位于 `%APPDATA%\Vivian\logs\metrics_YYYY-MM-DD.json`（每日轮转）
- 功能开关位于 `%APPDATA%\Vivian\config\feature_flags.json`
- MCP 配置位于 `%APPDATA%\Vivian\mcp\servers.json`
- 关系演化日志位于 `%APPDATA%\Vivian\psychology\relationship_log.json`
- 可通过 `tool_observability` 功能开关查看工具调用详情

---

## 故障排查

| 现象 | 可能原因 | 解决方案 |
|------|---------|---------|
| 启动后无问候 | 主 LLM API 未配置 | 在设置窗口配置 routing_matrix.chat 的 api_key / endpoint / model |
| 子窗口无法打开 | capabilities 权限缺失 | 检查 [capabilities/default.json](file:///g:/vivian-rs/src-tauri/capabilities/default.json) 是否包含对应窗口标签 |
| TTS 无声 | 后端未正确配置 | 检查 TTS 配置，或切换为 `windows` 后端（离线） |
| 联网搜索失败 | 代理配置或网络问题 | 检查 `network.proxy_mode` 与 `HTTPS_PROXY` 环境变量 |
| 记忆检索慢 | 嵌入未启用或向量库过大 | 启用 `memory.embedding.enabled`，或调小 retrieval_weights |
| 工具调用卡住 | 超时或权限被拒 | 查看 `tool:confirmation_request` 事件与 `metrics.json` |
| 智能避让不工作 | 配置关闭或屏幕无变化 | 检查 `window.smart_positioning_enabled` 是否为 true；无变化时轮询间隔自动延长 |
| 天气感知失效 | Open-Meteo 不可达或经纬度未配置 | 检查 `world.enable_weather` 与 `world.latitude` / `world.longitude`；失败时按"不知道"处理，不阻断其他功能 |
| 内心独白不生成 | 路由矩阵未配置 inner_monologue 任务 | 在设置窗口 → 真实世界页签确认 `enable_inner_monologue` 开启，并在 AI 页签为 `inner_monologue` 任务配置廉价快速模型 |
| 记忆巩固未执行 | 不在睡眠窗口或冷却未到期 | 确认 `world.enable_memory_consolidation` 开启；巩固仅在 `sleep_start_hour`..`sleep_end_hour` 窗口内且距上次 ≥ 6 小时触发 |
| 启动后窗口数量异常（多于角色数） | main 控制器窗口被错误加载 | main 窗口（`label="main"`）是隐藏控制器，不应可见；检查 `main.tsx` 是否对 `label="main"` 跳过加载 `App.tsx` |
| 角色窗口右键菜单无法打开子窗口 | 子窗口 label 与其他角色冲突 | 子窗口 label 需按角色区分（格式 `<char_id>_<base>`，如 `nana_chat`、`vivian_status`），避免多角色窗口 label 撞车 |
| 某角色模型未加载 | 模型目录缺失或未打包 | 检查 `public/<ModelName>/` 目录是否存在且含 `model_manifest.json`；并确认 `tauri.conf.json` 的 `bundle.resources` 已包含该目录 |
| 多角色心情状态串扰（A 角色心情影响 B 角色面板） | 全局静态未清理或事件未按角色过滤 | 检查 `commands/emotion.rs` 的 `LAST_TRIGGER` 是否按 `char_id` 索引、`psychology:state` 事件 payload 是否携带 `character_id` 字段；前端 StatusPanel 是否按 `character_id` 过滤事件；`tools/emotional_recovery.rs` 的 `EMOTIONAL_STATE` 是否按 `char_id` 索引 |

---

## 联系方式

- **项目地址**：https://github.com/SpacervalLam/Vivian-ai-desktop-pet
- **联系邮箱**：spacervallam@gmail.com

Bug 报告与功能建议请优先通过 [GitHub Issues](https://github.com/SpacervalLam/Vivian-ai-desktop-pet/issues) 提交；其他事宜可通过邮件联系。

---

## 许可证

[MIT License](file:///g:/vivian-rs/LICENSE)

Copyright (c) 2026 SpcervalLam