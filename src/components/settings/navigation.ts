import { Settings, Heart, Globe, Cpu, Waypoints, BriefcaseBusiness, Wrench, Database, Mic, Volume2, Phone, Wifi, Search, Cable, Puzzle, Activity, Archive, Info } from 'lucide-react';
import type { ElementType } from 'react';

export const settingsGroups = ['experience', 'intelligence', 'audio', 'services'] as const;
export type SettingsGroup = typeof settingsGroups[number];
export type SettingsPageKey = 'general' | 'companion' | 'world' | 'ai' | 'routing' | 'work' | 'tools' | 'memory' | 'voice' | 'speech' | 'realtime' | 'network' | 'search' | 'connections' | 'plugins' | 'usage' | 'data' | 'about';
interface SettingsPage { key: SettingsPageKey; group: SettingsGroup; icon: ElementType; searchKeys: string[]; keywords?: string[] }
export const settingsPages: SettingsPage[] = [
  { key: 'general', group: 'experience', icon: Settings, searchKeys: ["base.apartment_enabled", "base.auto_start", "base.language", "base.theme", "config.apartment_enabled_help", "config.apartment_not_installed", "config.auto_start_help", "config.field_apartment_enabled", "config.field_auto_start", "config.field_language", "config.field_mouse_follow", "config.field_smart_positioning", "config.field_theme", "config.mouse_follow_help", "config.section_general", "config.section_shortcuts", "config.field_shortcut_screen_analyze", "config.shortcut_screen_analyze_help", "config.smart_positioning_help", "config.theme_option_dark", "config.theme_option_light", "config.theme_option_system", "pet_render.always_follow_mouse", "window.smart_positioning_enabled"], keywords: [] },
  { key: 'companion', group: 'experience', icon: Heart, searchKeys: ["common.loading", "config.field_check_interval", "config.field_enable_app_duration_trigger", "config.field_enable_auto_diary", "config.field_enable_away_reminder", "config.field_enable_idle_trigger", "config.field_enable_late_night_trigger", "config.field_enable_music_trigger", "config.field_enable_proactive", "config.field_enable_screen_peek_trigger", "config.field_enable_social_urge_gating", "config.field_enable_system_pressure_trigger", "config.field_enable_window_change", "config.field_idle_threshold", "config.field_min_trigger_interval", "config.field_proactivity", "config.field_world_monologue", "config.monologue_greeting_help", "config.section_diary", "config.section_proactive", "config.section_world_monologue", "proactive.enable_app_duration_trigger", "proactive.enable_away_reminder", "proactive.enable_idle_trigger", "proactive.enable_late_night_trigger", "proactive.enable_music_trigger", "proactive.enable_screen_peek_trigger", "proactive.enable_social_urge_gating", "proactive.enable_system_pressure_trigger", "proactive.enable_window_change_trigger", "proactive.enabled", "proactive.idle_threshold", "proactive.min_trigger_interval", "proactive.proactivity", "proactive.tick_interval", "world.enable_inner_monologue"], keywords: ["聊天表情包", "贴纸", "stickers", "Vivian", "Nana"] },
  { key: 'world', group: 'experience', icon: Globe, searchKeys: ["config.field_world_enable", "config.field_world_inject_prompt", "config.field_world_latitude", "config.field_world_longitude", "config.field_world_weather", "config.field_world_weather_ttl", "config.section_world", "config.section_world_weather", "config.world_auto_detect", "config.world_auto_detect_loading", "config.world_inject_prompt_help", "config.world_latitude_help", "config.world_longitude_help", "config.world_weather_ttl_help", "world.enable", "world.enable_weather", "world.inject_into_prompt", "world.latitude", "world.longitude", "world.weather_cache_ttl_secs", "world.apple_weather", "config.world_apple_weather_enabled", "config.world_apple_weather_help"], keywords: ["Apple", "WeatherKit", "AQI", "气压", "空气质量", "十日", "逐小时"] },
  { key: 'ai', group: 'intelligence', icon: Cpu, searchKeys: ["ai.api_key", "ai.api_secret", "ai.app_id", "ai.context_window", "ai.enable_vision", "ai.endpoint", "ai.image_detail", "ai.max_tokens", "ai.model", "ai.provider", "ai.reasoning", "ai.temperature", "config.context_window_help", "config.field_api_key", "config.field_api_secret", "config.field_app_id", "config.field_context_window", "config.field_enable_vision", "config.field_enable_vision_help", "config.field_endpoint", "config.field_image_detail", "config.field_max_tokens", "config.field_model_name", "config.field_reasoning_pref", "config.field_temperature", "config.opt_image_detail_auto", "config.opt_image_detail_high", "config.opt_image_detail_low", "config.ph_api_key", "config.ph_api_secret", "config.ph_app_id", "config.ph_endpoint", "config.ph_model", "config.reasoning_pref_help", "config.section_ai", "config.section_multimodal", "config.update_presets_btn"], keywords: [] },
  { key: 'routing', group: 'intelligence', icon: Waypoints, searchKeys: ["ai.max_tokens", "ai.temperature", "config.field_api_key", "config.field_api_secret", "config.field_app_id", "config.field_enable_routing", "config.field_endpoint", "config.field_max_tokens", "config.field_model_name", "config.field_route_max_tokens_help", "config.field_route_temperature_help", "config.field_temperature", "config.llm_test_btn", "config.llm_test_failed", "config.llm_test_ok", "config.llm_test_skipped", "config.llm_test_summary", "config.llm_test_testing", "config.ph_api_key", "config.ph_api_secret", "config.ph_app_id", "config.ph_endpoint", "config.ph_model", "config.routing_description", "config.section_routing", "enable_routing_matrix"], keywords: [] },
  { key: 'work', group: 'intelligence', icon: BriefcaseBusiness, searchKeys: ["config.context_window_help", "config.default_workspace_browse", "config.default_workspace_description", "config.default_workspace_placeholder", "config.field_api_key", "config.field_api_secret", "config.field_app_id", "config.field_context_window", "config.field_default_workspace_path", "config.field_endpoint", "config.field_model_name", "config.field_reasoning_pref", "config.field_work_model_name", "config.ph_api_key", "config.ph_api_secret", "config.ph_app_id", "config.ph_endpoint", "config.ph_model", "config.reasoning_pref_help", "config.section_default_workspace", "config.section_work_models", "config.work_models_add", "config.work_models_description", "config.work_models_empty", "config.work_models_remove"], keywords: [] },
  { key: 'tools', group: 'intelligence', icon: Wrench, searchKeys: ["config.access_level_fsread", "config.access_level_fswrite", "config.access_level_fullcontrol", "config.access_level_readonly", "config.field_enable_native_fc", "config.field_tool_access_level", "config.field_tool_access_level_help", "config.field_tool_cache_max_size", "config.field_tool_cache_max_size_help", "config.field_tool_cache_ttl", "config.field_tool_cache_ttl_help", "config.field_tool_confirmation_timeout", "config.field_tool_confirmation_timeout_help", "config.field_tool_default_timeout", "config.field_tool_default_timeout_help", "config.field_tool_enable_cache", "config.field_tool_enable_cache_help", "config.field_tool_feedback_history_chars", "config.field_tool_feedback_history_chars_help", "config.field_tool_max_coding_rounds", "config.field_tool_max_coding_rounds_help", "config.field_tool_max_iterations", "config.field_tool_max_result_chars", "config.field_tool_max_result_chars_help", "config.field_tool_max_rounds", "config.field_unlimited_hint", "config.section_tools_cache", "config.section_tools_execution", "config.section_tools_native_fc", "config.section_tools_permission", "config.section_tools_switches", "config.tool_badge_custom", "config.tool_badge_locked", "config.tools_enabled_count", "config.tools_search_ph", "config.tools_switches_empty", "config.tools_switches_help", "tools.access_level", "tools.cache_max_size", "tools.cache_ttl_secs", "tools.confirmation_timeout_secs", "tools.default_tool_timeout_secs", "tools.enable_cache", "tools.enable_native_function_calling", "tools.feedback_history_chars", "tools.max_coding_rounds", "tools.max_iterations", "tools.max_result_chars", "tools.max_rounds"], keywords: [] },
  { key: 'memory', group: 'intelligence', icon: Database, searchKeys: ["config.btn_browse", "config.btn_ollama_pull", "config.btn_ollama_pulling", "config.btn_ollama_start", "config.btn_ollama_stop", "config.consolidation_description", "config.embedding_description", "config.embedding_provider_console", "config.embedding_provider_models", "config.embedding_provider_needs_key", "config.embedding_provider_no_key", "config.field_api_key", "config.field_embedding_dim", "config.field_embedding_endpoint", "config.field_embedding_model", "config.field_embedding_provider", "config.field_embedding_source", "config.field_enable_embedding", "config.field_enable_expiration", "config.field_ollama_auto_start", "config.field_ollama_auto_start_help", "config.field_ollama_model", "config.field_ollama_path", "config.field_recency_tau", "config.field_recency_tau_help", "config.field_rerank_enabled", "config.field_rerank_enabled_help", "config.field_rerank_endpoint", "config.field_rerank_model", "config.field_rerank_top_k", "config.field_retrieval_strategy", "config.field_retrieval_strategy_help", "config.field_short_term_limit", "config.field_short_term_limit_help", "config.field_stage1_idle_sec", "config.field_stage1_idle_sec_help", "config.field_stage1_threshold", "config.field_stage1_threshold_help", "config.field_vector_store_api_key", "config.field_vector_store_collection", "config.field_vector_store_ef_construction", "config.field_vector_store_hnsw_m", "config.field_vector_store_source", "config.field_vector_store_url", "config.field_weight_importance", "config.field_weight_importance_help", "config.field_weight_recency", "config.field_weight_recency_help", "config.field_weight_relevance", "config.field_weight_relevance_help", "config.field_world_consolidation", "config.ollama_current_model_loaded", "config.ollama_current_model_missing", "config.ollama_other_installed", "config.ollama_status_crashed", "config.ollama_status_running", "config.ollama_status_starting", "config.ollama_status_stopped", "config.ollama_status_stopping", "config.opt_auto", "config.opt_embedding_cloud", "config.opt_embedding_local", "config.opt_embedding_provider_custom", "config.opt_graph", "config.opt_hybrid", "config.opt_keyword", "config.opt_vector", "config.opt_vector_store_external", "config.opt_vector_store_local", "config.ph_api_key", "config.ph_embedding_endpoint", "config.ph_embedding_model", "config.ph_vector_store_api_key", "config.rerank_description", "config.retrieval_weights_description", "config.section_consolidation", "config.section_embedding", "config.section_memory", "config.section_rerank", "config.section_retrieval_weights", "config.section_vector_store", "config.vector_store_description", "config.world_consolidation_help", "memory.consolidation.stage1_idle_timeout_sec", "memory.consolidation.stage1_short_term_threshold", "memory.embedding.api_key", "memory.embedding.dimension", "memory.embedding.enabled", "memory.embedding.endpoint", "memory.embedding.model", "memory.embedding.ollama_auto_start", "memory.embedding.ollama_model", "memory.embedding.ollama_path", "memory.embedding.source", "memory.enable_expiration", "memory.max_short_term_memory", "memory.rerank.enabled", "memory.rerank.endpoint", "memory.rerank.model", "memory.rerank.top_k", "memory.retrieval_strategy", "memory.retrieval_weights.importance", "memory.retrieval_weights.recency", "memory.retrieval_weights.recency_tau_hours", "memory.retrieval_weights.relevance", "memory.vector_store.api_key", "memory.vector_store.collection", "memory.vector_store.ef_construction", "memory.vector_store.external_url", "memory.vector_store.hnsw_m", "world.enable_memory_consolidation"], keywords: [] },
  { key: 'voice', group: 'audio', icon: Mic, searchKeys: ["speechSettings.asr_intro", "speechSettings.connection", "speechSettings.connection_help", "config.btn_asr_help", "config.btn_browse", "config.btn_whisper_start", "config.btn_whisper_stop", "config.field_aliyun_access_key_id", "config.field_aliyun_access_key_secret", "config.field_aliyun_app_key", "config.field_aliyun_max_seconds", "config.field_asr_engine", "config.field_asr_language", "config.field_azure_conversation_mode", "config.field_azure_max_seconds", "config.field_azure_region", "config.field_azure_speech_key", "config.field_openai_whisper_api_key", "config.field_openai_whisper_base_url", "config.field_openai_whisper_max_seconds", "config.field_silence_timeout", "config.field_whisper_api_format", "config.field_whisper_api_key", "config.field_whisper_api_key_placeholder", "config.field_whisper_max_seconds", "config.field_whisper_realtime_language", "config.field_whisper_realtime_model", "config.field_whisper_server_url", "config.field_whisper_service_auto_start", "config.field_whisper_service_compute_type", "config.field_whisper_service_device", "config.field_whisper_service_install_path", "config.field_whisper_service_model", "config.field_whisper_service_port", "config.field_whisper_service_python_path", "config.field_whisper_streaming_mode", "config.opt_aliyun", "config.opt_azure", "config.opt_en_us", "config.opt_ja", "config.opt_openai_whisper", "config.opt_whisper", "config.opt_whisper_api_format_openai", "config.opt_whisper_api_format_whisper_cpp", "config.opt_whisper_compute_auto", "config.opt_whisper_device_auto", "config.opt_whisper_streaming_none", "config.opt_whisper_streaming_realtime_ws", "config.opt_whisper_streaming_sse", "config.opt_winrt", "config.opt_zh_cn", "config.placeholder_whisper_realtime_model", "config.placeholder_whisper_service_install_path", "config.placeholder_whisper_service_python_path", "config.section_advanced_settings", "config.section_aliyun", "config.section_asr", "config.section_azure", "config.section_openai_whisper", "config.section_whisper", "config.section_whisper_advanced", "config.section_whisper_realtime", "config.section_whisper_service", "config.whisper_hint_pip_required", "config.whisper_hint_streaming_mode", "config.whisper_status_crashed", "config.whisper_status_installing", "config.whisper_status_running", "config.whisper_status_starting", "config.whisper_status_stopped", "config.whisper_status_stopping", "speech_recognition.aliyun.access_key_id", "speech_recognition.aliyun.access_key_secret", "speech_recognition.aliyun.app_key", "speech_recognition.aliyun.max_audio_seconds", "speech_recognition.azure.conversation_mode", "speech_recognition.azure.max_audio_seconds", "speech_recognition.azure.speech_key", "speech_recognition.azure.speech_region", "speech_recognition.engine", "speech_recognition.language", "speech_recognition.openai_whisper.api_key", "speech_recognition.openai_whisper.base_url", "speech_recognition.openai_whisper.max_audio_seconds", "speech_recognition.silence_timeout_ms", "speech_recognition.whisper.api_format", "speech_recognition.whisper.api_key", "speech_recognition.whisper.max_audio_seconds", "speech_recognition.whisper.realtime_language", "speech_recognition.whisper.realtime_model", "speech_recognition.whisper.server_url", "speech_recognition.whisper.service_auto_start", "speech_recognition.whisper.service_compute_type", "speech_recognition.whisper.service_device", "speech_recognition.whisper.service_install_path", "speech_recognition.whisper.service_model", "speech_recognition.whisper.service_port", "speech_recognition.whisper.service_python_path", "speech_recognition.whisper.streaming_mode"], keywords: [] },
  { key: 'speech', group: 'audio', icon: Volume2, searchKeys: ["speechSettings.tts_intro", "speechSettings.models", "speechSettings.model_path_help", "speechSettings.shared_service", "ai.max_tokens", "ai.temperature", "common.loading", "config.btn_browse", "config.btn_fishspeech_start", "config.btn_fishspeech_stop", "config.btn_gptsovits_add_aux", "config.btn_gptsovits_start", "config.btn_gptsovits_stop", "config.btn_test_translation", "config.btn_test_tts", "config.btn_tts_help", "config.field_api_key", "config.field_api_secret", "config.field_app_id", "config.field_display_language", "config.field_enable_tts", "config.field_endpoint", "config.field_max_tokens", "config.field_model_name", "config.field_route_max_tokens_help", "config.field_route_temperature_help", "config.field_temperature", "config.field_translation_api_key", "config.field_translation_endpoint", "config.field_translation_provider", "config.field_tts_azure_key", "config.field_tts_azure_output_format", "config.field_tts_azure_pitch", "config.field_tts_azure_region", "config.field_tts_azure_role", "config.field_tts_azure_style", "config.field_tts_azure_style_degree", "config.field_tts_doubao_access_token", "config.field_tts_doubao_appid", "config.field_tts_doubao_cluster", "config.field_tts_doubao_format", "config.field_tts_doubao_sample_rate", "config.field_tts_doubao_voice_type", "config.field_tts_edgetts_voice", "config.field_tts_engine", "config.field_tts_fallback_engine", "config.field_tts_fishspeech_auto_start", "config.field_tts_fishspeech_character", "config.field_tts_fishspeech_compile", "config.field_tts_fishspeech_decoder_checkpoint", "config.field_tts_fishspeech_format", "config.field_tts_fishspeech_half", "config.field_tts_fishspeech_install_path", "config.field_tts_fishspeech_key", "config.field_tts_fishspeech_llama_checkpoint", "config.field_tts_fishspeech_port", "config.field_tts_fishspeech_python_path", "config.field_tts_fishspeech_ref_audio", "config.field_tts_fishspeech_ref_text", "config.field_tts_fishspeech_url", "config.field_tts_gptsovits_auto_start", "config.field_tts_gptsovits_aux_ref_audios", "config.field_tts_gptsovits_config_path", "config.field_tts_gptsovits_dual_instance", "config.field_tts_gptsovits_format", "config.field_tts_gptsovits_gpt_model", "config.field_tts_gptsovits_gpu", "config.field_tts_gptsovits_install_path", "config.field_tts_gptsovits_parallel_infer", "config.field_tts_gptsovits_port", "config.field_tts_gptsovits_prompt_lang", "config.field_tts_gptsovits_prompt_text", "config.field_tts_gptsovits_python_path", "config.field_tts_gptsovits_ref_audio", "config.field_tts_gptsovits_second_port", "config.field_tts_gptsovits_sovits_model", "config.field_tts_gptsovits_temperature", "config.field_tts_gptsovits_text_split_method", "config.field_tts_gptsovits_timeout", "config.field_tts_gptsovits_top_k", "config.field_tts_gptsovits_top_p", "config.field_tts_gptsovits_url", "config.field_tts_language", "config.field_tts_mimo_endpoint", "config.field_tts_mimo_key", "config.field_tts_mimo_style_prompt", "config.field_tts_mimo_voice_audio", "config.field_tts_mimo_voice_audio_help", "config.field_tts_minimax_format", "config.field_tts_minimax_key", "config.field_tts_minimax_model", "config.field_tts_minimax_sample_rate", "config.field_tts_minimax_voice_id", "config.field_tts_rate", "config.field_tts_retry_count", "config.field_tts_volume", "config.fishspeech_hint_install_required", "config.fishspeech_section_deploy", "config.fishspeech_status_crashed", "config.fishspeech_status_running", "config.fishspeech_status_starting", "config.fishspeech_status_stopped", "config.fishspeech_status_stopping", "config.gptsovits_hint_dual_instance", "config.gptsovits_hint_install_required", "config.gptsovits_hint_no_models", "config.gptsovits_hint_no_runtime", "config.gptsovits_hint_url_only", "config.gptsovits_section_advanced", "config.gptsovits_section_deploy", "config.gptsovits_section_local_service", "config.gptsovits_section_reference", "config.gptsovits_status_crashed", "config.gptsovits_status_running", "config.gptsovits_status_starting", "config.gptsovits_status_stopped", "config.gptsovits_status_stopping", "config.opt_gptsovits_cut0", "config.opt_gptsovits_cut1", "config.opt_gptsovits_cut2", "config.opt_gptsovits_cut3", "config.opt_gptsovits_cut4", "config.opt_gptsovits_cut5", "config.opt_gptsovits_format_raw", "config.opt_gptsovits_format_wav", "config.opt_gptsovits_model_none", "config.opt_gptsovits_parallel_off", "config.opt_gptsovits_parallel_on", "config.opt_lang_same_as_display", "config.opt_lang_same_as_system", "config.opt_tts_azure", "config.opt_tts_azure_role_none", "config.opt_tts_azure_style_none", "config.opt_tts_doubao", "config.opt_tts_edgetts", "config.opt_tts_edgetts_voice_default", "config.opt_tts_fishspeech", "config.opt_tts_gptsovits", "config.opt_tts_mimo", "config.opt_tts_minimax", "config.opt_tts_minimax_model_hd", "config.opt_tts_minimax_model_turbo", "config.opt_tts_none", "config.ph_gptsovits_timeout", "config.ph_tts_mimo_style_prompt", "config.ph_tts_mimo_voice_audio", "config.placeholder_fishspeech_decoder_checkpoint", "config.placeholder_fishspeech_install_path", "config.placeholder_fishspeech_llama_checkpoint", "config.placeholder_fishspeech_port", "config.placeholder_fishspeech_python_path", "config.placeholder_gptsovits_config_path", "config.placeholder_gptsovits_install_path", "config.placeholder_gptsovits_port", "config.placeholder_gptsovits_prompt_text", "config.placeholder_gptsovits_python_path", "config.placeholder_gptsovits_ref_audio", "config.placeholder_gptsovits_url", "config.placeholder_translation_api_key", "config.placeholder_translation_endpoint", "config.section_advanced_settings", "config.section_tts", "config.section_tts_azure", "config.section_tts_cross_lang", "config.section_tts_doubao", "config.section_tts_edgetts", "config.section_tts_fishspeech", "config.section_tts_gptsovits", "config.section_tts_mimo", "config.section_tts_minimax", "config.toast_translation_test_failed", "config.toast_translation_test_ok", "config.translation_llm_hint", "config.translation_provider_hint", "routing_matrix.translation.api_key", "routing_matrix.translation.api_secret", "routing_matrix.translation.app_id", "routing_matrix.translation.endpoint", "routing_matrix.translation.max_tokens", "routing_matrix.translation.model", "routing_matrix.translation.provider_type", "routing_matrix.translation.temperature", "toast:show"], keywords: [] },
  { key: 'realtime', group: 'audio', icon: Phone, searchKeys: ["config.field_realtime_access_key", "config.field_realtime_app_id", "config.field_realtime_enable", "config.field_realtime_end_smooth_window", "config.field_realtime_model", "config.field_realtime_provider", "config.field_realtime_speaker", "config.opt_realtime_provider_doubao", "config.opt_realtime_provider_gpt_live", "config.section_realtime", "realtime_voice.access_key", "realtime_voice.app_id", "realtime_voice.enabled", "realtime_voice.end_smooth_window_ms", "realtime_voice.model", "realtime_voice.provider", "realtime_voice.speaker"], keywords: [] },
  { key: 'network', group: 'services', icon: Wifi, searchKeys: ["config.diag_open", "config.diag_open_help", "config.field_proxy_mode", "config.field_proxy_url", "config.field_remote_access_enabled", "config.field_remote_access_port", "config.field_timeout", "config.help_remote_access_enabled", "config.help_remote_access_port", "config.opt_manual", "config.opt_no_proxy", "config.opt_system_proxy", "config.ph_proxy", "config.section_network", "config.section_remote_access", "network.proxy_mode", "network.proxy_url", "network.timeout"], keywords: [] },
  { key: 'search', group: 'services', icon: Search, searchKeys: ["config.field_ds_search_api_key", "config.field_ds_search_base_url", "config.field_ds_search_max_uses", "config.field_ds_search_model", "config.field_ds_search_timeout", "config.field_searxng_base_url", "config.field_searxng_token", "config.field_tavily_api_key", "config.field_tavily_include_raw", "config.field_tavily_search_depth", "config.field_web_search_bg_fetch", "config.field_web_search_language", "config.field_web_search_max_results", "config.field_web_search_providers", "config.field_web_search_timeout", "config.help_web_search_bg_fetch", "config.help_web_search_max_results", "config.help_web_search_providers", "config.opt_lang_auto", "config.opt_tavily_advanced", "config.opt_tavily_basic", "config.opt_web_search_deepseek", "config.opt_web_search_duckduckgo", "config.opt_web_search_searxng", "config.opt_web_search_tavily", "config.ph_ds_search_api_key", "config.ph_ds_search_model", "config.ph_optional", "config.section_web_search", "config.section_web_search_deepseek", "config.section_web_search_searxng", "config.section_web_search_tavily", "web_search.deepseek.api_key", "web_search.exa.api_key", "web_search.exa.base_url", "web_search.exa.model", "web_search.perplexity.api_key", "web_search.perplexity.base_url", "web_search.perplexity.model", "web_search.openai.api_key", "web_search.openai.base_url", "web_search.openai.model", "web_search.xai.api_key", "web_search.xai.base_url", "web_search.xai.model", "web_search.anthropic.api_key", "web_search.anthropic.base_url", "web_search.anthropic.model", "web_search.deepseek.base_url", "web_search.deepseek.max_uses", "web_search.deepseek.model", "web_search.deepseek.timeout_secs", "web_search.enable_background_knowledge_fetch", "web_search.language", "web_search.max_results", "web_search.searxng.auth_token", "web_search.searxng.base_url", "web_search.tavily.api_key", "web_search.tavily.include_raw_content", "web_search.tavily.search_depth", "web_search.timeout_secs"], keywords: [] },
  { key: 'connections', group: 'services', icon: Cable, searchKeys: ["config.tab_connections"], keywords: ["MCP", "OAuth", "连接", "connection", "接続"] },
  { key: 'plugins', group: 'services', icon: Puzzle, searchKeys: ["config.tab_plugins"], keywords: ["插件", "plugin", "扩展", "拡張"] },
  { key: 'usage', group: 'services', icon: Activity, searchKeys: ["config.section_work_models"], keywords: ["Token", "用量", "usage", "使用量"] },
  { key: 'data', group: 'services', icon: Archive, searchKeys: ["common.saving", "config.backup_btn", "config.backup_help", "config.clear_memories_btn", "config.restore_btn", "config.restore_btn_loading", "config.section_backup", "config.section_operations"], keywords: [] },
  { key: 'about', group: 'services', icon: Info, searchKeys: ["config.about_contact", "config.about_os", "config.about_project", "config.about_subtitle", "config.section_about"], keywords: [] },
];
interface SettingsCopy { search: string; clearSearch: string; noResults: string; contents: string; reload: string; groups: Record<SettingsGroup, string>; pages: Record<SettingsPageKey, readonly [string, string]> }
export const settingsCopy: Record<'zh' | 'en' | 'ja', SettingsCopy> = {
  zh: {
  "search": "搜索设置…",
  "clearSearch": "清除搜索",
  "noResults": "未找到匹配的设置，试试其他关键词。",
  "contents": "本页设置",
  "reload": "重新载入",
  "groups": {
    "experience": "日常体验",
    "intelligence": "智能与工作",
    "audio": "语音",
    "services": "服务与维护"
  },
  "pages": {
    "general": [
      "桌面与外观",
      "语言、主题、启动行为、桌宠与快捷键。"
    ],
    "companion": [
      "陪伴与互动",
      "调整主动问候、内心独白与自动日记。"
    ],
    "world": [
      "世界与天气",
      "配置环境感知、天气、位置与缓存。"
    ],
    "ai": [
      "对话模型",
      "连接日常对话服务，配置生成参数与图片理解。"
    ],
    "routing": [
      "任务模型分配",
      "为对话、后台思考、判断和工作分别选择模型。"
    ],
    "work": [
      "工作模型与目录",
      "管理工作模型、默认模型与工作目录。"
    ],
    "tools": [
      "工具与权限",
      "管理可用工具、执行限制、缓存及授权方式。"
    ],
    "memory": [
      "记忆与检索",
      "调整记忆存储、巩固、向量模型与检索质量。"
    ],
    "voice": [
      "语音输入",
      "选择识别引擎，配置麦克风识别与本地服务。"
    ],
    "speech": [
      "语音输出",
      "选择声音与合成引擎，调整播放及跨语言朗读。"
    ],
    "realtime": [
      "实时通话",
      "配置实时语音服务、凭据和通话声音。"
    ],
    "network": [
      "网络与远程访问",
      "设置代理、连接诊断及远程访问。"
    ],
    "search": [
      "联网搜索",
      "选择搜索服务，管理搜索凭据与请求参数。"
    ],
    "connections": [
      "外部连接",
      "管理与外部应用及服务的连接。"
    ],
    "plugins": [
      "扩展插件",
      "安装和管理扩展能力。"
    ],
    "usage": [
      "用量统计",
      "查看模型调用和 Token 消耗。"
    ],
    "data": [
      "备份与恢复",
      "导出备份、恢复数据或重置应用。"
    ],
    "about": [
      "关于 Vivian",
      "查看版本、项目链接与系统信息。"
    ]
  }
},
  en: {
  "search": "Search settings…",
  "clearSearch": "Clear search",
  "noResults": "No matching settings. Try another keyword.",
  "contents": "On this page",
  "reload": "Reload saved",
  "groups": {
    "experience": "Everyday experience",
    "intelligence": "Intelligence & work",
    "audio": "Speech",
    "services": "Services & maintenance"
  },
  "pages": {
    "general": [
      "Desktop & appearance",
      "Language, theme, startup, desktop pet and shortcuts."
    ],
    "companion": [
      "Companion & interaction",
      "Proactive greetings, inner monologue and automatic diary."
    ],
    "world": [
      "World & weather",
      "Environment awareness, weather, location and cache."
    ],
    "ai": [
      "Conversation models",
      "Connect your chat provider; configure generation and image understanding."
    ],
    "routing": [
      "Task model routing",
      "Assign models to conversation, background thinking, decisions and work."
    ],
    "work": [
      "Work models & workspace",
      "Manage work models, the active model and default workspace."
    ],
    "tools": [
      "Tools & permissions",
      "Available tools, execution limits, cache and authorization."
    ],
    "memory": [
      "Memory & retrieval",
      "Memory storage, consolidation, embeddings and retrieval quality."
    ],
    "voice": [
      "Speech input",
      "Speech recognition engines and local recognition services."
    ],
    "speech": [
      "Speech output",
      "Voices, synthesis engines, playback and multilingual speech."
    ],
    "realtime": [
      "Realtime calls",
      "Realtime voice providers, credentials and voices."
    ],
    "network": [
      "Network & remote access",
      "Proxy settings, connection diagnostics and remote access."
    ],
    "search": [
      "Web search",
      "Search providers, credentials and request options."
    ],
    "connections": [
      "Connections",
      "Manage connections to external apps and services."
    ],
    "plugins": [
      "Plugins",
      "Install and manage extensions."
    ],
    "usage": [
      "Usage",
      "Review model calls and token consumption."
    ],
    "data": [
      "Backup & restore",
      "Export backups, restore data or reset the application."
    ],
    "about": [
      "About Vivian",
      "Version, project links and system information."
    ]
  }
},
  ja: {
  "search": "設定を検索…",
  "clearSearch": "検索をクリア",
  "noResults": "一致する設定がありません。別のキーワードをお試しください。",
  "contents": "このページ",
  "reload": "保存済みを再読込",
  "groups": {
    "experience": "日常の体験",
    "intelligence": "知能と作業",
    "audio": "音声",
    "services": "サービスと管理"
  },
  "pages": {
    "general": [
      "デスクトップと外観",
      "言語、テーマ、起動、デスクトップペットとショートカット。"
    ],
    "companion": [
      "交流と寄り添い",
      "自発的な声かけ、内心の独白と自動日記。"
    ],
    "world": [
      "世界と天気",
      "環境の認識、天気、位置とキャッシュ。"
    ],
    "ai": [
      "会話モデル",
      "会話サービス、生成パラメータと画像理解。"
    ],
    "routing": [
      "タスク別モデル",
      "会話、バックグラウンド思考、判断と作業のモデルを指定。"
    ],
    "work": [
      "作業モデルとフォルダー",
      "作業モデル、既定モデルと作業フォルダー。"
    ],
    "tools": [
      "ツールと権限",
      "利用可能なツール、実行制限、キャッシュと許可方式。"
    ],
    "memory": [
      "記憶と検索",
      "記憶保存、統合、埋め込みと検索品質。"
    ],
    "voice": [
      "音声入力",
      "認識エンジン、マイク入力とローカルサービス。"
    ],
    "speech": [
      "音声出力",
      "音声と合成エンジン、再生と多言語読み上げ。"
    ],
    "realtime": [
      "リアルタイム通話",
      "音声サービス、認証情報と通話の声。"
    ],
    "network": [
      "ネットワークと遠隔アクセス",
      "プロキシ、接続診断と遠隔アクセス。"
    ],
    "search": [
      "ウェブ検索",
      "検索サービス、認証情報とリクエスト設定。"
    ],
    "connections": [
      "外部接続",
      "外部アプリとサービスへの接続を管理。"
    ],
    "plugins": [
      "プラグイン",
      "拡張機能のインストールと管理。"
    ],
    "usage": [
      "使用量",
      "モデル呼び出しとトークン消費を確認。"
    ],
    "data": [
      "バックアップと復元",
      "バックアップの出力、データ復元とアプリのリセット。"
    ],
    "about": [
      "Vivian について",
      "バージョン、プロジェクトとシステム情報。"
    ]
  }
},
};

/** Search translated setting labels, never user-entered values or credentials. */
export function findSettingsPages(query: string, copy: SettingsCopy, translate: (key: string) => string): SettingsPage[] {
  const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) return settingsPages;
  return settingsPages.filter((page) => {
    const text = [copy.pages[page.key][0], copy.pages[page.key][1], page.key,
      ...(page.keywords ?? []), ...page.searchKeys.map(translate)].join(' ').toLocaleLowerCase();
    return terms.every((term) => text.includes(term));
  });
}
