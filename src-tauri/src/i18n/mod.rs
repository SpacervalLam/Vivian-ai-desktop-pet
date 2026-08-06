//! 国际化 - 多语言翻译支持

use std::collections::HashMap;

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::Value;

/// 规范化语言代码：zh_CN -> zh-CN
fn normalize_locale(locale: &str) -> String {
    locale.replace('_', "-")
}

/// 国际化管理器 - 管理当前语言与翻译表
pub struct I18n {
    locale: String,
    translations: HashMap<String, Value>,
}

impl I18n {
    pub fn new(locale: &str) -> Self {
        let mut translations = HashMap::new();
        Self::load_defaults(&mut translations);
        Self {
            locale: normalize_locale(locale),
            translations,
        }
    }

    /// 翻译键值，支持点号分隔的嵌套键（如 "common.ok"）
    /// 若找不到翻译则返回键本身
    pub fn t(&self, key: &str) -> String {
        let locale_key = format!("{}.{}", self.locale, key);
        if let Some(val) = self.translations.get(&locale_key) {
            return val.as_str().unwrap_or(key).to_string();
        }
        // 回退到根节点嵌套查找
        if let Some(root) = self.translations.get(&self.locale) {
            let mut current = root;
            for part in key.split('.') {
                match current.get(part) {
                    Some(v) => current = v,
                    None => return key.to_string(),
                }
            }
            if let Some(s) = current.as_str() {
                return s.to_string();
            }
        }
        key.to_string()
    }

    pub fn set_locale(&mut self, locale: &str) {
        self.locale = normalize_locale(locale);
    }

    pub fn get_locale(&self) -> &str {
        &self.locale
    }

    pub fn add_translation(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.translations.insert(key.into(), Value::String(value.into()));
    }

    fn load_defaults(translations: &mut HashMap<String, Value>) {
        let zh_cn = serde_json::json!({
            // 通用
            "common": {
                "ok": "确定",
                "cancel": "取消",
                "save": "保存",
                "delete": "删除",
                "edit": "编辑",
                "search": "搜索",
                "loading": "加载中...",
                "error": "出错了",
                "success": "成功",
                "warning": "警告",
                "confirm": "确认",
                "close": "关闭",
                "yes": "是",
                "no": "否"
            },
            // 聊天窗口
            "chat": {
                "placeholder": "输入消息...",
                "send": "发送",
                "stop": "停止生成",
                "clear": "清空对话",
                "history": "聊天历史"
            },
            // 菜单
            "menu": {
                "settings": "设置",
                "about": "关于",
                "exit": "退出",
                "advanced": "高级设置",
                "chat": "聊天"
            },
            // 日记窗口
            "diary": {
                "title": "Vivian日记",
                "all_records": "全部记录",
                "empty_title": "浅落笔墨，留住时光",
                "empty_desc": "请在左侧列表中翻阅属于你们的心情故事",
                "no_records": "暂无日记记录",
                "mood": {
                    "happy": "开心",
                    "good": "不错",
                    "neutral": "平静",
                    "sad": "难过",
                    "angry": "生气",
                    "bored": "无聊",
                    "tired": "疲惫"
                },
                "events_title": "今日要事追踪",
                "no_events": "今天平静安稳，没有发生特别严重的打扰事件。",
                "search_placeholder": "搜索日记内容...",
                "stats_format": "📝 全文共 {word_count} 字",
                "stats": {
                    "total": "共编织了 {count} 个回忆"
                },
                "generate": "生成日记",
                "export": "导出日记"
            },
            // 记忆窗口
            "memory": {
                "title": "记忆管理",
                "search": "搜索记忆内容",
                "clear": "清空所有记忆",
                "clear_confirm": "确定要清空所有记忆吗？此操作不可恢复！",
                "type_short": "短期记忆",
                "type_mid": "中期记忆",
                "type_long": "长期记忆",
                "no_entries": "暂无记忆记录",
                "unknown": "未知",
                "mid_term": "中期记忆",
                "importance_level": "重要程度",
                "heat_value": "热度值",
                "record_time": "记录时间",
                "emotion_label": "情绪标签",
                "label": "标签",
                "role": "角色",
                "role_user": "用户",
                "role_ai": "AI",
                "stats_short_term": "短期: {count}",
                "stats_mid_term": "中期: {count}",
                "stats_long_term": "长期: {count}",
                "data_load_error": "数据加载错误: {error}",
                "timestamp_cleared": "时间戳记忆已清除",
                "chat_history_cleared": "聊天记录已清除"
            },
            // 主程序
            "main": {
                "welcome_html": "<p style='color:white;'>你好！我是 Vivian，你的AI 助手~</p>",
                "option_start": "开始聊天",
                "notify_chat": "正在等待消息...",
                "print_load_template": "正在加载模板: {a}",
                "print_load_history": "正在加载历史记录: {path}",
                "err_select_llm": "请先选择 LLM 提供商",
                "print_t2i_fail": "T2I 初始化失败: {e}",
                "print_tts_fail": "TTS 初始化失败: {e}",
                "print_bili_start": "正在启动 Bilibili 服务，房间号: {id}",
                "print_bili_import": "Bilibili 导入失败: {e}",
                "print_icon_fail": "图标加载失败: {e}"
            },
            // 启动画面
            "splash": {
                "dressing_up": "Vivian正在梳妆打扮"
            },
            // 微信风格聊天窗口
            "wechat": {
                "window_title": "与{name}聊天",
                "name": "薇薇安",
                "name_english": "Vivian",
                "status_online": "在线",
                "input_placeholder": "输入消息...",
                "send": "发送",
                "loading": "加载中..",
                "history_header": "— 以上是历史消息 —",
                "no_messages": "还没有消息，开始聊天吧",
                "no_more": "没有更多了",
                "load_failed": "加载失败，点击重试",
                "avatar_ai": "薇",
                "avatar_user": "我",
                "avatar_unknown": "?",
                "time_yesterday": "昨天",
                "time_month": "月",
                "time_day": "日",
                "time_year": "年",
                "select_avatar": "选择头像图片"
            },
            // 星期
            "weekday": {
                "mon": "一",
                "tue": "二",
                "wed": "三",
                "thu": "四",
                "fri": "五",
                "sat": "六",
                "sun": "日"
            },
            // AI 配置
            "ai_config": {
                "title": "AI 配置",
                "basic_model_settings": "基础模型设置",
                "service_provider": "服务提供商",
                "api_key": "API Key",
                "endpoint": "端点地址",
                "model": "模型",
                "use_legacy_input_format": "使用豆包API兼容性格式",
                "use_legacy_input_format_tooltip": "启用后将使用豆包API的输入格式",
                "temperature": "温度",
                "max_tokens": "最大Token数",
                "save_config": "保存配置",
                "api_key_empty": "请输入 API Key",
                "config_saved": "配置已保存",
                "save_failed": "保存失败: {error}"
            },
            // 扁平键
            "say_something": "想说点什么..",
            "initialization_complete": "初始化完成",
            "memory_management": "记忆管理",
            "settings": "设置",
            "quit": "退出",
            "memory_core": "记忆核心",
            "memory_index": "记忆索引",
            "memory_details": "记忆详情",
            "short_term_memory": "短期记忆",
            "long_term_memory": "长期记忆",
            "search_memory_content": "搜索记忆内容",
            "all_importance": "全部重要性",
            "high_importance": "高重要性",
            "medium_importance": "中重要性",
            "low_importance": "低重要性",
            "delete_memory": "删除记忆",
            "refresh": "刷新",
            "forget_all_memories": "清空所有记忆",
            "select_memory_to_view_details": "选择记忆查看详情",
            "total_memories": "总计记忆: {count}",
            "confirm_format": "确认格式化",
            "confirm_clear_memories": "确定要清空所有记忆吗？此操作不可恢复！",
            "confirm_delete": "确认删除",
            "confirm_delete_memory": "确定要删除此记忆吗？\n\n{content}",
            "delete_success": "删除成功",
            "delete_failed": "删除失败: {error}",
            "please_select_memory": "请选择要删除的记忆",
            "ok": "确定",
            "close_window": "关闭窗口",
            "reset_success": "重置成功",
            "brain_formatted": "记忆已格式化",
            "error": "错误",
            "clear_failed": "清空失败: {error}",
            "memory_id": "记忆ID",
            "memory_type": "记忆类型",
            "importance": "重要性",
            "source_channel": "来源渠道",
            "tags": "标签",
            "advanced_settings": "高级设置",
            "network_proxy_settings": "网络与代理设置",
            "proxy_mode": "代理模式",
            "direct_mode": "不使用代理",
            "system_proxy": "跟随系统",
            "custom_proxy": "自定义代理",
            "proxy_address": "代理地址",
            "proxy_address_placeholder": "http://127.0.0.1:7890",
            "test_connectivity": "测试连通性",
            "timeout": "超时时间",
            "ai_model_providers": "AI 模型提供商",
            "enter_provider_name": "输入新提供商名称",
            "add_provider": "添加提供商",
            "provider_exists": "该提供商已存在",
            "api_address": "API 地址",
            "api_address_placeholder": "如：https://api.siliconflow.cn/v1",
            "model_name": "模型名称",
            "model_name_placeholder": "如：deepseek-ai/DeepSeek-V3.1",
            "model_identifier": "模型标识",
            "ai_routing_matrix": "AI 模型路由矩阵",
            "task_type": "任务类型",
            "primary_model": "首选模型",
            "fallback_model": "备用模型",
            "secondary_model": "次选模型",
            "daily_chat": "日常闲聊",
            "tool_reasoning": "工具/思考",
            "smart_diary": "智能日记",
            "memory_summary": "记忆摘要",
            "task_memory_extract": "记忆提取",
            "task_memory_consolidate": "记忆合并",
            "task_graph_entity": "图谱实体抽取",
            "task_graph_community": "图谱社区摘要",
            "task_query_rewrite": "查询改写",
            "task_query_hyde": "HyDE 假想文档",
            "task_rerank": "检索重排（Cross-Encoder 优先）",
            "task_intent_judge": "意图判断",
            "task_pet_command": "桌宠指令",
            "none": "无",
            "select_option": "请选择",
            "enable_routing_matrix": "启用智能路由矩阵（关闭后所有请求使用基础模型配置）",
            "enable_fallback": "开启失败自动降级（当首选大模型故障时，自动切换备用模型）",
            "enter_proxy_address": "请输入代理地址",
            "testing": "测试中..",
            "test_failed": "测试失败",
            "network_connection": "网络连接",
            "enable_proxy_server": "启用代理服务器",
            "proxy_type": "代理类型",
            "proxy_host": "代理主机",
            "proxy_port": "代理端口",
            "language_settings": "语言设置",
            "diary_settings": "日记设置",
            "enable_auto_diary": "启用自动日记",
            "cancel": "取消",
            "warning": "警告",
            "api_key_empty": "请输入 API Key",
            "config_saved": "配置已保存",
            "save_failed": "保存失败: {error}",
            "save_config": "保存配置",
            "yes": "是",
            "no": "否",
            // 路由帮助文本
            "routing_help_title": "模型用途说明",
            "routing_help_chat": "用于日常对话、闲聊、告别消息等轻量级交互。\n当用户发送普通聊天消息且不需要工具调用时使用此模型。",
            "routing_help_reasoning": "用于工具调用、复杂推理、代码执行等需要逻辑分析的任务。\n当用户请求打开应用、操作文件、执行系统命令时使用此模型。",
            "routing_help_diary": "用于智能日记生成。\n每天结束时，Vivian 会调用此模型总结当天的对话内容，生成日记条目。",
            "routing_help_memory": "用于记忆写入增强、自动抽取、窗口压缩摘要、情绪分析、意图分类等后台任务。\n包括：\n• 写入时 LLM 增强（抽取 description/keywords/importance）\n• AutoExtractor 抽取 ADD/UPDATE/DELETE 长期记忆\n• 对话窗口压缩摘要（>21000 token 触发）\n• 远程嵌入服务（复用 api_key+endpoint）",
            "routing_help_memory_extract": "从对话内容中提取结构化记忆条目（事件、偏好、事实）。\n要求：JSON 严格、字段完整、错误抛出异常而非静默退。",
            "routing_help_memory_consolidate": "将多条相似记忆去重/合并/冲突消解。\nPhase 6 LightMem 离线批处理专用，调用频率低但质量要求高。",
            "routing_help_graph_entity": "从文本中抽取实体（人名、地名、概念）与关系。\n知识图谱 RAG 路径第一环，结构化输出必须严格。",
            "routing_help_graph_community": "对知识图谱社区（一组相关实体）生成摘要标签与描述。\n需要聚合能力强、对实体类型不敏感。",
            "routing_help_query_rewrite": "在 RAG 检索前重写用户查询，消解指代、补充上下文。\n短输入时延敏感，建议选响应快的模型。",
            "routing_help_query_hyde": "HyDE（Hypothetical Document Embeddings）：生成假想答案用于提升检索召回。\n要求生成质量高、与查询主题强相关。",
            "routing_help_rerank": "检索重排：Cross-Encod优先，LLM 仅作后备。\n建议此处选 None（用 cross-encoder），LLM 仅在 Cross-Encoder 不可用时启用。",
            "routing_help_intent_judge": "桌宠/桌面前端的意图判断（闲聊/工具/系统/触发动作）。\n调用频繁、要求延迟低、JSON 严。",
            "routing_help_pet_command": "桌宠语音/文本指令解析（打开应用、操作、互动动作）。\n要求结构化输出，错误抛异常。",
            // 语音识别
            "speech_recognition_settings": "语音识别设置",
            "asr_engine": "识别引擎",
            "asr_language": "识别语言",
            "silence_timeout": "静默停止时间"
        });

        let en = serde_json::json!({
            // Common
            "common": {
                "ok": "OK",
                "cancel": "Cancel",
                "save": "Save",
                "delete": "Delete",
                "edit": "Edit",
                "search": "Search",
                "loading": "Loading...",
                "error": "Error",
                "success": "Success",
                "warning": "Warning",
                "confirm": "Confirm",
                "close": "Close",
                "yes": "Yes",
                "no": "No"
            },
            // Chat window
            "chat": {
                "placeholder": "Type a message...",
                "send": "Send",
                "stop": "Stop",
                "clear": "Clear",
                "history": "Chat History"
            },
            // Menu
            "menu": {
                "settings": "Settings",
                "about": "About",
                "exit": "Exit",
                "advanced": "Advanced Settings",
                "chat": "Chat"
            },
            // Diary window
            "diary": {
                "title": "Vivian Diary",
                "all_records": "All Records",
                "empty_title": "Pen and Ink, Capture the Moments",
                "empty_desc": "Browse your emotional stories in the left panel",
                "no_records": "No diary entries yet",
                "mood": {
                    "happy": "Happy",
                    "good": "Good",
                    "neutral": "Neutral",
                    "sad": "Sad",
                    "angry": "Angry",
                    "bored": "Bored",
                    "tired": "Tired"
                },
                "events_title": "Today's Events",
                "no_events": "A calm day with no significant events.",
                "search_placeholder": "Search diary entries...",
                "stats_format": "📝 {word_count} words total",
                "stats": {
                    "total": "{count} memories woven"
                },
                "generate": "Generate Diary",
                "export": "Export Diary"
            },
            // Memory window
            "memory": {
                "title": "Memory Management",
                "search": "Search Memory",
                "clear": "Clear All Memories",
                "clear_confirm": "Are you sure you want to clear all memories? This cannot be undone!",
                "type_short": "Short-term",
                "type_mid": "Mid-term",
                "type_long": "Long-term",
                "no_entries": "No memory entries",
                "unknown": "Unknown",
                "mid_term": "Mid-term",
                "importance_level": "Importance",
                "heat_value": "Heat",
                "record_time": "Recorded",
                "emotion_label": "Emotion",
                "label": "Label",
                "role": "Role",
                "role_user": "User",
                "role_ai": "AI",
                "stats_short_term": "Short: {count}",
                "stats_mid_term": "Mid: {count}",
                "stats_long_term": "Long: {count}",
                "data_load_error": "Load error: {error}",
                "timestamp_cleared": "Timestamp memories cleared",
                "chat_history_cleared": "Chat history cleared"
            },
            // Main
            "main": {
                "welcome_html": "<p style='color:white;'>Hello! I'm Vivian, your AI assistant~</p>",
                "option_start": "Start Chat",
                "notify_chat": "Waiting for messages...",
                "print_load_template": "Loading template: {a}",
                "print_load_history": "Loading history: {path}",
                "err_select_llm": "Please select an LLM provider first",
                "print_t2i_fail": "T2I init failed: {e}",
                "print_tts_fail": "TTS init failed: {e}",
                "print_bili_start": "Starting Bilibili service, room: {id}",
                "print_bili_import": "Bilibili import failed: {e}",
                "print_icon_fail": "Icon load failed: {e}"
            },
            // Splash
            "splash": {
                "dressing_up": "Vivian is getting dressed up"
            },
            // WeChat-style chat window
            "wechat": {
                "window_title": "Chat with {name}",
                "name": "Vivian",
                "name_english": "Vivian",
                "status_online": "Online",
                "input_placeholder": "Enter message...",
                "send": "Send",
                "loading": "Loading..",
                "history_header": "— Above are history messages —",
                "no_messages": "No messages yet, start chatting",
                "no_more": "No more",
                "load_failed": "Load failed, tap to retry",
                "avatar_ai": "V",
                "avatar_user": "Me",
                "avatar_unknown": "?",
                "time_yesterday": "Yesterday",
                "time_month": "/",
                "time_day": "",
                "time_year": "/",
                "select_avatar": "Select avatar image"
            },
            // Weekday
            "weekday": {
                "mon": "Mon",
                "tue": "Tue",
                "wed": "Wed",
                "thu": "Thu",
                "fri": "Fri",
                "sat": "Sat",
                "sun": "Sun"
            },
            // AI config
            "ai_config": {
                "title": "AI Configuration",
                "basic_model_settings": "Basic Model Settings",
                "service_provider": "Service Provider",
                "api_key": "API Key",
                "endpoint": "Endpoint",
                "model": "Model",
                "use_legacy_input_format": "Use legacy API format",
                "use_legacy_input_format_tooltip": "Use legacy input format for compatibility",
                "temperature": "Temperature",
                "max_tokens": "Max Tokens",
                "save_config": "Save Config",
                "api_key_empty": "Please enter API Key",
                "config_saved": "Config saved",
                "save_failed": "Save failed: {error}"
            },
            // Flat keys
            "say_something": "Say something..",
            "initialization_complete": "Initialization Complete",
            "memory_management": "Memory Management",
            "settings": "Settings",
            "quit": "Quit",
            "memory_core": "Memory Core",
            "memory_index": "Memory Index",
            "memory_details": "Memory Details",
            "short_term_memory": "Short-term",
            "long_term_memory": "Long-term",
            "search_memory_content": "Search Memory",
            "all_importance": "All Importance",
            "high_importance": "High Importance",
            "medium_importance": "Medium Importance",
            "low_importance": "Low Importance",
            "delete_memory": "Delete Memory",
            "refresh": "Refresh",
            "forget_all_memories": "Clear All Memories",
            "select_memory_to_view_details": "Select to view details",
            "total_memories": "Total: {count}",
            "confirm_format": "Confirm Format",
            "confirm_clear_memories": "Clear all memories? Irreversible!",
            "confirm_delete": "Confirm Delete",
            "confirm_delete_memory": "Are you sure you want to delete this memory?\n\n{content}",
            "delete_success": "Delete successful",
            "delete_failed": "Delete failed: {error}",
            "please_select_memory": "Please select a memory to delete",
            "ok": "OK",
            "close_window": "Close Window",
            "reset_success": "Reset successful",
            "brain_formatted": "Memory formatted",
            "error": "Error",
            "clear_failed": "Clear failed: {error}",
            "memory_id": "Memory ID",
            "memory_type": "Memory Type",
            "importance": "Importance",
            "source_channel": "Source Channel",
            "tags": "Tags",
            "advanced_settings": "Advanced Settings",
            "network_proxy_settings": "Network & Proxy Settings",
            "proxy_mode": "Proxy Mode",
            "direct_mode": "No Proxy",
            "system_proxy": "System Proxy",
            "custom_proxy": "Custom Proxy",
            "proxy_address": "Proxy Address",
            "proxy_address_placeholder": "http://127.0.0.1:7890",
            "test_connectivity": "Test Connectivity",
            "timeout": "Timeout",
            "ai_model_providers": "AI Model Providers",
            "enter_provider_name": "Enter new provider name",
            "add_provider": "Add Provider",
            "provider_exists": "This provider already exists",
            "api_address": "API Address",
            "api_address_placeholder": "e.g. https://api.siliconflow.cn/v1",
            "model_name": "Model Name",
            "model_name_placeholder": "e.g. deepseek-ai/DeepSeek-V3.1",
            "model_identifier": "Model Identifier",
            "ai_routing_matrix": "AI Model Routing Matrix",
            "task_type": "Task Type",
            "primary_model": "Primary Model",
            "fallback_model": "Fallback Model",
            "secondary_model": "Secondary Model",
            "daily_chat": "Daily Chat",
            "tool_reasoning": "Tool/Reasoning",
            "smart_diary": "Smart Diary",
            "memory_summary": "Memory Summary",
            "task_memory_extract": "Memory Extraction",
            "task_memory_consolidate": "Memory Consolidation",
            "task_graph_entity": "Graph Entity Extraction",
            "task_graph_community": "Graph Community Summary",
            "task_query_rewrite": "Query Rewrite",
            "task_query_hyde": "HyDE Hypothetical Document",
            "task_rerank": "Rerank (Cross-Encoder preferred)",
            "task_intent_judge": "Intent Judgment",
            "task_pet_command": "Desktop Pet Command",
            "none": "None",
            "select_option": "Please select",
            "enable_routing_matrix": "Enable smart routing matrix (when off, all requests use basic model config)",
            "enable_fallback": "Enable automatic fallback (switch to fallback model when primary model fails)",
            "enter_proxy_address": "Please enter proxy address",
            "testing": "Testing..",
            "test_failed": "Test failed",
            "network_connection": "Network Connection",
            "enable_proxy_server": "Enable Proxy Server",
            "proxy_type": "Proxy Type",
            "proxy_host": "Proxy Host",
            "proxy_port": "Proxy Port",
            "language_settings": "Language Settings",
            "diary_settings": "Diary Settings",
            "enable_auto_diary": "Enable Auto Diary",
            "cancel": "Cancel",
            "warning": "Warning",
            "api_key_empty": "Please enter API Key",
            "config_saved": "Configuration saved",
            "save_failed": "Save failed: {error}",
            "save_config": "Save Config",
            "yes": "Yes",
            "no": "No",
            // Routing help text
            "routing_help_title": "Model Usage Description",
            "routing_help_chat": "Used for daily conversations, casual chat, farewell messages and other lightweight interactions.\nThis model is used when the user sends a normal chat message that does not require tool calls.",
            "routing_help_reasoning": "Used for tool calls, complex reasoning, code execution and other tasks requiring logical analysis.\nThis model is used when the user requests to open apps, operate files, or execute system commands.",
            "routing_help_diary": "Used for smart diary generation.\nAt the end of each day, Vivian calls this model to summarize the day's conversations and generate diary entries.",
            "routing_help_memory": "Used for memory write enhancement, auto-extraction, window compression summaries, emotion analysis, intent classification and other background tasks.\nIncludes:\n- Write-time LLM enhancement (extract description/keywords/importance)\n- AutoExtractor for ADD/UPDATE/DELETE long-term memories\n- Conversation window compression summary (triggered at >21000 tokens)\n- Remote embedding service (reuses api_key+endpoint)",
            "routing_help_memory_extract": "Extract structured memory entries (events, preferences, facts) from conversation content.\nRequires strict JSON, complete fields, and exceptions thrown rather than silent fallback.",
            "routing_help_memory_consolidate": "Deduplicate/merge/resolve conflicts among similar memories.\nDedicated to Phase 6 LightMem offline batch processing; low call frequency but high quality requirements.",
            "routing_help_graph_entity": "Extract entities (people, places, concepts) and relationships from text.\nThe first link in the knowledge graph RAG pipeline; structured output must be strict.",
            "routing_help_graph_community": "Generate summary labels and descriptions for knowledge graph communities (a group of related entities).\nRequires strong aggregation capability and insensitivity to entity types.",
            "routing_help_query_rewrite": "Rewrite user queries before RAG retrieval to resolve references and supplement context.\nSensitive to short-input latency; recommend choosing a fast-response model.",
            "routing_help_query_hyde": "HyDE (Hypothetical Document Embeddings): generate hypothetical answers to improve retrieval recall.\nRequires high generation quality and strong relevance to the query topic.",
            "routing_help_rerank": "Retrieval reranking: Cross-Encoder preferred, LLM only as fallback.\nRecommend selecting None here (use cross-encoder); LLM only enabled when Cross-Encoder is unavailable.",
            "routing_help_intent_judge": "Desktop pet/front-end intent judgment (chat/tool/system/trigger actions).\nFrequent calls, low latency requirements, strict JSON.",
            "routing_help_pet_command": "Desktop pet voice/text command parsing (open apps, operations, interaction actions).\nRequires structured output; errors should throw exceptions.",
            // Speech recognition
            "speech_recognition_settings": "Speech Recognition",
            "asr_engine": "Engine",
            "asr_language": "Language",
            "silence_timeout": "Silence Timeout"
        });

        translations.insert("zh-CN".to_string(), zh_cn);
        translations.insert("en".to_string(), en);

        // 补充冲突的扁平键：diary/weekday 既是字符串值又是嵌套前缀
        translations.insert("zh-CN.diary".to_string(), Value::String("日记本".to_string()));
        translations.insert("zh-CN.weekday".to_string(), Value::String("星期".to_string()));
        translations.insert("en.diary".to_string(), Value::String("Diary".to_string()));
        translations.insert("en.weekday".to_string(), Value::String("Weekday".to_string()));
    }
}

impl Default for I18n {
    fn default() -> Self {
        Self::new("zh-CN")
    }
}

// ─── 全局状态管理 ───

static CURRENT_LANGUAGE: Lazy<Mutex<String>> =
    Lazy::new(|| Mutex::new("zh-CN".to_string()));
static GLOBAL_I18N: Lazy<Mutex<I18n>> = Lazy::new(|| Mutex::new(I18n::default()));

/// 初始化国际化设置
pub fn init_i18n(language: &str) {
    let normalized = normalize_locale(language);
    *CURRENT_LANGUAGE.lock() = normalized.clone();
    GLOBAL_I18N.lock().set_locale(&normalized);
}

/// 设置当前语言并重新加载翻译
pub fn set_language(language: &str) {
    init_i18n(language);
}

/// 获取当前语言代码
pub fn get_language() -> String {
    CURRENT_LANGUAGE.lock().clone()
}

/// 翻译键为当前语言文本
pub fn tr(key: &str) -> String {
    GLOBAL_I18N.lock().t(key)
}

/// `tr` 别名，简化翻译调用
pub fn tr_(key: &str) -> String {
    tr(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translate_zh() {
        let i18n = I18n::new("zh-CN");
        assert_eq!(i18n.t("common.ok"), "确定");
        assert_eq!(i18n.t("chat.send"), "发送");
    }

    #[test]
    fn test_translate_en() {
        let i18n = I18n::new("en");
        assert_eq!(i18n.t("common.ok"), "OK");
        assert_eq!(i18n.t("chat.send"), "Send");
    }

    #[test]
    fn test_fallback_to_key() {
        let i18n = I18n::new("zh-CN");
        assert_eq!(i18n.t("nonexistent.key"), "nonexistent.key");
    }

    #[test]
    fn test_diary_keys_zh() {
        let i18n = I18n::new("zh-CN");
        assert_eq!(i18n.t("diary.title"), "Vivian日记");
        assert_eq!(i18n.t("diary.mood.happy"), "开心");
        assert_eq!(i18n.t("diary.search_placeholder"), "搜索日记内容...");
    }

    #[test]
    fn test_memory_keys_zh() {
        let i18n = I18n::new("zh-CN");
        assert_eq!(i18n.t("memory.title"), "记忆管理");
        assert_eq!(i18n.t("memory.type_short"), "短期记忆");
        assert_eq!(i18n.t("memory.unknown"), "未知");
    }

    #[test]
    fn test_weekday_keys() {
        let zh = I18n::new("zh-CN");
        assert_eq!(zh.t("weekday.mon"), "一");
        assert_eq!(zh.t("weekday.sun"), "日");
        let en = I18n::new("en");
        assert_eq!(en.t("weekday.mon"), "Mon");
    }

    #[test]
    fn test_flat_diary_key() {
        let zh = I18n::new("zh-CN");
        assert_eq!(zh.t("diary"), "日记本");
        let en = I18n::new("en");
        assert_eq!(en.t("diary"), "Diary");
    }

    #[test]
    fn test_routing_help_keys() {
        let i18n = I18n::new("zh-CN");
        assert!(i18n.t("routing_help_chat").contains("日常对话"));
        assert_eq!(i18n.t("routing_help_title"), "模型用途说明");
    }

    #[test]
    fn test_locale_normalization() {
        let i18n = I18n::new("zh_CN");
        assert_eq!(i18n.get_locale(), "zh-CN");
        assert_eq!(i18n.t("common.ok"), "确定");
    }

    #[test]
    fn test_global_tr() {
        init_i18n("zh-CN");
        assert_eq!(tr("common.ok"), "确定");
        assert_eq!(tr("diary.title"), "Vivian日记");
        assert_eq!(tr_("common.cancel"), "取消");
    }

    #[test]
    fn test_global_set_language() {
        set_language("en");
        assert_eq!(get_language(), "en");
        assert_eq!(tr("common.ok"), "OK");
        // 恢复默认
        set_language("zh-CN");
    }
}
