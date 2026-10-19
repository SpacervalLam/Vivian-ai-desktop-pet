//! Deferred, character-scoped access to bounded local physical-interaction evidence.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::tools::types::{PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult};

#[derive(Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct Query {
    lookback_seconds: u64,
    limit: usize,
}

impl Default for Query {
    fn default() -> Self { Self { lookback_seconds: 120, limit: 3 } }
}

impl Query {
    fn parse(args: &Value) -> Result<Self, String> {
        let mut query: Self = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
        query.lookback_seconds = query.lookback_seconds.clamp(1, 1800);
        query.limit = query.limit.clamp(1, 8);
        Ok(query)
    }
}

pub struct GetPetInteractionsTool;

#[async_trait]
impl Tool for GetPetInteractionsTool {
    fn name(&self) -> &str { "get_pet_interactions" }
    fn description(&self) -> &str {
        "Query your own avatar's recent observed drag/fling/edge-hit episodes, including silent interactions. Use for counts, timing or details of physical desktop interaction. Runtime cache retains up to 30 minutes / 64 completed episodes. Counts cover whole episodes; respect time-boundary and eviction flags. Not dialogue history or proof of feelings."
    }
    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "查询自己桌宠最近被拖动、松手甩飞、撞到屏幕边缘的互动段落，包括未开口的互动。用于回答次数、时间和过程。运行时缓存最多保留30分钟/64段；计数属于完整段落，注意跨查询边界、截断和淘汰标记，不能据此认定感受或动机。",
            "ja" => "自分のデスクトップペットのドラッグ・投げ・画面端への衝突の観測を検索。回数や時刻の質問に使用。無言の操作も含む。実行中のみ最大30分・64区間。件数は区間全体の観測で、境界・省略に注意。感情の証拠ではない。",
            _ => self.description(),
        }
    }
    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "飞得爽吗刚才\n我刚才甩了你几次\n刚才是不是把你拖起来了\n你撞到屏幕边缘了吗\n刚才怎么把你扔出去了",
            "ja" => "さっき飛んだ？\n何回ドラッグした？\n画面の端にぶつかった？",
            _ => "did you fly just now\nhow many times did I drag or fling you\ndid you hit the screen edge",
        }
    }
    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "additionalProperties": false, "properties": {
            "lookback_seconds": {"type": "integer", "minimum": 1, "maximum": 1800, "description": "Recent seconds to inspect; default 120. Returns overlapping whole episodes, not exact window totals."},
            "limit": {"type": "integer", "minimum": 1, "maximum": 8, "description": "Maximum latest episodes returned; default 3."}
        }})
    }
    async fn validate_input(&self, input: &Value, _context: &ToolUseContext) -> ValidationResult {
        match Query::parse(input) {
            Ok(query) => ValidationResult::success(Some(json!(query))),
            Err(error) => ValidationResult::failure(&error, 2),
        }
    }
    async fn check_permissions(&self, _input: &Value, _context: &ToolUseContext) -> PermissionResult { PermissionResult::allow() }
    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        let query = match Query::parse(&args) {
            Ok(query) => query,
            Err(error) => return ToolResult::standard_error(&error, None, None),
        };
        // Never use active-character fallback or accept a model-supplied character ID.
        let Some(manager) = (!context.char_id.is_empty()).then(||
            crate::character_registry::get_psychology_manager(&context.char_id)).flatten() else {
            return ToolResult::standard_error("Character-scoped interaction observations are unavailable.", None, None);
        };
        let mut data = manager.query_pet_interactions(crate::memory::types::current_timestamp(), query.lookback_seconds, query.limit);
        data["character_id"] = json!(context.char_id);
        tracing::debug!("[PetInteractionQuery] char={} lookback_seconds={} episodes={}",
            context.char_id, query.lookback_seconds, data["matching_episode_count"]);
        ToolResult::standard_success("Recent observed physical-interaction episodes (see scope and retention flags).", Some(data))
    }
    fn is_read_only(&self) -> bool { true }
    fn category(&self) -> ToolCategory { ToolCategory::Memory }
    fn risk(&self) -> ToolRiskTier { ToolRiskTier::Safe }
    fn should_defer(&self) -> bool { true }
    fn search_hint(&self) -> &str { "recent physical pet drag fling flight collision counts 刚才拖动甩飞碰撞次数" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_limits_are_bounded_and_cannot_override_character() {
        let query = Query::parse(&json!({})).unwrap();
        assert_eq!((query.lookback_seconds, query.limit), (120, 3));
        let query = Query::parse(&json!({"lookback_seconds": u64::MAX, "limit": 1000000})).unwrap();
        assert_eq!((query.lookback_seconds, query.limit), (1800, 8));
        assert!(Query::parse(&json!({"character_id": "nana"})).is_err());
        assert!(Query::parse(&json!({"limit": -1})).is_err());
        assert!(GetPetInteractionsTool.should_defer());
    }

    #[tokio::test]
    async fn query_fails_without_character_instead_of_reading_active_pet() {
        let result = GetPetInteractionsTool.call(json!({}), &ToolUseContext::default()).await;
        assert!(!result.success);
    }
}
