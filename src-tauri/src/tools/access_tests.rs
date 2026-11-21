use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
fn context() -> ToolUseContext {
    ToolUseContext {
        access_level: Some(AgentAccessLevel::FsWrite),
        ..ToolUseContext::default()
            .with_agent_kind("work")
            .with_session_id(uuid::Uuid::new_v4().to_string())
    }
}
#[tokio::test]
async fn promotions_preserve_running_authority_and_downgrades_revoke_before_ack() {
    let (ctx, changes) = snapshot(&context());
    let killed = Arc::new(AtomicBool::new(false));
    let flag = killed.clone();
    let _process = register_process(
        &ctx,
        Arc::new(move || {
            flag.store(true, Ordering::Release);
        }),
    )
    .unwrap();
    set_permission(&ctx.session_id, AgentAccessLevel::FullControl);
    assert!(!killed.load(Ordering::Acquire));
    assert!(can_continue(&ctx));
    assert!(!current(&ctx));
    let pending = invalidated(
        changes.clone(),
        Arc::new(AtomicBool::new(false)),
        AgentAccessLevel::FsWrite,
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), pending)
        .await
        .unwrap();
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(30),
        invalidated(
            changes,
            Arc::new(AtomicBool::new(true)),
            AgentAccessLevel::FsWrite
        )
    )
    .await
    .is_err());
    set_permission(&ctx.session_id, AgentAccessLevel::ReadOnly);
    assert!(killed.load(Ordering::Acquire));
    assert!(!can_continue(&ctx));
    let fixture = tempfile::tempdir().unwrap();
    assert!(write_file(&ctx, fixture.path().join("stale.txt"), "bad").is_err());
    assert!(!fixture.path().join("stale.txt").exists());
    assert!(register_process(&ctx, Arc::new(|| {})).is_err());
    assert!(process_snapshot(&ctx).is_err());
    assert_eq!(
        snapshot(&ctx).0.access_level,
        Some(AgentAccessLevel::ReadOnly)
    );
}
#[test]
fn grant_identity_changes_with_scope_and_child_cap_and_never_revives() {
    let (ctx, _) = snapshot(&context());
    let identity = write_identity(&ctx).unwrap();
    assert_eq!(write_identity(&ctx).unwrap(), identity);
    let mut child = ctx.clone();
    child.subagent_depth = 1;
    child.access_level = Some(AgentAccessLevel::ReadOnly);
    assert_ne!(write_identity(&child).unwrap(), identity);
    set_permission(&ctx.session_id, AgentAccessLevel::FullControl);
    set_permission(&ctx.session_id, AgentAccessLevel::FsWrite);
    assert_ne!(write_identity(&snapshot(&ctx).0).unwrap(), identity);
    let (ctx, _) = snapshot(&ctx);
    invalidate_scope(&ctx.session_id);
    assert!(!current(&ctx));
}
#[test]
fn credential_environment_keys_are_not_worker_inputs() {
    for key in [
        "API_KEY",
        "TOKEN",
        "PASSWORD",
        "OPENAI_API_KEY",
        "GH_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
        "AZURE_CLIENT_SECRET",
        "db_password",
    ] {
        assert!(secret_environment_key(key), "{key}");
    }
    for key in ["PATH", "SystemRoot", "USERPROFILE", "TEMP", "PYTHONUTF8"] {
        assert!(!secret_environment_key(key), "{key}");
    }
}

#[tokio::test]
async fn promotion_cancels_an_inflight_policy_decision_before_the_tool_executes() {
    use crate::tools::types::{
        PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ValidationResult,
    };
    struct WaitingRead {
        entered: Arc<tokio::sync::Semaphore>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl Tool for WaitingRead {
        fn name(&self) -> &str {
            "read_file"
        }
        fn description(&self) -> &str {
            "policy decision fixture"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type":"object"})
        }
        async fn validate_input(
            &self,
            _: &serde_json::Value,
            _: &ToolUseContext,
        ) -> ValidationResult {
            ValidationResult::success(None)
        }
        async fn check_permissions(
            &self,
            _: &serde_json::Value,
            _: &ToolUseContext,
        ) -> PermissionResult {
            self.entered.add_permits(1);
            std::future::pending().await
        }
        async fn call(&self, _: serde_json::Value, _: &ToolUseContext) -> ToolResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ToolResult::standard_success("executed", None)
        }
        fn is_read_only(&self) -> bool {
            true
        }
        fn risk(&self) -> ToolRiskTier {
            ToolRiskTier::FsRead
        }
        fn category(&self) -> ToolCategory {
            ToolCategory::File
        }
    }
    let system = Arc::new(crate::tools::ToolSystem::new());
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    system.register_tool(Arc::new(WaitingRead {
        entered: entered.clone(),
        calls: calls.clone(),
    }));
    let ctx = context();
    let active = ctx.clone();
    let future = tokio::spawn(async move {
        crate::tools::execute_tool_use(
            "read_file",
            serde_json::json!({"path":"ordinary.txt"}),
            &system,
            &active,
            None,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    set_permission(&ctx.session_id, AgentAccessLevel::FullControl);
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), future)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.error.as_deref(), Some("PermissionChanged"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
