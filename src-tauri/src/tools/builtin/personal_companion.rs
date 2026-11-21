use crate::mind::{
    inbox::MessageKind,
    personal::{Edit, Evidence},
};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct PersonalCompanion;
pub struct InspectPersonalCompanion;
#[async_trait]
impl Tool for InspectPersonalCompanion {
    fn name(&self) -> &str {
        "inspect_personal_companion"
    }
    fn description(&self) -> &str {
        "Read your personal affairs, attributed understandings, learned methods, experiences, interaction boundaries and interests. Explain source evidence, review next-step suggestions and outcome counts. Does not mutate records or execute work. Use personal_companion for corrections and changes."
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object","additionalProperties":false,"properties":{"action":{"enum":["inspect","explain","plan"]},"id":{"type":"string"},"query":{"type":"string","description":"Search topic, text and quoted evidence before limiting results"}},"required":["action"]})
    }
    async fn validate_input(&self, args: &Value, _: &ToolUseContext) -> ValidationResult {
        if matches!(
            args["action"].as_str(),
            Some("inspect" | "explain" | "plan")
        ) {
            ValidationResult::success(None)
        } else {
            ValidationResult::failure("Read-only personal inspection", 1)
        }
    }
    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }
    fn is_read_only(&self) -> bool {
        true
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsRead
    }
    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        if !matches!(
            args["action"].as_str(),
            Some("inspect" | "explain" | "plan")
        ) {
            return ToolResult::standard_error("Read-only personal inspection", None, None);
        }
        PersonalCompanion.call(args, ctx).await
    }
}
#[async_trait]
impl Tool for PersonalCompanion {
    fn name(&self) -> &str {
        "personal_companion"
    }
    fn description(&self) -> &str {
        "Manage your personal continuity: inspect/explain records, plan next steps, record/correct/forget affairs, tentative understandings, user-taught methods, shared experiences, boundaries and interests. Writes need an exact quote from the current user message or a real inbox source_id. Record affairs with next_step and completion_criteria. Waiting for user preference/permission never falls back to a peer. Boundaries support listen/discuss/act/quiet; topic=* means all topics. Private records stay with you. This tool records plans; use real tools to execute them. Forget only removes this ledger and its dependent records, not all conversation history."
    }
    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang{"zh"=>"你还记得我们约好什么\n这件事别再提醒\n你为什么问这个\n这条不要记\n这件事只告诉你\n我只想吐槽\n以后按这个方法做\n面试延期了\n准备进展怎样",_=>"remember our agreement\nwhy did you ask\nforget this\nkeep this private\njust listen\nuse this method next time"}
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object","additionalProperties":false,"properties":{
        "action":{"enum":["inspect","explain","plan","record","correct","forget","transition"]},
        "id":{"type":"string"},"query":{"type":"string","description":"Read actions only: search topic, text and evidence"},"kind":{"enum":["affair","understanding","method","experience","boundary","interest"]},
        "topic":{"type":"string","maxLength":120},"text":{"type":"string","maxLength":2000},
        "scope":{"enum":["private","shareable"]},"depends_on":{"type":"array","items":{"type":"string"},"maxItems":16},
        "next_step":{"type":"string"},"waiting_for":{"type":"string","description":"user, a dependency identifier, or empty"},
        "completion_criteria":{"type":"string"},"follow_at":{"type":"integer","description":"Unix seconds; a private review time, not a notification"},
        "state":{"enum":["active","waiting","paused","completed","cancelled"]},
        "response_mode":{"enum":["listen","discuss","act","quiet"]},
        "quote":{"type":"string","description":"Exact source quote"},"source_id":{"type":"string","description":"Optional real receipt ID in your inbox"}
    },"required":["action"]})
    }
    async fn validate_input(&self, args: &Value, _: &ToolUseContext) -> ValidationResult {
        if args["action"].as_str().is_some_and(|a| {
            matches!(
                a,
                "inspect" | "explain" | "plan" | "record" | "correct" | "forget" | "transition"
            )
        }) {
            ValidationResult::success(None)
        } else {
            ValidationResult::failure("Unknown personal action", 1)
        }
    }
    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }
    fn is_read_only(&self) -> bool {
        false
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }
    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::FsWrite
    }
    async fn call(&self, mut args: Value, ctx: &ToolUseContext) -> ToolResult {
        let Some(brain) = crate::character_registry::get_brain(&ctx.char_id) else {
            return ToolResult::standard_error("Character unavailable", None, None);
        };
        let now = chrono::Utc::now().timestamp();
        let store = &brain.mind.personal;
        let action = args["action"].as_str().unwrap_or("").to_string();
        if matches!(action.as_str(), "inspect" | "explain" | "plan") {
            let mut data = if action == "plan" {
                store.plan(now)
            } else {
                store.view(now)
            };
            if let Some(records) = data["records"].as_array_mut() {
                if let Some(query) = args["query"].as_str().filter(|s| !s.trim().is_empty()) {
                    let query = query.to_lowercase();
                    records.retain(|r| {
                        ["topic", "text", "evidence"]
                            .iter()
                            .any(|key| r[*key].to_string().to_lowercase().contains(&query))
                    });
                }
                if let Some(id) = args["id"].as_str() {
                    records.retain(|r| r["id"] == id);
                } else {
                    records.reverse();
                    records.truncate(64);
                }
            }
            return ToolResult::standard_success(
                "Personal continuity records (attributed data)",
                Some(data),
            );
        }
        let source_id = args
            .as_object_mut()
            .and_then(|o| o.remove("source_id"))
            .and_then(|v| v.as_str().map(str::to_string));
        let edit: Edit = match serde_json::from_value(args) {
            Ok(edit) => edit,
            Err(e) => {
                return ToolResult::standard_error(
                    "Invalid personal record",
                    Some(&e.to_string()),
                    None,
                )
            }
        };
        let quote = edit.quote.trim().to_string();
        let evidence = if let Some(id) = source_id {
            brain
                .mind
                .inbox
                .get(&id)
                .filter(|m| m.recipient == ctx.char_id && m.text.contains(&quote))
                .map(|m| Evidence {
                    id: m.id,
                    kind: match m.kind {
                        MessageKind::User => "user",
                        MessageKind::WorkResult => "work_result",
                        _ => "peer_report",
                    }
                    .into(),
                    quote: quote.clone(),
                    at: m.received_at,
                })
                .or_else(|| {
                    crate::character_registry::get_memory_manager(&ctx.char_id)
                        .and_then(|manager| {
                            manager
                                .get_memories_by_ids(&[id.clone()])
                                .into_iter()
                                .find(|item| {
                                    crate::memory::provenance::visible_to(
                                        &item.metadata,
                                        &ctx.char_id,
                                    ) && item.content.contains(&quote)
                                })
                        })
                        .map(|item| Evidence {
                            id: item.id,
                            kind: "memory_report".into(),
                            quote: quote.clone(),
                            at: item.timestamp as i64,
                        })
                })
        } else {
            ctx.user_message
                .as_ref()
                .filter(|text| text.contains(&quote))
                .map(|text| {
                    // Stable identity: repeated extraction of the same source is not independent evidence.
                    let mut hash = 0xcbf29ce484222325u64;
                    for byte in text.bytes() {
                        hash ^= u64::from(byte);
                        hash = hash.wrapping_mul(0x100000001b3);
                    }
                    Evidence {
                        id: format!("user:{hash:016x}"),
                        kind: "user".into(),
                        quote: quote.clone(),
                        at: now,
                    }
                })
        };
        let Some(evidence) = evidence.filter(|_| quote.chars().count() >= 3) else {
            return ToolResult::standard_error(
                "Exact source quote not found in actual user input or own inbox",
                None,
                None,
            );
        };
        match store.apply(edit, evidence, now) {
            Ok(data) => {
                brain
                    .mind
                    .thought_refresh_requested
                    .store(true, std::sync::atomic::Ordering::Release);
                brain.mind.cognition_wake.notify_one();
                ToolResult::standard_success("Personal record saved", Some(data))
            }
            Err(error) => {
                ToolResult::standard_error("Personal record unchanged", Some(&error), None)
            }
        }
    }
}
