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
    /// 各角色的 ActivityJournal 通过 add_foreground_listener 订阅前台事件，独立记录。
    pub world_provider: Arc<WorldStateProvider>,
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

        Self {
            config: Arc::new(RwLock::new(config)),
            characters: Arc::new(RwLock::new(HashMap::new())),
            active_character_id: Arc::new(RwLock::new(String::new())),
            model_router: Arc::new(RwLock::new(None)),
            tool_system,
            generation_cancel: Arc::new(RwLock::new(HashMap::new())),
            asr: AsrManager::new_with_config(asr_config),
            scheduler,
            text_shortcuts: parking_lot::Mutex::new(HashMap::new()),
            window_shortcuts: parking_lot::Mutex::new(HashMap::new()),
            voice_shortcut_timer: parking_lot::Mutex::new(None),
            active_shortcut_role: parking_lot::Mutex::new(None),
            mcp_manager,
            reasoning_traces: crate::mind::reasoning_trace::TRACE_STORE.clone(),
            factory_reset_in_progress: Arc::new(AtomicBool::new(false)),
            rebuild_in_progress: Arc::new(AtomicBool::new(false)),
            playback_gate: crate::utils::PlaybackGate::new(),
            session_coordinator: crate::utils::SessionCoordinator::new(),
            leader_coordinator: crate::utils::ProactiveLeaderCoordinator::new(),
            world_provider,
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

        // Shared ModelRouter
        let router = ModelRouter::new(&config)?;
        let verifier_llm: Arc<dyn VerifierLlmClient> = Arc::new(router.clone());
        *self.model_router.write() = Some(router);

        // Register builtin tools to the shared tool_system
        crate::tools::builtin::register_builtin_tools(&self.tool_system);

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

        let scheduler_for_run = self.scheduler.clone();
        tauri::async_runtime::spawn(async move {
            scheduler_for_run.run().await;
        });

        // 为每个角色创建实例
        //
        // 容错策略：单角色初始化失败（如 SQLite 打不开、模型加载失败）只跳过该角色，
        // 不影响其他角色初始化。此前用 `?` 直接返回 Err 会导致循环中断——
        // Nana 的 SQLite 失败会让后续 Vivian 也不注册，表现为"偶发只创建一个窗口"。
        let characters_config = config.characters.clone();
        let mut active_id = String::new();
        let mut failed_ids: Vec<String> = Vec::new();

        for (index, entry) in characters_config.list.iter().enumerate() {
            let char_config = self.config.read().get_all();

            // 单角色初始化结果：Ok(instance) 或 Err（跳过该角色）
            let result: Result<CharacterInstance, String> = async {
                // 每角色独立的 MemoryManager
                let memory = MemoryManager::new(&char_config, &entry.id)
                    .map_err(|e| format!("记忆系统: {e}"))?;
                let memory_for_rt = memory.clone();

                // 每角色独立的 PetController + manifest
                let (pc, manifest) = Self::init_pet_controller(&entry.live2d_model);
                pc.set_character_id(entry.id.clone());
                let pet_controller = Arc::new(pc);

                // Independent Brain for each character
                let router_clone = self
                    .model_router
                    .read()
                    .as_ref()
                    .ok_or_else(|| "ModelRouter not initialized".to_string())?
                    .clone();
                let brain = Brain::new_with_pet_controller(
                    char_config.clone(),
                    router_clone,
                    memory.clone(),
                    pet_controller.clone(),
                    manifest.clone(),
                    &entry.id,
                    self.tool_system.clone(),
                    self.world_provider.clone(),
                )
                .await
                .map_err(|e| format!("Brain: {e}"))?;

                // 注册到角色资源注册表（按 char_id 索引，供工具系统使用）
                crate::character_registry::register_character(
                    &entry.id,
                    memory.clone(),
                    brain.psychology.clone(),
                    manifest.clone(),
                    verifier_llm.clone(),
                );
                // 注册 Brain 到全局注册表（供 write_diary 等工具按 char_id 获取 Brain）
                crate::character_registry::register_brain(&entry.id, brain.clone());

                // 每角色独立的 RealtimeVoiceManager
                let realtime_voice = Arc::new(RealtimeVoiceManager::new());
                realtime_voice.set_memory(memory_for_rt);
                realtime_voice.set_psychology(brain.psychology.clone());
                if let Some(chat_chain) = &brain.chat_chain {
                    realtime_voice.set_user_facts(chat_chain.user_facts.clone());
                }

                Ok(CharacterInstance {
                    id: entry.id.clone(),
                    name: entry.name.clone(),
                    brain,
                    pet_controller,
                    manifest,
                    realtime_voice,
                    online: Arc::new(RwLock::new(entry.default_online)),
                    think_lock: Arc::new(tokio::sync::Mutex::new(())),
                })
            }
            .await;

            match result {
                Ok(instance) => {
                    self.characters.write().insert(entry.id.clone(), instance);
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
        if self.characters.read().is_empty() {
            return Err(VivianError::Other(format!(
                "所有角色初始化均失败: {}",
                failed_ids.join(", ")
            )));
        }

        *self.active_character_id.write() = active_id;

        Ok(())
    }

    pub fn is_initialized(&self) -> bool {
        !self.characters.read().is_empty()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
