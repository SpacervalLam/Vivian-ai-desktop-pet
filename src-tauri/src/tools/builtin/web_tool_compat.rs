//! 历史调用的参数迁移。只在旧名称被直接调用时构造，不进入工具目录或模型 schema。
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn target(name: &str) -> Option<&'static str> {
    match name {
        "platform_search" => Some("web_search"),
        "web_fetch" | "read_platform_content" | "read_bilibili_danmaku" => Some("read_content"),
        _ => None,
    }
}

pub fn wrap(name: &str, tool: Arc<dyn Tool>) -> Arc<dyn Tool> {
    Arc::new(Legacy {
        name: name.to_string(),
        tool,
    })
}

fn arguments(name: &str, mut args: Value) -> Result<Value, String> {
    if !args.is_object() {
        return Err("工具参数必须是对象".into());
    }
    if name == "platform_search" {
        if args.get("platforms").is_none() {
            args["platforms"] = json!(super::platform_search_policy::DEFAULT_PLATFORMS);
        }
        if args.get("max_results").is_none() {
            args["max_results"] = args.get("limit").cloned().unwrap_or(json!(4));
        }
        if let Some(obj) = args.as_object_mut() {
            obj.remove("limit");
        }
    } else if name == "read_bilibili_danmaku" && args.get("bvid").is_some() {
        let bvid = args["bvid"].as_str().unwrap_or("").trim();
        if bvid.len() != 12
            || !bvid.starts_with("BV")
            || !bvid.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err("bvid 必须是 BV 开头的 12 位视频编号".into());
        }
        args["url"] = json!(format!("https://www.bilibili.com/video/{bvid}"));
        args["include"] = json!(["danmaku"]);
        args["danmaku_offset"] = args.get("offset").cloned().unwrap_or(json!(0));
        args["danmaku_limit"] = args.get("limit").cloned().unwrap_or(json!(100));
        if let Some(obj) = args.as_object_mut() {
            obj.remove("bvid");
            obj.remove("offset");
            obj.remove("limit");
        }
    }
    Ok(args)
}

struct Legacy {
    name: String,
    tool: Arc<dyn Tool>,
}
#[async_trait]
impl Tool for Legacy {
    fn name(&self) -> &str {
        self.tool.name()
    }
    fn description(&self) -> &str {
        self.tool.description()
    }
    fn parameters_schema(&self) -> Value {
        self.tool.parameters_schema()
    }
    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> ValidationResult {
        match arguments(&self.name, input.clone()) {
            Ok(args) => {
                let mut result = self.tool.validate_input(&args, ctx).await;
                if result.result {
                    result.data = Some(args);
                }
                result
            }
            Err(error) => ValidationResult::failure(error, 2),
        }
    }
    async fn check_permissions(&self, input: &Value, ctx: &ToolUseContext) -> PermissionResult {
        self.tool.check_permissions(input, ctx).await
    }
    async fn call(&self, input: Value, ctx: &ToolUseContext) -> ToolResult {
        match arguments(&self.name, input) {
            Ok(args) => self.tool.call(args, ctx).await,
            Err(error) => ToolResult::standard_error(&error, Some("InvalidLegacyWebInput"), None),
        }
    }
    fn is_read_only(&self) -> bool {
        self.tool.is_read_only()
    }
    fn category(&self) -> ToolCategory {
        self.tool.category()
    }
    fn risk(&self) -> ToolRiskTier {
        self.tool.risk()
    }
    fn execution_timeout(&self, args: &Value) -> Option<std::time::Duration> {
        self.tool.execution_timeout(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn historical_arguments_migrate_idempotently() {
        assert_eq!(target("web_fetch"), Some("read_content"));
        let fetch = json!({"url":"https://example.com","offset":120,"find":"title"});
        assert_eq!(arguments("web_fetch", fetch.clone()).unwrap(), fetch);
        let search = arguments("platform_search", json!({"query":"摄影"})).unwrap();
        assert_eq!(
            search["platforms"],
            json!(super::super::platform_search_policy::DEFAULT_PLATFORMS)
        );
        assert_eq!(
            arguments("platform_search", search.clone()).unwrap(),
            search
        );
        let dm = arguments(
            "read_bilibili_danmaku",
            json!({"bvid":"BV1234567890","page":2,"offset":8,"limit":10}),
        )
        .unwrap();
        assert_eq!(dm["danmaku_offset"], 8);
        assert_eq!(dm["include"], json!(["danmaku"]));
        assert!(dm.get("offset").is_none());
        assert_eq!(arguments("read_bilibili_danmaku", dm.clone()).unwrap(), dm);
        assert!(arguments("read_bilibili_danmaku", json!({"bvid":"bad"})).is_err());
    }
}
