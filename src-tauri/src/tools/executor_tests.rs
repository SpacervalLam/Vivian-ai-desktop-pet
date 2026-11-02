use super::super::types::{PermissionResult, Tool, ToolCategory, ValidationResult};
use super::*;
use async_trait::async_trait;
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Probe {
    calls: Arc<AtomicUsize>,
    allowed: Arc<AtomicBool>,
    modifier: bool,
    panic_validation: bool,
    replacement: Option<Value>,
    name: &'static str,
    wait: Option<Arc<tokio::sync::Notify>>,
}

#[tokio::test]
async fn user_correction_during_permission_check_invalidates_the_approved_call() {
    struct PermissionProbe {
        control: crate::pipeline::execution_control::ExecutionControl,
        calls: Arc<AtomicUsize>,
    }
    #[async_trait]
    impl Tool for PermissionProbe {
        fn name(&self) -> &str { "permission_lease_probe" }
        fn description(&self) -> &str { "test observation" }
        fn parameters_schema(&self) -> Value { json!({"type":"object","properties":{}}) }
        async fn validate_input(&self, _: &Value, _: &ToolUseContext) -> ValidationResult { ValidationResult::success(None) }
        async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
            tokio::task::yield_now().await;
            self.control.interrupt();
            PermissionResult::allow()
        }
        async fn call(&self, _: Value, _: &ToolUseContext) -> ToolResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            ToolResult::success(json!({"observed":true}))
        }
        fn is_read_only(&self) -> bool { true }
        fn category(&self) -> ToolCategory { ToolCategory::Memory }
    }
    let control = crate::pipeline::execution_control::ExecutionControl::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let system = ToolSystem::new();
    system.register_tool(Arc::new(PermissionProbe { control:control.clone(), calls:calls.clone() }));
    let result = control.lease().scope(execute_tool_use("permission_lease_probe", json!({}), &system, &ToolUseContext::default(), None)).await;
    assert_eq!(result.error.as_deref(), Some("ExecutionInterrupted"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn work_file_edits_use_selected_workspace_without_a_blanket_approval_callback() {
    let system = ToolSystem::new();
    system.register_tool(Arc::new(
        crate::tools::builtin::coding_tools::WriteFileTool::new(),
    ));
    let directory = tempfile::tempdir().unwrap();
    let ctx = ToolUseContext::default()
        .with_working_directory(directory.path().to_string_lossy())
        .with_agent_kind("work");
    let ctx = ToolUseContext {
        access_level: Some(AgentAccessLevel::FsWrite),
        ..ctx
    };
    let path = directory.path().join("approved.txt");
    let result = execute_tool_use(
        "write_file",
        json!({"path":path,"content":"bounded"}),
        &system,
        &ctx,
        None,
    )
    .await;
    assert!(result.success, "{result:?}");
    assert_eq!(std::fs::read_to_string(path).unwrap(), "bounded");
}

#[tokio::test]
async fn companion_side_defers_file_writes_and_commands_to_the_work_agent() {
    let system = ToolSystem::new();
    system.register_tool(Arc::new(crate::tools::builtin::coding_tools::WriteFileTool::new()));
    system.register_tool(Arc::new(crate::tools::builtin::coding_tools::RunCommandTool::new()));
    let directory = tempfile::tempdir().unwrap();
    let ctx = ToolUseContext::default().with_working_directory(directory.path().to_string_lossy());
    let path = directory.path().join("must-not-exist.txt");
    let result = execute_tool_use(
        "write_file",
        json!({"path": path, "content": "blocked"}),
        &system,
        &ctx,
        None,
    )
    .await;
    assert_eq!(result.error.as_deref(), Some("CompanionSimpleOnly"));
    let message = result
        .data
        .as_ref()
        .and_then(|data| data["message"].as_str())
        .unwrap_or_default();
    assert!(message.contains("delegate_to_work_agent"), "{message}");
    assert!(!path.exists(), "the companion side must not write files itself");
    let result = execute_tool_use(
        "run_command",
        json!({"command": "Write-Output 'blocked'"}),
        &system,
        &ctx,
        None,
    )
    .await;
    assert_eq!(result.error.as_deref(), Some("CompanionSimpleOnly"));
}

#[tokio::test]
async fn readonly_work_cannot_write_files_even_with_an_approval_callback() {
    let system=ToolSystem::new();
    system.register_tool(Arc::new(crate::tools::builtin::coding_tools::WriteFileTool::new()));
    let directory=tempfile::tempdir().unwrap();
    let ctx=ToolUseContext {access_level:Some(AgentAccessLevel::ReadOnly),..ToolUseContext::default().with_working_directory(directory.path().to_string_lossy()).with_agent_kind("work")};
    let result=execute_tool_use("write_file",json!({"path":"forbidden.txt","content":"bad"}),&system,&ctx,Some(Arc::new(|_,_|true))).await;
    assert!(!result.success,"{result:?}");assert!(!directory.path().join("forbidden.txt").exists());
}

impl Probe {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            allowed: Arc::new(AtomicBool::new(true)),
            modifier: false,
            panic_validation: false,
            replacement: None,
            name: "cache_probe",
            wait: None,
        }
    }
}

#[tokio::test]
async fn composite_tools_do_not_bypass_disabled_dependencies_or_child_allowlists() {
    let system = ToolSystem::new();
    let mut probe = Probe::new();
    probe.name = "computer_action";
    let calls = probe.calls.clone();
    system.register_tool(Arc::new(probe));
    system.set_disabled_tools(vec!["take_screenshot".into()], Vec::new());
    let ctx = ToolUseContext::default();
    let result = execute_tool_use(
        "computer_action",
        json!({"action":"move"}),
        &system,
        &ctx,
        None,
    )
    .await;
    assert_eq!(result.error.as_deref(), Some("DependencyDisabled"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let ctx = ToolUseContext {
        subagent_depth: 1,
        allowed_tools: Some(vec!["read_file".into()]),
        ..ctx
    };
    let result = execute_tool_use(
        "computer_action",
        json!({"action":"move"}),
        &system,
        &ctx,
        None,
    )
    .await;
    assert_eq!(result.error.as_deref(), Some("ToolNotAuthorized"));
}
#[async_trait]
impl Tool for Probe {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Tool execution contract probe"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object","properties":{"path":{"type":"string"}}})
    }
    async fn validate_input(&self, _: &Value, _: &ToolUseContext) -> ValidationResult {
        assert!(!self.panic_validation, "validation panic probe");
        ValidationResult::success(self.replacement.clone())
    }
    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        if self.allowed.load(Ordering::SeqCst) {
            PermissionResult::allow()
        } else {
            PermissionResult::deny("probe permission revoked")
        }
    }
    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(started) = &self.wait {
            started.notify_one();
            std::future::pending::<()>().await;
        }
        let result = ToolResult::success(json!({"invocation":n,"args":args,"user":ctx.user_id}))
            .with_goal_completed();
        if self.modifier {
            result.with_context_modifier(Arc::new(|ctx| ctx.user_id = "changed".into()))
        } else {
            result
        }
    }
    fn is_read_only(&self) -> bool {
        true
    }
    fn is_cacheable(&self) -> bool {
        true
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }
}

#[tokio::test]
async fn cache_hits_recheck_permissions_and_preserve_completion_and_metrics() {
    let system = ToolSystem::new();
    let probe = Probe::new();
    let (calls, allowed) = (probe.calls.clone(), probe.allowed.clone());
    system.register_tool(Arc::new(probe));
    let ctx = ToolUseContext::default();
    for _ in 0..2 {
        let result = execute_tool_use("cache_probe", json!({}), &system, &ctx, None).await;
        assert!(result.success && result.goal_completed);
    }
    allowed.store(false, Ordering::SeqCst);
    let result = execute_tool_use("cache_probe", json!({}), &system, &ctx, None).await;
    assert_eq!(result.error.as_deref(), Some("PermissionDenied"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let metrics = system.observability.get_metrics("cache_probe").unwrap();
    assert_eq!(
        (
            metrics.total_calls,
            metrics.cached_calls,
            metrics.failed_calls
        ),
        (3, 1, 1)
    );
    let records = system
        .observability
        .get_recent_records("cache_probe", 3, false);
    assert_eq!(records[0].phase, "permission");
    assert!(records[1].cache_hit);
}

#[tokio::test]
async fn cache_isolates_callers_and_hot_replacement() {
    let system = ToolSystem::new();
    let probe = Probe::new();
    let calls = probe.calls.clone();
    system.register_tool(Arc::new(probe));
    let mut ctx = ToolUseContext::default();
    for field in [
        "initial",
        "session",
        "character",
        "workspace",
        "user",
        "message",
    ] {
        match field {
            "session" => ctx.session_id = "second".into(),
            "character" => ctx.char_id = "nana".into(),
            "workspace" => ctx.working_directory = "C:/another-workspace".into(),
            "user" => ctx.user_id = "another-user".into(),
            "message" => ctx.user_message = Some("new request".into()),
            _ => {}
        }
        assert!(
            execute_tool_use("cache_probe", json!({}), &system, &ctx, None)
                .await
                .success
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    let replacement = Probe::new();
    let replacement_calls = replacement.calls.clone();
    system.register_tool(Arc::new(replacement));
    assert!(
        execute_tool_use("cache_probe", json!({}), &system, &ctx, None)
            .await
            .success
    );
    assert_eq!(replacement_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cached_results_still_pass_guard_and_post_policy() {
    let system = ToolSystem::new();
    let probe = Probe::new();
    let calls = probe.calls.clone();
    system.register_tool(Arc::new(probe));
    let ctx = ToolUseContext::default();
    assert!(
        execute_tool_use("cache_probe", json!({}), &system, &ctx, None)
            .await
            .success
    );
    let policies = Arc::new(crate::cordis::RuntimeContext::new());
    system.set_policy_ctx(policies.clone());
    let deny_guard = super::super::policy::register_guard(&policies, |mut flow| {
        Box::pin(async move {
            flow.decision = Some(super::super::policy::ToolGuardDecision::Deny {
                reason: "test guard".into(),
            });
            Some(flow)
        })
    });
    let denied = execute_tool_use("cache_probe", json!({}), &system, &ctx, None).await;
    assert_eq!(denied.error.as_deref(), Some("GuardDenied"));
    drop(deny_guard);
    let _deny_post =
        super::super::policy::register_post_execute(&policies, |_| Box::pin(async { None }));
    let blocked = execute_tool_use("cache_probe", json!({}), &system, &ctx, None).await;
    assert_eq!(blocked.error.as_deref(), Some("PostExecuteBlocked"));
    assert!(!blocked.goal_completed);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let record = system
        .observability
        .get_recent_records("cache_probe", 1, false)
        .pop()
        .unwrap();
    assert!(record.cache_hit && !record.success);
}

#[tokio::test]
async fn non_serializable_context_modifiers_are_never_cached() {
    let system = ToolSystem::new();
    let mut probe = Probe::new();
    probe.modifier = true;
    let calls = probe.calls.clone();
    system.register_tool(Arc::new(probe));
    let ctx = ToolUseContext::default();
    for _ in 0..2 {
        let result = execute_tool_use("cache_probe", json!({}), &system, &ctx, None).await;
        let mut changed = ctx.clone();
        result.context_modifier.unwrap()(&mut changed);
        assert_eq!(changed.user_id, "changed");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(system.cache.stats()["total_entries"], 0);
}

#[tokio::test]
async fn validated_targets_are_checked_by_the_sandbox() {
    let system = ToolSystem::new();
    let mut probe = Probe::new();
    probe.name = "write_file";
    probe.replacement = Some(json!({"path":"C:/outside/private.txt"}));
    let calls = probe.calls.clone();
    system.register_tool(Arc::new(probe));
    let ctx = ToolUseContext::default()
        .with_working_directory("C:/workspace")
        .with_agent_kind("work");
    let result = execute_tool_use(
        "write_file",
        json!({"path":"C:/workspace/safe.txt"}),
        &system,
        &ctx,
        None,
    )
    .await;
    assert!(!result.success);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        system
            .observability
            .get_recent_records("write_file", 1, false)[0]
            .phase,
        "sandbox"
    );
}

#[tokio::test]
async fn validation_panics_and_unknown_tools_produce_observable_failures() {
    let system = ToolSystem::new();
    let mut probe = Probe::new();
    probe.panic_validation = true;
    system.register_tool(Arc::new(probe));
    let ctx = ToolUseContext::default();
    let panic = execute_tool_use("cache_probe", json!({}), &system, &ctx, None).await;
    assert_eq!(panic.error.as_deref(), Some("ToolExecutionPanic"));
    let unknown = execute_tool_use("missing", json!({}), &system, &ctx, None).await;
    assert!(!unknown.success);
    assert_eq!(
        system
            .observability
            .get_recent_records("cache_probe", 1, false)[0]
            .phase,
        "validation"
    );
    assert_eq!(
        system.observability.get_recent_records("missing", 1, false)[0].phase,
        "lookup"
    );
}

#[tokio::test]
async fn cancelled_execution_is_recorded_and_does_not_populate_cache() {
    let system = Arc::new(ToolSystem::new());
    let mut probe = Probe::new();
    let started = Arc::new(tokio::sync::Notify::new());
    probe.wait = Some(started.clone());
    system.register_tool(Arc::new(probe));
    let task_system = system.clone();
    let task = tokio::spawn(async move {
        execute_tool_use(
            "cache_probe",
            json!({}),
            &task_system,
            &ToolUseContext::default(),
            None,
        )
        .await
    });
    started.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let record = system
        .observability
        .get_recent_records("cache_probe", 1, false)
        .pop()
        .unwrap();
    assert_eq!(record.error.as_deref(), Some("ToolCancelled"));
    assert_eq!(system.cache.stats()["total_entries"], 0);
}

struct BatchProbe {
    read_only: bool,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    stage: Arc<AtomicUsize>,
}
#[async_trait]
impl Tool for BatchProbe {
    fn name(&self) -> &str {
        if self.read_only {
            "batch_read"
        } else {
            "batch_write"
        }
    }
    fn description(&self) -> &str {
        "Read/write barrier probe"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type":"object"})
    }
    async fn validate_input(&self, _: &Value, _: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }
    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }
    async fn call(&self, args: Value, _: &ToolUseContext) -> ToolResult {
        assert_eq!(
            args["stage"].as_u64().unwrap() as usize,
            self.stage.load(Ordering::SeqCst)
        );
        if self.read_only {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(5)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
        } else {
            assert_eq!(self.active.load(Ordering::SeqCst), 0);
            self.stage.fetch_add(1, Ordering::SeqCst);
        }
        ToolResult::success(args)
    }
    fn is_read_only(&self) -> bool {
        self.read_only
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }
}

#[tokio::test]
async fn parallel_entrypoint_bounds_reads_and_keeps_writes_as_ordered_barriers() {
    let system = Arc::new(ToolSystem::new());
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let stage = Arc::new(AtomicUsize::new(0));
    for read_only in [true, false] {
        system.register_tool(Arc::new(BatchProbe {
            read_only,
            active: active.clone(),
            peak: peak.clone(),
            stage: stage.clone(),
        }));
    }
    let mut calls = Vec::new();
    for stage in 0..2 {
        for slot in 0..20 {
            calls.push(("batch_read".into(), json!({"stage":stage,"slot":slot})));
        }
        calls.push(("batch_write".into(), json!({"stage":stage,"slot":20})));
    }
    let results =
        execute_tool_calls_parallel(calls, system.clone(), ToolUseContext::default(), None).await;
    assert_eq!(results.len(), 42);
    assert!(results.iter().all(|result| result.success));
    for (index, result) in results.iter().enumerate() {
        assert_eq!(result.data.as_ref().unwrap()["slot"], index % 21);
    }
    assert!((2..=MAX_PARALLEL_READS).contains(&peak.load(Ordering::SeqCst)));
    assert_eq!(stage.load(Ordering::SeqCst), 2);
    assert_eq!(system.cache.generation(), 6); // two registrations, two mutation boundaries each
}

#[tokio::test]
async fn sequential_chains_pass_results_and_context_modifiers_to_the_next_step() {
    let system = ToolSystem::new();
    let mut probe = Probe::new();
    probe.modifier = true;
    system.register_tool(Arc::new(probe));
    let chain = super::super::chainer::build_sequential_chain(
        "probe",
        vec!["cache_probe".into(), "cache_probe".into()],
    );
    let result = chain
        .execute(json!({"input":1}), &system, &ToolUseContext::default())
        .await;
    assert!(result.success);
    assert_eq!(result.final_output["args"]["args"]["input"], 1);
    assert_eq!(result.final_output["user"], "changed");
}

#[tokio::test]
async fn skipped_chain_failure_preserves_the_previous_output() {
    use super::super::chainer::{FailurePolicy, ToolChain};
    let system = ToolSystem::new();
    system.register_tool(Arc::new(Probe::new()));
    let chain = ToolChain::new("skip")
        .then("missing", |_, _| json!({"rejected":true}))
        .with_failure_policy(FailurePolicy::Skip)
        .then("cache_probe", |previous, original| {
            assert_eq!(previous, original);
            previous.clone()
        });
    let result = chain
        .execute(json!({"input":1}), &system, &ToolUseContext::default())
        .await;
    assert_eq!(result.steps.len(), 2);
    assert!(!result.steps[0].success && result.steps[1].success);
    assert_eq!(result.final_output["args"]["input"], 1);
}

#[tokio::test]
async fn adapter_and_chain_cannot_bypass_live_tool_disabling() {
    use crate::pipeline::base::Runnable;
    let system = Arc::new(ToolSystem::new());
    let probe = Arc::new(Probe::new());
    let calls = probe.calls.clone();
    system.register_tool(probe.clone());
    system.set_disabled_tools(vec!["cache_probe".into()], Vec::new());
    let adapter = super::super::runnable_adapter::ToolRunnableAdapter::new(probe)
        .with_tool_system(system.clone())
        .with_skip_permissions(true);
    assert!(adapter.ainvoke(json!({}), None).await.is_err());
    let chain =
        super::super::chainer::build_sequential_chain("disabled", vec!["cache_probe".into()]);
    assert!(
        !chain
            .execute(json!({}), &system, &ToolUseContext::default())
            .await
            .success
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        system
            .observability
            .get_metrics("cache_probe")
            .unwrap()
            .failed_calls,
        2
    );
}

#[tokio::test]
async fn chain_step_timeout_cancels_execution_and_retains_a_failure_receipt() {
    let system = ToolSystem::new();
    let mut probe = Probe::new();
    probe.wait = Some(Arc::new(tokio::sync::Notify::new()));
    system.register_tool(Arc::new(probe));
    let chain = super::super::chainer::ToolChain::new("timeout")
        .then("cache_probe", |previous, _| previous.clone())
        .with_timeout(1);
    let result = chain
        .execute(json!({}), &system, &ToolUseContext::default())
        .await;
    assert!(!result.success);
    assert_eq!(result.steps[0].error.as_deref(), Some("ChainStepTimeout"));
    assert_eq!(
        system
            .observability
            .get_recent_records("cache_probe", 1, false)[0]
            .error
            .as_deref(),
        Some("ToolCancelled")
    );
}

#[tokio::test]
async fn workspace_relative_file_paths_use_session_directory() {
    let system = ToolSystem::new();
    system.register_tool(Arc::new(crate::tools::builtin::file_tools::ReadFileTool::default()));
    system.register_tool(Arc::new(crate::tools::builtin::coding_tools::WriteFileTool::new()));
    system.register_tool(Arc::new(crate::tools::builtin::coding_tools::ListDirTool::new()));
    system.register_tool(Arc::new(crate::tools::builtin::coding_tools::GrepSearchTool::new()));
    system.register_tool(Arc::new(crate::tools::builtin::coding_tools::GlobTool::new()));
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    std::fs::create_dir_all(workspace.join(".vivian")).unwrap();
    let ctx = ToolUseContext {
        access_level: Some(AgentAccessLevel::FullControl),
        ..ToolUseContext::default()
            .with_working_directory(workspace.to_string_lossy())
            .with_agent_kind("work")
    };
    let relative = ".vivian/memory.md";
    let result = execute_tool_use("write_file", json!({"path":relative,"content":"session memory marker"}), &system, &ctx, None).await;
    assert!(result.success, "{result:?}");
    assert_eq!(std::fs::read_to_string(workspace.join(relative)).unwrap(), "session memory marker");
    let result = execute_tool_use("read_file", json!({"path":relative}), &system, &ctx, None).await;
    assert!(result.success, "{result:?}");
    assert_eq!(result.data.as_ref().unwrap()["data"]["content"], "session memory marker");
    for (name, args) in [
        ("list_dir", json!({"directory":".vivian"})),
        ("grep_search", json!({"directory":".vivian","query":"session memory marker"})),
        ("glob", json!({"directory":".vivian","pattern":"*.md"})),
    ] {
        let result = execute_tool_use(name, args, &system, &ctx, None).await;
        assert!(result.success, "{name}: {result:?}");
        assert!(result.data.as_ref().unwrap().to_string().contains("memory.md"), "{name}: {result:?}");
    }
    let result = execute_tool_use("read_file", json!({"path":workspace.join(relative)}), &system, &ctx, None).await;
    assert!(result.success, "absolute paths must still work: {result:?}");
    let result = execute_tool_use("read_file", json!({"path":".vivian/missing.md"}), &system, &ctx, None).await;
    assert!(!result.success);
    assert_eq!(result.data.as_ref().unwrap()["message"], "文件不存在");
    // Full access permits external reads and writes without another approval,
    // independently of the legacy sandbox warning mode.
    system.sandbox.set_protection_mode(crate::tools::sandbox::ProtectionMode::Permissive);
    let outside = directory.path().join("outside.md");
    std::fs::write(&outside, "private").unwrap();
    for path in [outside.to_string_lossy().into_owned(), "../outside.md".into()] {
        let result = execute_tool_use("read_file", json!({"path":path}), &system, &ctx, None).await;
        assert!(result.success, "{result:?}");
        assert_eq!(result.data.as_ref().unwrap()["data"]["content"], "private");
        let result = execute_tool_use("write_file", json!({"path":path,"content":"full access"}), &system, &ctx, Some(Arc::new(|_,_|false))).await;
        assert!(result.success, "{result:?}");
        std::fs::write(&outside, "private").unwrap();
    }
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "private");
    let locked = workspace.join("locked");
    std::fs::create_dir(&locked).unwrap();
    let ctx = ctx.with_extra_working_directory(locked.to_string_lossy(), true);
    let result = execute_tool_use("write_file", json!({"path":"locked/new.md","content":"blocked"}), &system, &ctx, None).await;
    assert!(!result.success, "{result:?}");
    assert!(!locked.join("new.md").exists());
}

#[tokio::test]
async fn external_file_reads_can_be_confirmed_without_granting_external_writes() {
    let system = ToolSystem::new();
    system.register_tool(Arc::new(crate::tools::builtin::file_tools::ReadFileTool::default()));
    system.register_tool(Arc::new(crate::tools::builtin::coding_tools::EditFileTool::new()));
    system.register_tool(Arc::new(crate::tools::builtin::coding_tools::ListDirTool::new()));
    system.register_tool(Arc::new(crate::tools::builtin::coding_tools::GrepSearchTool::new()));
    system.register_tool(Arc::new(crate::tools::builtin::coding_tools::GlobTool::new()));
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    let external = directory.path().join("external");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&external).unwrap();
    std::fs::write(external.join("outside.md"), "external marker").unwrap();
    for level in [AgentAccessLevel::ReadOnly, AgentAccessLevel::FsWrite, AgentAccessLevel::FullControl] {
        let ctx = ToolUseContext {
            access_level: Some(level),
            ..ToolUseContext::default().with_working_directory(workspace.to_string_lossy()).with_agent_kind("work")
        };
        for (name, args) in [
            ("read_file", json!({"path":"../external/outside.md"})),
            ("edit_file", json!({"path":"../external/outside.md","action":"view"})),
            ("list_dir", json!({"directory":"../external"})),
            ("grep_search", json!({"directory":"../external","query":"external marker"})),
            ("glob", json!({"directory":"../external","pattern":"*.md"})),
        ] {
            let requested = Arc::new(AtomicUsize::new(0));
            let calls = requested.clone();
            let approve: CanUseTool = Arc::new(move |_, _| { calls.fetch_add(1, Ordering::SeqCst); true });
            let result = execute_tool_use(name, args.clone(), &system, &ctx, Some(approve)).await;
            assert!(result.success, "{level:?} {name}: {result:?}");
            assert_eq!(requested.load(Ordering::SeqCst), 0, "{level:?} {name}");
            if level != AgentAccessLevel::FullControl {
                let reject: CanUseTool = Arc::new(|_, _| false);
                let result = execute_tool_use(name, args.clone(), &system, &ctx, Some(reject)).await;
                assert!(result.success, "{level:?} {name}: ordinary reads should not request approval");
            }
            let child = ToolUseContext { subagent_depth: 1, ..ctx.clone() };
            let result = execute_tool_use(name, args, &system, &child, None).await;
            assert!(!result.success, "{name}: child scope must remain bounded");
        }
        if level != AgentAccessLevel::FullControl {
            let result = execute_tool_use("edit_file", json!({"path":external.join("outside.md"),"action":"create","file_text":"blocked"}), &system, &ctx, Some(Arc::new(|_, _| true))).await;
            assert!(!result.success, "{level:?}: external writes remain bounded");
        }
    }
    assert_eq!(std::fs::read_to_string(external.join("outside.md")).unwrap(), "external marker");
}
