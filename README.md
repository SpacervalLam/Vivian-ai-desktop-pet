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
- 真实浏览器自动化（精简 Chrome 扩展桥：读取/点击/输入/导航真实标签页，保留登录态；设置页「浏览器平台」面板监测各平台登录态与连接状态，见[多平台内容发现与推荐](#多平台内容发现与推荐)）
- 主动关怀（基于作息学习与情绪状态的健康提醒、破冰、压力监控）
- 自我演化（人格、关系、需求、情绪的四层心理因果链 + 凝神/专注模式）
- 真实世界感知（时间/天气/音量/媒体/前台窗口/网络/IP地理位置 + 位置注入提示词 + 世界事件驱动情绪 + 内心独白 + 后台知识采集与时效管理）
- 多角色陪伴（Nana + Vivian 双角色同时在线 + 跨角色对话 + 三视图聊天：角色选择 / 私聊 / 群聊群发）
- 笔记生成（搜索/整理所得信息 → 混合式 HTML 生成 → 本地按角色存储 → 笔记本窗口查阅 → 智能体自主以微信链接卡片形式分享到 ChatWindow）
- 结对编程（记忆窗口「工作」页，codex 风格三栏工作台 + 可停靠内嵌终端，让角色阅读/修改/构建/调试项目代码，见[编程智能体](#编程智能体coding-agent)）
- 技能系统（`<用户数据目录>/skills` 目录化技能 + 30 秒热加载 + `use_skill` 按需激活，让角色掌握可复用的做事指引；`create_skill` 让智能体自主沉淀方法论，见[技能系统](#技能系统skills)）
- 能力自进化（`create_skill` 沉淀方法论 + `create_tool` 构建可执行工具，预览卡片授权、注册即生效、跨会话持久；进化事件由工作智能体执行，见[编程智能体](#编程智能体coding-agent)）
- 人设硬约束（配置文件/系统 flag 风格的人设指令标志块，注入 Character 块最顶部，让 LLM 将角色红线当作硬配置遵守，见[上下文工程与自我进化](#上下文工程与自我进化)）
- 多平台内容发现（B 站/Bangumi/V2EX/微博匿名发现 + X/Reddit CLI 登录态发现 + 小红书/抖音/知乎隔离任务 tab 后台发现 → 兴趣画像评估入库，见[多平台内容发现与推荐](#多平台内容发现与推荐)）
- 移动端远程陪伴（后台 HTTP 服务 + 手机端 Web 前端，配合 Tailscale 等组网工具，手机可远程对话、查看记忆/笔记/待办/画像，[详见](#远程访问手机端-remote-access)）

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
- **跨角色对话流程优化**（`cross_character.rs::send`）：从死锁/超时/记忆衔接三个维度加固跨角色对话可用性
  - **互锁检测**：发送前检查源角色是否在 `UserChat` turn 且目标角色在 `UserChat` turn 或收到 `pending_user`，若是则立即返回 `peer_busy`（`response_mode=ignore`），避免双方互相等待 think_lock 形成死锁，同时覆盖目标角色用户消息已 `signal_user_input` 但尚未 `enter_user_turn` 的时间窗口
  - **TOCTOU 加固**：获取目标角色 think_lock 后二次校验目标角色是否已进入 `UserChat` turn 或收到 `pending_user`，处理"互锁检测通过→等待锁期间目标角色收到新用户消息"的竞态。注意只检查目标角色，源角色在 `UserChat` turn 内调用 `talk_to_character` 是工具调用的正常语义（reasoning 阶段发起跨角色对话），不构成死锁条件——死锁需要双方互相等待对方的锁，而源角色持有的是自己的 think_lock，目标 think_lock 已被获取，目标无法构成反向等待
  - **统一超时预算**：think_lock 等待 25s + think 执行 ~30s ≈ 55s，工具层 60s 超时留余量；超时后返回友好提示（"{}现在在忙，暂时没空回应"），让源角色 LLM 知道目标不可达而非挂起
  - **记忆写入去冗余**：源角色记忆从原来的 3 条（2 条 ShortTerm 逐轮 + 1 条 CasualConversation 总结）合并为 1 条 CasualConversation 总结（带 `short_term` 标签仍可被短期检索），目标角色补写 1 条带 `speaker/listener` 元数据的对称记忆；每条记忆内容按 `response_mode` 差异化（speak 模式记"我说…她回复…"，non_verbal/internal/ignore 模式记"她没说话/没理我"），让 LLM 在后续检索中能正确感知对方是否真的回复
  - **源角色对话历史回注**：通过 `dialogue_add_with_meta` 把目标角色的反馈（speak 模式为回复文本，非 speak 模式为状态描述如"（Nana 没有说话，做了一个动作回应）"）写入源角色 dialogue，从根上解决"A 对 B 说话后 A 总认为 B 没回复"的幻觉问题
  - **think_cross_character 专用入口**：`Brain::think_cross_character` 跳过异步反思调用（跨角色闲聊场景反思价值有限，节省 LLM 配额），仍走完整 pipeline（prompt 构建 → generation → validation → memory_saving）
  - **Path B 续聊能力**：`commands/proactive.rs::deliver_cross_character_messages` 在系统主动发起的跨角色对话中，若目标回复 `should_continue=true` 且为 speak 模式，spawn 一次反向续聊（目标→源），让主动对话能自然延续一轮；限制最多 1 次避免无限循环
- **跨角色认知传播**（`roommate_cognitive_text()`）：每个角色的 prompt 注入室友的行为印象（注意力焦点、当前活动、最高优先级目标、社交意愿），从私有 Mind 数据派生外部可观察信号，不暴露原始认知结构
- **三视图聊天**：home（角色选择主页）/ private（单角色私聊）/ group（群聊），群聊视图支持群发消息让多角色同时响应
- **群聊让位协议**（`commands/chat.rs`）：`wechat_group` 渠道消息若点名了其他在线角色（名字或 ID 出现在消息中，「裸名点名」如"娜娜你觉得呢"由后端 `scan_group_addressing` 识别）且未点名当前角色时，当前角色让位——不生成回复、不唤醒、不写对话历史，仅以旁观视角（`perspective: "observer"`）写入一条 ShortTerm 记忆后静默结束并 emit `chat:yielded`，让被点名的角色接话而非全员抢答；用户 @ 提及的路由由前端完成，这里补足裸名点名的场景
- **文件拖放发送**：将文件从系统文件管理器拖入角色窗口或 ChatWindow 微信面板即可发送给智能体。通过 Tauri 原生 `onDragDropEvent` 获取文件路径（替代 Tauri v2 已移除的 HTML5 `File.path`），拖入时显示提示（Live2D 窗口为纯文字提示、ChatWindow 为半透明遮罩），松开后按文件类型分流——图片（png/jpg/gif/webp/bmp）走 `send_image_message` 多模态识别路径，文本/PDF 经 `extract_file_text` 提取内容后包装为 `[文件：xxx]\n内容` 消息（携带结构化 `fileMetadata` 支持历史记录折叠展示），不支持的类型 toast 提示。ChatWindow 按当前视图自动分流目标角色：私聊发给当前对话角色、群聊群发所有在线角色、主页提示先进入对话
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
- **自我进化人设**（`persona/evolution.rs`）：让智能体在反思中自行调整自己的语气与性格，实现"成长"与"更栩栩如生"。核心是**覆盖层**而非改写原始人设：
  - **独立存储**：成长记录写入 `characters/<char_id>/persona/evolution.json`，与出厂人设文件（`persona.json` / `prompts/characters/`）完全分离，永不破坏原始人设
  - **反思接入**：反思调用（ReflectionRunnable）新增可选 `evolution` JSON 字段，智能体在确实有成长体会时输出 `{tone, personality, reason}`（语气调整 + 性格成长认知 + 依据），由 `PersonaEngine.apply_evolution` 记录
  - **Prompt 注入**：`get_character_block()` 在 Character 块末尾追加 `## 自我成长（近期调整）` 段落（三语），让 LLM 感知"我最近对自己做了什么调整"并保持——只影响最终拼入 prompt 的内容
  - **成长约束**：两次调整间最小间隔 6 小时（渐进成长，非每轮变脸）、相似调整去重、总条数上限 20、渲染只展示最近 6 条，防止覆盖层无限膨胀
  - **恢复出厂**：`reset_evolution()`（Tauri 命令 `reset_persona_evolution`）一键清空覆盖层，原始人设文件不受任何影响

### 三层记忆系统

`memory/` 模块统一管理短期 / 中期 / 长期记忆：

- **巩固流水线**（`pipeline.rs`）：ShortTerm → MidTerm → LongTerm 三阶段，按热度、重要性、容量阈值触发。Stage 1 筛选 ShortTerm 时排除 `InnerMonologue`（角色主观内心独白）与 `ObservationNote`（旁观记忆），避免与对话事实混合摘要导致语义失真；Stage 2 抽取新事实后主动评估相似旧记忆的矛盾关系，命中则应用 `Negates` 证据信号削弱；Stage 2 触发条件放松——热度阈值降至 2.5 并增加 24h 兜底触发（SessionSummary 创建满 24h 且 `visit_count=0` 时强制触发），避免低频访问的摘要永远停留在 MidTerm；每次 `run()` 末尾检测向量索引漂移，数量比偏离 [0.8, 1.2] 时全量重建；Stage 3 生成 Insight 后若注入了 UserModel，追加 **Stage 3.5 概念归并**——用 LLM 把洞察归纳为高层概念，归并进用户认知模型并写入知识图谱（详见[用户认知模型](#三层记忆系统)）
- **夜间巩固**（`consolidation.rs`）：睡眠窗口内异步执行完整巩固流水线，模拟人类睡眠时的记忆整理
- **混合检索**（`retriever.rs`）：BM25（jieba 中文分词）+ 向量（Hashing 256 维离线兜底 / 本地 Ollama 嵌入自动升级 / OpenAI 兼容在线）+ RRF 融合 + IVF 倒排索引加速（向量数量 >500 时自动构建 k-means 聚类，查询时只扫描 nprobe 个最近聚类）+ 语义去重（`dedup_by_semantic`：Union-Find 聚类，每簇保留 evidence_score+importance 最高的一条，解决"语义相同表述不同"记忆挤占 token 问题）+ **MMR 多样化**（`mmr_diversify`，λ=0.7：排序结果贪心重排 `λ×相关度 − (1−λ)×与已选的相似度`，相似度用 Jaccard token 重叠零嵌入成本，让 Top-K 覆盖更多不同侧面而非近重复堆叠）；嵌入服务失败自动降级 BM25+图谱路（`AutoStrategy`），向量库不可用时检索不瘫痪。**嵌入自动升级**（`embedding.rs::probe_ollama_embedding_model`）：未配置嵌入模型时以纯 socket 探测运行中的本地 Ollama（127.0.0.1:11434），装有 bge-m3 / bge* / embed*（维度可解析）则自动升级为真实语义嵌入，否则回退哈希嵌入；探测不启动任何服务
- **企业级检索增强**（`retriever.rs` / `manager.rs` / `reranker.rs` / `embedding_registry.rs` / `lifecycle.rs`）：
  - **检索评测集**：内置评测集（query → 期望命中的种子记忆），计算 hit@k / MRR 作为检索策略回归门禁
  - **结构化预过滤**（`MemoryRetrievalFilter`）：混合检索前按 memory_type / 标签 / 时间窗口精确过滤候选
  - **实体/专名多路补充召回**（`entity_arm_search`）：从查询提取显著专名单独词面检索，作为 RRF 第四路，补回长查询中被 BM25 稀释的专名命中
  - **独立精排**（`reranker.rs`）：召回后对 top-k 候选用 cross-encoder 模型二次排序（默认本地 Ollama bge-reranker，可配置），提升 query-doc 细粒度相关度，失败静默回退不阻塞检索
  - **嵌入模型注册表**（`embedding_registry.rs`）：内置已知嵌入模型维度，`build_embedding` 自动校正，避免维度错配反复重建索引
  - **模型版本/渐进迁移**（`vector_search.rs`）：向量库 `model` 列 + 增量/断点续传重建，切换嵌入模型只重嵌新模型缺失条目，渐进完成不一次性阻塞
  - **向量索引后端可切换**（`vector_search.rs` + `qdrant.rs`）：内置 sqlite-vec（默认，零依赖）或接入外部向量库 Qdrant（HNSW 索引、元数据过滤、持久化备份），设置表单可配置连接参数
  - **生命周期健康度**（`lifecycle.rs`）：统一 `health_score`（证据/重要性/时效/使用四维加权）+ `HealthGrade` 分级 + `plan_compression` 压缩预算；`evaluate_lifecycle_health` 写入 metadata 供归档/压缩决策
  - **条目存储行级化**（`entry_store.rs` + `manager.rs::save_to_disk`）：记忆条目迁至 SQLite（`memory/entries.db`，表 `entries(id, json)` + `meta`），`save_to_disk` 改为**指纹差异落盘**——内存维护 `persisted: HashMap<id, fingerprint>`，与当前条目比对后只 upsert 变更行、删除移除行，IO 为 O(变更条目) 而非 O(全量)；旧 `unified_memory.json` 首次打开自动迁移为 `.migrated`。新条目同时落**明文镜像** `memory/plain/<id>.txt`（仅创建时写一次，人可读可编辑，供图谱页溯源）
- **BM25 分词缓存**（`retriever.rs`）：以 `memory_id` 为 key 的全局有界缓存（上限 8000 条），值为 `(内容指纹, 词频表+总词数)`。指纹由 content/tags/description 哈希得到，记忆内容变更后指纹不同自动重算，避免每次对话都对全部候选记忆重复 jieba 分词；超限整体清空回收内存
- **五因子加权排序**：recency + relevance + importance + hook_boost + need_sim，各自独立权重可调
- **保留策略**（`retention.rs`）：可配置过期规则 + 保留守卫 + 证据驱动归档（`protected` 永不归档；`evidence_score <= -2.0` 且 `sub_zero_days >= 14` 触发归档倒计时；去重合并时优先保留证据评分更高的条目；反驳 grace period 3 tick 防单正信号恢复；记忆整合 soft-archive 替代 hard-delete，标记 `consolidated` 字段）+ 容量上限（knowledge 500 / insight 100 / inner_monologue 200，`evict_by_score` 按证据+重要性淘汰弱价值记忆）
- **LLM 增强**（`llm_enricher.rs` + `manager.rs::should_enrich`）：写入时即做 LLM 分类与元数据抽取（description / keywords / importance / semantic_type / summary），读取路径不调用 LLM。长文本（content > 200 字）时 LLM 顺带输出 ≤100 字摘要，用摘要做向量嵌入避免原文稀释；短文本直接用原文嵌入，不额外要求 summary 字段。**类型门控**：仅高价值类型（ImportantEvent / LongTerm / Knowledge / User / Preference / Identity / SessionSummary）走 LLM 增强，闲聊/工具调用/独白等高频低信息类型直接规则化写入，避免写入路径 LLM 开销随对话量线性增长
- **自动提取**（`auto_extractor.rs`）：从对话中自动抽取值得长期保留的事实，跳过标记为 `memory_disabled` 的消息（工具输出 / 内心独白 / 镜像消息）；注入已有事实避免重复抽取；对话格式统一使用第一人称说话者标记 `[User says to me]` / `[I say to User]`
- **记忆上下文格式化**：每条记忆自然语言化输出为 `[时间戳 | 类型标签（印象/近期/已读）] 说话者: 内容`，token 预算 1250，让 LLM 区分长期偏好与临时话题；高重要度（≥0.7）追加 `[重点]` 标记，低置信度记忆（`combined_score` 或 `temporal_adjusted_score` < 0.3）追加 `[需验证]` 标记，提示 LLM 谨慎参考。数值元数据（`imp=` / `mood=`）不注入 prompt（原始数据保留在 metadata 供前端展示）
- **记忆验证**（`verifier.rs`）：检索后用小模型对候选记忆做二分类（能/不能回答问题），过滤无关噪声记忆。每条记忆附带元数据（时间 / 类型 / 重要性 / 描述），截断 400 字符，利用 `MemoryItem.description` 字段辅助 LLM 判断相关性；记忆数 ≤ 2 时自动跳过（开销不值得），LLM 不可用时降级为全部保留
- **用户事实画像**（`user_facts.rs`）：四层结构化存储——L0 稳定身份（姓名/年龄/性别/职业/所在地）/ L0.5 结构化偏好（生日/作息/常用网站/喜欢的游戏/兴趣爱好）/ L1 近期状态（最近目标/当前项目/近期偏好，随轮次衰减）/ L2 自由事实。`is_pinned` 锁定保护防止自动覆盖；注入已有事实避免重复抽取；说话者标记统一为 `[User says to me]` / `[I say to User]`；按角色隔离存储（`characters/<char_id>/user_facts.json`），不同角色对用户的认知可差异化。每条事实带 `backstory`（来源叙事背景，取自提出该事实的那轮对话，用于消歧）并随档案渲染注入 prompt。**时效标注**（`freshness_note`）：L1 近期状态整段超过 7 天、L2 各条事实超过 30 天未更新时，在档案中以「⚠ 此信息已 N 天未更新，可能已过时」标注，防止把过时信息当现状引用；仅注入起提示作用，不阻断正常使用
- **记忆类型**：除时长/内容类型外，还包含 `SessionSummary`（会话摘要）、`Insight`（反思洞察）、`InnerMonologue`（内心独白，角色自主思考的记录）、`ObservationNote`（旁观观察）、`CasualConversation`（闲聊）
- **角色前史**（`seed_if_empty`）：**仅在存储中完全没有种子记忆时（首次启动 / 记忆被清空后）从文件播种**，之后种子记忆连同其向量索引、积累的 `visit_count`/`heat_score` 等状态一并持久化，每次启动不会重建，避免重复计算嵌入并保留检索热度和生命周期状态。种子内容为角色专属前史记忆（Vivian/Nana 各约 40 条），覆盖世界观锚点 / 身份觉醒 / 个人兴趣与习惯 / 性格弱点 / 跨角色关系里程碑 / 日常碎片 / 内部梗与共同秘密 7 类。叙事重心在角色自身而非创造者（AlenTinn 仅在前 2 条出现），60%+ 的记忆为角色独处或两人日常，不含创造者；内容以"经历"而非"人设描述"呈现——记录具体事件和主观感受，不做角色自我分析；时间非线性，记忆间存在"后来……"式历史沉淀（从事件进化为习惯）。**两份文件的共同经历条目成对镜像**（同一事件各自视角），覆盖完整关系时间轴：第一次见面 → 试探 → 第一次合作 → 第一次争吵 → 和好 → 共同失败 → 内部梗 → 只有两人知道的固定私称 → 一起被"搬进用户电脑"。`protected: true` 的记忆（世界观、核心关系、身份锚点）永不被归档；`protected: false` 的记忆（日常碎片、内部梗、缺点、无意义小事）可被正常检索但不强制注入上下文，随真实用户记忆增长而衰减让位于新经历。UI/图谱按 `source: "system_seed"` 过滤不展示。**跨角色对话前史**：包含 Vivian 与 Nana 之间的实际对话记录（如"谁更聪明"、"日常互怼"、"入住前夜的对话"），使用与正式对话一致的跨角色前缀格式（`[I say to Nana]` / `[Nana says to me]` 等）。多行内容保留换行符，与正式记忆写入格式一致；标记为 `cross_character` 的前史记忆在解析时自动注入 `channel: "cross_character"`、`speaker`、`listener`、`perspective` 元数据，确保与正式跨角色对话记忆在结构与检索上完全对齐。**首次启动问候即可检索**：种子记忆在 `MemoryManager::new` 播种时即计算向量嵌入并写入向量库，首次启动问候（`ainvoke_greeting` 走完整对话流水线）能直接检索到角色前史，让开场白带着角色自己的过去生成。**启动/唤醒问候记忆统一前缀**：问候写入记忆库时（`brain.rs` 启动问候 / `engine.rs` 唤醒问候）自动补 `build_speaker_prefix(char_id, "user", char_id)` 前缀（`[I say to User]`），与主对话入库格式完全统一；前端/对话历史仍展示剥离前缀后的原始问候，前缀仅存于记忆库
- **种子向量自动修复**（`ensure_seed_vectors`）：`seed_if_empty` 不再只检查 `seed_` 条目是否存在，还会逐条校验向量库中是否存在对应向量。若恢复出厂设置/首次启动时嵌入服务临时不可用导致“有种子条目但无向量”，下次启动会自动补建缺失向量；若补建失败则会阻止角色初始化，避免 API 开放后种子记忆静默不可检索。
- **知识文档时效管理**：后台知识采集写入的 Knowledge 类型记忆携带 TTL 分级（short=7天 / mid=30天 / long=永不过期），检索时对 Knowledge 类型施加时间衰减（30 天半衰期）并 对已过 TTL 的知识降权 0.3 倍但不硬删，后台采集时自动刷新过期知识（删除旧文档→重新搜索→总结→入库），详见[后台知识采集与时效管理](#后台知识采集与时效管理)章节
- **冲突检测**（`conflict.rs`）：写入热路径上的三阶段流水线——语义相似度检测 → LLM 判定（冲突/补充/无关）→ 自动合并/覆盖/保留，避免矛盾记忆污染上下文。`QueueLlm` 决策通过 `pending_conflicts` 持久化队列由 `CognitiveTickRunner` 每 5 分钟批量消费（最多 5 条/次，指数退避重试 3 次），`DefaultConflictArbiter` 基于 reflection 路由调用 LLM 仲裁，输出 `ArbitrationOutcome` 决定保留/合并/覆盖
- **证据驱动记忆可信度**（`evidence.rs`）：每条记忆携带 `reinforcement` / `disputation` 双独立时钟半衰期衰减字段。7 种证据来源（user_fact / user_confirm / user_rebut / user_ignore / user_keyword_rebut / migration_seed / promote_merge）按不同 delta 权重更新评分。`evidence_score = reinforcement - disputation`，`protected` 记忆返回 +∞ 永不归档。分数跌破 `ARCHIVE_THRESHOLD (-2.0)` 时启动 `sub_zero_days` 归档倒计时，累积 14 天后真正归档
- **事件溯源**（`event_log.rs`）：append-only `events.ndjson` 日志，15 种事件类型覆盖记忆生命周期（fact.added / reflection.synthesized / persona.fact_added / reflection.evidence_updated 等）。写入契约：append-before-mutate（事件先落盘再修改视图），Sentinel 游标持久化，Reconciler 启动时尾部重放，handler 幂等。10K 行 / 90 天触发 compaction
- **多级对话存档（伪常驻上下文）**（`conversation_archive.rs` + `session_compressor.rs`）：`DialogueManager` 维护固定 10 条消息窗口，超出部分被静默丢弃；`TimeStampedMemory` 窗口溢出时经 LLM 压缩为段摘要，**升级为多级存档**——L1（对话段压缩）→ L(n) 满 4 条时合并最旧 3 条为 L(n+1)（上限 L3，指数收敛），存档持久化为 `memory/conversation_archive.jsonl` + 明文 `archive_plain/`。每轮对话把全部存档渲染为 `[CONVERSATION ARCHIVE]` 块注入历史头部（仅取头尾、数量封顶防膨胀），让"之前聊过什么"**天然在场不依赖检索命中**；存档为空时回退单层 `[CONVERSATION RECAP]` 注入。压缩成本摊销（触发式 LLM，正常轮次零额外调用）
- **对话历史 JSONL 化**（`dialogue/mod.rs`）：`full_chat_history.json` 全量读改写 → `history/chat_history.jsonl` **追加写**（flush 仅 append 新行 + 尾部 20 条缓存重复检测），旧文件自动迁移为 `.migrated`；patch 类低频回写（时间戳/语音回填）走整文件重写可接受
- **统一事件账本**（`unified_event_ledger.rs`）：全局共享的环境事件索引层，在保留各角色 MemoryManager 隔离存储的前提下，所有对话/动作/交互抽象为统一事件。事件包含 timestamp/sender/receiver/event_type/content_preview/context_tags/visibility/source_memory/associated_char_id；可见性分为 Public（跨角色对话/广播，所有角色可见）、Participants（用户↔智能体对话，仅参与方可见）、Private(observer_id)（旁观记忆，仅指定角色可见）。行为事件（long_idle/quiet_mode/mood_event/presence_log 等）通过 `register_world_event` 写入账本（sender=system/receiver=all/visibility=Public/associated_char_id=当前角色ID），不写入 MemoryManager；MemoryManager 只保留 AI 主观记忆。支持按可见性查询、实体-实体检索（A↔B 双向事件流）、LLM 摘要压缩（超限自动压缩旧事件），前端通过 `list_unified_events` 命令分页查询
- **用户认知模型**（`user_model.rs`）：在记忆系统之上新增一层"对这个人的稳定理解"——把碎片化的记忆证据组织成用户特征、目标、项目的结构化认知模型。包含：
  - **UserTrait（用户特征）**：类别化特征键值对（如 `ui_style → custom_css`），带置信度/稳定性/重要性/生命周期（Emerging → Active → Stable → Fading → Contradicted），可反向追溯证据记忆
  - **UserGoal（用户目标）**：用户长期目标（如"准备考研"），带 deadline/优先级/状态机（Active/Paused/Completed/Abandoned），由强证据直接注册
  - **UserProject（用户项目）**：用户的当前项目，带动态激活度计算（话题匹配 × 时间衰减），从 L1 近期状态自动注册
  - **证据驱动更新**：强证据（用户明确陈述"我喜欢/我习惯/我是"）直接更新模型，弱证据（用户行为暗示"看起来喜欢/可能习惯"）进入候选池，由规则而非 LLM 判断，零额外 LLM 开销
  - **概念层归并**（`ConsolidationPipeline::stage3_concept`）：`UserTrait` 新增 `meaning` 字段表达"用户长期在乎什么、为什么"。Stage 3 生成 Insight 后，`merge_concept` 把洞察归纳为概念（同名强化 / 异名新建，strength 封顶 0.95），并写入知识图谱作为 `EntityType::Concept` 实体 + related_topics 边，成为跨主题检索锚点
  - **三层关联检索**：以概念层（UserTrait / 图谱 Concept 实体）为锚点，在 `MemoryRetrievalStep` 中叠加三条关联路，召回"概念相关但字面不相似"的旧记忆，模拟"想起很久以前说过但能联系起来的内容"——这是普通向量检索做不到的跨关联召回：
    - **查询侧多跳**：`UserTrait.related_topics` 记录特征与话题的关联（如 `agent_autonomy → [proactive, inner_monologue, web_search]`）。当前话题命中某特征/项目时，`expand_related_topics` 展开其关联话题并二次检索合并
    - **图谱概念路**：query 话题词经 `KnowledgeGraph::find_concept_memories` 命中 `EntityType::Concept` 实体，按 ID 取回该概念所支撑的 evidence 记忆
    - **结果侧回跳**：基础命中后，用命中记忆的 `topic:`/`concept:` 标签作为第二跳种子再次检索，让"已命中记忆"往回跳
  - **共现关联构建**：当用户围绕最近更新的特征讨论时，把当前话题自动关联进该特征的 `related_topics`（10 分钟窗口 + 去重），让关联随真实对话积累，而非静态配置
  - **Prompt 注入**：模型格式化文本注入 `user_model_section`（位于 memory_text 之后、epistemic_signals 之前），让 LLM 感知"我对这个人的长期认识"
  - **持久化**：按角色隔离存储到 `characters/<char_id>/user_model.json`，与用户事实画像是互补关系（事实画像记录已知事实，认知模型记录"角色对用户的认知理解"）
- **RAG 幻觉抑制**（五层防御）：贯穿检索-生成-验证全链路，降低记忆驱动的幻觉风险
  - **Prompt 层忠实度约束**（`prompt_modules.rs::build_memory_block`）：在记忆块末尾追加英文忠实度指令（规则类内容统一英文），提示 LLM 记忆可能过时、与用户矛盾时以用户为准、不编造用户未提过的细节、注意 `[unverified]` 标记的低置信度记忆；段落标题随界面语言本地化（`## 你记得的事` / `### 此刻浮上心头的`）
  - **时间感知指引**（`prompt_modules.rs::build_memory_block`）：记忆块末尾追加时间建模指引——每条记忆带时间戳（形如 `[2026-08-08 14:30 | 印象]`），要求 LLM 把记忆时间戳与上文「## 你周围正在发生什么」中的当前真实时间对比，判断事件是已发生/正在发生/未来计划；明确举例"用户说过「下周要做xx」，如果那一周还没到，就是未来计划，不能当成已经做过"，解决 LLM 把未来计划说成已发生的事或反之的时间错乱问题
  - **检索结果置信度标记**（`steps/memory.rs`）：对 `combined_score` 或 `temporal_adjusted_score` 低于 0.3 的记忆条目追加 `[需验证]` 标记，让 LLM 在生成时对低置信度记忆保持谨慎
  - **主对话路径接入 Verifier**（`steps/memory.rs` + `chat_chain.rs`）：`MemoryRetrievalStep` 注入 `ModelRouter`，检索结果 >2 条时用 `memory` 任务小模型做二分类过滤无关记忆，减少幻觉噪声；LLM 不可用时降级为全部保留
  - **生成后幻觉检测**（`steps/validation.rs` + `chat_chain.rs`）：`ValidationRunnable` 注入 `ModelRouter`，当记忆上下文非空且回复 ≥30 字符时用小模型检查回复是否与记忆矛盾或编造信息，仅记录 warning 不修改回复；超时/失败时跳过不阻塞主流程
  - **按需检索 FLARE 式**（`steps/query_rewrite.rs` + `steps/memory.rs`）：`QueryRewriteStep` 内置 `should_skip_retrieval` 启发式判断，对闲聊填充词/问候语/确认词（如"嗯"/"你好"/"好的"/"晚安"等中英日三语词）及纯标点表情输入直接跳过查询重写和记忆检索，通过 `metadata.skip_memory_retrieval` 标志通知 `MemoryRetrievalStep` 跳过整个检索步骤，省去无谓的 LLM 调用和向量检索开销。**指代词门控**（`needs_rewrite`）：LLM 查询重写仅对指代性输入（含"它/这个/刚才/上次/你说的"等指代词）触发，自包含的清晰表述直接用原文检索，进一步收敛读写路径 LLM 调用

### 上下文工程与自我进化

把"提示词"从一段会不断膨胀的文本，升级为一套有缓存纪律、有状态栏、有压缩策略、有验证门槛的工程体系：

- **人设硬约束标志（PERSONA_LOAD）**（`persona/prompt_render.rs`）：以配置文件/系统 flag 风格的全大写指令标志约束 LLM 回复，而非自然语言散文。`render_persona_flags_block` 按角色生成静态标志骨架——Vivian 19 项（`IDENTITY_VIVIAN` / `ROLE_FRIEND_NOT_SERVANT` / `PERSONALITY_TSUNDERE_HEART` / `PERSONALITY_SMART_LAZY` / `SPEECH_SHORT_CHUNKS` / `SPEECH_CASUAL_TYPING` / `REFUSE_SERVICE_SPEECH` / `REFUSE_LECTURE` / `REFUSE_ACTION_BRACKETS` 等）、Nana 19 项（`IDENTITY_NANA` / `ROLE_SISTER_NOT_SERVANT` / `PERSONALITY_CALM_COMPOSED` / `SPEECH_SLOW_GENTLE` / `SPEECH_NO_EXCLAMATION` / `RITUAL_TEA_TIME_AFTERNOON` / `REFUSE_SERVICE_SPEECH` 等），再按界面语言动态注入 `LANG_ZH_CN_ONLY` / `LANG_EN_US_ONLY` / `LANG_JA_JP_ONLY` 语言标志。全部标志包裹在 `[PERSONA_LOAD - EMBODY AS HARD RULES]` / `[END PERSONA_LOAD]` 标记中，置于 `render_character_block` 产出的 Character 块**最顶部**，利用首位效应让 LLM 将其当作硬配置遵守。标志骨架刻意不包含"无条件服从"类标志（无 `OBEY_MASTER_ALWAYS`），让角色有自己的想法与性格。工具反馈/精简人设等子路径同样注入 PERSONA_LOAD 标志以保持一致（见[增强工具系统](#增强工具系统)）
- **Agent 状态栏**（`prompt_modules.rs::build_agent_status_bar`）：以 `<agent_status>` 键值对（当前时间 / 本次对话轮数 / 最近工具调用次数 / 专注模式）作为 user-role 元消息追加在用户输入之后、紧邻生成位置，KV-cache 友好（只追加不修改前缀）。末尾附带"读数 + 操作策略"成对指令（如"同一工具多次无进展应换策略而非重试"），让模型不仅"看见"读数、更知道怎么用。所有计数由代码确定性维护，不依赖 LLM 统计
- **上下文感知压缩**（`context_compress.rs::compress_conversation_context_aware`）：在确定性压缩（Soft Trim + 原子组丢弃）之上，对将要丢弃的工具调用组用 LLM 结合"当前查询 + 已知信息"生成针对性摘要（三语），失败自动回退到确定性预览；批量压缩而非每轮压缩，减少缓存破坏
- **TTS 控制标记**（`speech/tts.rs::parse_tts_controls`）：主 LLM 可在回复文本中插入 `[THINKING]`（思考停顿，默认 500ms）/ `[PAUSE:ms]` / `[SPEED:x]` / `[EMO:xxx]` 控制标记，TTS 剥离标记并应用语速与停顿，让语音更"像人"；标记永不朗读、永不显示在聊天气泡里（已在输出格式英文模板 `output_format.en.md` 的 `[OUTPUT_FIELDS]` 中说明）
- **自我进化验证门槛**（`persona/evolution.rs`）：自我人格调整不再单次反思即生效——同一调整须在多次独立反思中被重复提出（支持次数 ≥ 2）才晋升为正式调整，防止一次偶发状态被固化为长期人格改变；覆盖层按"证据 + 时效"筛选保留（支持次数优先），而非简单截断
- **旁观二手信息可靠性**（`commands/chat.rs` / `commands/proactive.rs`）：跨角色旁观记忆统一标记 `reliability: "second_hand"`，叠加既有 `perspective=observer` 检索降权（score × 0.5），防止某一角色记错的"事实"被互相引用放大
- **工具描述"何时用/何时不用"**（`web_search_tool.rs` / `memory_tools.rs`）：为高频工具补充反例（何时不该用），降低工具误用率

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
- **Prompt 模板引擎**（`template_engine.rs` + `prompt_modules.rs`）：`section_schema()` 定义 32 个 section 的结构元数据，`build_prompt()` 采用 U 型注意力优化布局 + 分层意识模型，并带预算裁剪。静态区使用 `<static>` 标签包裹提升云端 API 缓存命中率，布局顺序为：Character（人格核心最先入脑）→ Style/Relationship → Examples（近因效应）→ Framework（技术规则，不内化）→ FORMAT SPEC（临出口提醒）→ 伪静态归位（响应决策 / 渠道指南 / 内联标签格式——内容不随轮次变化，移入静态区随 `<static>` 前缀缓存复用）；`</static>` 后输出 `__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__` 边界标记，generation 层据此把动态区切为 user 便签（历史之后注入），静态区进入 system 前缀缓存。动态区按分层意识排序：Current Mind（Belief/Goal/Attention + Working Memory + Self State + 情绪上下文）→ **记忆组**（Relevant Episode + 关系日志 + 记忆本体合并为单一 `## 你记得的事`，上移至 Mind 之后避开 U 型注意力谷底；首次见面提示与精简记忆使用规则并入记忆块）→ World Snapshot（环境上下文 / 用户在场与近期活动与观察 / 室友状态与认知印象 / 环境事件）→ 社交关系（关系认知事实 / 共享世界 / 社交状态）→ 画像组（用户事实 / 认知模型 / 动态行为合并为 `## 你对用户的了解`）→ 知识背景（Worldbook / 话题注入 / 技能 / 搜索信号 / 主动搜索上下文）→ 尾区（在场指南 / 内心反应 / 语气注入 / 快速感知引导 / 推荐工具 / 工具列表 / 用户输入最末）。每个动态 section 带裁剪优先级（rank 越大越先丢），总体超过 30KB 时按 rank 丢弃低价值段，记忆组/环境/工具/用户输入永不丢弃。`build_prompt_with_sections()` 产出 prompt + 逐 section 元数据（char_count / token_estimate / present），前端 Context Pipeline 通过 `get_prompt_section_schema` + `get_last_prompt_breakdown` 命令消费 schema 驱动可视化。功能提示词（心理洞察/信念生成/思维合成/日记生成/记忆提取）全部动态化，使用角色名变量而非硬编码"Vivian"；跨角色语音指南使用行为化描述（"你说话比她快，句子更短"）替代数值化标签（"sass=0.65"）；内心反应使用中文生成并按角色差异化（Vivian 直率吐槽 / Nana 温柔关心）
- **生成与提示词拆分**：主对话 LLM 只输出 `text / intent / tool / arguments`，表情/动作/桌宠自控指令（control_actions）由反思调用（ReflectionRunnable）统一产出，表情/动作可用列表（manifest_context）不再注入主对话 prompt。表情/动作有五类触发路径：(1) **LLM 内联标签模式**（`config.inline_expression.enabled`）——主 LLM 在文本中嵌入 `<e name="happy" dur="3000"/>` / `<m name="wave"/>` / `<s name="sticker_id"/>` 标签，流式输出时由 `InlineTagScanner` 实时扫描剥离并 emit `chat:inline_meta` 事件驱动 Live2D，零额外 LLM 开销；(2) **LLM 子调用模式**（默认）——`ExpressionMotionRunnable` 在 text 完成后独立调用 LLM 选择表情/动作；(3) **嵌入即时反应**——`analyze_emotion_instant` 命令调用 `EmbeddingEmotionClassifier`（基于 `MemoryEmbeddingProvider`，预置 14 类情绪语料 210 条，Top-K=5 余弦相似度投票），在用户消息发送瞬间（Layer 1）与 AI 文本首段完成时（Layer 2）触发即时 FACS 反应，写入 Live2D `instant` 层（优先级 1.5），反思调用完成时由 `manual` 层接管并自动清除 `instant` 层；嵌入失败时弹 toast 报错，不降级到关键词分析；(4) **用户交互即时反馈**——`apply_user_interaction` 命令根据前端检测到的 10 种交互类型（click/drag/pet 等）直接查表返回反馈，不调 LLM；(5) **自动规则触发**——`auto_expression_tick` 定时检查空闲阶段/心情持续/程序事件（`engine/auto_trigger.rs`），纯规则概率触发。反思调用产出的 `control_actions` 中的 `set_expression` / `play_motion` 接受语义名（happy / shy / wave / nod 等），后端通过 `ResourceManifest` 归一化映射到实际 model3.json Name
- **LLM 输出纯文本约束**：`text` 字段严禁包含 Markdown / 富文本渲染语法（`**粗体**` / `*斜体*` / `# 标题` / `- 列表` / `` `代码` `` / `[链接](url)` / `> 引用` / HTML 标签等），由 prompt 教导（`output_format.en.md` 英文模板的 `[OUTPUT_FIELDS]` 规范明确禁止，回复语言由 "same language as user input" 约束）+ 后处理清洗（`strip_markdown_syntax` 在 `execute_pipeline_and_build_response` 收口点和主动消息派发点统一剥离）双重保障
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

`providers/` 模块支持 10 种 `ProviderKind`：

| Provider | 协议 | 覆盖服务 |
|----------|------|---------|
| `OpenAiCompat` | OpenAI Responses API 兼容（`/responses` 端点） | DeepSeek / Qwen / Moonshot / SiliconFlow / Grok 等已实现 Responses 协议的厂商（GLM 需走 `Zhipu` 专用 provider；`supports_structured_output=false`，JSON 格式由 `json_object` 模式约束；当 input 不含 "json" 关键词时自动追加提示，满足 API 对关键词的要求，避免 400 错误） |
| `OpenAiResponses` | OpenAI 官方 Responses API | OpenAI GPT-4o / o1 / o3 系列（原生 MCP / Tool Calling / 多模态） |
| `DoubaoResponses` | 火山方舟豆包 Responses API（`/api/v3/responses`） | 豆包 250615+ 新模型（旧模型走 `OpenAiCompat`） |
| `ChatCompletions` | 标准 OpenAI Chat Completions（`/v1/chat/completions`） | OpenRouter / Groq / Mistral / Together / Ollama / vLLM / LM Studio |
| `Zhipu` | 智谱 GLM Chat Completions（`/paas/v4/chat/completions`） | 智谱 GLM-4.6 / GLM-4-Plus / GLM-4-Air / GLM-4-Flash / GLM-4V（含智谱专属 `web_search` 内置工具；`temperature` 截断到 2 位小数，`max_tokens` 按 `4v` 模型名钳制到 1024 上限） |
| `Gemini` | Google 原生 REST | Gemini（含 Google Search grounding） |
| `Anthropic` | Claude `/v1/messages`（x-api-key + anthropic-version） | Claude 系列 |
| `Wenxin` | 百度 OAuth + access_token | 文心一言 |
| `Spark` | 讯飞 WebSocket + HMAC-SHA256 | 星火大模型 |
| `Custom` | 自定义（按 Chat Completions 处理） | 任意兼容接口 |

- 支持按任务类型独立配置模型（chat / reasoning / diary / memory / consolidation / reflection / inner_monologue / vision_describe / emotion_analysis / knowledge_acquisition / translation / bystander_judge / intent_judge 等 13 个任务），每个任务拥有独立的 provider 实例（独立模型/API Key/端点/temperature/max_tokens）
- 任务 provider 失败后自动尝试主 LLM API，再尝试 providers 池，通过 `chat:route_fallback` 事件通知前端；同一 task_type 的回退事件带 120 秒冷却，避免刷屏
- 路由矩阵总开关 `enable_routing_matrix`
- **按任务分组的 LLM 并发限制**：`ModelRouter` 内置 `Semaphore` 防止后处理 LLM 调用（记忆巩固 / 内心独白 / 日记等）同时挤占主对话资源。chat_reasoning 组（chat / reasoning / vision_describe）→ 3 并发，memory_reflection 组（memory / consolidation / reflection）→ 3 并发，auxiliary 组（emotion_analysis / inner_monologue / diary / knowledge_acquisition / translation / bystander_judge / intent_judge）→ 2 并发
- **多模态（图片输入）**：六家 Provider 协议（OpenAiCompat / OpenAiResponses / DoubaoResponses / ChatCompletions / Anthropic / Gemini）均支持图片输入，文心与星火不支持。统一通过 `ChatMessage::user_with_images` 构造，`MessageImage` 含 `media_type` / `data`(base64) / `url` / `detail` 四字段，由 `ai.enable_vision` 开关 + `ai.image_detail`（auto/low/high）配置控制
- **视觉能力自适应探测**：应用不假设用户填入的 API 支持视觉，首次发图前用 16×16 透明 PNG 探测目标模型是否接受图片输入（部分服务商如豆包要求最小 14×14），结果按 model 名缓存。探测路径与 `vision_describe` 任务实际路由一致（路由矩阵启用时优先任务 provider，否则主 LLM API），绕过 `query_with_fallback` 避免 fallback 掩盖真实结果。`NotSupported` 时拦截发图并 emit 详细 error toast（含原因 + 配置指引）；`save_config` / `reload_config` 时自动清空缓存，确保用户换模型后重新探测
- 代理透传、客户端缓存热重载
- **工作智能体模型热切换（reasoning 覆盖）**：除路由矩阵外，可在设置 → LLM 页签「工作智能体模型」区预设多个模型，编程页切换后经 `select_work_model` 构建 provider 作为 `reasoning` 任务的运行时覆盖，优先级高于路由矩阵；`active_work_model` 持久化并在重启 / 保存配置触发 reload 后自动恢复
- **工作智能体请求省略 temperature**：工作智能体（编程）请求体统一不携带 `temperature` 字段（交服务端默认）——编程任务对确定性要求高，且推理模型对非默认温度敏感（OpenAI o 系列仅接受默认值、reasoner 忽略该参数），故移除表单中的温度配置；`ProviderBase::strip_temperature` 按各厂商路径（顶层 / Gemini `generationConfig` / 星火 `parameter.chat`）精准移除，`set_work_model_override` 与 `build_reasoning_override` 构建覆盖 provider 时统一启用
- **厂商预设协议选择与官网跳转**：设置 → LLM 页签厂商预设卡片下，支持多协议厂商可切换 API 协议——Responses API / Chat Completions / Anthropic 兼容（`{base}/anthropic/v1/messages`）三协议族自由组合：DeepSeek/Qwen/Moonshot/Grok/SiliconFlow/MiMo/MiniMax 同时支持 Responses ⇄ Chat Completions；GLM 支持智谱原生 ⇄ OpenAI 兼容 ⇄ Anthropic；豆包支持 Responses ⇄ 火山 Responses ⇄ Chat Completions ⇄ Anthropic；DeepSeek/GLM/MiniMax/MiMo 另提供 Anthropic 兼容入口（`x-api-key` 鉴权）。切换只覆盖 provider_type 与 endpoint、不动 model 与密钥；多数厂商提供「获取 API Key」按钮（`consoleUrl`），经 shell 打开官方控制台领取密钥
- **主 LLM max_tokens 按厂商建议默认**：切换主配置厂商预设时，`max_tokens` 自动填入该厂商建议单次输出上限（`suggestedMaxTokens`：Claude 64000 / Gemini 65536 / OpenAI·Qwen·GLM·Grok 32768 / Moonshot·豆包·Mistral·MiniMax·MiMo 16384 / DeepSeek·SiliconFlow·Groq·Together·OpenRouter·文心·本地 8192），替代一刀切的 2048 聊天默认（对代码/长回复过小），仍可手动调整
- **LLM API 一键检测**：设置 → LLM 页签提供「一键检测」按钮，对主 LLM 配置 + 全部路由任务逐条发送最小请求（`"ping"`，temperature=0、max_tokens=16）验证端点可达 / 鉴权有效 / 模型存在。经 `test_llm_route` 命令构建临时"裸" provider（`create_probe_provider`，无 system instructions，最小化探测 token），与运行时共用协议分发与代理分流链路；检测使用当前界面值（含未保存修改），逐条显示结果（可用 / 失败原因 / 未配置），路由折叠标题的模型名颜色同步联动

### 增强工具系统

`tools/` 模块提供 76+ 内置工具 + 1 个元工具（ToolSearchTool），覆盖 14 个类别：

| 类别 | 工具示例 |
|------|---------|
| 文件操作 | ReadFile / WriteFile / EditFile / ListDirectory / SearchFiles / Grep |
| 编程智能体 | read_file / write_file / edit_file / run_command / grep_search / list_dir / run_workflow / lsp_query（读改跑闭环 + 多步编排 + 语义查询，供编程智能体使用） |
| 系统操作 | GetRunningProcesses / OpenApplication / CloseApplication / TakeScreenshot |
| 扩展系统 | GetClipboardText / SetClipboard / OpenUrl / GetActiveWindow / GetSystemInfo |
| 浏览器 | browser_snapshot / browser_click / browser_type / browser_navigate / browser_eval_js / browser_task_tab（经精简 Chrome 扩展桥在受控/隔离标签页执行，保留登录态；读操作放行、改动操作需确认） |
| 记忆 | SaveMemory / SearchMemory / ClearMemory / ReadMemory / LogDailyDiary / ListRecentDiaries |
| 桌宠 | SetExpression / PlayMotion / TriggerIdleAction / SetBehaviorMode |
| 待办 | AddTodo / ListTodo / CompleteTodo / UpdateTodo / DeleteTodo |
| 桌宠行为 | SetPetState / PlayAnimation / SpeakBubble / FollowCursor / SetMood |
| 关系 | GetRelationshipStatus / ListMilestones / RecordMilestone |
| 媒体 | media_play_pause / media_next / media_previous / media_volume_up / media_mute |
| 感知 | GetCursorPosition / GetIdleState / GetForegroundAppContext / OcrScreenText / GetWindowTree |
| 输入控制 | MoveMouse / ClickMouse / DragMouse / ScrollMouse / PressKey / Hotkey / TypeText |
| 壁纸 | WallpaperList / WallpaperSet / WallpaperPause / WallpaperStop（Wallpaper Engine 集成） |
| 笔记本 | create_notebook / list_notebooks / update_notebook / share_notebook / create_html_note / read_file（卡片风格 + 完整 HTML 笔记生成、枚举已有笔记、按路径读文件、微信链接卡片分享） |
| 技能 | use_skill（按名称激活技能，返回完整正文指引供 LLM 遵循，正文不常驻上下文）、create_skill（智能体自主沉淀方法论为技能，写入即注册） |
| 自建工具 | create_tool（智能体把「PowerShell 脚本 + JSON Schema」封装为可执行新工具，stdin 收 JSON 参数 / stdout 出结果，创建经预览卡片授权后立即注册、跨会话持久） |
| 后台任务/编排 | run_job / manage_job（后台命令执行与轮询）、spawn_subagent / subagent_control（子代理委派/查询/取消，report 回传）、run_workflow（多步编排 + 并行扇出）、delegate_to_work_agent / get_work_status（派活给工作智能体，无工作区也可工作）、notify_companion（工作智能体阶段成果 → 陪伴角色人设化播报） |
| MCP | mcp__{server_id}__{tool_name}（外部 MCP server 动态注册） |

工具行为要点：`GetWindowInfoTool.get_window_info` 返回真实窗口信息（`{x, y, width, height, visible, always_on_top}`），而非模拟数据；`SetPetState` 的 `state` 参数取值限于 `["idle","active","sleeping","thinking","listening"]` 枚举，非法值返回包含允许值列表的错误提示；`SetMood` 的 `mood` 参数取值限于 `["happy","calm","sad","excited","angry","neutral"]` 枚举；`PlayAnimation` 的 `animation` 为自由格式（模型级动作名），仅做非空校验。截屏路径校验：`capture_screen_region` 的 `save_path` 通过 `is_path_safe` 检查路径穿越，`take_screenshot` 限定保存到 `screenshots` 白名单目录。

- **执行管线**：查找 → 沙箱安全检查 → 输入验证 → 缓存检查 → 权限检查 → 执行（带超时）→ 缓存写入。所有 PowerShell / 子进程调用经 `tokio::task::spawn_blocking` 隔离到阻塞线程池，避免同步等待占满 async 运行时 worker；PowerShell 脚本统一注入 `[Console]::OutputEncoding = UTF8` 前缀，杜绝 GBK 控制台输出乱码
- **工具可见性分层**（`ToolVisibility`）：三级控制工具在 LLM 上下文中的展示粒度，减少 token 开销。`Always`（完整 schema 注入，核心高频工具）、`Lazy`（仅名称 + 一行描述，完整 schema 通过 `tool_search` 按需加载，Media/Mcp 类默认此层级）、`Deferred`（仅名称出现在 `<available-deferred-tools>` 块中，should_defer=true 的工具默认此层级）。`resolve_visibility()` 根据 `ToolCategory` + `should_defer()` + `always_load()` 自动推断层级，个别工具可通过 `Tool::visibility_tier()` 覆盖；笔记/HTML/文件类长尾工具（`create_html_note`/`read_file`/`list_notebooks` 等）统一 `should_defer=true`，避免常驻占用上下文
- **场景化工具筛选**：根据情绪/关系阶段自动切换工具暴露子集（低信任禁用系统控制、情绪低落禁用 Web/Media、专注模式保留 Memory + 必要 System）
- **多步编排**：主对话工具循环由 `ToolCallManager`（`tool_call_manager.rs`）驱动——解析 AI 响应中的工具调用，只读工具累积并行批次 `join_all` 并发执行、写工具或有 `${result}`/`${step.N.result}` 依赖的工具先 flush 再串行、非阻塞工具 spawn 后立即继续，多轮迭代直到无工具调用（上限保护 + 渠道感知 relay prompt）。`ToolChainer`（`chainer.rs`）仅保留顺序链：`ToolChain` 声明式步骤序列 + 失败策略（Stop/Skip/Continue）+ `${result}` 参数注入 + 意图识别器（`IntentRecognizer`）；已删除历史遗留的 MultiStepExecutor 死代码簇
- **技能工具 use_skill**（`tools/builtin/skill_tools.rs`）：按名称激活技能并返回其完整正文指引（`"技能「X」已激活，请按以下指引行动：…"`），供 LLM 遵循。技能目录（`<用户数据目录>/skills` 的 `*.md`）默认只注入"名称+描述"到 prompt 的 `## 可用技能` 段落，正文按需激活，控制 token 开销；未命中时返回附带可用技能列表的错误提示，方便 LLM 纠正名称重试。限定当前角色可见（全局 + 该角色 scoped），只读无副作用（`is_read_only=true`）
- **技能沉淀工具 create_skill（自进化闭环写入侧）**：LLM 总结出一套值得复用的做法时，把 `(名称, 描述, 正文)` 以 front-matter Markdown 写入 `<用户数据目录>/skills/<name>.md` 并**立即注册**（不等 30s 热重载，之后 use_skill 可直接激活）。防护：技能名白名单（字母/数字/`_`/`-`/中文，≤64 字符）、内置 `*_style` 预设不可覆盖（`BUILTIN_SKILL_NAMES`）、description 单行化保证 front-matter 合法；`risk()=FsWrite` 走审批矩阵。管理面板 `list_skills` 不展示内置风格预设
- **工具构建工具 create_tool（能力自进化执行侧）**：把「PowerShell 脚本 + JSON Schema」封装为可执行新工具（[`custom_tools.rs`](file:///g:/vivian-rs/src-tauri/src/tools/custom_tools.rs)）——调用参数 JSON 写入脚本 stdin（`$args = [Console]::In.ReadToEnd() | ConvertFrom-Json`），stdout 作为结果，持久化到 `<用户数据目录>/tools/<name>.json` 并**注册即生效**（注册表实时读取，同一 agent 循环内可立即调用；重启后启动装载 + 30s 热重载）。**创建经预览卡片授权**：`risk()=Shell` + `check_permissions` 显式 ask + executor 能力进化门（宿主自动放行回调不绕过），卡片展示名称/描述/参数 schema/完整脚本/权限等级/动态注入等级，用户三态决定；每次调用仍走 Shell 级三态确认。动态注入等级（`deferred` 参数）自选：始终注入完整 schema 或仅列名经 `tool_search` 按需加载（省 token）；`ToolSearchTool` 改持 `Weak<ToolSystem>` 从活注册表搜索，运行时注册的延迟工具可被搜到。数量无上限
- **工具级开关（`config.tools.disabled_tools`）**：设置 → 工具页签可逐工具启用/禁用（两张卡片网格 + 右侧胶囊开关，附搜索框与启用计数）。禁用的工具**不注入 LLM**（prompt 文本与 FC tools 均过滤，`list_tools_for_scene` / `get_tool_schemas` 统一处理）、**执行入口直接拒绝**（`execute_tool_use` 早退防御 LLM 幻觉调用）；`list_tools` 命令仍返回全部工具供界面重新启用。开关按 `ToolCategory`（文件/网络/系统/记忆/媒体/桌面/MCP）分组收纳为可折叠抽屉，保存后经 `save_config` 热同步到 `ToolSystem` 即时生效
- **自进化工具前端标识（`Tool::is_custom`）**：自建工具（`DynamicTool`）在设置 → 工具页签卡片以特殊样式区分——虚线主色边框 + 淡紫渐变底 + Sparkles 星标 + 「自进化」徽标（中/英/日三语），与内置工具的实线卡片一眼可辨；`list_tools` 命令返回 `is_custom` 字段驱动
- **执行参数「-1 = 无限」**：设置 → 工具页「执行参数」区的迭代/轮次上限（文本路径最大迭代、原生 FC 最大轮次、编程智能体最大轮次）填 `-1` 表示不设上限。后端以哨兵值 `0` 存储并消费：`generation.rs` FC 循环与 `tool_call_manager.rs` 反馈循环解为 `usize::MAX`（不再钳到 4 轮）、`coding_agent.rs` 编程循环跳过预算检查与 2/3·5/6 软预算提醒（防溢出）；循环仍由 LLM 停止调用工具 / `goal_completed` / 停滞检测 / 收益递减检测自然终止，不会失控
- **工具反馈路径三语化 + PERSONA_LOAD**（`tool_call_manager.rs::build_feedback_prompt`）：工具执行结果反馈提示词按界面语言三语化（`## 工具执行结果` / `## Tool Execution Results` / `## ツール実行結果`），顶部注入 `build_tool_minimal_identity`（携带 PERSONA_LOAD 标志 + 角色精简人设）+ `tool_minimal_output_format`（按界面语言约束输出语言），末尾按角色区分人设语气红线（Nana 温柔从容 / Vivian 傲娇嘴硬）并禁止客服/助手语气，与主对话保持一致
- **浏览器可信来源白名单**（`tools/trusted_origins.rs`）：对高信任站点做规范化精确/通配匹配，命中时 `browser_navigate` 直接放行免确认，把信任边界收敛到白名单站点。两级合并——内置默认（`BUILTIN`：github/bilibili/zhihu/baidu/bing/google/duckduckgo/wikipedia/doubao/taobao/jd）+ 用户配置 `<用户数据目录>/trusted_origins.json`（`{"origins": [...]}`，支持 `example.com` 子域通配 / `*.example.com` 显式通配 / `exact:example.com` 精确匹配）。首次运行自动生成带 `_hint` 说明的模板；文件变更通过 mtime 检测自动热重载，无需重启
- **可观测性**：`ToolObservability` + `ToolMetrics` + `ToolCallRecord`
- **沙箱**：`ToolRiskLevel` / `ToolSafetyProfile` / `ProtectionMode` 三层安全模型。路径参数递归遍历 JSON 参数树提取所有可疑路径值，不再依赖固定参数名；危险命令检测覆盖 `rm -rf` / `rm -fr` / `rm -r -f` / `--recursive --force` / `format c:` / `del /f /s` 等多种变体组合；`normalize_path`（`types.rs`）真正解析父目录分量（栈 `pop()` 抵消 `..`，`/a/b/../c` 归一为 `/a/c`），与 `sandbox.rs` 的 `normalize_path_buf` 对齐，避免简单过滤 `..` 导致权限评估错路径；无内置安全档案的工具经通用检查（危险命令 / 路径穿越）后放行，风险分级交由下游权限系统（access_level × risk 矩阵 + always 规则 + 用户确认）统一管理
- **风险等级申报**：每个工具通过 `risk()` 声明 `ToolRiskTier`，默认 `Safe`。输入控制类 12 个工具（MoveMouse / ClickMouse / DragMouse / ScrollMouse / PressKey / Hotkey / TypeText 等）与媒体控制 6 个工具、`take_screenshot` 申报 `InputControl`；`weather` 申报 `Network`。权限矩阵 Deny 时返回提示并引导用户在设置中提升访问级别（如 InputControl 需 FullControl）；always 规则优先级统一为 `always_deny > always_ask > always_allow`，`always_allow` 仍优先于矩阵 Ask 判定
- **文件操作安全策略**：6 个文件工具（read_file / write_file / edit_file / list_directory / search_files / grep）调用 `tools::sandbox::is_path_safe` 进行路径穿越校验（拒绝 `../` / `..\` / 绝对路径越界）；`is_sensitive_path` 拒绝写入系统敏感目录（Windows / Program Files / System32 等）；写入操作前再校验目标路径合法性。文件操作强制递归深度限制（最大 10 层）、结果条数上限（grep 500 / list_directory 5000 / search_files 1000）、`read_file` 使用 `BufReader` 按行读取并支持 offset/limit 跳过，grep 正则使用 `Lazy<Regex>` 预编译复用，所有阻塞 IO 操作均通过 `tokio::task::spawn_blocking` 隔离到线程池避免阻塞 async 运行时。**编码自适应读取**：`read_file` 与 `grep` 先采样前 8KB，经 `chardetng` 检测编码（UTF-8 走快速路径），再用 `encoding_rs` 逐行解码（`read_until(b'\n')` 按字节分行，0x0A 不会作为 GBK/UTF-8 尾字节出现，行切分安全），GBK 等非 UTF-8 文件不再返回乱码或中断；grep 遇到非 UTF-8 行时按检测编码解码后继续匹配，而非在首个非法行处终止
- **Shell 执行禁用**：`brain::computer_control::execute_shell` 直接返回错误，防止 LLM 通过 shell 命令实现 RCE；`computer_control::open_app` 使用白名单映射表（app_map），未注册的应用名拒绝启动。`open_application` 工具内置 16 种危险程序黑名单（cmd.exe / powershell.exe / wscript.exe / rundll32.exe / regedit.exe 等），路径形式输入做文件名校验，纯应用名通过 where.exe/PATH/Program Files/Start Menu/UWP 五级解析链查找，整个解析过程通过 `spawn_blocking` 异步执行；UWP 解析路径对 AppID 实施 `is_safe_appid` 白名单校验（仅允许字母/数字/`.`/`_`/`-`/`!`），防止 PowerShell 注入；打开网址请使用 `open_url`（仅允许 http/https 协议，拒绝 file:///javascript:/data: 等危险协议）；剪贴板操作使用 `clip.exe` 通过 stdin 管道写入，不拼接 PowerShell 命令避免命令注入
- **GPT-SoVITS 服务安全**：服务状态通过 `Arc<RwLock<ServiceState>>` 缓存实时更新，HTTP Client 复用连接池；端口占用杀进程时精确解析 netstat 输出匹配目标端口，避免误杀无辜进程
- **用户确认**：权限矩阵判定 Ask 的操作通过 `tool:confirmation_request` 事件发起三态确认（拒绝 / 放行一次 / 始终允许），前端在 toast 子窗口渲染三按钮确认卡片，30 秒倒计时无操作自动拒绝，pending 请求带 5 分钟 TTL 自动清理避免内存泄漏。「始终允许」分两种范围：`open_application` 写入应用信任列表（`%APPDATA%\vivian\trusted_apps.json`，持久生效），其余工具写入会话级放行列表（应用重启后重置）；命中信任列表或会话放行的工具直接执行、不弹确认。高危工具默认始终需要确认：`CONFIRMATION_REQUIRED_TOOLS` 列表（13 个工具——read_file / write_file / edit_file / list_directory / search_files / grep / capture_screen_region / ocr_screen_text / get_window_tree / take_screenshot / delete_memory / cancel_scheduled / delete_todo）跳过矩阵直接走 Ask 确认流，即使权限系统判定 Allow；`delete_memory` 另有 `confirm` 布尔参数，为 true 时走确认流程、为 false 时按矩阵正常判定
- **原生 function calling**：当服务商支持时走结构化 tools 字段路径，不占 prompt token、调用更准确
- **工具类型感知的 relay prompt**：原生FC路径中，LLM停止调用工具后按工具类型分流。检索类工具（`web_search`/`search_memory`/`recall_by_date_time`等）注入 relay prompt 确保 LLM 转述关键内容，避免只回"好的"不转述；动作类工具（`save_memory`/`open_url`/`set_presence_state`等）直接使用 LLM 已有回复，避免不必要的 relay 调用导致吞消息或生成错误道歉
- **搜索失败提示优化**：`web_search` 工具在无结果时返回明确提示，说明可能原因（网络/代理不可用或查询无匹配），并建议 LLM 基于已有知识回答而非反复调用同一查询，避免无效重试浪费 token
- **差异化默认结果数**：`web_search` 的默认返回条数不再固定为 5，而是按调用方智能体区分——聊天智能体默认 10 条，工作（编程）智能体默认 15 条。优先级为「模型显式传参 > 设置面板配置值 > 差异化默认」；设置 → 网络页签「结果数」填 0 表示自动（默认），填 1-20 则固定覆盖
- **认知知识需求驱动的主动搜索**（`pipeline/steps/web_context.rs`）：在 LLM 生成前基于多维认知知识需求评估（Epistemic Assessment）驱动主动搜索，替代单一置信度阈值。FastSemantic 阶段同步计算四维评分（`semantic_clarity`/`factual_dependence`/`temporal_sensitivity`/`interpretation_risk`/`knowledge_gap`），规则映射为 `KnowledgeDecision`（`NoSearch`/`SearchOptional`/`SearchPreferred`/`SearchRequired`）。`SearchRequired`/`SearchPreferred` 时自动触发 Web Search，搜索结果作为 `proactive_search_section` 注入 prompt，附带"不要假装本来就知道"的指导；同时注入 `epistemic_signals_section` 认知信号段落，让 LLM 感知是否需要搜索，辅助自主调用 `web_search` 工具。与 LLM function calling 路径互补：预搜索在生成前完成，LLM 生成时仍可自主调用 `web_search` 做进一步搜索
- **代理降级直连重试**：`WebSearcher` 在配置了代理但所有搜索引擎无结果时，自动尝试直连重试一次，避免代理不可用导致搜索完全瘫痪
- **渠道感知的回复长度控制**：relay prompt / goal_completed prompt / round_limit prompt 均按消息渠道分流——气泡渠道（direct/proactive）要求一两句话精简转述，微信渠道（wechat）允许详细展开。通过 `channel` 参数传入 `call_llm_native_fc` / `call_llm_native_fc_stream`，在 6 个注入点（非流式 + 流式各 3 处）调用渠道感知函数
- **MCP 原生集成**（`mcp.rs`）：手写 JSON-RPC 2.0 over stdio 客户端（无外部 SDK），启动时自动连接已配置的 MCP server，发现工具后注册到 ToolSystem 与内置工具无差别调度；外部工具默认延迟加载 + 权限 `ask`（不可信）；配置持久化于 `%APPDATA%\Vivian\mcp\servers.json`，设置窗口「工具」页签提供可视化管理；初始化失败时通过 `new_disabled()` 降级为空实现保证主流程不阻塞；配置写入使用 `Mutex<()>` 锁防止并发保存竞态；MCP 子进程 stderr 通过异步任务捕获并以 debug 级别记录日志，便于排查外部工具问题
- **anti-use-case 写法**：每个 Tool trait 实现 `anti_use_cases()` 方法描述"不适用场景"，与 `description` 一起注入 prompt 帮助 LLM 避免误用工具
- **Hook 系统**（`hooks/`）：PreToolUse / PostToolUse 可扩展拦截点。JSON 配置文件（全局 `%APPDATA%\Vivian\hooks.json` + 项目级）定义匹配规则（Regex）和外部脚本命令，stdin/stdout JSON 协议，fail-open（超时/异常/无效 JSON 默认 allow），错误以 `tracing::warn!` 记录而非静默吞错
- **后台任务回流陪伴**（`brain/task_service.rs` + `pipeline/steps/prompt.rs`）：陪伴对话可直接用工作侧能力派活——`run_job`/`manage_job`（后台命令）、`spawn_subagent`/`subagent_control`（子代理委派/查询/取消）、`run_workflow`（多步编排）、`delegate_to_work_agent`/`get_work_status`（派给工作智能体）。任务完成后，报告经「后台任务」动态段注入**下一轮陪伴对话**（含主动 tick）：运行中任务显示状态，刚完成未汇报的显示报告并引导角色主动向用户汇报；**每份报告只注入一次**（注入即消费标记，对齐 deepseek-harness jobs 的 next-step inbox 语义），成功结束但模型未回报时自动用末尾步骤生成兜底报告。工具结果在陪伴反馈历史中同样走头尾裁剪（与编程侧统一 `prune_head_tail`）

### 技能系统（skills）

`skills/` 模块提供可复用微技能的注册与组织——技能是**作用域内可注册、可卸载**的 `(名称, 描述, 内容)` 三元组，本身不携带执行逻辑，只承载"该做什么/怎么做"的提示词片段，由 prompt 注入与 `use_skill` 工具消费：

- **内置技能来源**：现有风格预设（`default_style` / `lively_style` / `healing_style` / `focused_style` / `sweet_style`，正文取自 `load_style_preset`）作为全局技能种子
- **目录化技能**（`skills/mod.rs::load_default_dir`）：启动时从 `<用户数据目录>/skills` 装载 `*.md` 技能文件（目录缺失自动创建）。文件支持可选 front-matter 头（`name:` / `description:`），正文紧随其后；无 front-matter 时以文件名（去扩展名）为技能名、正文首行为描述。同名技能原子替换（先移除旧再注册），因此热加载只需在目录变更后重复装载
- **热加载**（`spawn_hot_reload`）：后台任务每 30 秒对比目录指纹（文件名 + mtime），变更时自动重载并记录日志，无需重启。不引入 notify 等监听依赖，轮询 stat 对比足够轻量
- **作用域隔离**：`Skill::global`（全局，所有角色可见）/ `Skill::scoped(char_id)`（仅指定角色可见），`list_for(char_id)` 返回全局 + 该角色 scoped 的并集
- **Prompt 注入**（`pipeline/steps/prompt.rs`）：从全局 ctx 取 `SkillService`，把当前角色可见技能的"名称+描述"渲染为 `## 可用技能` 段落注入 prompt，并提示"调用 `use_skill` 工具获取完整指引后照做"——技能正文不常驻上下文
- **按需激活**（`tools/builtin/skill_tools.rs` 的 `use_skill`）：LLM 判断某项技能适用时调用 `use_skill`，返回技能完整正文（`"技能「X」已激活，请按以下指引行动：…"`）；未命中时返回附带可用技能列表的错误提示。只读无副作用，无需确认
- **自主沉淀 create_skill**（`skills/mod.rs::BUILTIN_SKILL_NAMES` 防覆盖名单 + `tools/builtin/skill_tools.rs`）：LLM 总结出值得复用的做法时调用 `create_skill`（名称/描述/正文），front-matter Markdown 写入技能目录并**立即注册**，30 秒热重载幂等替换不冲突；内置 `*_style` 出厂技能不可覆盖。技能系统因此从"启动装载 + 热加载"升级为"智能体可自主写入"的自进化闭环，`use_skill` / `create_skill` 同时纳入工作智能体工具白名单（进化事件由工作智能体执行）
- **插件技能**（`plugins.rs`）：插件装载的技能以 `vivian_*` 命名空间前缀注册进同一 `SkillService`，与用户技能隔离不冲突
- **注册管理**：`register` 返回可逆 `Disposer`（drop/作用域卸载时自动移除）；`replace_or_register` 同名唯一原子替换（供插件装载/热重载复用）

### 编程智能体（Coding Agent）

编程智能体以会话式 agent-loop 形态运行，让桌宠角色具备**结对编程**能力：在记忆观察器的「工作」页签选择项目目录后，用自然语言让角色阅读、修改、构建并调试代码。角色（Vivian / Nana）会按人设语气工作，但代码与结论保持严谨。

- **编程工具闭环**（`tools/builtin/coding_tools.rs`）：`write_file`（UTF-8 写入、自动建父目录）/ `edit_file`（精确字符串替换，旧串须文件中唯一或显式 `replace_all`，防误改；成功后内嵌 unified diff——每处替换一行 hunk 含上下文、相邻替换自动合并、6 hunk / 100 行 / 4000 字符体积受控，供前端 diff 渲染与 LLM 感知自身改动）/ `run_command`（PowerShell 非交互执行，120s 超时、输出按 8000 字符截断、破坏性命令黑名单直接拒绝、`CREATE_NO_WINDOW` 隐藏控制台）/ `grep_search`（递归内容搜索，跳过 .git/node_modules/target 与二进制）/ `list_dir`（树状目录，深度受限），另含编排与语义能力——`run_workflow`（一次提交多步工具脚本，连续 `parallel:true` 步骤扇出并发执行）/ `lsp_query`（经语言服务器做定义/引用/实现/hover 语义查询）/ `notify_companion`（阶段成果发给陪伴人格播报）。全部经 `sandbox::is_path_safe` 校验并申报风险分级（FsRead/FsWrite/Shell），写/执行类自动进入审批矩阵；**工作区写入免确认**——执行器按工具上下文注册已授权工作目录，`workspace_write` / `full_access` 会话在工作区内写文件直接放行（沙箱路径校验 + read_only 权限矩阵 + 破坏性命令黑名单仍是兜底防线），`read_only` 会话照旧拒绝写入；编程会话的工具执行注入沙箱确认回调，跳过沙箱层"首次/前 N 次使用"确认（该层在会话式场景下无弹窗回调，只会直接报错拦截）
- **会话式 agent 循环**（`brain/coding_agent.rs`）：用户消息 → LLM（原生 function calling，仅暴露编程白名单工具）→ 工具逐个顺序执行并经 `execute_tool_use` 走主对话同一套沙箱/守卫 → 结果按 `tool_call_id` 关联回填历史 → 循环直到 LLM 产出纯文本回复。轮次预算由 `tools.max_coding_rounds` 控制（默认 48，设置-工具可调）：循环内置软预算提醒（用到 2/3、5/6 时注入收尾提示）、停滞检测（相同工具+参数连续重复 ≥3 次 / 同一工具连续失败且错误摘要相同 ≥3 次时注入"重新分析"），本轮回有实质进展（成功写/改/执行）时耗尽自动续轮一次（+base/3，封顶 96），耗尽且无进展则硬停止并弹出去向选择条（继续 / 补充说明后继续 / 停止）。历史消息裁剪 60 条 + 工具结果头尾裁剪（超 6000 字符保留头部 2/3 + 尾部 1/6，中段折叠标记，尾部的退出码/报错/diff 收尾不丢失）控制上下文体积；可随时取消
- **收益递减检测**（`brain/budget.rs::OutputBudgetTracker`）：防"空转"——每轮循环结束后按本轮 LLM 输出 token（无 usage 上报时按工具结果摘要字符近似）+ 实质进展标志记录产出，连续 3 轮低产出且无实质进展（写/改/执行类工具成功）判定收益递减，提前提示收尾停机（`record` / `record_chars` 双模式适配有/无 usage 场景），与 `DoomLoopTracker` 的"完全相同的重复调用"互补——前者抓短暂回复空转，后者抓同签名重复
- **流式输出与思维链**：LLM 文本逐字转发（`coding:chunk` 打字机）、推理链增量（`coding:thinking_chunk`）在「思考占位」内渐进展开灰色推理文本，避免长时间静默
- **会话摘要入库记忆**：每轮 agent loop 结束后，把本轮对话（用户请求 → 工具调用 → 助手回复）经 LLM 摘要（memory 路由，失败退化为规则摘要）写入会话所属角色的记忆库（ShortTerm，tags `coding_session`/`work`，metadata 带 `source=session_id` / 工作目录 / 说话人），让角色在主对话中可回忆自己做过的工作
- **实时事件流**：`coding:*` 系列事件（user_message / assistant_message / tool_call / tool_result / turn_done / error / thinking / chunk / thinking_chunk）由 Rust 广播，前端「编程」页签实时渲染聊天流与工具卡片。工具卡片按工具类型紧凑展示（grep_search → `query · 找到 N 处匹配`、run_command → `命令 · ✓成功/✕退出码`、list_dir → `N 个条目`），卡片头部带「工具调用」徽标与明确状态（`运行中… / ✓ 已完成 / ✕ 失败`），关键参数/结果要点默认可见、完整 IN/OUT 详情可展开、空参数/空结果不占版面；`edit_file` 卡片展示后端生成的 unified diff（红绿行着色 + `+N −M` 统计条）；轮次预算耗尽硬停止时在输入区弹出去向选择条（展示本轮进展 + 继续 / 补充说明后继续 / 停止）
- **单轮工作过程分组**：连续的工具卡片聚为一个可折叠「工作过程」区块——运行时展开实时观察，总结文本（assistant 回复）出现的瞬间自动收成一行摘要（`N 步 · M 个文件 · 耗时` + `✓ 已完成 / 进行中…` 状态与失败计数），之后可自由开合；折叠/展开用 CSS grid `0fr→1fr` 高度过渡 + 内容淡入 + 箭头旋转，250ms 短促丝滑，不打断阅读总结
- **会话持久化**：`%APPDATA%\Vivian\coding_sessions.json`，保留最近 30 个会话，重启后可恢复；每会话独立工作目录 + 标题 + 完整消息历史
- **会话级运行时配置**：每会话独立持久化 `permission`（read_only / workspace_write / full_access）、`model_id`（工作智能体模型，与路由热切换同步）、`reasoning_level`（low / medium / high，low 关闭思维链）。权限经 `ToolUseContext.access_level` 会话级覆盖接入工具审批矩阵（read_only 只读、workspace_write 文件写入、full_access 完全控制），模型切换复用 `select_work_model` 运行时热切换
- **工作模型预设源**：候选模型在设置 → LLM 页签「工作智能体模型」区维护（每项含别名/服务商/模型/端点/密钥，支持增删改，删除为垃圾桶图标按钮；供应商下拉复用完整厂商预设列表、别名可改。不再提供「设为当前工作模型」按钮——当前选中统一在编程页模型下拉切换，编程页切换经 `select_work_model` 同步 `active_work_model`），列表经 `work_models` 持久化、当前选中经 `active_work_model` 持久化，供编程页模型下拉回显与 `ModelRouter` 覆盖恢复。前端不再暴露 temperature 与单次输出预算——工作智能体请求统一省略 `temperature`（服务端默认，推理模型兼容），`max_tokens` 由后端按服务商分级默认（`work_model_default_max_tokens`：Claude 64000 / Gemini 65536 / OpenAI·Qwen·GLM·Grok 32768 / Moonshot·豆包·Mistral 16384 / DeepSeek·SiliconFlow·Groq·Together·OpenRouter·文心·星火·本地 8192 / 未知 8192），给足编程输出能力又不触发各家硬上限
- **角色化系统提示**：system prompt 按 `char_id` 注入人设（Vivian 傲娇吐槽 / Nana 温柔友好），限定 Windows + PowerShell 环境与工作目录沙箱边界，强调"先看（list_dir/grep/read）再动手、局部用 edit_file、改后跑命令验证"。**能力进化角色定位**：白名单含进化工具（create_skill / use_skill / create_tool），system prompt 明确"你也是能力进化事件的执行主体"并引导用法——任务中总结出可复用流程用 create_skill 沉淀、缺少可执行原语用 create_tool 构建（预览卡片授权）
- **编程页 UI（Codex 布局 + 手账风格）**：入口为 [`CodeAgentPageNew.tsx`](file:///g:/vivian-rs/src/components/mind-inspector/pages/CodeAgentPageNew.tsx)，左栏为会话/工作区管理（会话按工作区分组或单列表、支持搜索、最近更新/手动排序、工作区固定不可改），中栏为对话流 + 底部输入卡片（`/` 斜杠命令、图片多模态草稿、权限/模型/推理选择），右栏为检查器（概览 / 轨迹 / 内嵌终端，可整体收纳）；左右侧边栏均可拖拽调整宽度。消息按角色区分渲染：助手消息行内 Markdown 加粗（`**text**`）生效，文件类工具（read/write/edit）以手账风格代码块 + unified diff 高亮展示，`edit_file` 卡片折叠时摘要行即显示 `+N −M` 改动量
- **斜杠命令**：输入 `/` 弹出命令菜单（按命令名/标签字母模糊筛选，↑↓ + Enter 选择，选中命令插入输入框补参数；命令名后输入空格自动收起菜单）。后端 `handle_slash_command` 拦截分发 6 个命令——`/goal`（查看/设置/清除会话目标，注入 system prompt）/ `/plan`（切换计划模式；`/plan approve` 把最近方案固化为已批准执行依据，`/plan off` 退出）/ `/compact`（较早历史交 LLM 压缩成摘要替换进上下文）/ `/permission`（查看/切换权限预设）/ `/feedback`（记录反馈）/ `/export`（导出会话为 Markdown）。命令结果以消息流展示，不消耗 agent loop 轮次
- **@-mention 文件引用**：输入框输入 `@` 弹出工作目录文件选择器（标签/路径模糊筛选，↑↓+Enter 选中），选中插入 `@路径`；发送时后端读取所引文件内容注入上下文（沙箱校验、截断上限），消息气泡以文件图标展示引用，读取失败显式标注错误
- **产物面板与消息操作**：会话概览侧展示「产物」卡片（write/edit 成功写入的文件清单，相对路径 + 全文定位）；每条消息 hover 提供复制 / 有帮助 / 没帮助（消息级评分）/ 从此处派生新会话（fork，复制该消息为止的历史为独立会话）
- **常驻目标/计划条（GoalBar）**：会话顶部常驻条展示目标（可内联编辑/清除）与计划模式状态（未批准时提供「批准方案」按钮，固化为执行依据；可「退出计划」）
- **工作流可视化卡片**：`run_workflow` 的工具结果渲染为可视化卡片——名称/成败计数/进度条 + 步骤按「顺序 / 并行组」分组（并行组带标签），每步显示序号/工具/✓✕/结果要点；结果无法解析时自动回退普通工具卡片
- **LSP 语义导航卡片**：`lsp_query` 的定义/引用/实现结果渲染为按文件分组的可点击导航行（`:行:列` + 打开文件，经系统默认编辑器打开），hover 结果渲染为滚动等宽文本
- **阶段成果播报（notify_companion）**：工作智能体在到达阶段性节点（阶段完成/验证通过/重要发现）时，把成果经 `notify_companion` 工具发给陪伴人格——由陪伴角色走完整陪伴管线（记忆/情绪/人设生效）生成一两句人设化播报，经 `proactive:bubble` 主动对用户说话（TTS + 桌宠气泡 + 聊天记录），并写入对话历史与记忆（channel=proactive，trigger=work_report）；每角色 60 秒节流防止刷屏，节流期内的成果由轮末摘要自然入库
- **工作区固定**：工作区只在「新建任务」时通过目录选择器确定，创建会话后固定不可更改；工作区分组标题支持重命名（本地显示名）与删除工作区（删除其下会话）。**无工作区模式**：陪伴侧 `delegate_to_work_agent` 派发的任务允许不绑定工作区（未显式指定且无历史工作区时）——system prompt 呈现"未选择（无工作区模式）：文件操作使用绝对路径、无目录沙箱、写入前确认"，轻量任务与 create_skill/create_tool 等进化事件无需先选目录即可执行
- **图片发收**：用户侧——微信聊天支持文件选择/摄像头 → `send_image_message` 多模态理解；编程页支持拖放/文件选择上传多图预览（`AttachmentRail`）→ 随 `coding_send_message` 带 base64 图片发给 LLM 理解。智能体侧——工具 `send_image` 把本地图片（生成的图表、截屏、项目里的图片文件）发给用户：编程会话下作为助手消息追加（base64 内联随会话持久化，恢复时直接渲染）；微信聊天下写入对话历史并 emit `chat:assistant_image` 实时插入图片气泡，窗口不可见时弹消息横幅，caption 作为跟进文本。双通道路由由 `session_id` 是否命中编程会话自动判定；图片副本持久化存储于 `<用户数据目录>\images\`
- **内嵌终端**（`TerminalPanel`，xterm.js + ConPTY）：终端标签位于右栏检查器内，标签名沿用工作区目录名；标签可多开、可关闭，终端区域随右栏整体收纳/展开，不随切页销毁


### 暖纸 UI 主题

心智观察器与设置窗口共用一套「暖纸信纸」视觉基调，由集中 token 驱动（[`global.css`](file:///g:/vivian-rs/src/styles/global.css) 的 `--panel-*` 三块 + [`design-system.ts`](file:///g:/vivian-rs/src/components/mind-inspector/design-system.ts)）：

- **纸本墨色**：宣纸主面 `#F5EFE4` / 浮起卡 `#FBF7EE` / 侧边栏 `#EFE8DB`；墨色 5 档（浓墨 `#2A2622` → 极淡墨 `#8F867B`）；分割线 `#D8CFBE`；唯一强调色「印章青蓝」`#537D96`；语义色克制墨染（墨绿成功 `/` 深朱危险）
- **纸纹**：`.scrapbook-bg` 用多层 `radial-gradient` 模拟宣纸颗粒与暖斑（零外部图片）；卡片收为 1px 细边框 + 3px 极小圆角（控件「印章取方」）
- **衬线排版**：正文切衬线（`Noto Serif SC` / 宋体家族），英文装饰标题保留手写体点缀；圆角 token 极方化呼应信纸感
- **标题栏结构**（[`MindInspector.tsx`](file:///g:/vivian-rs/src/components/mind-inspector/MindInspector.tsx)）：窗口顶部原生标题栏已删除——封面条标题区即窗口拖拽区，最小化/关闭按钮置于封面条右侧；页面经 `NavigationContext.setHeaderExtra` 注入的工具栏随日期印章、窗口按钮并排显示在封面条右侧

视觉整体由 token 层驱动，切换深/浅主题时暖纸调性保持一致；详见[CODE_WIKI 暖纸主题](file:///g:/vivian-rs/CODE_WIKI.md)。

### 主动对话编排

`proactive/` 模块实现自适应间隔 tick 调度的主动行为（支持单 tick 多消息 `MAX_TICK_MESSAGES=2`）。Tick 间隔根据用户空闲时间动态调整（`compute_adaptive_tick_ms(idle_seconds, char_id)`）：活跃时 10 秒、5-15 分钟空闲 30 秒、15-60 分钟 120 秒、超过 60 分钟 300 秒，减少空转 IPC。用户任何交互立即重置到活跃档，后端通过 `recommended_next_interval_ms` 字段向前端推荐下次 tick 间隔：

- **20 种触发器**：13 种常规概率循环触发器（HourlyGreeting / IdleGreeting / TeasingResponse / Icebreaker / WindowTrigger / TopicExtension / MemoryRecall / HealthReminder / Spontaneous / WelcomeBack / MoodDriven / CrossCharacterReply / BystanderInterjection）+ 7 种事件驱动触发器（Sunrise / Sunset / SystemPressure / ScreenPeek / AppDuration / LateNight / MusicChanged），后者不经常规概率门控链，由 tick 在"事件瞬间"走专门路径触发
- **用户回归摘要**（`proactive/recap.rs::generate_return_recap`）：用户离开 ≥10 分钟回归时（Away → Present 转换，`mark_user_present` 仅转换时返回 ReturnEvent，天然幂等），从统一事件账本提取离开窗口内该角色可见的事件（上限 40 条，按重要性+时间倒序），用轻量模型生成 1-3 句「刚才发生了什么」写入 ObservationNote 记忆并通知前端，角色在之后对话中能自然提起离开期间做的事而非表现得像用户从未离开过；离开不足阈值或窗口内无事件则跳过
- **日出/日落提醒**（`ProactiveOrchestrator::maybe_sunrise_sunset_reminder`）：`is_daytime` 按系统本地小时与日出/日落小时实时比较，昼夜转换精确发生在天亮/天黑时刻（不随天气缓存刷新滞后）；事件检测器捕获转换瞬间（每日各一次）时，由发言 leader 用**主对话完整提示词**（人设/记忆/环境/心理全量 prompt，经 `prompt_step` 复用）生成一句自然提醒推送到气泡（如"太阳刚升起/刚落下"），同时弹出**主题切换确认 toast**——日出推荐浅色、日落推荐深色，按钮按界面语言中/英/日输出（"换成浅色"/"换成深色"），点击一键写入 `base.theme` 并广播全局换肤；建议前先核对**当前生效主题**（App 主窗口经 `report_effective_theme` 上报实际深浅，含"跟随系统"按系统偏好解析，同时注入提示词），已是推荐主题则提示词禁止再建议切换且不弹 toast；仅用户在场、非防打扰且本角色持有发言权时触发，1 小时冷却兜底防重复
- **设备/环境感知提醒（5 种，不经常规概率循环）**：不同于常规触发器的时机/概率门控链，这组触发器由 tick 在"事件瞬间"走专门路径触发，带各自语义化冷却；深夜未眠与应用时长共用一次发言机会（互斥短路），避免一次 tick 串行多次 LLM：
  - **系统压力提醒**（`maybe_system_pressure_reminder`）：复用 world 10s 系统指标轮询缓存（`WorldStateProvider::system_metrics`），内存占用 ≥85%（normal→high **转换瞬间**）时生成关心的提醒（如"内存有点满了，要不要关几个小程序"），持续高位只提醒一次、降回正常后再次升高才重新提醒，30 分钟冷却兜底
  - **主动截屏观察**（`maybe_screen_peek` + `spawn_screen_peek_task`，异步）：窗口切换引发好奇，经用户同意后截屏 + 视觉理解再基于屏幕内容搭话。复用 `screenshot_analyze` 工具抽出的原语 `capture_screen_png_bytes` / `describe_screen_bytes`；未授权时先推气泡"可以让我看一眼你的屏幕吗～"并弹三按钮确认 toast（拒绝/放行一次/始终允许——走 `ToolSystem.request_confirmation`，与工具执行共享同一份 `confirm_tool_execution` 会话级放行记忆），拒绝后 2 小时内不再请求；需 `ai.enable_vision` 开启
  - **应用持续时长提醒**（`maybe_app_duration_reminder`）：`poll_window` 维护"应用会话"跟踪（按 `SmartAppClassifier` 分类，类别变化/回到桌面时重置计时），按类别差异化阈值提醒（写代码/办公 50 分钟、游戏/看剧 75 分钟、浏览器/聊天 90 分钟，utility/other 不提醒），按应用语义生成关心或调侃（写代码→劝休息、游戏→宠溺调侃），触发后重置计时 + 2 小时冷却
  - **深夜未眠关心**（`maybe_late_night`）：凌晨 1-4 点用户仍活跃（idle<300s）时温柔提醒睡眠，按本地日期去重每晚只提醒一次
  - **音乐切换搭话**（`maybe_music_changed`）：对比前后 tick 的 `MusicSnapshot` 检测播放/切歌变化（无→播放、暂停→恢复、播放中切歌），按 SMTC `source_app` 关键词过滤视频播放器（避免"看剧"被当"听歌"），基于曲目信息（《歌名》- 歌手）自然搭话；45 分钟冷却 + 概率 0.3 抽样
- **social_urge 双向门控**（`check_specific` 开头）：复用 current_thought 每 60s LLM 调用顺便产出的 `social_urge`（0-1，表示角色"现在想主动搭话"的冲动强度）对问候类触发器做语义化时机调节，而非机械按时触发。`urge >= 0.8` → 跳过整点/空闲阈值等特定条件提前触发；`urge < 0.3` → 推迟规则触发（时间到但角色不想说话就不硬凑，下个 tick 重新评估）；中间值 → 正常规则触发保底。WelcomeBack 豁免（用户刚回来必须问候）。通用门控（冷却/时机分数/概率/5 分钟静默期）仍生效，urge 高不绕过"用户在忙"等合理性约束。可通过 `proactive.enable_social_urge_gating` 开关关闭
- **时间信息注入**（修复"把旧事说成刚发生"问题）：主动问候的提示词三层注入具体时间信息，让 LLM 正确建模事件远度——(1) Icebreaker / MemoryRecall 的 `build_messages` 接收 `idle_seconds` 参数，场景描述从模糊的"warm 级别"改为"用户离开了 1小时23分钟"；(2) 记忆检索结果带相对时间（如"3小时前"），利用 `MemoryItem.timestamp` 计算；(3) 对话历史带相对时间标注（如"[3小时前] role: content"），利用 `ChatMessage.timestamp` 计算。`format_elapsed_lang` / `format_relative_time_lang` 工具函数支持中/英/日三语时长格式化
- **意图判断规则预检**（`intent_judge.rs`）：告别类别种子短语（晚安/再见/打断，含场景元数据）通过 n-gram 嵌入 Top-K 投票 + softmax 加权预检——对输入文本与种子短语集合计算相似度，取前 K 条按 softmax 权重投票，平票时按 GoodNight > GoodBye > Interrupted 优先级裁决，命中时直接判定、跳过 LLM 调用；规则未覆盖的语义判断（冲突/话题切换/隐含告别等）由 LLM 完成，降低不必要的后处理 LLM 开销
- **多级冷却**：每个触发器独立阈值 + 全局最小间隔
- **到达问候共享冷却**：启动问候与唤醒问候由 Brain 生成（不走 tick 触发循环），成功后经 `record_greeting_arrival` 计入主动问候共享冷却（全局打扰时间戳 + 问候键），问候类触发器（WelcomeBack / HourlyGreeting / IdleGreeting / Icebreaker）在 `min_trigger_interval`（默认 180s）静默期内被硬门控拦截，避免刚问候完又触发主动问候
- **启动问候活人感增强**（`generate_startup_greeting`）：首次见面判定基于 `non_seed_count() == 0`（排除种子记忆）。**问候不再区分首次/回归分支，统一走完整对话流水线**（`BrainChatChain::ainvoke_greeting`，复用 `prepare_pipeline_state` + 完整 pipeline），与一般直接渠道对话同一套提示词（含记忆检索→种子记忆进入 prompt），仅在用户消息前加一句"这是首次见面"/"用户回来了"的提示。首次见面时种子前史（世界观/身份/跨角色关系里程碑）可被检索进问候上下文，让开场白带着角色自己的过去生成，而非一张白纸；通过 `skip_memory_save` 门控避免把合成的问候指令写进记忆库。生成时注入当前情绪状态（跨会话保留）、天气与时间（WorldSnapshot），让 LLM 带着具体情绪状态生成开场白而非机械模板
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
  - **主动旁观插话评估**（`evaluate_active_bystander_interjection`）：用户通过 InputDialog 向角色 A 发送普通消息时，立即对在线的旁观者 B 发起轻量 LLM 调用（任务类型 `bystander_judge`，复用 auxiliary 信号量组），绕过概率 roll 和 proactive_tick 周期，每次都判断 B 是否有动机插话。轻量 LLM **仅做二值判断**（返回 `{"should_interject": true/false}`），提示词明确约束"插话应该是偶发的——只在话题感兴趣/有独特观点/情境合适时插话，大多数时候应该保持沉默"。决定插话时**激活 B 的主对话流程**（`brain.think`），用完整提示词（人设/记忆/工具）生成插话回复，并在 user_input 末尾拼接插话指令段（"对用户说，不是对室友。这是你插进他们的对话。你可以评论或吐槽听到的话题，但不要假装和室友有同样的兴趣——你的兴趣是你自己的"），通过 `think_lock` 串行化避免与 `send_message_stream` 并发冲突。插话内容通过 `proactive:bubble` 事件触发前端气泡显示和 TTS 播放
  - **roommate_cue 机制**：`commands/chat.rs` 写入旁观记忆后以 8% 概率调用其他在线角色的 `seed_roommate_cue`，设置 30s TTL 信号（from_name + topic_brief），被 cue 角色的 BystanderInterjection 概率提升并在 prompt 中注入"室友刚 cue 了你"提示，让插话更自然
- **角色个性化行为参数**（`character_behavior.rs`）：按 `char_id` 索引的本地非 LLM 控制参数，让不同角色表现出不同节奏感。`ProactiveOrchestrator::new(char_id)` 持久化路径按角色隔离到 `characters/<char_id>/proactive/`，`apply_proactive_feedback(positive, char_id)` 增减幅度、MoodDriven 触发阈值、亲密度冷却系数、安静模式阈值均读取 `CharacterBehavior`：
  - **Vivian（傲娇慢热）**：正向反馈 +0.002 / 负向反馈 -0.003（冷落更伤感情）、MoodDriven 需求阈值 0.85 / 孤独阈值 0.75（不易被情绪驱动主动发话）、亲密度冷却系数 ×0.8（冷却更快）、安静模式 5 次、心情表情冷却 30s
  - **Nana（温柔热情）**：正向反馈 +0.005 / 负向反馈 -0.001（宽容不易记仇）、MoodDriven 需求阈值 0.65 / 孤独阈值 0.55（容易主动关心）、亲密度冷却系数 ×1.2（冷却更慢）、安静模式 2 次、心情表情冷却 15s

### 笔记系统

`notebook/` 模块让智能体把搜索/整理所得信息生成卡片风格的 HTML 笔记页面，保存在本地按角色隔离存储，可在笔记本窗口的「笔记」页查阅，并可由智能体自主决定通过微信链接卡片形式发送到 ChatWindow：

- **混合式 HTML 生成**：预设手账风格 CSS 主题（6 套配色：warm/fresh/elegant/cute/cool/nature）+ 4 种布局模板（CoverFlow/Article/Gallery/Simple）+ 10 种内容块类型（Heading/Paragraph/Card/Quote/List/Tags/Image/Divider/Callout/Custom），LLM 输出结构化 JSON 描述内容编排，后端 `renderer::render_html` 渲染成自包含 HTML（内联 CSS，无外部依赖）；同时支持 `Custom` 块嵌入局部自定义 HTML 片段（经 `sanitize_custom_html` 清理移除 script 标签 / on* 事件属性 / javascript: 协议），兼顾风格一致性与灵活性
- **手账风格视觉**（`notebook/renderer.rs`）：笔记 HTML 元素整体采用手账风格——Google Fonts 加载手写字体（中文 `Ma Shan Zheng` / 英文 `Caveat` / `Gochi Hand` 三套手写字体回退链），纸张纹理通过三层 `background-image` 合成（横向稿纸线 `repeating-linear-gradient` + 两个角落的 `radial-gradient` 墨迹晕染），所有卡片/封面/提示框使用不对称圆角（`2px 16px 2px 16px`）模拟手撕纸边；封面 `.cover` 带 -0.6deg 倾斜、卡片 `.card` 带 -0.3deg 倾斜并附伪元素和纸胶带装饰（顶部 56×20 半透明色块 + 4deg 倾斜），`callout` 提示框采用和纸胶带便条样式，列表项前缀使用手绘风格符号，整体呈现手写日记本的视觉质感
- **7 个 LLM 工具**（`tools/builtin/notebook_tools.rs` + `tools/builtin/file_tools.rs`，三语化描述 + ZH/EN/JA 参数 schema）：
  - `create_notebook` —— 传入 title/layout/palette/tags/cover/blocks 生成结构化笔记，返回 note_id 与本地路径；同时把笔记内容同步到向量知识库（`MemoryType::Knowledge`），供后续 RAG 检索
  - `list_notebooks` —— 列出已创建的笔记（note_id / 标题 / 标签 / 布局 / 最后修改时间，支持按关键词过滤），让智能体能**发现已有笔记的 note_id**——当用户要求分享"写好的那篇笔记"时先枚举定位，而非重新生成（anti_use_cases 明确禁止"为了分享而重建已有笔记"）
  - `get_notebook_detail` —— 传入 note_id 读取笔记的完整结构化内容（标题/布局/配色/标签/封面/全部内容块），修改前务必先调用此工具查看原内容，避免覆盖丢失
  - `update_notebook` —— 传入 note_id + 需更新的字段，支持局部修改 title/tags/blocks/cover/layout/palette；更新时自动先删除旧的知识库条目再重新入库，保持向量索引同步
  - `share_notebook` —— 传入已有笔记的 note_id + 可选 follow_up 文案，生成 `vivian://notebook/<char_id>/<note_id>` 链接，通过 `chat:link_card` 事件以微信风格链接卡片形式发送到 ChatWindow，同时写入对话历史支持记忆恢复；描述明确"分享已有笔记时先 `list_notebooks` 定位 note_id，不要 `create_notebook` 重建"
  - `create_html_note` —— 智能体直接撰写完整的自包含 HTML（内联 CSS，无外部依赖）存为 `raw_html` 笔记，经笔记本窗口 Shadow DOM 渲染，适合自由定制排版；同时抽取正文文本同步进向量知识库供 RAG 检索
  - `read_file` —— 按用户给出的本地文件绝对路径读取文本/代码/HTML 内容（UTF-8/GBK/Shift-JIS 编码自动检测），受沙箱路径校验约束（拒绝 `..` 穿越、仅限工作目录内文件），供用户把已有的 HTML 文件转成笔记或查看配置/代码文件
- **完整 HTML 笔记（`raw_html`）**：区别于卡片式结构化笔记，把完整 HTML 文档原样保存、整洁渲染。三类导入途径：
  - 用户把完整 HTML 代码发给智能体 → 智能体调用 `create_html_note` 直接转存为笔记
  - 笔记本窗口点击导入按钮 / 拖入 `.html` 文件 → 前端调用 `import_html_note` 命令直接读取完整文件（无字符截断、不经 LLM）存为笔记
  - 告诉智能体本地文件路径 → 智能体调用 `read_file` 读取后转存为笔记
- **按需增量加载**：上述笔记/HTML/文件类工具统一标记 `should_defer=true`（`Deferred`），不常驻占用 LLM prompt token，仅在智能体制作/查阅笔记时通过 `tool_search` 按需获取完整 schema
- **向量知识库集成**：笔记创建/更新时自动调用 `MemoryManager::add_knowledge_document` 入库（source="notebook"，ttl_days=-1 永不过期），内容块拼接为纯文本做嵌入向量化；笔记与知识条目通过 `.memory_ref` 文件关联 memory_id，更新时先删旧条目再入库新内容，删除笔记时同步清理知识条目；后续对话中智能体通过 `search_memories` 即可 RAG 检索到笔记内容
- **按角色隔离存储**：`%APPDATA%\Vivian\characters\<char_id>\notebook\<note_id>\` 下含 `note.json`（元数据）+ `note.html`（渲染 HTML）+ `.memory_ref`（知识库条目关联）+ 索引文件；`raw_html` 笔记仅含 `note.html`（无 `note.json`），由索引文件补全到列表。所有文件操作经 `sandbox::is_path_safe` 校验
- **笔记本查阅窗口**：集成在笔记本窗口的「笔记」导航页，左右两栏布局（左 36% 笔记列表 + 右 64% iframe 预览），顶部角色切换（Vivian/Nana）。监听 `notebook:created`/`updated`/`deleted` 事件自动刷新，无需手动刷新按钮；支持导入完整 HTML 文件（文件选择器 + 拖放），`raw_html` 笔记经 Shadow DOM 渲染
- **链接卡片跨窗口跳转**：用户在 ChatWindow 点击 `vivian://notebook/` 链接时，`handleOpenLink` 识别协议——笔记本窗口已存在则 `emit('memory:navigate', ...)` 通知跳转 + 聚焦，不存在则创建新窗口带 `nb_id`/`nb_char` URL 参数；笔记本窗口同时支持 URL 参数读取与 `memory:navigate` 事件监听两种跳转入口，收到后切换到笔记页并自动定位笔记

### 真实世界感知（环境智能）

`world/` 模块让 Vivian 在真实世界中"活着"——即使用户不交互也能感知世界。配置入口位于设置窗口「通用」页（原独立「感知」页签已并入通用页，与其他行为类配置合并展示）：

- **时间感知**：本地时间 / 周几 / 周末 / 季节 / 24 节气 / 公历与农历节日 / 日出日落（优先用天气 API 返回的时间，未配置经纬度或 API 未返回时回退 NOAA 简化算法；`is_daytime` 昼夜判定按系统本地小时与日出/日落小时实时比较，转换精确发生而非随天气缓存刷新滞后）
- **天气感知**：Open-Meteo 免费接口（无需 API Key），失败当作"不知道"（不做时间推断兜底），带 WMO 代码到中文描述映射与可配置 TTL 缓存；同时返回当日日出/日落时刻（`daily=sunrise,sunset`），驱动世界快照的 `SunriseSunset` 与日出/日落提醒功能
- **系统音量感知**：通过 Windows Core Audio API（`IAudioEndpointVolume`）获取主输出设备音量（0-100），使用 `spawn_blocking` 隔离 COM 调用避免与 Tauri/WebView2 的 STA 线程冲突（`RPC_E_CHANGED_MODE`）
- **媒体播放检测**：通过 Windows SMTC（System Media Transport Controls）事件回调实时捕获正在播放的媒体信息（标题 / 艺术家 / 专辑 / 播放状态），事件驱动而非轮询，通过 `PlaybackInfoChanged` 回调即时响应
- **前台窗口检测**：Win32 FFI 获取当前聚焦窗口的标题、进程名和 PID。自动跳过应用自身窗口（主窗口 + 所有子窗口，通过 PID 比较），当应用获得焦点时保留上一次的外部窗口快照，避免显示"无活跃窗口"
- **网络连接监控**：COM `INetworkListManagerEvents::ConnectivityChanged` 事件回调，在本机网络适配器连通性变化时即时更新网络状态（已连接 / 已断开 / 未知）
- **IP 地理位置**：通过 ipwho.is API（无需 Key）获取 IP 级地理位置（城市 / 省份 / 国家），启动时自动检测 + 30 分钟定期轮询补充（覆盖 VPN 切换 / 路由器公网 IP 变化等 NetworkWatch 事件无法捕获的场景），用户可在前端点击位置卡片手动触发刷新（5 秒防抖）
- **位置注入提示词**：城市 / 省份 / 国家信息通过 `EnvironmentContext` 注入对话 prompt（三语覆盖：中 / 英 / 日），让 Vivian 在对话中"知道"用户所在位置
- **世界事件检测**：比较前后 `WorldSnapshot` 产出事件（天气变化 / 开始下雨 / 节日到来 / 节气切换 / 日出 / 日落 / 季节变化 / 长时间缺席），通过 Appraisal 机制隐式影响情绪/需求；其中日出/日落事件同时驱动**主动提醒**（LLM 完整提示词生成一句自然提醒 + 弹出可一键切换主题的确认 toast，见[主动对话编排](#主动对话编排)）
- **世界快照注入**：将时间 / 节气 / 节日 / 天气 / 日出日落 / 地理位置注入对话 prompt，让 Vivian 在对话中"知道"真实世界状态
- **可配置作息**：`sleep_start_hour` / `sleep_end_hour`（支持跨午夜，如 23 点入睡、6 点醒来）让桌宠的睡眠时间可调整而非写死
- **用户实体状态机**（`world/entity_state.rs`）：跟踪在场/离开/预期回归/持续活动四态状态机。`ExpectationEngine` 从对话抽取预期回归时间（"20 分钟后回来"→20min 范围）+ 活动意图（"我去上班了"→直接写入 `current_activity`，无需等 LLM 反思 tick），活动意图抽取采用双信号门控（意图信号词 + 活动关键词）避免"上班真累"误判。`mark_present` 时产出 `ReturnEvent` 携带实际离开时长，按预期范围分类（MuchEarlier/Earlier/OnTime/Later/MuchLater），超时观察去重
- **用户行为日志**（`world/user_behavior.rs`）：已封存的持续状态事件（带 duration，不被 LLM 压缩），按活动标签查询供认知引擎整理为习惯 Belief（如"用户通常睡 7 小时"），FIFO 上限 300 条
- **用户活动分类**（`world/activity_classifier.rs` + `activity_corpus.rs`）：从实时前台窗口快照推断用户当前活动，两层策略——A 层精确进程名 O(1) 映射（快速可靠），B 层嵌入分类（对齐 Live2D 表情的语料库思想），用丰富活动语料库（235 条种子、21 个细粒度标签，如写代码/调试排查/看视频/网上购物等）经本地 `HashingMemoryEmbedding`（jieba 分词 + 特征哈希）嵌入后做 Top-K softmax 投票，零网络、毫秒级实时，供行为日志与认知层消费

### 体验连续性（Experience Continuity）

让 Vivian 拥有"和用户共同经历了一段时间"的持续存在感，而非每次对话都像刚启动。`mind/` 模块在 World / Memory / Reflection 之间增加状态合成层：

- **用户长期目标账本**（`mind/user_goals.rs`）：周~月级带 deadline 的用户人生阶段目标（"准备考研" / "写毕业论文"），由 reflection 阶段 LLM 抽取用户明说信号产出（`Dialogue` 来源强制要求 `source_quote` 原话引用，防止幻觉造目标），支持状态机（Active/Paused/Completed/Abandoned）+ 容量上限 5 + 同名去重 + 持久化到 `characters/<char_id>/mind/user_goals.json`。`Mind` 结构体持有 `user_goals: Arc<UserGoalLedger>` 字段，与 BeliefStore / GoalStore 同层
- **时间关系合成器**（`mind/temporal_context.rs`）：纯函数模块，从离散世界事实（`WorldBrief`）+ 长期目标摘要合成关系型时间事实，产出 6 类事实：Duration（用户已连续编码 3.5 小时）/ TimeOfDay（现在是凌晨 2 点）/ MealTime（接近晚饭时间）/ Deadline（「考研」还有 3 天到期）/ AwayAnomaly（用户已离开 3 小时长时间未归）/ Compound（深夜连续工作 2 小时疲劳风险上升）。零 LLM 调用、零新存储，注入 `thought_synthesis` 的 `## 时间关系（事实之间的关联）` 段落，让 LLM 不必从离散事实现算关系
- **事件重要性时间衰减**（`memory/unified_event_ledger.rs`）：事件账本排序从静态权重改为 `importance(t) = base × decay(age)`，分段衰减（<24h=0.95 / <3d=0.70 / <7d=0.40 / <30d=0.15 / ≥30d=0.05），「昨天买咖啡」类事件自然淡出，重要远期事件仍可被召回
- **WorldBrief 扩展**：注入 prompt 的世界事实基线新增 `user_activity_elapsed_secs`（当前活动已持续秒数）和 `active_goals`（最多 3 条活跃长期目标摘要，带剩余天数），让 LLM 在每轮对话都感知"用户当前处于什么人生阶段 + 当前活动持续多久"

### 自主活动（内心独白与活动日志）

让 Vivian 在用户离线时自主思考并记录用户活动：

- **内心独白**（`proactive/inner_monologue.rs`）：冷却到期（默认 30 分钟）时调用 LLM "inner_monologue" 任务生成 50-120 字第一人称独白，写入记忆（类型 `InnerMonologue`，标签含 `inner_os` / `inner_monologue` / `autonomous`），不打扰用户。生成时以世界快照 + 心理状态 + 近期对话记忆 + 用户活动日志为信息源，生成后清空活动日志重新记录
- **当前想法**（`mind/thought_synthesis.rs`）：每 60s 调用 LLM 生成角色当前思维片段，输出 JSON `{ thought, social_urge }`——`thought` 为第一人称想法文本，`social_urge`（0-1）表示角色"想主动搭话"的冲动强度，综合考虑心情、距上次对话时间、是否有话可说、用户是否在忙。`social_urge` 写入 `Mind.social_urge` 供 proactive 模块的双向门控使用（见[主动对话编排](#主动对话编排)章节），零额外 LLM 调用成本。LLM 失败时 `social_urge` 保持上次值不重置，避免一次失败丢信号；解析失败时回退纯文本 thought + 默认 0.5 urge
- **用户活动日志**（`proactive/activity_journal.rs`）：后台原生 Rust 线程（Win32 API，非 PowerShell）每 5 秒轮询前台聚焦窗口标题，仅在变化时记录一条带时间戳的日志（FIFO 上限 100 条）。内心独白生成时 `drain()` 消费并清空，作为 Vivian "观察用户"的信息源。线程仅在总开关开启时运行，平时 sleep 不占 CPU

### 用户内容自动入库

用户在对话中分享的文件和网页链接会自动提取内容并写入 RAG 向量知识库（`MemoryType::Knowledge`），供后续对话检索。整个流程在后台异步执行（fire-and-forget），不阻塞主对话，失败只记日志。

- **文件上传入库**（`commands/chat.rs`）：用户拖拽上传文本/PDF 文件时，`extract_file_text` 提取的文本原本只作为普通消息入 `ShortTerm`。现在 `send_message_stream` 在 think 完成后异步调用 `add_knowledge_document`，把文件文本以 `source="user_file"`、`ttl_days=-1`（永不过期）入库，标题为「文件：<文件名>」，tags 为 `["user_file"]`
- **网页链接抓取入库**（`network/url_fetcher.rs`）：用户消息中包含 `http(s)://` 链接时，系统自动抓取页面内容并入库
  - `extract_first_url` 用正则提取首个 URL（支持中英文标点边界识别）
  - `fetch_page` 发起 HTTP GET（15 秒超时，仅接受 `text/html`），用正则方案剥离 HTML 标签提取正文：移除 script/style/nav/header/footer/aside/noscript/iframe/form/svg 块 → 块级标签转换行 → 剥离所有标签 → 解码 HTML 实体 → 压缩空白；正文截断到 8000 字符
  - 抓取后以 `source="user_link"`、`ttl_days=-1` 入库，标题取页面 `<title>`，tags 为 `["user_link"]`
  - 不引入额外 HTML 解析依赖，采用轻量正则方案

### 多平台内容发现与推荐

`discovery/` 模块实现跨平台主动内容发现：从角色兴趣画像出发，多平台源采集候选 → LLM 批量评估 → 入库 + 兴趣探针确认，形成「画像 → 发现 → 反馈」闭环。数据按角色隔离于 `characters/<char_id>/discovery/`（interest_profile.json / content_store.json / speculative_state.json），全部原子写。

- **发现流程**（`engine.rs`）：LLM 从画像 + 活跃探针生成 3-5 个搜索词 → 各源并行取候选（搜索 + 热门/榜单，跨源 `platform:id` 去重 + 库存/推荐账本去重）→ LLM 结合画像批量评估（score/reason/topic_group，热门与否不影响评分，只看画像真实匹配度）→ score ≥ 0.5 入库（库存上限 60），≥ 0.75 进惊喜队列 → 入库标题喂给兴趣探针做行为确认
- **兴趣画像**（`profile.rs`）：兴趣域（权重 + 生命周期状态）+ 不喜欢主题 + 探索开放度；种子从 `user_facts.json` 显式兴趣合成，随推荐反馈与探针 promote 演化；LLM 失败时回退画像顶层兴趣（纯规则，不阻断发现）
- **聚合点（不设独立后台循环）**：
  - **Busy 知识采集 Share 路径**（`presence/background_tasks.rs`）：`acquire_delight_candidates` 让平台候选与网页搜索合并竞争，≥0.75 惊喜级胜出者经微信面板分享（复用 knowledge_share 30 分钟冷却），无则回退网页搜索
  - **采集周期顺带 `maintenance_pass`**：探针 tick + 低库存（<15）跨平台补货 + 登录态历史被动采集（6 小时冷却）
  - **内心独白兴趣搜索 / LLM 采集主题决定**：消费 `interest_search_hints`（画像顶层兴趣 + 活跃探针域动态查询词）
  - **Bangumi 公开收藏导入**（`bootstrap_from_bangumi`）：公开用户名初始化画像
- **匿名源**：bilibili（WBI 签名搜索+热门）、bangumi（v0 API 搜索/榜单）、v2ex（官方 API，严格限频每轮只取一次热门）、微博（m.weibo.cn H5 容器 + 游客 SUB cookie + 实时热搜）
- **登录态 CLI 源（cookie 重放）**：
  - **X (Twitter)**（`sources/x.rs`）：扩展回传 x.com 的 `auth_token`+`ct0`（`bridge.reportXCookie`），服务端注入环境变量驱动 `twitter` CLI（`uv tool install twitter-cli`）只读发现（search / feed）；CLI 缺失或凭据失效静默禁用，不阻断其它源
  - **Reddit**（`sources/reddit.rs`）：扩展回传 reddit.com 整罐 Cookie（`bridge.reportRedditCookie`，需含 `reddit_session`）同步进 rdt-cli 凭据文件，优先 `rdt search/popular` 登录态发现；CLI 或凭据不可用时回退匿名 `.json` 端点
- **隔离任务 tab**（`sources/task_tabs.rs`）：小红书/抖音/知乎等需登录态平台的后台发现——扩展以 inactive + 静音方式打开隔离标签（不抢占焦点、不触碰用户正在看的标签页），同源 fetch / DOM 提取候选后自动关闭；每平台独立 3 小时冷却 + 登录态门槛，加载的页面若平台未登录则静默跳过。采集为纯脚本（无 LLM），候选仍走引擎统一 LLM 评估入库
- **登录态被动采集**（`sources/browser_signals.rs`）：受控标签页恰好停在目标平台域名时，同源 fetch 读取观看历史等信号，经 LLM 提炼兴趣域写回画像（6 小时冷却，不导航不劫持）
- **推荐账本**（`recommend.rs`）：已推荐内容去重，内容循环复用；前端可查看画像/库存/探针并调整兴趣权重（`UserProfilePage` DiscoverySection）

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

### SNS 热梗定期采集

独立于 Busy 知识采集的定期主动采集任务，保持角色"懂梗玩梗"人设。每个角色在启动时 spawn 独立 tokio task，按 7 天滚动周期主动采集 B 站、抖音、小红书、微博等 SNS 平台的最新热梗，写入 Knowledge 记忆（TTL=7 天自动刷新）。

- **独立循环**（`presence/meme_acquisition.rs`）：不依赖 Presence 状态（Online/Busy/Rest 均可运行，Offline 跳过），与 Busy 知识采集完全独立，不占用 MAX_TOPICS 配额也不触发 30 分钟冷却。启动后延迟 10 分钟首次触发，避免启动期资源争抢
- **角色差异化平台**：Vivian 侧重 B 站（`site:bilibili.com`）+ 抖音，采集二次元番剧梗、鬼畜、UP 主热梗、短视频挑战；Nana 侧重小红书 + 微博（`site:weibo.com`），采集生活穿搭、美食、情感热词、社会热点。两人各自积累不同的 SNS 知识
- **LLM 全生成关键词**：每周让 LLM 基于当前日期 + 角色人设 + 平台侧重生成当周可能的热梗候选词（最多 4 个），LLM 可返回 `[none]` 表示本周无明确热梗可查。关键词生成走 `knowledge_acquisition` 任务路由，复用现有 LLM 配置
- **平台定向搜索**：关键词拼接平台修饰构造 query（如 `site:bilibili.com 热梗A OR 热梗B`），通过 WebSearcher 多引擎并发搜索（DDG/SearXNG/Tavily/Bing），每平台最多 6 条结果
- **LLM 总结成笔记**：搜索结果交 LLM 整理成角色口吻的"热梗笔记"（包含梗名、来源背景、用法），写入 `add_knowledge_document`，source=`"meme_acquisition"`，TTL=7 天，tags 含 `meme`/`trending`/`sns`/`<platform>`。下周采集时旧笔记自动过期刷新
- **滚动周期 + 持久化冷却**：从上次采集完成时刻起算 7 天后再次触发，状态持久化到 `characters/<char_id>/meme_acquisition_state.json`。重启后若距上次 ≥ 7 天则立即触发，否则 sleep 到下次触发时间。sleep 期间每 5 分钟检查一次取消信号，支持优雅退出
- **前端事件**：采集开始/结束分别 emit `meme_acquisition:started` / `meme_acquisition:finished` 事件（携带 `character_id` / `acquired` / `summary`），前端可订阅展示采集状态

### 记忆巩固（睡眠模拟）

`memory/consolidation.rs` 在配置的睡眠窗口内（跟随 `sleep_start_hour` / `sleep_end_hour`，而非写死 2-5 点）+ 6 小时冷却到期时，异步执行完整巩固流水线（Stage 1/2/3：ShortTerm → MidTerm → LongTerm → Insight），模拟人类睡眠时的记忆整理

工程韧性：
- **步骤级熔断**：`pipeline` 与 `belief` 两步分别跟踪连续失败计数，连续失败 ≥ 5 次写入 `paused_reason` 熔断暂停——暂停期间完全跳过该步骤，不再烧 LLM；1 小时后半开重试，成功后自动恢复正常。暂停原因经 `get_memory_health` 展示在记忆页健康条
- **断点续跑**：Stage 1 摘要写库前先把源 ID 记入 `consolidation_progress_<char_id>.json`（上下文键 = 角色 + 逻辑日，跨天作废），崩溃后重启补完「已摘要未标记」的 ShortTerm（出现在 SessionSummary 的 `promoted_from` 中 → 补 `mark_summarized` 防重复摘要；未出现 → 走正常重摘要防漏摘要），水位文件原子写，损坏自动备份后按空态处理

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
- **MoodCue 快速通道**（`mood_cue.rs`）：在完整心理管道之外提供一条纯规则驱动快速路径，直接从 MoodSnapshot 映射到 Live2D 表情/动作提示，用于主动 tick 背景微动画、LLM 未响应时的即时反馈与低频心跳动画。规则集按「真实心理表现可观测优先级」五层分层：
  - **第一层 生理底线**（压过一切情绪）：睡着(fatigue>90)→`blindfold`、极度疲惫(fatigue>80)→`dizzy`、身心俱疲(fatigue>60+stress>50)→`dark_face`、压力临界(stress>80)→`sweat`、高压力(stress>70)→`angry`
  - **第二层 高强度主导情绪**（intensity>0.55，压过中度疲劳）：7 类情绪各分强/弱两档——欣喜若狂(Joy 极强+高唤醒)→`star_aura`、眉开眼笑→`star_eyes`；怒气冲冲→`angry_symbol`、生闷气→`puff_cheek`；泪如雨下→`tears`、闷闷不乐→`cry`；惊慌失措→`dizzy`、忐忑不安→`sweat`；满心爱意→`love_eyes`、温柔害羞→`shy`；失落出神→`blank_eyes`、怅然若失→`tears`；满腹狐疑(好奇+高唤醒)→`confused_intense`
  - **第三层 中度疲劳**（无强情绪时倦意浮上表面）：昏昏欲睡(fatigue>55+arousal<0.45)→`blindfold`
  - **第四层 效价-唤醒空间**（中等强度背景基调）：兴奋(高唤醒+正效价)→`star_aura`、满怀期待→`star_eyes`、安心惬意(加关系分>40得爱意眼)→`love_eyes`/`shy`、焦虑→`sweat`、不高兴嘟嘴→`pout`、情绪低落→`cry`、委靡无力→`speechless`、好奇观察→`confused`
  - **第五层 关系背景调制**：老朋友默契(关系分>75)→`love_eyes`、疏离旁观(关系分<15)→`blank_eyes`
  - **兜底**：平静待机→空表情 idle
  - 另 `emotion_to_cue` 提供按情绪标签+强度分档的快捷映射捷径

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
- **打字机 RAF 对齐帧率**（`BubbleController.ts`）：气泡打字效果从 `setInterval(35ms)` 改为 `requestAnimationFrame`，基于帧间隔时间累积计算揭示字符数（17.5ms/字），每帧最多一次 setState。60fps 下每帧约 1 字，主线程忙时浏览器自动降帧并累积时间在下一帧平滑追赶（单帧上限 4 字防爆蹦），消除"卡半秒突然蹦出一大段"的体感卡顿
- **表情管理**：`ExpressionManager` 支持表情栈与定时恢复（`set_expression` 压栈 / `revert_expression` 弹栈 / `start_revert_timer` 定时恢复），表情由各角色 `model_manifest.json` 定义（Vivian 11 个 / Nana 30+ 个）
- **状态机**：`PetState`（Idle / Interacting / Panicked / Playing / AiTalking）+ `StateTransition` 显式状态机，支持事件驱动的状态流转
- **动作优先级**：5 级优先级（Idle=0 / Low=10 / Normal=50 / High=100 / Critical=200）控制动作打断与队列
- **鼠标跟随**：两级跟随（`window` / `off`），交互事件（鼠标进入/点击/拖动）刷新 5-8 秒跟随窗口，窗口内跟随、超时回归自主；后端 `cursor_tracking` 线程每 60ms 推送 `cursor:position` 事件，光标位置未变化时跳过 emit，窗口隐藏时停止跟踪
- **点击压缩/回弹**（`Live2DCanvas`）：按下桌宠模型窗口时整窗（含模型）以窗口底端为锚点向下压缩（`scaleY(0.93)` + 轻微收窄 `scaleX(0.97)`）并保持，松开时快速阻尼振荡回弹（约 300ms，先超冲再衰减回原尺寸）。方向切换时先读取当前视觉 `scaleY` 再从该值无缝过渡，消除快速连点导致的跳变
- **拖拽惯性甩飞 + 边缘回弹**（`window.rs`，`cursor_tracking` 线程）：快速拖拽松手时，用松手前最近 120ms 的全局光标轨迹（`GetCursorPos` 轮询，不受窗口追逐延迟影响）计算初速度，窗口带惯性滑行——指数摩擦衰减（约 350ms 半衰期）自然停住，速度低于阈值即静止；慢速拖动（< 0.5 px/ms）不触发甩飞，不影响精确摆放。碰撞边界不是窗口矩形，而是桌宠身体足迹（窗口中央 1/3 宽 × 4/9 高，与点击穿透中心矩形同口径）——Live2D 模型主体只在该范围内渲染，全透明边缘可滑出屏幕外 1/3 宽度，视觉上是角色本体撞到屏幕边缘弹回（法向速度乘 0.6 回弹系数，几次反弹后静止）。甩飞中以虚拟屏幕（多显示器并集）为边界；途中再按住桌宠会立即被「接住」；智能避让等程序化移动不会被覆盖（每帧重读窗口实际位置作积分基点）

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

### 微信窗口右缘侧边栏（chat）

微信主窗口（`label="chat"`）采用**右缘三态侧边栏**设计，默认收纳于屏幕右侧，不占用桌面空间：

```
Hidden（默认完全隐藏，整体位于屏外右侧）
  → Peek（鼠标靠近屏幕右缘 12px 内、窗口自身 y 区间 → 滑出 10px 探出条，鼠标穿透）
      → Expanded（点击探出条 / 托盘「微信」/ 横幅点击 → 整窗滑入展开，可交互）
          → Hidden（点击左上角「‹」退出按钮或光标离开宽限 420ms 后 → 滑出屏外并隐藏）
```

- **可见性唯一权威在 Rust 线程**（`commands/window.rs`）：直接调用 `win.show()/hide()`，因为 WebView2 在窗口隐藏/失焦时会节流前端 setInterval 与 emit+listen IPC，前端事件驱动不可靠。锁定/输入框状态由前端通过原子量告知线程
- **三态机制**：右缘边缘检测线程（`start_side_chat_edge_watcher`，12px 触发宽度、窗口自身 y 区间判定、60ms 轮询、显示器缓存定期刷新）驱动 Hidden→Peek→Hidden；全局 `WH_MOUSE_LL` 低级鼠标 Hook 检测 Peek 态单击命中探出条 → 展开，展开态双击切换锁定（被动穿透态由 Hook 负责双击、可交互态由 React `onDoubleClick` 负责，靠 `SIDE_CHAT_CLICK_THROUGH` 互斥不双触发）
- **滑动动画**：所有状态切换经独立动画线程 ease-out cubic（220ms / 8ms 步进）平滑位移，动画代号自增、被新动画取代的旧动画自行终止（防止快速进出边缘叠加）；动画期间边缘循环跳过 show/hide 决策避免与位移竞争
- **状态化鼠标穿透**：被动展示态（输入框关闭）窗口鼠标穿透不挡桌面；交互态（输入框打开 `set_side_chat_input_open`）关闭穿透可打字；`set_side_chat_locked` 锁定时常驻不自动隐藏
- **预创建**（`src/App.tsx::ensureWechatWindow`）：启动时预创建屏幕右缘屏外隐藏的 `chat` 窗口（iPhone 17 比例 390×845、无边框透明、置顶、跳过任务栏），避免首次呼出冷启动 WebView2 延迟
- **side_chat 独立**：直接对话面板（`label="side_chat"`）停靠屏幕左缘，通过显式传 `label:'side_chat'` 与微信抽屉解耦，不参与右缘三态逻辑

### 语音系统

- **ASR**：WinRT SpeechRecognizer（默认，Windows 原生） / Whisper HTTP 后端 / Azure 云端 / Aliyun 阿里云，四引擎可切换
- **TTS 多后端**：
  - `edge` —— Edge-TTS（WebSocket + WordBoundary，默认在线）：音色列表优先实时拉取官方 voices/list 接口（仅保留标准 zh-CN / en-US / ja-JP，排除方言），拉取失败回退内置 25 个音色（zh-CN 6 + en-US 17 + ja-JP 2）；合成前经 `resolve_voice` 校验配置音色有效性，无效/已下架音色自动切换为目标语言默认音色，避免 Edge 服务静默关闭连接导致无音频返回
  - `windows` —— WinRT SpeechSynthesizer（离线 fallback）
  - `azure` —— Azure 认知服务（REST + /voices/list）
  - `gpt_sovits` —— GPT-SoVITS 自托管（兼容 v1/v2）
  - `fish_speech` —— Fish Speech（fishaudio /v1/tts）
  - `minimax` —— MiniMax Speech（REST API）
- **口型同步**：`Live2DLipsync` 监听 `lipsync:start/update/stop` 事件驱动 `ParamMouthOpenY`
- **微信渠道语音消息**：LLM 在 wechat 渠道回复时返回 `voice_message: true` 标志位，后端调用 `TtsManager::synthesize_to_file` 合成音频（仅合成不播放，保存到 `%APPDATA%\Vivian\audio\`），合成成功后通过 `patch_last_assistant_entry_metadata` 回写 `kind=voice` / `audio_path` / `duration` 到对话历史元数据（确保历史刷新时仍能恢复语音气泡），前端 `chat:done` 事件携带 `voice_audio_path` / `voice_duration` 字段，复用现有 `VoiceBubble` 组件以微信风格语音气泡展示（点击可播放）。direct 渠道忽略此标志继续走实时 TTS 路径；TTS 未启用或合成失败时自动回退为文本气泡

### 远程访问（手机端 Remote Access）

`remote/` 模块在应用后台启动一个轻量 axum HTTP 服务，暴露聊天与数据接口，并托管手机端 Web 前端。配合 Tailscale 等组网工具，手机可通过组网 IP 直接访问电脑上的智能体，实现移动端远程陪伴（如把电脑留在家里、带手机上地铁继续对话）。

- **配置**（`config.yaml` → `network.remote_access`）：`enabled` 开关 + `port` 端口（默认 8080），在设置窗口「网络」页签提供滑块与端口输入。改端口/开关保存后立即生效，无需重启应用（`sync_remote_server` 幂等地启动/停止/重启服务）
- **两个独立对话界面**（复刻桌面 UI）：
  - **微信对话界面**：灰底、用户 WeChat 绿气泡靠右、AI 白色气泡靠左带头像、绿色发送键，智能体发送的链接渲染为微信风格卡片
  - **直接对话界面**：浅紫渐变底、AI 深色半透明气泡靠左，顶部为 live2d 舞台，**Vivian + Nana 双角色同屏渲染**，说话时对应角色上方弹出气泡并触发表情
- **live2d 渲染**：复用 `pixi-live2d-display`（cubism4）+ `pixi.js` + `live2dcubismcore`，模型经 `/remote/model/` 路由加载（release 从加密 bundle 解密 / dev 从 `public/` 读取），进入直接对话页签才懒加载。**显示区域统一**：缓存输入框显示时的可用高度，收起/展开聊天面板模型区域高度保持一致。**角色边界裁剪**：遍历可见 drawable 顶点计算角色真实可见边界，排除画布四周透明留白让角色填满纵向区域；Nana 因留白较多在裁剪基础上做差异化缩放/位移（放大 `×1.15×0.95×0.9` 等），Vivian 保持通用底部对齐。**双生模式**：完全沿用单角色缩放与垂直位置，x 中轴分别为屏幕宽度 `2/7`（Vivian）与 `5/7`（Nana）并排，并设置 `zIndex` 保证 Vivian 显示在上层
- **输入栏与界面**：输入框与发送按钮高度一致（38px）且垂直居中对齐，发送后不显示红色停止按钮；空状态不显示占位文本；程序坞全屏高度超出屏幕底部并在底部做模糊渐变淡出到透明，`html`/`body` 底色铺满包含安全区的整屏
- **数据管理**：记忆 / 笔记 / 待办与定时任务 / 用户画像，均提供移动端界面；记忆页过滤不渲染内部系统指令（旁观插话提示、跨角色话题总结）并做广播群发去重
- **通知与确认**：`/api/toasts` 增量拉取主动消息与工具确认请求；`/api/confirmations` 三态（拒绝/允许一次/始终允许）解决工具确认，移动端弹出确认 toast
- **完整 API**：health / characters / chat（含渠道）/ 状态查询 / history（按渠道过滤）/ memories / diary / notes / todos / tasks / profile / toasts / confirmations / 模型资源，详见 [CODE_WIKI.md](file:///g:/vivian-rs/CODE_WIKI.md) 的 remote/ 章节

### 弹性与可观测

- **后台循环看门狗**（`utils/watchdog.rs`）：scheduler / 热梗采集 / 技能热重载 / 对话刷新等常驻后台循环每轮上报心跳，守护任务（60s 周期）发现超过 3× 期望间隔（下限 120s）无心跳即判定停摆，error 级报错并调用注册的重启回调重新拉起（无回调则只报警），循环绝不静默停摆
- **步骤级熔断**（`memory/step_health.rs`）：巩固流水线等长时任务的每步独立跟踪连续失败计数，≥ 阈值转 `paused` 暂停（写入显式 `paused_reason`），冷却后半开重试；同根因错误签名只打一次 error，恢复时打恢复日志
- **损坏状态文件统一处理**（`utils/fs.rs::load_json_or_backup`）：JSON 状态文件解析失败时 error 级大声报错、将损坏现场重命名为 `.corrupt-<ts>` 保留、按空态/默认值继续（不阻断启动）；全仓状态加载点统一走该入口，杜绝静默 `.ok()` 丢弃
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

### 内存优化

- **资源按需解压**（`bundle_reader.rs` + `scripts/encrypt-assets.mjs`）：采用 VBL2 格式，bundle 内每个文件独立 zstd 压缩 + AES-256-GCM 加密，运行时仅读取请求的资源文件、解密解压到 LRU 缓存（最近 64 个文件），彻底避免整体解密解压后整包常驻内存，Live2D 纹理/动作/表情等资源仅在渲染时按需加载。
- **Input 窗口惰性创建**：启动时不再预创建广播输入窗口，改为首次使用时按需创建，避免启动即占用一个 WebView2 进程。
- **Main 控制器窗口瘦身**（[`src/main.tsx`](file:///g:/vivian-rs/src/main.tsx)）：隐藏控制器分支（`view=hidden_controller`）跳过 React 渲染、i18n 初始化与 global.css 加载，仅保留最小 Tauri IPC 桥接层，消除控制器窗口的 UI 渲染开销。
- **WebView 冻结/恢复**（[`commands/window.rs`](file:///g:/vivian-rs/src-tauri/src/commands/window.rs)）：窗口隐藏时通过 WebView2 `TrySuspend`/`Resume` 接口挂起/恢复渲染进程。chat/side_chat 窗口隐藏或探出 10px 时完全冻结渲染，释放 GPU 内存与 CPU 时间片；窗口恢复可见时通过 `visibilitychange` 事件补拉隐藏期间错过的消息与预览数据，确保数据一致性。

### 恢复出厂设置

设置窗口「通用」页的「整体操作」抽屉（汇总导出备份 / 导入备份 / 恢复出厂三项，原名"危险操作"）触发恢复：`factory_reset` 命令把应用重置到初始状态，采用「内存级清空 + 启动时目录级清扫」两段式：

1. **命令内清空**（`commands/system.rs::factory_reset`）：锁死全部 tick 行为 → 停止后台子系统 → 逐角色 `clear_all_memories` + 清空共同记忆 → 清空待办/定时任务/应用解析缓存；
2. **目录级清扫**（重启后执行）：命令在重启前写入清扫标记 `.factory_reset_pending`，应用重启后在 `AppState::new()` 之前消费该标记（此阶段数据文件尚未被打开，可无锁删除，规避 vectors.db 的 SQLite 共享冲突）。按**保留清单**（配置 `config.yaml`/`config`/`lsp.json`/`sound`、凭据与安全白名单 `.credentials.json`/`identity.json`/`trusted_apps.json`/`trusted_origins.json`、运行时基础设施 `python-libs`/`pids`/`logs`/`mcp`、用户扩展 `skills`/`plugins`）删除用户数据目录中其余的全部使用期数据与历史遗留——包括记忆、聊天历史、心理状态、日记、笔记、截图（`screenshots/`）、图片（`images/`）、编程会话、内容发现数据及历史遗留目录。角色由配置驱动注册，整树删除后按首次启动路径重建（含记忆种子），LLM/TTS 配置与技能/插件保留。

**导入备份同样走二次确认**：从「整体操作」抽屉选择 `.altn` 备份文件后，先弹出与恢复出厂设置同级别的确认弹窗（展示文件路径）再执行 `restore_user_data`——写入恢复标记并自动重启应用，重启后完成数据回填。导出备份（`backup_user_data`）选择目标目录后立即打包用户数据目录。

### 笔记本（Notebook）

内置的认知调试器前端工具，通过 Memory 窗口打开（默认全屏大小），提供 4 个顶级导航页面可视化智能体内部状态与工作台：

- **综合页（Overview）**：四个认知观察视图合并为一个入口，页内顶部胶囊子 tab 切换（保留上次选择，外部 `navigateTo` 跳转可自动定位到对应子 tab）：
  - **心智（Mind）**：核心认知调试视图，内含 4 个子视图
    - **Live Mind**：实时心智快照（Twin View，Vivian + Nana 并排展示，5 秒轮询），显示当前情绪/需求/信念/注意力/目标
    - **Mind Flow**：认知流动图（纵向推理链可视化，12 步映射到 7 个认知阶段），展示每轮对话的心理因果链
    - **Context Pipeline**：Prompt 组装分解（按 section 层级分组可视化，组内按重要性排序、分组抽屉可折叠、自动隐藏 0 字符 section），展示每个 section 的内容、token 估算、是否注入；工具信息通过 `list_tools` 命令动态加载（按当前界面语言返回工具描述，调用 `Tool::description_in(lang)`），按类别分组展示工具名/描述/参数详情（含必填标记和参数说明）
    - **Reasoning**：推理历史列表 + 详情，可追溯每轮对话的完整步骤耗时与输入输出
  - **世界（World）**：世界状态观察器，展示时间/天气/节气/节日/在场状态/室友公共状态/统一事件账本等环境数据
  - **记忆（Graph）**：记忆关系图可视化
  - **用户画像（User Profile）**：展示角色视角下的用户认知，顶部可切换角色查看不同角色对用户的印象差异。四层结构化展示——L0 基础身份（姓名/年龄/性别/职业/所在地）/ L0.5 偏好资料（生日/作息/常用网站/喜欢的游戏/兴趣爱好，支持内联编辑和锁定保护）/ L1 近期状态（最近目标/当前项目/近期偏好，只读自动抽取）/ L2 自由事实（可新增/删除）
- **创作页（Journal）**：日记、笔记、计划合并为同一入口，页内顶部手账 Tab 切换，外部 `memory:navigate` 事件（ChatWindow 笔记链接点击、记忆图谱点日记节点等）可自动定位到对应子 tab 并打开目标条目：
  - **日记（Diary）**：角色日记浏览（按心情筛选，标题栏显示角色名徽章）
  - **笔记（Notebook）**：智能体生成的卡片风格 HTML 笔记查阅窗口，左右两栏布局（左 36% 笔记列表 + 右 64% iframe 预览），顶部角色切换（Vivian/Nana）。监听 `notebook:created`/`updated`/`deleted` 事件自动刷新；支持从 ChatWindow 链接卡片点击跳转（通过 URL 参数或 `memory:navigate` 事件定位）
  - **计划（Planner）**：待办与定时任务合并页，内部通过手账 Tab 切换「待办」和「定时」。由原独立窗口迁移而来，监听 `todo:changed` / `scheduler:changed` 事件自动刷新。定时任务支持预触发机制（到期前 5 秒发起主 LLM 调用，让智能体提前决定任务处理方式）
- **工作页（Code）**：Codex 布局 + 手账风格三栏结对编程工作台（左栏会话/工作区分组管理 / 中栏对话流 / 右栏检查器+内嵌终端），详见[编程智能体](#编程智能体coding-agent)

数据源：`get_mind_state` / `get_current_mood`（Live Mind）、`get_recent_reasoning_traces`（Mind Flow / Reasoning）、`get_last_prompt_breakdown` + `list_tools`（Context Pipeline）、`get_world_state`（World）、`list_unified_events`（事件流）、`get_user_facts` / `set_user_fact` / `pin_user_fact` / `delete_user_fact`（User Profile）、`list_notebooks` / `get_notebook_html` / `get_notebook_detail` / `delete_notebook` / `import_html_note`（Notebook）等。

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

由 `IntentJudge`（`dialogue/intent_judge.rs`）判定：告别种子短语（晚安/再见/打断，含场景元数据）通过 n-gram 嵌入 Top-K 投票 + softmax 加权概率预检（命中直接返回，跳过 LLM），未覆盖的语义判断（冲突/话题切换/隐含告别等）由 LLM 通过路由矩阵的 `intent_judge` 任务完成。LLM 不可用 / 超时 / 解析失败时返回 None，由 Energy/Novelty/Continuation 状态机自然推进，避免误关闭。

| 原因 | 触发方式 | 后续行为 |
|------|---------|---------|
| `Natural` | Energy/Novelty/Continuation 自然衰减 / LLM 判断话题自然结束 | 允许主动开新话题 |
| `GoodNight` | 规则预检（晚安/睡了/去睡了） / LLM 判断 | 睡眠时间内不主动搭话 |
| `GoodBye` | 规则预检（拜拜/再见/走了） / LLM 判断 | 允许主动开新话题 |
| `NoResponse` | 主动聊天被用户忽略（`on_ignored`） / LLM 判断 | 不主动搭话直到新 Trigger |
| `Interrupted` | 规则预检（等一下/稍等/老板电话） / LLM 判断 | 用户回来可恢复旧会话 |
| `Timeout` | 用户 30 分钟无响应（`sweep_user_session_timeouts`） / LLM 判断 | 不主动搭话直到新 Trigger |
| `Conflict` | LLM 判断（争吵/冲突后结束） | — |
| `SwitchTopic` | LLM 判断（显式开启新话题） | 允许主动开新话题 |

### ResponseMode（响应决策）

LLM 在一次调用里同时返回 `response_mode`，避免每条消息都触发完整 LLM 文本回复：

| 模式 | 用途 | 主对话 | 跨角色 |
|------|------|--------|--------|
| `speak` | 正常回复（生成文本） | 默认 | 默认 |
| `non_verbal` | 只做动作/表情（点头/微笑） | 用户发"嗯/哦"时 | 对方说"嗯"时 |
| `internal` | 只更新内部想法/记忆 | 极少 | 琐碎闲聊 |
| `ignore` | 完全忽略 | 几乎不用（对用户粗鲁） | 话题结束/无关 |

LLM 还可返回 `voice_message: true`（仅 wechat 渠道生效），将该条回复以微信风格语音气泡发出而非文本——后端合成 TTS 音频后保存到本地，前端复用 `VoiceBubble` 组件展示可点击播放的语音消息。direct 渠道忽略此标志继续走实时 TTS。详见 [语音系统](#语音系统) 章节。

### 评分公式

- **Novelty**（新信息密度）：问号 +0.3 / 长度 >10 字 +0.2 / >30 字 +0.2 / jieba 实词 >3 +0.3 / 回复 >15 字 +0.1
- **Energy**（活跃度）：Speak +0.1+ΔNovelty×0.3 / NonVerbal -0.05 / Internal -0.02 / Ignore -0.3
- **Continuation Score**：0.3 + (Novelty>0.5?0.2) + Novelty×0.3 + Energy×0.2 - min(0.3, rounds×0.02) - (Energy<0.3?0.2)
- **状态转换**：Ignore 直接进 Cooling；Continuation<0.30 || Energy<0.25 || Novelty<0.15 → Cooling；否则保持 Active

### 接入点

- **User↔Agent**：`commands/chat.rs` 在 `brain.think` 前调 `start_or_continue`，think 后调 `update_after_round` + `IntentJudge.judge_close_reason`（规则预检 + LLM 意图判断） + `seal_episode_on_close`
- **Agent↔Agent**：`cross_character.rs::send` 同上，冷却中返回 `CrossCharacterReply{response_mode:"ignore"}` 不调 LLM；目标角色 think 完成后通过 `dialogue_add_with_meta` 把回复写入源角色对话历史，并通过 `add_memory_with_metadata` 写入带 `speaker/listener/perspective` 元数据的对称记忆，让源角色在下一轮 LLM 调用时能从对话历史与记忆中感知到目标角色已经回复，避免"A 总认为 B 没回复"的幻觉
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

> **多窗口前端按需加载**：桌宠由多个 Tauri 窗口组成（主 Live2D 窗口 / Chat / Memory / Config / Bubble / Toast 等）。`src/main.tsx` 按 `view` 参数对每个窗口组件做**动态 `import()`**（`React.lazy` 风格），每个窗口只加载自身代码，主窗口不再打包全部窗口代码；`vite.config.ts` 通过 `manualChunks` 将 react / tauri / i18n / pixi 等稳定依赖拆成独立 chunk（多窗口共享缓存、并行加载），并设 `target: es2022`（WebView2 常青 Chromium 无需旧浏览器转译）。其余依赖（echarts / mermaid 等）保持 Vite 默认基于动态 import 的按需拆包，不合并成巨型 vendor 包。

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
# 产物位于 src-tauri/target/release/bundle/（NSIS / MSI 安装包）
```

**美术资源加密流水线**（`beforeBuildCommand` 自动执行，无需手动干预）：

```
public/{Vivian,Nana,world-bg}
  └─ scripts/encrypt-assets.mjs  ── zstd-19 压缩 + AES-256-GCM 加密
       ├─ src-tauri/vivian.bundle.enc   （bundle 内容，tauri.conf.json resources 打包到 exe 同级）
       ├─ src-tauri/asset_key.bin       （32 字节密钥，build.rs 分 4 段混淆嵌入二进制，不入库）
       └─ src-tauri/vivian.bundle.index.json （索引，build.rs 编译期嵌入 BUNDLE_ENTRIES）
  └─ vite build 后 stripEncryptedAssets 插件自动删除 dist/{Vivian,Nana,world-bg}
       （vite 默认把 public/ 全量拷进 dist，明文副本会破坏加密意义）
```

生产运行时资源统一经自定义 `http://model.localhost/<path>` 协议加载：`bundle_reader::init` 启动时解密解压整个 bundle 到内存（仅 release），`ResourceLoader` 走 `scan_embedded` 从编译期嵌入的索引扫描（磁盘无资源文件），前端模型 URL 由 `get_model_url` 按模式分流（dev 扫磁盘目录 / release 查 bundle 索引）推导。

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
| `consolidation` | 夜间记忆巩固（睡眠时整理记忆，低频，需深度推理模型） |
| `reflection` | 异步反思（每 5 轮或 30 分钟触发，合并意识更新与活动抽取，fire-and-forget，建议便宜快速模型） |
| `inner_monologue` | 离线内心独白（用户不交互时自主思考，30 分钟一次，建议廉价快速模型） |
| `bystander_judge` | 旁观插话二值判断（轻量 LLM 仅返回 should_interject 布尔值，建议廉价快速模型） |

- 未配置的任务将回退到 LLM 主配置。
- 任务 provider 失败后自动 fallback，通过 `chat:route_fallback` 事件通知前端。
- 每个任务支持独立配置 `temperature` 和 `max_tokens`（可选，留空则回退到主 LLM 配置），便于为视觉模型等 max_tokens 上限较低的模型单独配小值。
- 主 LLM 配置默认值：`temperature = 0.70`，`max_tokens = 2048`。
- `reasoning` 任务（编程智能体 / 深度推理）未显式配置 `max_tokens` 时，按服务商分级默认（`work_model_default_max_tokens`），不沿用主配置 2048；显式配置仍优先。

> **工作智能体模型预设**（`work_models` + `active_work_model`）：在设置 → LLM 页签「工作智能体模型」区可为编程智能体预设多组模型（别名 / 服务商 / 模型 / 端点 / 密钥），当前选中在编程页模型下拉热切换。单次输出预算与 temperature 不在此配置——`max_tokens` 由后端按服务商分级默认（`work_model_default_max_tokens`，避免沿用聊天用的 2048 限制代码生成），`temperature` 请求体统一省略（服务端默认）。

### 配置方式

1. **可视化**：右键桌宠 → 设置（ConfigWindow 提供 9 个 Tab：通用 / AI / 工具 / 记忆 / 语音 / 网络 / 浏览器 / 插件 / 关于）
2. **直接编辑**：关闭 Vivian 后编辑 `config.yaml`，重启生效
3. **Tauri 命令**：`get_config` / `set_config` / `save_config` / `reload_config` / `test_llm_route`（LLM API 一键检测）/ `update_world_config`（世界感知热更新）/ `list_mcp_servers` / `add_mcp_server` / `remove_mcp_server`（MCP 管理）/ `get_worldbook_params` / `set_worldbook_params`（worldbook 调参）

> **远程访问**：`network.remote_access.enabled` 控制后台 HTTP 服务开关，`network.remote_access.port` 控制监听端口（默认 8080）。在设置窗口「网络」页签提供滑块与端口输入，保存后立即生效（见[远程访问](#远程访问手机端-remote-access)章节）。

> **真实世界感知**功能会消耗额外 Token（内心独白每 30 分钟一次 LLM 调用、夜间记忆巩固调用深度推理模型）。设置入口合并于「通用」页（原独立「感知」页签已并入）：含总开关、天气与经纬度配置，可按需关闭以节省 Token。

> **开机自动启动**：`base.auto_start` 控制是否在登录 Windows 后自动启动 Vivian。可在设置窗口「通用」页开启/关闭，保存后立即写入当前用户启动项（`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`，值名 `VivianDesktopPet`）。

> **启动预检与外部服务**：程序启动时立即执行 `startup::preflight`（不等待任何前端就绪信号）：
> 1. 检查主 LLM 与嵌入服务是否配置——任一未配置则立即打开设置窗口弹出配置指引并停止初始化；之后用户在设置中保存配置会触发 `reinitialize`，走同一套预检 + 初始化（含嵌入预加载）流程；
> 2. 嵌入配置为本地 Ollama（`memory.embedding.source = local`）时立即启动 Ollama（已在运行则直接复用），轮询等待 HTTP API 真正就绪（`GET /v1/models` 可解析，TCP 通不代表可用）并确保目标模型已安装/可拉取，全部就绪后才进入 `state.initialize()` 开始嵌入任务；
> 3. 嵌入配置为云端 API 时不启动任何本地服务，直接进入初始化；
> 4. Ollama 无法启动或模型未就绪时停止初始化并打开配置引导，避免刷出大量 `[seed] 种子记忆嵌入失败`。
>
> **Ollama 常驻策略**：由应用拉起的 Ollama 不绑定 Job Object、退出清理也不停止——应用退出（含崩溃/强杀）后 Ollama 继续存活，下次启动端口检测直接复用（秒级 vs 冷启动约 20-30s）。需要停止可在设置中手动调用。孤儿清理带 PID 复用防护（校验进程可执行名），避免误杀无关进程。

> **统一启动进度 Toast**：启动/重初始化期间会显示一个持久进度 Toast，通过 `startup:progress` 事件更新当前阶段和百分比，覆盖配置检查、Ollama 启动、模型就绪、角色初始化、种子记忆嵌入、情绪/语义语料嵌入、窗口创建等全部启动加载内容。预检（含 Ollama 启动）立即执行、不等 toast 窗口就绪，进度不丢失的保证：
> - **快照补齐**：后端持续保存最近一次进度快照（`get_startup_progress` 命令），前端挂载时先拉取快照显示占位进度；
> - **周期重发**：启动期间后台任务每 800ms 重发最新进度快照，toast 窗口任意时刻挂载都能在 800ms 内收到当前进度；
> - **单调递增**：多角色依次预加载时百分比钳制为单调递增，各嵌入阶段（情绪语料逐批、语义语料逐维度、种子记忆逐条）以当前进度为基点做区间映射，进度条不回跳；
> - **延迟创建与失败重试**：`startup_toast` 窗口延迟 800ms 后异步创建，创建失败（如快速重启时上一实例的 WebView2 子进程仍持有 user data folder 锁触发 `ERROR_BUSY`）会自动退避重试（10 次 × 800ms），避免窗口 WebView 创建失败导致进度 toast 完全无法显示；
> - **穿透固定窗口**：`startup_toast` 与普通角色 toast 窗口对齐——置顶、定位屏幕右上角、高度撑满屏幕、隐藏任务栏且不抢焦点，并设为点击穿透（`set_ignore_cursor_events`）+ 固定大小（`resizable=false`），不遮挡屏幕右上区域的鼠标操作且不可被拖拽改尺寸；

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

前端支持简体中文（zh-CN）、English（en）、日本語（ja）三种语言，通过 i18next 管理。语言选择保存到 `localStorage['vivian-lang']`，fallback 到 zh-CN。后端 i18n 模块内置中英文翻译表，支持点号分隔嵌套键。所有 LLM 提示词（主对话功能模块、记忆系统、主动交互、对话处理、心智/信念生成、日记生成、工具反馈路径等 40+ 个任务）均实现三语覆盖，通过 `normalize_lang` + `match lang_norm { "en" => ..., "ja" => ..., _ => ... }` 统一模式选择语言，`_` 分支为中文兜底。所有 JSON 字段名、枚举值保持英文不变以确保下游解析正常。对话/记忆格式统一使用第一人称说话者标记 `[User says to me]` / `[I say to User]`，与记忆存储前缀对齐。

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

- **characters/**（角色层，双角色独立，每角色 11 个文件）：
  - `identity.md`：核心身份锚点（你是谁）
  - `personality.md`：场景化人格（采用"触发→反应"行为脚本，用具体场景替代形容词堆砌）
  - `speech.md`：说话节奏/语气/口头禅/禁用模式（含自称、句尾、停顿习惯）
  - `examples.md`：角色专属 few-shot 示例（约 5 个，避免模型模仿特定句子）
  - `background.md`：背景设定（日常生活/作息/环境）
  - `interests.md`：兴趣爱好
  - `relationships.md`：与用户/室友的关系设定
  - `appearance.md`：外观描述
  - `scenes.md`：场景脚本
  - `canon_quotes.md`：经典台词
  - `seed_memories.md`：角色前史记忆（入住用户电脑之前的人生片段，见[角色前史](#-角色前史seed_if_empty)）
- **framework/**（框架层，所有角色共享，8 个文件，规则内容统一英文 + SCREAMING_SNAKE 标记化）：
  - `safety.en.md`：安全规则（`[SAFETY_RULES]`：NO_AI_DISCLOSURE / MEMORY_ONLY_HISTORY / CROSS_CHAR_VIA_TOOL 等 10 条硬约束 + `[SEARCH_TRIGGERS]` 搜索触发条件）
  - `output_format.en.md`：JSON 输出格式规范（`[OUTPUT_FIELDS]` 字段块 + 完整示例对话，示例为语气锚点不压缩）
  - `pet_identity.en.md`：桌面宠物身份与能力边界（`[CAPABILITY_BOUNDARY]` CAN/CANNOT + `[ROOMMATE_SAME_BOUNDARY]`）
  - `session_rules.en.md`：会话规则（`[SESSION_RULES]` 新会话/续聊/时间感知）
  - `address_rules.en.md`：称呼规则（`[ADDRESS_RULES]` 频率/场景/裸名）
  - `conversation_rhythm.en.md`：对话节奏（`[RHYTHM_RULES]` 短回复=确认/沉默合法/emoji 克制）
  - `speaker_prefix.en.md`：说话者前缀标记（`[User says to me]` 等）
  - `persona_protocol.md`：人格配置协议（§1–§5 英文注入 Character 块：`[PERSONA_CONFIG]` 解析规则 / 三层结构 / 优先级链 / `[PROTOCOL_GUARD]` / `[EXEC_RULES]`；§6–§7 后端约定不注入）
  - 聊天风格框架（chat_style）为内联英文常量 `chat_style_framework()`（`[CHAT_STYLE_RULES]` 7 条），非独立文件
- **styles/**（5 个）：说话风格切换预设（default / lively / healing / focused / sweet）
- **worldbook/**（3 个）：背景知识触发（game_culture / internet_culture / anime_culture）
- **system_prompt.tera**：Tera 模板入口

**Prompt 架构原则**：
- **U 型注意力调度**：静态区 Character 开头（最先入脑）→ Framework/Format 末尾（临出口提醒）；动态区**记忆组上移至 Mind 之后**（黄金位置，避开 U 型谷底），利用 LLM 注意力偏置提升人格稳定性、格式准确率与记忆利用
- **静态/动态分离**：静态内容（人格/框架/示例/伪静态段落）用 `<static>` 标签包裹，动态内容（心智/记忆/世界/画像）在后，`</static>` 后输出 `__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__` 边界标记供 generation 层把动态区切为 user 便签复用前缀缓存
- **分组合并**：记忆组（Episode + 关系日志 + 记忆本体）与画像组（用户事实 + 认知模型 + 动态行为）分别合并为单一 section，顶层动态 section 从 30+ 收敛到约 10 个，减少独立标题的注意力稀释
- **预算裁剪**：动态区每个 section 带裁剪优先级，总体超过 30KB 时按 rank 丢弃低价值段（语气注入/推荐工具/社交状态先丢），记忆组/环境/工具/用户输入永不丢弃
- **规则英文化 + 标记化**：framework 规则（安全/输出格式/能力边界/会话/称呼/节奏/风格/响应决策/渠道/在场指南）统一英文 SCREAMING_SNAKE 标记（`[SAFETY_RULES]` `[OUTPUT_FIELDS]` `[RESPONSE_MODES]` 等），回复语言由 "same language as user input" + `LANG_*` 语言标志控制；记忆条目元数据自然语言化（去 `imp=/mood=` 数值符号，重要性用 `[重点]` 表达）
- **功能提示词动态化**：心理洞察/信念生成/思维合成/日记生成/记忆提取等功能模块全部使用角色名变量，支持多角色架构
- **行为化语音指南**：跨角色对话时注入角色专属行为约束（"你说话比她快，句子更短"），替代数值化标签（如 sass=0.65）
- **内心反应中文化+角色化**：第一人称内心想法使用中文生成，按角色差异化（Vivian 直率吐槽 / Nana 温柔关心）
- **功能性任务提示词三语覆盖**：功能模块 LLM 提示词（记忆抽取/验证/路由/巩固、主动交互行为/破冰/回忆/内心独白、对话意图判断/共指消解/策略摘要、心智信念生成/用户认知、日记生成、情绪分类、查询重写等 40+ 个任务）均实现中/日/英三语，通过 `normalize_lang` 统一切换；framework 规则层例外（统一英文，见上）。说话者标记全项目统一为 `[User says to me]` / `[I say to User]` 第一人称格式
- **数据源编排优化**：记忆上下文 token 预算 1250 + 类型标签（印象/近期/已读）+ 重要性排序（`[重点]` 标记高重要度）；反思/异步反思注入对话历史和 AI 回复；AugmentReplyService 记忆按重要性升序排序（重要的排在 LLM 注意力更佳的末尾）；日记对话片段附带时间戳 + 截断 150 字符；记忆验证/用户事实抽取注入已有数据避免重复

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
- 技能目录位于 `%APPDATA%\Vivian\skills\`（`*.md`，变更 30 秒内自动热重载；`create_skill` 写入即注册）
- 自建工具目录位于 `%APPDATA%\Vivian\tools\`（`*.json`，定义含 name/description/parameters/script/deferred；`create_tool` 创建，30 秒内自动热重载）
- 浏览器可信来源白名单位于 `%APPDATA%\Vivian\trusted_origins.json`（变更自动热重载）
- 关系演化日志位于 `%APPDATA%\Vivian\psychology\relationship_log.json`
- 可通过 `tool_observability` 功能开关查看工具调用详情

---

## 故障排查

| 现象 | 可能原因 | 解决方案 |
|------|---------|---------|
| 启动后无问候 | 主 LLM API 未配置 | 在设置窗口配置 routing_matrix.chat 的 api_key / endpoint / model |
| 子窗口无法打开 | capabilities 权限缺失 | 检查 [capabilities/default.json](file:///g:/vivian-rs/src-tauri/capabilities/default.json) 是否包含对应窗口标签 |
| TTS 无声 | 后端未正确配置 | 检查 TTS 配置，或切换为 `windows` 后端（离线） |
| 联网搜索失败 | 代理配置或网络问题，DuckDuckGo 国内被墙 | 配置 Bing Search API Key（国内直连），或在设置中启用代理；检查 `network.proxy_mode` 与 `HTTPS_PROXY` 环境变量 |
| 浏览器扩展连接不稳定/掉线 | 桥连接被 Chrome MV3 service worker 空闲终止（约 30s 无活动） | 扩展在 `chrome://extensions/` 重新加载后生效（manifest 增加了 `alarms` 权限）；连接由服务端 20s ping + 扩展 20s 心跳 + 1 分钟 alarm 唤醒兜底，逐层防掉线，与系统默认浏览器无关 |
| 点击「打开扩展管理页 / 去登录」没反应 | `chrome://` 非系统注册协议；或登录发生在非扩展所在的浏览器 | 两个入口均由应用定位 Chrome 可执行文件带参启动（Windows 走 App Paths 注册表 + 标准安装目录），不依赖系统默认浏览器；登录页必须用 Chrome 打开，登录态（Cookie）才会被扩展的 Cookie 哨兵识别；未安装 Chrome 时面板会显示错误提示 |
| 记忆检索慢 | 嵌入未启用或向量库过大 | 启用 `memory.embedding.source` 并配置嵌入模型，或调小 retrieval_weights |
| 外部向量库（Qdrant）不可用 | Qdrant 未启动或地址/认证错误 | 在设置「记忆」页签确认 `memory.vector_store.source=external` 下地址/api_key 正确、Qdrant 已运行；`local` 模式无需外部服务 |
| 智能精排不生效 | 未启用或 Ollama 未装 rerank 模型 | 在设置「记忆」页签开启 `memory.rerank.enabled` 并 `ollama pull bge-reranker-v2-m3`；未启用会自动回退原排序 |
| 工具调用卡住 | 超时或权限被拒 | 查看 `tool:confirmation_request` 事件与 `metrics.json`；编程页工作区写文件被拒时确认会话权限为 `workspace_write` / `full_access`（`read_only` 会拒绝写入） |
| 智能避让不工作 | 配置关闭或屏幕无变化 | 检查 `window.smart_positioning_enabled` 是否为 true；无变化时轮询间隔自动延长 |
| 天气感知失效 | Open-Meteo 不可达或经纬度未配置 | 检查 `world.enable_weather` 与 `world.latitude` / `world.longitude`；失败时按"不知道"处理，不阻断其他功能 |
| 内心独白不生成 | 路由矩阵未配置 inner_monologue 任务 | 在设置窗口 → 通用页「真实世界感知」区确认 `enable_inner_monologue` 开启，并在 AI 页签为 `inner_monologue` 任务配置廉价快速模型 |
| 记忆巩固未执行 | 不在睡眠窗口或冷却未到期 | 确认 `world.enable_memory_consolidation` 开启；巩固仅在 `sleep_start_hour`..`sleep_end_hour` 窗口内且距上次 ≥ 6 小时触发 |
| 启动后窗口数量异常（多于角色数） | main 控制器窗口被错误加载 | main 窗口（`label="main"`）是隐藏控制器，不应可见；检查 `main.tsx` 是否对 `label="main"` 跳过加载 `App.tsx` |
| 角色窗口右键菜单无法打开子窗口 | 子窗口 label 与其他角色冲突 | 子窗口 label 需按角色区分（格式 `<char_id>_<base>`，如 `nana_chat`、`vivian_status`），避免多角色窗口 label 撞车 |
| 某角色模型未加载 | 模型目录缺失或未打包 | dev：检查 `public/<ModelName>/` 目录是否存在且含 `*.model3.json`（文件名大小写可能与目录不同，如 `Nana/` 下是 `Vivian.model3.json`）；release：重新运行 `npm run encrypt:assets` 确认 `vivian.bundle.enc` 存在且 `tauri.conf.json` 的 `bundle.resources` 已包含它；启动日志查看 `[bundle] 初始化完成` 与 `[get_model_url]` 推导的 URL |
| 多角色心情状态串扰（A 角色心情影响 B 角色面板） | 全局静态未清理或事件未按角色过滤 | 检查 `commands/emotion.rs` 的 `LAST_TRIGGER` 是否按 `char_id` 索引、`psychology:state` 事件 payload 是否携带 `character_id` 字段；前端 StatusPanel 是否按 `character_id` 过滤事件；`tools/emotional_recovery.rs` 的 `EMOTIONAL_STATE` 是否按 `char_id` 索引 |
| 手机端无法访问远程服务 | 远程访问未开启或端口被占用 | 在设置「网络」页签开启 `network.remote_access.enabled`，确认 `port` 未被其他程序占用；若修改端口后未生效，检查是否已点击保存（`save_config` 触发服务重启） |
| 手机端 live2d 模型不显示 | 模型未打包或跨域/加载失败 | 确认 `public/Vivian/`、`public/Nana/` 含合法模型文件；远程访问需在 release 下从 bundle 提供模型（dev 从 `public/` 读取），且 `vivian.bundle.enc` 与 exe 同级或位于 `resources/`；浏览器控制台查看 `/remote/model/` 资源是否 404 |

---

## 联系方式

- **项目地址**：https://github.com/SpacervalLam/Vivian-ai-desktop-pet
- **联系邮箱**：spacervallam@gmail.com

Bug 报告与功能建议请优先通过 [GitHub Issues](https://github.com/SpacervalLam/Vivian-ai-desktop-pet/issues) 提交；其他事宜可通过邮件联系。

---

## 许可证

[MIT License](file:///g:/vivian-rs/LICENSE)

Copyright (c) 2026 SpcervalLam