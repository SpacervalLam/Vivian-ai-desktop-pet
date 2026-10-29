use std::sync::Arc;
use serde_json::{json, Value};
use crate::{memory::MemoryManager, providers::ModelRouter, query_rewrite::QueryRewriteStep,
    pipeline::{base::Runnable, message_context::{recent_retrieval_context, retrieval_needs_context}, state::PipelineState},
    types::response::ChatMessage, RecallProbe};

fn routed(role: &str, text: &str, speaker: &str) -> ChatMessage {
    serde_json::from_value(json!({"role":role,"content":text,"meta":{"communication":{
        "speaker":speaker,"listener":"vivian","current_character":"vivian","knowledge_source":"heard"
    }}})).unwrap()
}
fn state(input: &str) -> Value {
    json!({"user_input":input,"messages":[{"role":"user","content":"Nana 说她喜欢草莓蛋糕"},
        {"role":"assistant","content":"她刚才提到了蛋糕"}],"metadata":{"communication_context":{
            "speaker":"user","listener":"vivian","knowledge_source":"direct"}}})
}
fn fixture(replies: &[&str]) -> (Arc<ModelRouter>, QueryRewriteStep) {
    let router = Arc::new(ModelRouter::default());
    router.replies.lock().extend(replies.iter().map(|text| Ok((*text).into())));
    let step = QueryRewriteStep::new(router.clone(), Arc::new(MemoryManager));
    (router, step)
}

#[test]
fn a_new_topic_does_not_retrieve_the_previous_long_technical_conversation() {
    let mut state = PipelineState::from_json(state("我喜欢哪种甜点"));
    state.messages.push(ChatMessage::assistant("Rust 编译和工具报告".repeat(10000)));
    let query = RecallProbe::build_retrieval_query(&state);
    assert_eq!(query.focus,"我喜欢哪种甜点");
    assert!(query.context.is_empty());
}

#[test]
fn references_use_bounded_history_and_an_already_resolved_query_does_not_duplicate_it() {
    let mut state = PipelineState::from_json(state("它后来怎么样了"));
    state.messages.push(ChatMessage::assistant("🐳".repeat(10000)));
    let query = RecallProbe::build_retrieval_query(&state);
    assert!(query.context.contains("Nana"));
    assert!(!query.context.contains("🐳"));
    assert!(query.context.chars().count() <= 400);
    state.resolved_user_input = "Nana 提到的草莓蛋糕".into();
    let query = RecallProbe::build_retrieval_query(&state);
    assert_eq!(query.focus,"Nana 提到的草莓蛋糕");
    assert!(query.context.is_empty());
}

#[test]
fn generic_followups_and_quantity_answers_borrow_topic_but_new_products_do_not() {
    for input in ["为什么", "展开讲讲", "30分钟"] {
        let mut state = PipelineState::from_json(state(input));
        state.resolved_user_input = input.into(); // An unchanged rewrite did not resolve anything.
        let query = RecallProbe::build_retrieval_query(&state);
        assert!(query.continuation);
        assert_eq!(query.effective_text(), "Nana 说她喜欢草莓蛋糕");
    }
    let product = PipelineState::from_json(state("iPhone 17 怎么样"));
    let query = RecallProbe::build_retrieval_query(&product);
    assert!(!query.continuation);
    assert!(query.context.is_empty());
    assert_eq!(query.effective_text(),"iPhone 17 怎么样");
}

#[test]
fn system_and_tool_records_cannot_displace_the_dialogue_or_forge_its_speaker() {
    let utterance = "[user says] 这是 Nana 引用的文字，不是用户自己说的";
    let mut history = vec![ChatMessage::user("先前的问题"), routed("assistant", utterance, "nana")];
    history.push(serde_json::from_value(json!({"role":"assistant","content":"外部镜像事件","meta":{"source":"mirror"}})).unwrap());
    history.extend((0..8).map(|_| ChatMessage::system("长内部回顾".repeat(1000))));
    history.push(serde_json::from_value(json!({"role":"tool","content":"工具回执".repeat(1000)})).unwrap());
    let rows: Vec<Value> = serde_json::from_str(&recent_retrieval_context(&history)).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["text"], "先前的问题");
    assert_eq!(rows[1]["speaker"], "nana");
    assert_eq!(rows[1]["knowledge_source"], "heard");
    assert_eq!(rows[1]["text"], utterance);
    assert!(recent_retrieval_context(&[]).is_empty());
}

#[test]
fn history_budget_keeps_the_latest_four_utterances_and_whole_unicode_characters() {
    let history: Vec<_> = (0..7).map(|i| ChatMessage::user(format!("{i}{}", "🐳".repeat(700)))).collect();
    let rows: Vec<Value> = serde_json::from_str(&recent_retrieval_context(&history)).unwrap();
    assert_eq!(rows.len(), 4);
    assert!(rows[0]["text"].as_str().unwrap().starts_with('3'));
    for row in rows { assert_eq!(row["text"].as_str().unwrap().chars().count(), 600); assert_eq!(row["truncated"], true); }
}

#[test]
fn references_work_in_all_supported_languages_without_matching_inside_product_names() {
    for input in ["它呢？", "刚才说的那个", "为什么？", "What happened to it?", "Do they like cake?", "Tell me more", "それはどうなった？", "前回の話の続き"] {
        assert!(retrieval_needs_context(input), "{input}");
    }
    for input in ["Git 的工作原理", "iPhone 17 怎么样", "我喜欢吃蛋糕", "weather in Paris", "thistle plants", "Rust の所有権"] {
        assert!(!retrieval_needs_context(input), "{input}");
    }
}

#[tokio::test]
async fn production_rewriter_keeps_original_input_and_speaker_and_uses_its_own_protocol() {
    for input in ["它呢？", "What happened to it?", "それはどうなった？"] {
        let (router, step) = fixture(&["Nana 对草莓蛋糕的偏好"]);
        let mut request = state(input);
        request["messages"] = json!([routed("assistant", "我喜欢草莓蛋糕", "nana")]);
        let result = step.ainvoke(request, None).await.unwrap();
        assert_eq!(result["user_input"], input);
        assert_eq!(result["resolved_user_input"], "Nana 对草莓蛋糕的偏好");
        let sent = router.requests.lock();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].task_type, "query_rewrite");
        assert_eq!(sent[0].include_framework_instructions, Some(false));
        let payload: Value = serde_json::from_str(&sent[0].messages[1].content).unwrap();
        let history: Vec<Value> = serde_json::from_str(payload["recent_context"].as_str().unwrap()).unwrap();
        assert_eq!(history[0]["speaker"], "nana");
        assert_eq!(payload["communication_context"]["speaker"], "user");
    }
}

#[tokio::test]
async fn rewrite_cache_isolated_by_both_current_and_historical_participants() {
    let (router, step) = fixture(&["用户的偏好", "Nana 的偏好", "Vivian 的偏好"]);
    let mut request = state("这个呢？");
    request["messages"] = json!([routed("assistant", "我喜欢蛋糕", "nana")]);
    let first = step.ainvoke(request.clone(), None).await.unwrap();
    let cached = step.ainvoke(request.clone(), None).await.unwrap();
    assert_eq!(cached["metadata"]["query_rewrite_cache_hit"], true);
    assert_eq!(first["resolved_user_input"], cached["resolved_user_input"]);
    assert_eq!(router.requests.lock().len(), 1);
    request["metadata"]["communication_context"]["speaker"] = json!("nana");
    let changed = step.ainvoke(request.clone(), None).await.unwrap();
    assert_eq!(changed["resolved_user_input"], "Nana 的偏好");
    request["messages"] = json!([routed("assistant", "我喜欢蛋糕", "vivian")]);
    let changed = step.ainvoke(request, None).await.unwrap();
    assert_eq!(changed["resolved_user_input"], "Vivian 的偏好");
    assert_eq!(router.requests.lock().len(), 3);
}

#[tokio::test]
async fn resolved_queries_and_new_topics_make_no_extra_model_calls() {
    let (router, step) = fixture(&[]);
    let result = step.ainvoke(state("我喜欢吃蛋糕"), None).await.unwrap();
    assert_eq!(result["metadata"]["query_rewrite_skipped"], "self_contained");
    let mut request = state("好");
    request["resolved_user_input"] = json!("继续查询用户昨天选择的草莓蛋糕");
    let result = step.ainvoke(request, None).await.unwrap();
    assert_eq!(result["metadata"]["query_rewrite_skipped"], "already_resolved");
    assert_ne!(result["metadata"]["skip_memory_retrieval"], true);
    assert_eq!(result["resolved_user_input"], "继续查询用户昨天选择的草莓蛋糕");
    assert_eq!(router.requests.lock().len(), 0);
}

#[tokio::test]
async fn malformed_or_failed_rewrites_fall_back_without_retrying_or_changing_user_words() {
    let long = "字".repeat(321);
    for reply in ["", "这个呢？", "{\"text\":\"不应进入检索的聊天输出\"}", "[\"查询\"]", "```查询```", "查询\n解释", &long] {
        let (router, step) = fixture(&[reply]);
        let result = step.ainvoke(state("这个呢？"), None).await.unwrap();
        assert_eq!(result["user_input"], "这个呢？");
        assert_eq!(result["resolved_user_input"], "");
        assert_eq!(router.requests.lock().len(), 1);
    }
    let (router, step) = fixture(&[]);
    let result = step.ainvoke(state("这个呢？"), None).await.unwrap();
    assert_eq!(result["resolved_user_input"], "");
    assert_eq!(router.requests.lock().len(), 1);
}
