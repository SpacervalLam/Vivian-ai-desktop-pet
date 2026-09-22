use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::brain::scheduler::Scheduler;
use crate::brain::Brain;
use crate::config::ConfigManager;
use crate::engine::animation::AnimationManager;
use crate::engine::expression::ExpressionManager;
use crate::engine::manifest::ResourceManifest;
use crate::engine::resource_loader::ResourceLoader;
use crate::engine::state_machine::StateMachine;
use crate::error::{VivianError, VivianResult};
use crate::memory::{MemoryManager, VerifierLlmClient};
use crate::pet_controller::PetController;
use crate::providers::ModelRouter;
use crate::speech::{AsrConfig, AsrManager, RealtimeVoiceManager};
use crate::tools::McpManager;
use crate::tools::ToolSystem;
use crate::world::WorldStateProvider;

/// 单个角色实例 — 每个角色拥有独立的 Brain、PetController、manifest 等
#[derive(Clone)]
pub struct CharacterInstance {
    pub id: String,
    pub name: String,
    pub brain: Brain,
    pub pet_controller: Arc<PetController>,
    pub manifest: Arc<ResourceManifest>,
    pub realtime_voice: Arc<RealtimeVoiceManager>,
    /// 目标服务（包住 `brain.mind.goals`，增改目标时广播 `goal/*` 类型事件）
    pub goal_service: Arc<crate::mind::GoalService>,
    pub online: Arc<RwLock<bool>>,
    pub think_lock: Arc<tokio::sync::Mutex<()>>,
}

pub struct AppState {
    pub config: Arc<RwLock<ConfigManager>>,
    /// All character instances (indexed by ID)
    pub characters: Arc<RwLock<HashMap<String, CharacterInstance>>>,
    /// Currently active character ID
    pub active_character_id: Arc<RwLock<String>>,
    // Shared fields
    pub model_router: Arc<RwLock<Option<ModelRouter>>>,
    pub tool_system: Arc<ToolSystem>,
    /// 浏览器自动化桥：token 认证的本地回环 WS 服务，`browser_*` 工具经它控制用户 Chrome。
    pub browser_bridge: Arc<crate::browser_bridge::BridgeState>,
    /// 技能服务（ctx.skills）：可复用微技能的注册/查询，注册进运行时，跨角色共享
    pub skill_service: Arc<crate::skills::SkillService>,
    /// 自治任务服务（ctx.tasks）：LLM 驱动的多步工具任务循环，注册进运行时
    pub task_service: Arc<crate::brain::TaskService>,
    /// 按角色索引的生成取消标志（消除跨角色取消干扰）
    pub generation_cancel: Arc<RwLock<HashMap<String, bool>>>,
    pub asr: AsrManager,
    pub scheduler: Arc<Scheduler>,
    /// 文字输入快捷键跟踪：key 为角色标识（"vivian"/"nana"/"broadcast"），value 为已注册的快捷键字符串
    pub text_shortcuts: parking_lot::Mutex<HashMap<String, String>>,
    /// 窗口快捷键跟踪：key 为动作标识（"chat"/"settings"/"memory"），value 为已注册的快捷键字符串
    pub window_shortcuts: parking_lot::Mutex<HashMap<String, String>>,
    /// 长按计时器：按下文字快捷键时启动，松开时取消；满 400ms 触发语音输入（仅 vivian/nana）
    pub voice_shortcut_timer: parking_lot::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    /// 当前按住快捷键的角色（互斥锁）：
    /// 按下任一角色快捷键后占用，松开该角色时释放；期间其他角色快捷键按下事件被忽略，
    /// 避免多角色快捷键并发按下导致计时器句柄被覆盖、无法 abort 的问题。
    pub active_shortcut_role: parking_lot::Mutex<Option<String>>,
    /// 初始化完成标志：仅在 `initialize()` 全部完成（含种子记忆注入、
    /// 情绪/语义语料嵌入预加载）后置位，供 `is_initialized` 查询。
    pub initialized: Arc<AtomicBool>,
    /// 串行化设置保存触发的完整重初始化，避免并行构建多套 Brain/语料索引造成内存峰值。
    pub reinitialize_lock: tokio::sync::Mutex<()>,
    /// 调度主循环已启动标志：`initialize()` 可能因 reinitialize 重跑，
    /// 常驻的 scheduler.run() 循环只允许 spawn 一次。
    scheduler_loop_spawned: AtomicBool,
    pub mcp_manager: Arc<McpManager>,
    /// 推理轨迹存储（按角色索引，用于 Mind Inspector 前端）
    ///
    /// 持有全局 `TRACE_STORE` 单例的 `Arc` clone，`BrainChatChain::ainvoke`
    /// 通过同一单例写入，这里供 Tauri 命令读取。
    pub reasoning_traces: crate::mind::reasoning_trace::SharedTraceStore,
    /// 恢复出厂设置进行中标志
    ///
    /// 一旦置位，所有前端定时器驱动的 tick 命令（proactive_tick / psychology_micro_tick /
    /// mood_expression_tick / auto_expression_tick）立即返回跳过，避免在数据重置期间产生新数据。
    /// 进程重启后随 AppState 一起销毁，自然恢复为 false。
    pub factory_reset_in_progress: Arc<AtomicBool>,
    /// 记忆向量重建进行中标志
    ///
    /// 切换嵌入模型后后台重建全部记忆向量期间置位，`proactive_tick` 等
    /// 会产生记忆的自主交互命令立即返回跳过，避免重建期间写入干扰。
    /// 重建完成（或进程重启）后恢复为 false。
    pub rebuild_in_progress: Arc<AtomicBool>,
    /// TTS 播放边界感知门控（供主动消息投递前检查是否正在播放）
    pub playback_gate: crate::utils::PlaybackGate,
    /// 会话状态机：单点收口 session_id 设置 + sticky preempt
    pub session_coordinator: crate::utils::SessionCoordinator,
    /// 主动交互 leader 选举协调器
    pub leader_coordinator: crate::utils::ProactiveLeaderCoordinator,
    /// 全局共享的世界状态提供者：天气/音乐/音量/前台窗口/网络状态/系统指标
    /// 跨角色共享同一份缓存与 Windows Hook，避免 N 个角色启动 N 套监听循环。
    /// 各角色的 ActivityJournal 通过 set_foreground_listener 订阅前台事件，独立记录。
    pub world_provider: Arc<WorldStateProvider>,
    /// Cordis 式运行时上下文：服务注册表 + 类型化事件总线 + 作用域。
    ///
    /// 作为统一的能力缝挂载点，供后续工具流水线（pre/post-execute、guard、
    /// approval、sandbox）、目标/规划、技能等插件式能力接入。
    pub ctx: Arc<crate::cordis::RuntimeContext>,
}

impl AppState {
    pub fn new() -> Self {
        let config = ConfigManager::new();
        let asr_config = AsrConfig::from_speech_config(&config.get_all().speech_recognition);
        let tool_config = &config.get_all().tools;
        let tool_system = Arc::new(ToolSystem::with_tool_config(
            tool_config.cache_ttl_secs,
            tool_config.cache_max_size,
            tool_config.enable_cache,
            tool_config.confirmation_timeout_secs,
        ));
        // 用户禁用的工具（设置-工具的开关状态），按陪伴侧 / 工作侧分别注入
        tool_system.set_disabled_tools(
            tool_config.disabled_tools.companion.clone(),
            tool_config.disabled_tools.work.clone(),
        );
        // 浏览器自动化桥：创建即注入到 browser_* 工具可读的全局，供其派发操作。
        let browser_bridge = crate::browser_bridge::BridgeState::new();
        crate::browser_bridge::tools::set_bridge(Arc::clone(&browser_bridge));
        let scheduler = Arc::new(Scheduler::new(true));
        let mcp_manager = Arc::new(
            McpManager::new(Arc::clone(&tool_system)).unwrap_or_else(|e| {
                tracing::warn!(
                    error = %e,
                    "[state] MCP manager 初始化失败，降级为临时目录模式（配置不持久化）"
                );
                McpManager::new_disabled(Arc::clone(&tool_system))
            }),
        );

        // 全局共享的 WorldStateProvider：跨角色共用一份天气/音乐/音量/前台/网络监听
        // 避免 N 个角色启动 N 套 Windows Hook 与 HTTP 请求
        let world_cfg = config.get_all().world.clone();
        let world_provider = Arc::new(WorldStateProvider::new(world_cfg));
        if config.get_all().world.enable_weather {
            world_provider.set_weather_source(Arc::new(crate::world::WeatherSource::new()));
        }
        world_provider.set_music_source(Arc::new(crate::world::MusicSource::new()));

        // Cordis 式运行时上下文：后续把共享服务（tool_system / world_provider /
        // scheduler / mcp_manager / model_router）以"服务"方式挂到 ctx 上，
        // 并把工具流水线策略注册为 ctx 事件总线上的监听器。
        let ctx = Arc::new(crate::cordis::RuntimeContext::new());
        // 注册共享服务到运行时（可逆注册；进程生命周期内持存）
        {
            ctx.set_service(Arc::clone(&tool_system));
            tool_system.set_policy_ctx(Arc::clone(&ctx));
            // 挂载默认策略：guard 纵深扫描 + post-execute 结果脱敏
            let _ = crate::tools::register_default_policies(&ctx);
        }
        // 记录为进程级全局 ctx，供不穿透构造链的组件（如 Goals/Skills 服务）取用
        crate::cordis::set_global((*ctx).clone());
        // 技能服务：注册进运行时，作为 ctx.skills 能力缝
        let skill_service = crate::skills::SkillService::new();
        // 装载默认技能目录（<用户数据目录>/skills），启动即加载目录化技能
        skill_service.load_default_dir();
        // 热刷新：后台定期对比目录指纹，新增/修改 *.md 自动重载，无需重启
        skill_service.spawn_hot_reload(std::time::Duration::from_secs(30));
        ctx.set_service((*skill_service).clone());
        // 插件贡献点装载（<用户数据目录>/plugins/*/plugin.json）：
        // 技能注册进 SkillService（命名空间隔离）；MCP 声明合并进 servers.json
        // —— 必须早于 initialize() 内的 mcp_manager.init_all()，插件 server 才会被连接
        {
            // 先播种内置插件（llm-providers：供应商预设 + 预设核对技能），
            // 首次启动或版本升级时写入用户插件目录，随后随 load_all 一并装载
            crate::plugins::ensure_builtin_plugins();
            let report = crate::plugins::load_all(&skill_service, &mcp_manager);
            if !report.plugins.is_empty() {
                tracing::info!(
                    "[state] 插件装载完成：{} 个插件，{} 条技能，{} 个 MCP server（跳过: {:?}）",
                    report.plugins.len(),
                    report.skills.len(),
                    report.mcp_servers.len(),
                    report.skipped
                );
            }
        }
        // 自治任务服务：注册进运行时，作为 ctx.tasks 能力缝
        let task_service = crate::brain::TaskService::new();
        ctx.set_service((*task_service).clone());
        // 同步注册全局句柄，供管线步骤（陪伴上下文注入任务状态/报告）访问
        crate::brain::task_service::set_global(Arc::clone(&task_service));

        Self {
            config: Arc::new(RwLock::new(config)),
            characters: Arc::new(RwLock::new(HashMap::new())),
            active_character_id: Arc::new(RwLock::new(String::new())),
            model_router: Arc::new(RwLock::new(None)),
            tool_system,
            browser_bridge,
            skill_service,
            task_service,
            generation_cancel: Arc::new(RwLock::new(HashMap::new())),
            asr: AsrManager::new_with_config(asr_config),
            scheduler,
            text_shortcuts: parking_lot::Mutex::new(HashMap::new()),
            window_shortcuts: parking_lot::Mutex::new(HashMap::new()),
            voice_shortcut_timer: parking_lot::Mutex::new(None),
            active_shortcut_role: parking_lot::Mutex::new(None),
            initialized: Arc::new(AtomicBool::new(false)),
            reinitialize_lock: tokio::sync::Mutex::new(()),
            scheduler_loop_spawned: AtomicBool::new(false),
            mcp_manager,
            reasoning_traces: crate::mind::reasoning_trace::TRACE_STORE.clone(),
            factory_reset_in_progress: Arc::new(AtomicBool::new(false)),
            rebuild_in_progress: Arc::new(AtomicBool::new(false)),
            playback_gate: crate::utils::PlaybackGate::new(),
            session_coordinator: crate::utils::SessionCoordinator::new(),
            leader_coordinator: crate::utils::ProactiveLeaderCoordinator::new(),
            world_provider,
            ctx,
        }
    }

    /// 恢复出厂设置是否进行中
    pub fn is_factory_reset_in_progress(&self) -> bool {
        self.factory_reset_in_progress.load(Ordering::SeqCst)
    }

    /// 标记恢复出厂设置开始 / 结束
    pub fn set_factory_reset_in_progress(&self, value: bool) {
        self.factory_reset_in_progress.store(value, Ordering::SeqCst);
    }

    /// 记忆向量重建是否进行中
    pub fn is_rebuild_in_progress(&self) -> bool {
        self.rebuild_in_progress.load(Ordering::SeqCst)
    }

    /// 标记记忆向量重建开始 / 结束
    pub fn set_rebuild_in_progress(&self, value: bool) {
        self.rebuild_in_progress.store(value, Ordering::SeqCst);
    }

    /// 初始化 PetController 及其依赖的引擎管理器
    ///
    /// 返回 (PetController, ResourceManifest)，manifest 供调用方按角色管理。
    fn init_pet_controller(model_name: &str) -> (PetController, Arc<ResourceManifest>) {
        let base_dir = crate::utils::path::get_resource_dir();
        let resource_loader = Arc::new(ResourceLoader::new(base_dir, model_name));
        resource_loader.load();

        let manifest = Arc::new(ResourceManifest::from_loader(&resource_loader));

        let animation_manager = Arc::new(AnimationManager::new(resource_loader.clone()));
        let expression_manager = Arc::new(ExpressionManager::new(resource_loader.clone()));
        expression_manager.set_manifest(manifest.clone());
        let state_machine = Arc::new(StateMachine::new(
            animation_manager.clone(),
            expression_manager.clone(),
            resource_loader.clone(),
        ));

        let pc = PetController::new();
        pc.set_managers(
            Some(animation_manager),
            Some(expression_manager),
            Some(state_machine),
        );
        pc.set_resource_loader(resource_loader);
        (pc, manifest)
    }

    /// 获取指定角色的实例 clone（None = 活跃角色）
    pub fn get_character(&self, character_id: Option<&str>) -> Result<CharacterInstance, String> {
        let id = character_id
            .map(String::from)
            .unwrap_or_else(|| self.active_character_id.read().clone());
        self.characters
            .read()
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("角色未找到: {}", id))
    }

    /// 重置指定角色的生成取消标志
    pub fn reset_generation_cancel(&self, char_id: &str) {
        self.generation_cancel.write().insert(char_id.to_string(), false);
    }

    /// 设置指定角色的生成取消标志
    pub fn set_generation_cancel(&self, char_id: &str, value: bool) {
        self.generation_cancel.write().insert(char_id.to_string(), value);
    }

    /// 查询指定角色的生成取消标志
    pub fn is_generation_cancelled(&self, char_id: &str) -> bool {
        self.generation_cancel.read().get(char_id).copied().unwrap_or(false)
    }

    /// 获取活跃角色的 Brain clone
    pub fn brain(&self) -> Result<Brain, String> {
        self.get_character(None).map(|c| c.brain)
    }

    /// 获取活跃角色的 MemoryManager
    pub fn memory(&self) -> Result<Arc<MemoryManager>, String> {
        self.get_character(None).map(|c| c.brain.memory.clone())
    }

    /// 获取活跃角色的 PetController
    pub fn pet_controller(&self) -> Result<Arc<PetController>, String> {
        self.get_character(None).map(|c| c.pet_controller)
    }

    /// 获取活跃角色的 think_lock
    pub fn think_lock(&self) -> Result<Arc<tokio::sync::Mutex<()>>, String> {
        self.get_character(None).map(|c| c.think_lock)
    }

    /// 获取活跃角色的 RealtimeVoiceManager
    pub fn realtime_voice(&self) -> Result<Arc<RealtimeVoiceManager>, String> {
        self.get_character(None).map(|c| c.realtime_voice)
    }

    pub async fn initialize(&self) -> VivianResult<()> {
        let config = self.config.read().get_all();

        crate::startup::emit_progress(50, 100, "正在初始化模型路由与工具系统…");

        // Shared ModelRouter
        let router = ModelRouter::new(&config)?;
        let verifier_llm: Arc<dyn VerifierLlmClient> = Arc::new(router.clone());

        // Register builtin tools to the shared tool_system
        crate::tools::builtin::register_builtin_tools(&self.tool_system);

        // 插件贡献的工具（tools/*.json → DynamicTool）：须在内置工具注册之后
        // （防影子化校验才有效）、自建工具装载之前（同名时自建覆盖插件）
        let plugin_tools = crate::plugins::load_all_tools(&self.tool_system);
        if !plugin_tools.is_empty() {
            tracing::info!("[state] 已装载 {} 个插件工具: {:?}", plugin_tools.len(), plugin_tools);
        }

        // 自建工具（智能体 create_tool 沉淀的 PowerShell 工具）：
        // 启动装载历史工具 + 注册 create_tool 元工具 + 目录热重载
        let custom_loaded = crate::tools::custom_tools::load_all(&self.tool_system);
        if !custom_loaded.is_empty() {
            tracing::info!("[state] 已装载 {} 个自建工具: {:?}", custom_loaded.len(), custom_loaded);
        }
        self.tool_system.register_tool(std::sync::Arc::new(
            crate::tools::builtin::tool_tools::CreateToolTool::new(self.tool_system.clone()),
        ));
        // 插件创建（运行时插件创造：打包技能/工具/MCP/预设为完整插件，落盘即装载）
        self.tool_system.register_tool(std::sync::Arc::new(
            crate::tools::builtin::plugin_tools::CreatePluginTool::new(
                self.skill_service.clone(),
                self.mcp_manager.clone(),
                self.tool_system.clone(),
            ),
        ));
        self.tool_system.register_tool(std::sync::Arc::new(
            crate::tools::builtin::plugin_tools::DeletePluginTool::new(
                self.skill_service.clone(),
                self.mcp_manager.clone(),
                self.tool_system.clone(),
            ),
        ));
        crate::tools::custom_tools::spawn_hot_reload(
            self.tool_system.clone(),
            std::time::Duration::from_secs(30),
        );

        // Connect MCP servers
        let mcp_manager = self.mcp_manager.clone();
        tauri::async_runtime::spawn(async move {
            mcp_manager.init_all().await;
            mcp_manager.start_health_check_loop();
        });

        // Scheduler
        crate::tools::builtin::todo_tools::set_scheduler(self.scheduler.clone());
        let tool_system_for_cb = self.tool_system.clone();
        self.scheduler
            .set_callback(std::sync::Arc::new(move |task| {
                let tool_system = tool_system_for_cb.clone();
                tauri::async_runtime::spawn(async move {
                    crate::tools::builtin::todo_tools::handle_task_trigger(task, tool_system).await;
                });
            }));

        // 预触发回调：在 scheduled_time - 5s 发起主 LLM 调用，
        // 把定时任务内容说明作为 user_input 注入完整提示词，
        // 让智能体提前决定如何进行该定时任务。
        let characters_for_pre = self.characters.clone();
        let active_char_id_for_pre = self.active_character_id.clone();
        self.scheduler
            .set_pre_trigger_callback(std::sync::Arc::new(move |task| {
                let characters = characters_for_pre.clone();
                let active = active_char_id_for_pre.clone();
                tauri::async_runtime::spawn(async move {
                    // 解析 char_id：优先任务记录的 char_id，回退到当前激活角色
                    let char_id = if !task.char_id.is_empty() {
                        task.char_id.clone()
                    } else {
                        active.read().clone()
                    };

                    // 从角色表中获取 brain
                    let brain = {
                        let chars = characters.read();
                        chars.get(&char_id).map(|c| c.brain.clone())
                    };

                    let brain = match brain {
                        Some(b) => b,
                        None => {
                            tracing::warn!(
                                task_id = %task.id,
                                char_id = %char_id,
                                "[Scheduler] 预触发任务找不到角色，跳过 LLM 调用"
                            );
                            return;
                        }
                    };

                    crate::tools::builtin::todo_tools::handle_task_pre_trigger(task, brain).await;
                });
            }));

        // 调度主循环是常驻任务（仅 shutdown 唤醒退出）：reinitialize 会重跑
        // initialize()，不设守卫的话每次改配置都会多 spawn 一个循环，
        // 并存循环会重复触发到期任务。进程内只允许启动一次。
        if !self.scheduler_loop_spawned.swap(true, Ordering::SeqCst) {
            let scheduler_for_run = self.scheduler.clone();
            tauri::async_runtime::spawn(async move {
                scheduler_for_run.run().await;
            });
        }

        // 为每个角色创建实例
        //
        // 容错策略：单角色初始化失败（如 SQLite 打不开、模型加载失败）只跳过该角色，
        // 不影响其他角色初始化。此前用 `?` 直接返回 Err 会导致循环中断——
        // Nana 的 SQLite 失败会让后续 Vivian 也不注册，表现为"偶发只创建一个窗口"。
        let characters_config = config.characters.clone();
        let mut active_id = String::new();
        let mut failed_ids: Vec<String> = Vec::new();
        // 先在局部表完成整套构建，全部结束后再一次性替换运行中快照。
        // 这样旧角色在重初始化期间仍可服务请求，失败也不会留下半新半旧状态。
        let mut new_characters: HashMap<String, CharacterInstance> = HashMap::new();

        let total_chars = characters_config.list.len().max(1);
        crate::startup::emit_progress(55, 100, "正在初始化角色记忆与大脑…");

        for (index, entry) in characters_config.list.iter().enumerate() {
            let char_config = self.config.read().get_all();

            let char_progress = 55 + (index * 25 / total_chars);
            crate::startup::emit_progress(
                char_progress,
                100,
                &format!("正在初始化角色 {}（记忆 / 大脑 / 语料预加载）…", entry.id),
            );

            // 单角色初始化结果：Ok(instance) 或 Err（跳过该角色）
            let result: Result<CharacterInstance, String> = async {
                // 每角色独立的 MemoryManager
                let memory = MemoryManager::new(&char_config, &entry.id)
                    .map_err(|e| format!("记忆系统: {e}"))?;
                let memory_for_rt = memory.clone();

                // 每角色独立的 PetController + manifest
                let (pc, manifest) = Self::init_pet_controller(&entry.model_dir);
                pc.set_character_id(entry.id.clone());
                let pet_controller = Arc::new(pc);

                // Independent Brain for each character
                let brain = Brain::new_with_pet_controller(
                    char_config.clone(),
                    router.clone(),
                    memory.clone(),
                    pet_controller.clone(),
                    manifest.clone(),
                    &entry.id,
                    self.tool_system.clone(),
                    self.world_provider.clone(),
                )
                .await
                .map_err(|e| format!("Brain: {e}"))?;

                // 每角色独立的 RealtimeVoiceManager
                let realtime_voice = Arc::new(RealtimeVoiceManager::new());
                realtime_voice.set_memory(memory_for_rt);
                realtime_voice.set_psychology(brain.psychology.clone());
                if let Some(chat_chain) = &brain.chat_chain {
                    realtime_voice.set_user_facts(chat_chain.user_facts.clone());
                }

                // 目标服务使用与 Mind 相同的 SharedGoalStore（同一份 Arc，不复制存储）
                let brain_instance_goals = brain.mind.goals.clone();

                Ok(CharacterInstance {
                    id: entry.id.clone(),
                    name: entry.name.clone(),
                    brain,
                    pet_controller,
                    manifest,
                    realtime_voice,
                    goal_service: crate::mind::GoalService::new(
                        entry.id.clone(),
                        brain_instance_goals.clone(),
                    ),
                    online: Arc::new(RwLock::new(entry.default_online)),
                    think_lock: Arc::new(tokio::sync::Mutex::new(())),
                })
            }
            .await;

            match result {
                Ok(instance) => {
                    new_characters.insert(entry.id.clone(), instance);
                    if entry.id == characters_config.active_id
                        || (index == 0 && active_id.is_empty())
                    {
                        active_id = entry.id.clone();
                    }
                }
                Err(e) => {
                    tracing::error!(
                        "[state] 角色 {} 初始化失败，跳过（不影响其他角色）: {}",
                        entry.id,
                        e
                    );
                    failed_ids.push(entry.id.clone());
                }
            }
        }

        // 若有角色失败，通过 toast 通知用户
        if !failed_ids.is_empty() {
            tracing::warn!(
                "[state] {} 个角色初始化失败: {}",
                failed_ids.len(),
                failed_ids.join(", ")
            );
        }

        // 至少有一个角色初始化成功才算成功
        if new_characters.is_empty() {
            return Err(VivianError::Other(format!(
                "所有角色初始化均失败: {}",
                failed_ids.join(", ")
            )));
        }

        // 发布新快照前同步全局按角色注册表；同 ID 为替换，已移除 ID 会被清理。
        let active_character_ids: std::collections::HashSet<String> =
            new_characters.keys().cloned().collect();
        for (char_id, instance) in &new_characters {
            crate::character_registry::register_character(
                char_id,
                (*instance.brain.memory).clone(),
                instance.brain.psychology.clone(),
                instance.manifest.clone(),
                verifier_llm.clone(),
            );
            crate::character_registry::register_brain(char_id, instance.brain.clone());
        }
        crate::character_registry::retain_characters(&active_character_ids);
        self.world_provider
            .retain_foreground_listeners(&active_character_ids);

        *self.model_router.write() = Some(router);
        *self.characters.write() = new_characters;
        *self.active_character_id.write() = active_id;

        // 角色表整体重建后，把全新的 TtsManager 实例同步给全局 SpeechPlanner。
        // 必须在此处（initialize 内）完成，而不是只在启动 setup 里做一次：
        // reinitialize（设置面板保存后热重载）也会走 initialize，若不同步，
        // Planner 会继续持有上一次的旧 TtsManager —— 旧实例的内存配置可能是
        // enabled=false，于是每条朗读意图都被判为"未启用"而静默丢弃，
        // 表现为"设置里启用了 TTS，但桌宠就是不说话"。
        self.sync_tts_managers().await;

        // 全部角色初始化完成且预加载流程（种子记忆注入、情绪/语义语料嵌入）已执行，
        // 此时才允许开放智能体 API 请求。
        self.initialized.store(true, Ordering::SeqCst);

        Ok(())
    }

    /// 把当前所有角色的 `TtsManager` 同步注册到全局 SpeechPlanner
    ///
    /// - 幂等：实例未变时不产生任何日志与副作用
    /// - 替换：实例已变（角色重建）时用新实例覆盖旧实例
    ///
    /// 调用时机：`initialize()` 末尾。任何重建角色的路径都必须经过它，
    /// 因此无需在各调用点重复注册。
    pub async fn sync_tts_managers(&self) {
        let tts_list: Vec<(String, Arc<crate::speech::TtsManager>)> = self
            .characters
            .read()
            .iter()
            .map(|(id, instance)| (id.clone(), instance.brain.tts.clone()))
            .collect();

        if tts_list.is_empty() {
            return;
        }

        let planner = crate::speech::get_planner().await;
        planner.register_all(tts_list).await;
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
