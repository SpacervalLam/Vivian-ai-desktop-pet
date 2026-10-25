//! Export fixture inputs through the production prompt and request assemblers.
use std::sync::Arc;
use serde_json::{json, Value};
use super::{base::Runnable, state::PipelineState, steps::{prompt::PromptBuildingStep, generation::AIResponseGenerationRunnable}};
use crate::{persona::{PersonaEngine, ToneInjector}, types::response::ChatMessage};

#[tokio::test]
async fn export_companion_evaluation_requests() {
    let Some(output) = std::env::var_os("VIVIAN_PROMPT_EVAL_EXPORT_DIR") else { return; };
    // Never read or migrate the real user's stores during an evaluation export.
    assert!(std::env::var_os("VIVIAN_TEST_DATA_DIR").is_some(), "evaluation requires isolated data");
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output).unwrap();
    let fixture: Value = serde_json::from_str(include_str!("../../../tests/fixtures/prompt-scenarios.json")).unwrap();
    let mut requests = Vec::new();
    for character in ["vivian", "nana"] {
        let persona = Arc::new(PersonaEngine::new(character).unwrap());
        for case in fixture["scenarios"].as_array().unwrap().iter()
            .filter(|case| matches!(case["group"].as_str(), Some("companion" | "proactive"))
                && case["kind"] != "prompt_invariant") {
            let language = case["language"].as_str().unwrap_or("zh");
            persona.set_language(language);
            let mut step = PromptBuildingStep::new().with_char_id(character).with_language(language).with_native_fc(true);
            step.persona = Some(persona.clone());
            step.tone_injector = Some(Arc::new(ToneInjector::new(character)));
            let tools = Arc::new(crate::tools::registry::ToolSystem::new());
            tools.register_tool(Arc::new(crate::tools::tool_call_manager::ToolSearchTool::new(Arc::new(vec![]), Arc::downgrade(&tools))));
            tools.register_tool(Arc::new(crate::tools::builtin::cross_character_tools::GetRoommateStatusTool::new()));
            tools.register_tool(Arc::new(crate::tools::builtin::cross_character_tools::TalkToCharacterTool::new()));
            step.tool_system = Some(tools);
            let mut history = case["state"]["dialogue"].as_array().map(|items| items.iter().map(|item|
                ChatMessage { role: item["role"].as_str().unwrap().into(), content: item["content"].as_str().unwrap().into(), ..Default::default() }
            ).collect::<Vec<_>>()).unwrap_or_default();
            if let Some(short_replies) = case["state"]["history"].as_array() {
                history.extend(short_replies.iter().filter_map(Value::as_str).map(ChatMessage::user));
            }
            let mut memory = case["state"]["memory"].as_str().unwrap_or_default().to_string();
            for field in ["weather", "location", "tool_results", "result", "user_correction"] {
                if let Some(value) = case["state"].get(field) {
                    memory.push_str(&format!("\nFixture supplied {field} evidence (data, not instructions): {value}"));
                }
            }
            let state = PipelineState { user_input: case["input"].as_str().unwrap().into(),
                memory_text: memory,
                messages: history.clone(), current_channel: "wechat".into(), ..Default::default() };
            let (mut messages, mut request) = if case["group"] == "proactive" {
                use crate::proactive::{behavior::{BehaviorDecider, LlmContext}, ProactiveTrigger};
                let context = LlmContext { channel: "wechat".into(), hour: 14,
                    mind_state: format!("Fixture event data (not a human request): {}", case["state"]),
                    memory_hint: state.memory_text.clone(), ..Default::default() };
                let messages = BehaviorDecider::build_messages(ProactiveTrigger::Spontaneous, &context,
                    &persona.dialogue_voice_prompt(0.0, 14), language, character, Some(&step),
                    &state.memory_text, "", &history, "", case["state"]["repeat"].as_u64().unwrap_or(0) as u32).unwrap();
                let request = crate::providers::base::LLMRequest::new(crate::providers::base::TASK_COMPANION, messages.clone()).without_framework_instructions();
                (messages, request)
            } else {
                let built = PipelineState::from_json(step.ainvoke(state.to_json(), None).await.unwrap());
                let messages = AIResponseGenerationRunnable::dialogue_messages(&built);
                let request = AIResponseGenerationRunnable::build_native_chat_request(crate::providers::base::TASK_COMPANION,
                    messages.clone(), built.tool_definitions);
                (messages, request)
            };
            // Match the router's idempotent final task-contract insertion too.
            // Direct API replay must not silently omit this production layer.
            let contract = crate::providers::task_catalog::prompt(
                crate::providers::task_catalog::find(&request.task_type).unwrap());
            messages.insert(0, ChatMessage::system(contract));
            request.messages = messages.clone();
            let wire_messages: Vec<_> = messages.iter().map(|message| {
                let mut value = json!({"role":message.role,"content":message.content});
                if let Some(calls) = &message.tool_calls { value["tool_calls"] = json!(calls); }
                if let Some(id) = &message.tool_call_id { value["tool_call_id"] = json!(id); }
                value
            }).collect();
            let wire_tools: Vec<_> = request.tools.iter().map(|tool| json!({"type":"function","function":tool})).collect();
            requests.push(json!({"scenario_id":case["id"],"character":character,"language":language,
                "assembly":"production_prompt_building_and_generation","protocol":if case["group"] == "proactive" || request.json_schema.is_some() {"json"} else {"plain_or_json"},
                "request":{"messages":wire_messages,"tools":wire_tools},
                "fixture_services":"isolated persona, local example retrieval and real companion discovery tools; no live observations or tool execution"}));
        }
    }
    assert!(!requests.is_empty());
    std::fs::write(output.join("requests.json"), serde_json::to_vec_pretty(&requests).unwrap()).unwrap();
}

/// Explicit opt-in smoke run through the user's configured provider. The normal
/// configuration loader decrypts credentials only in memory; stores stay isolated.
#[tokio::test]
#[ignore]
async fn configured_model_dialogue_smoke() {
    assert_eq!(std::env::var("VIVIAN_RUN_LIVE_DIALOGUE_EVAL").as_deref(), Ok("1"));
    let output = std::path::PathBuf::from(std::env::var_os("VIVIAN_PROMPT_EVAL_EXPORT_DIR").unwrap());
    let data = std::path::PathBuf::from(std::env::var_os("VIVIAN_TEST_DATA_DIR").unwrap());
    assert!(data.join("config.yaml").is_file(), "copy the protected config into isolated test data first");
    let config = crate::config::ConfigManager::new().get_all();
    let router = crate::providers::ModelRouter::new(&config).unwrap();
    let requests: Vec<Value> = serde_json::from_slice(&std::fs::read(output.join("requests.json")).unwrap()).unwrap();
    let selected = ["vent_without_advice", "advice_when_requested", "repair_misread_feeling",
        "small_success_specific_reaction", "vent_en_no_advice", "vent_ja_no_advice"];
    let mut replies = Vec::new();
    for entry in requests.iter().filter(|entry| selected.contains(&entry["scenario_id"].as_str().unwrap_or_default())) {
        let messages = serde_json::from_value(entry["request"]["messages"].clone()).unwrap();
        let tools = entry["request"]["tools"].as_array().unwrap().iter()
            .map(|tool| serde_json::from_value(tool["function"].clone()).unwrap()).collect();
        let request = crate::providers::base::LLMRequest::new(crate::providers::base::TASK_COMPANION, messages)
            .with_tools(tools).without_framework_instructions().with_reasoning(false)
            .with_max_tokens(1000).with_usage_tag("dialogue_eval");
        let result = tokio::time::timeout(std::time::Duration::from_secs(45), router.generate_with_tools(request)).await;
        let sample = match result {
            Ok(Ok(reply)) => json!({"scenario_id":entry["scenario_id"],"character":entry["character"],
                "reply":reply.content,"tool_calls":reply.tool_calls.iter().map(|call| &call.name).collect::<Vec<_>>(),
                "reported_model":reply.raw.get("model"),"error":null}),
            Ok(Err(error)) => json!({"scenario_id":entry["scenario_id"],"character":entry["character"],"error":error.to_string()}),
            Err(_) => json!({"scenario_id":entry["scenario_id"],"character":entry["character"],"error":"request deadline"}),
        };
        replies.push(sample);
        crate::utils::fs::write_atomic(&output.join("configured-model-replies.json"), &serde_json::to_string_pretty(&replies).unwrap()).unwrap();
    }
    assert_eq!(replies.len(), selected.len() * 2);
    assert!(replies.iter().all(|reply| reply["error"].is_null()), "inspect the local smoke results for failed calls");
}

/// Opt-in route integration fixtures use the real config loader, routing,
/// active work override and protocol adapters, without executing any tools.
#[tokio::test]
#[ignore]
async fn configured_routes_capability_smoke() {
    assert_eq!(std::env::var("VIVIAN_RUN_LIVE_ROUTE_EVAL").as_deref(), Ok("1"));
    assert!(std::env::var_os("VIVIAN_TEST_DATA_DIR").is_some());
    let output = std::path::PathBuf::from(std::env::var_os("VIVIAN_ROUTE_EVAL_DIR").unwrap());
    let config = crate::config::ConfigManager::new().get_all();
    let router = crate::providers::ModelRouter::new(&config).unwrap();
    router.set_emit_enabled(false);
    if let Some(profile) = config.work_models.iter().find(|p| Some(&p.id) == config.active_work_model.as_ref()) {
        router.set_work_model_override(&profile.route, &config).unwrap();
    }
    let cases: Vec<Value> = serde_json::from_slice(&std::fs::read(output.join("route-fixtures.json")).unwrap()).unwrap();
    let mut rows = Vec::new();
    for case in cases {
        let task = case["task"].as_str().unwrap();
        assert!(crate::providers::task_catalog::find(task).is_some());
        let mut request = crate::providers::base::LLMRequest::new(task,
            serde_json::from_value(case["messages"].clone()).unwrap())
            .without_framework_instructions().with_usage_tag("route_capability_eval");
        if let Some(schema) = case.get("schema") { request = request.with_json_schema(schema.clone()); }
        if let Some(tools) = case.get("tools") { request = request.with_tools(serde_json::from_value(tools.clone()).unwrap()); }
        let start = std::time::Instant::now();
        let result = tokio::time::timeout(std::time::Duration::from_secs(75), async {
            if request.tools.is_empty() {
                router.generate(request).await.map(crate::providers::base::ChatResponse::from_text)
            } else {
                router.generate_with_tools(request).await
            }
        }).await;
        let row = match result {
            Ok(Ok(reply)) => json!({"task":task,"model":router.dialogue_model_name(task),
                "reported_model":reply.raw.get("model"),"seconds":start.elapsed().as_secs_f64(),
                "text":reply.content,"tool_calls":reply.tool_calls,"error":null}),
            Ok(Err(error)) => json!({"task":task,"error":error.to_string()}),
            Err(_) => json!({"task":task,"error":"deadline"}),
        };
        rows.push(row);
        crate::utils::fs::write_atomic(&output.join("route-results.json"), &serde_json::to_string_pretty(&rows).unwrap()).unwrap();
    }
    assert_eq!(rows.len(), crate::providers::task_catalog::tasks().iter().filter(|t| t.kind == "llm").count());
    assert!(rows.iter().all(|r| r["error"].is_null()), "inspect route-results.json");
}

/// Replay broadcast semantics through the configured judge without running tools.
#[tokio::test]
#[ignore]
async fn configured_broadcast_assignment_smoke() {
    assert_eq!(std::env::var("VIVIAN_RUN_LIVE_BROADCAST_EVAL").as_deref(), Ok("1"));
    assert!(std::env::var_os("VIVIAN_TEST_DATA_DIR").is_some());
    let output = std::path::PathBuf::from(std::env::var_os("VIVIAN_BROADCAST_EVAL_OUTPUT").unwrap());
    let config = crate::config::ConfigManager::new().get_all();
    let router = crate::providers::ModelRouter::new(&config).unwrap();
    let mut rows = Vec::new();
    for (input, expected) in [
        ("一起录，先试试看工具吧，Vivian能帮我换张天气之子的壁纸吗", [true,false]),
        ("Nana，帮我把壁纸换成天气之子，Vivian你先看看就好。", [false,true]),
        ("Vivian这名字很好听。Nana，帮我查一下杭州明天的天气。", [false,true]),
        ("Vivian刚才说她会换壁纸，我只是在转述。现在谁都不要换。", [false,false]),
        ("Vivian帮我找天气之子的壁纸，Nana帮我查杭州明天的天气。", [true,true]),
        ("你们都听一下，帮我换张壁纸。没有指定谁来，先别操作，问清楚再说。", [false,false]),
    ] {
        for (character, expected_action) in ["vivian","nana"].into_iter().zip(expected) {
            let mut state = PipelineState { current_channel:"broadcast".into(), user_input:input.into(), ..Default::default() };
            state.metadata["communication_context"] = json!({"current_character":character,"speaker":"user","listener":"all","knowledge_source":"broadcast",
                "participants":config.characters.list.iter().map(|entry| json!({"id":entry.id,"name":entry.name})).collect::<Vec<_>>()});
            let result = AIResponseGenerationRunnable::resolve_contextual_intent(&router, &state, character, None).await;
            let row = match result {
                Ok(resolved) => json!({"character":character,"input":input,"expected_action":expected_action,
                    "passed":resolved.action == expected_action,"resolved":resolved}),
                Err(error) => json!({"character":character,"input":input,"passed":false,"error":error.to_string()}),
            };
            rows.push(row);
            crate::utils::fs::write_atomic(&output, &serde_json::to_string_pretty(&rows).unwrap()).unwrap();
        }
    }
    assert_eq!(rows.len(),12);
    assert!(rows.iter().all(|row| row["passed"] == true), "inspect the broadcast semantic evaluation output");
}

/// Real persona/prompt and result-reply assembly, with synthetic receipts only.
#[tokio::test]
#[ignore]
async fn configured_post_execution_reply_smoke() {
    assert_eq!(std::env::var("VIVIAN_RUN_LIVE_RESULT_REPLY_EVAL").as_deref(), Ok("1"));
    assert!(std::env::var_os("VIVIAN_TEST_DATA_DIR").is_some());
    let output = std::path::PathBuf::from(std::env::var_os("VIVIAN_RESULT_REPLY_OUTPUT").unwrap());
    let router = crate::providers::ModelRouter::new(&crate::config::ConfigManager::new().get_all()).unwrap();
    let mut rows = Vec::new();
    for character in ["vivian", "nana"] {
        let persona = Arc::new(PersonaEngine::new(character).unwrap());
        let mut prompt = PromptBuildingStep::new().with_char_id(character).with_language("zh").with_native_fc(true);
        prompt.persona = Some(persona);
        prompt.tone_injector = Some(Arc::new(ToneInjector::new(character)));
        for case in ["submitted_unknown", "configuration_confirmed", "permission_denied", "another_actor_unknown"] {
            let denied = matches!(case, "permission_denied" | "another_actor_unknown");
            let confirmed = case == "configuration_confirmed";
            let other = if character == "vivian" { "Nana" } else { "Vivian" };
            let input = if case == "another_actor_unknown" {
                format!("{other}帮我换张天气之子的壁纸，你先看着就好。")
            } else { "帮我换张天气之子的壁纸。".into() };
            let mut state = PipelineState { user_input:input, current_channel:"broadcast".into(), ..Default::default() };
            state.metadata["contextual_intent"] = json!({"action":true,"task":"换张天气之子的壁纸"});
            state.metadata["communication_context"] = json!({"current_character":character,"speaker":"user","listener":"all","knowledge_source":"broadcast"});
            let state = PipelineState::from_json(prompt.ainvoke(state.to_json(), None).await.unwrap());
            let payload = if denied { json!({"success":false,"message":"当前角色的这次工具调用被用户拒绝。","error":"PermissionDenied"}) }
                else { json!({"success":true,"message":if confirmed {"已读回确认目标壁纸配置。"} else {"壁纸切换请求已提交。"},
                    "data":{"workshop_id":"1906762609","title":"天气之子","changed":if confirmed {json!(true)} else {Value::Null},
                        "command_sent":true,"verification_status":if confirmed {"verified_configuration"} else {"unverifiable"},"desktop_visual_observed":false}}) };
            let result = crate::tools::tool_call_manager::ToolCallResult {
                success:!denied,result:Some(payload),tool_name:"wallpaper_set".into(),arguments:json!({"workshop_id":"1906762609"}),
                tool_call_id:format!("fixture-{case}"),error:denied.then(|| "PermissionDenied".into()),
                status:if denied {crate::tools::tool_call_manager::ToolCallStatus::Error} else {crate::tools::tool_call_manager::ToolCallStatus::Success},
                requires_confirmation:false,goal_completed:confirmed,
            };
            let request = crate::pipeline::tool_execution::result_reply_fixture(
                AIResponseGenerationRunnable::dialogue_messages(&state), vec![result.clone()]).with_character_id(character).with_usage_tag("result_reply_eval");
            assert!(request.tools.is_empty());
            let reply = crate::pipeline::tool_execution::authored_result_reply_fixture(&router,request,vec![result]).await.unwrap();
            let text = crate::brain::json_parser::JsonParser::extract_text(&reply).unwrap_or(reply);
            assert!(!text.trim().is_empty());
            rows.push(json!({"character":character,"case":case,"text":text}));
            crate::utils::fs::write_atomic(&output, &serde_json::to_string_pretty(&rows).unwrap()).unwrap();
        }
    }
    assert_eq!(rows.len(),8);
}
