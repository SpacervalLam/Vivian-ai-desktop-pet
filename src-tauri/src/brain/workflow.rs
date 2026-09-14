//! 编排引擎 —— 执行模型产出的多步工作流脚本（可扇出并行步骤）。
//!
//! 模型用 `run_workflow` 工具提交一个 JSON 编排：步骤序列中标记 `parallel: true`
//! 的连续步骤会被分为一组并发执行，其余顺序执行；每步是一个工具调用（完整经过
//! 沙箱/审批管线）。执行完返回逐步结果汇总，任一失败不中断后续（失败信息进入汇总）。

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::tools::executor::execute_tool_use;
use crate::tools::types::ToolUseContext;
use crate::tools::ToolSystem;

/// 单个编排步骤。
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowStep {
    pub tool: String,
    pub arguments: Value,
    pub parallel: bool,
}

/// 单步执行结果。
#[derive(Debug, Clone, Serialize)]
pub struct StepOutcome {
    pub index: usize,
    pub tool: String,
    pub success: bool,
    /// 是否属于并行扇出组（前端按连续 parallel 分组展示）
    pub parallel: bool,
    pub summary: String,
}

/// 整个工作流运行结果。
#[derive(Debug, Clone, Serialize)]
pub struct WorkflowRun {
    pub name: String,
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub steps: Vec<StepOutcome>,
}

/// 执行一个工作流。
///
/// 执行语义：
/// - 顺序步骤依次执行（前一步结果可通过 `${{steps.N}}` 占位引用，暂不支持——保持简单）
/// - 连续 `parallel: true` 的步骤并发执行并等待整组完成
/// - 单步失败不中断工作流，失败信息进入汇总
pub async fn run_workflow(
    name: &str,
    steps: Vec<WorkflowStep>,
    tool_system: &Arc<ToolSystem>,
    context: &ToolUseContext,
) -> WorkflowRun {
    let total = steps.len();
    let mut outcomes: Vec<StepOutcome> = Vec::with_capacity(total);

    let mut i = 0usize;
    while i < steps.len() {
        if steps[i].parallel {
            // 收集本并行组
            let mut group: Vec<(usize, WorkflowStep)> = Vec::new();
            while i < steps.len() && steps[i].parallel {
                group.push((i, steps[i].clone()));
                i += 1;
            }
            // 并发执行（克隆所需字段进 task，避免借用组容器）
            let mut futs = Vec::with_capacity(group.len());
            for (idx, step) in group.iter() {
                let ts = Arc::clone(tool_system);
                let ctx = context.clone();
                let tool = step.tool.clone();
                let args = step.arguments.clone();
                let idx = *idx;
                futs.push(tokio::spawn(async move {
                    let result = execute_tool_use(&tool, args, &ts, &ctx, None).await;
                    (idx, tool, result.success, summarize_result(&result))
                }));
            }
            let mut group_outcomes: Vec<StepOutcome> = Vec::with_capacity(futs.len());
            for f in futs {
                if let Ok((idx, tool, success, summary)) = f.await {
                    group_outcomes.push(StepOutcome { index: idx, tool, success, parallel: true, summary });
                }
            }
            group_outcomes.sort_by_key(|o| o.index);
            outcomes.extend(group_outcomes);
        } else {
            let step = &steps[i];
            let result = execute_tool_use(&step.tool, step.arguments.clone(), tool_system, context, None).await;
            outcomes.push(StepOutcome {
                index: i,
                tool: step.tool.clone(),
                success: result.success,
                parallel: false,
                summary: summarize_result(&result),
            });
            i += 1;
        }
    }

    let succeeded = outcomes.iter().filter(|o| o.success).count();
    WorkflowRun {
        name: name.to_string(),
        total,
        succeeded,
        failed: total - succeeded,
        steps: outcomes,
    }
}

fn summarize_result(result: &crate::tools::types::ToolResult) -> String {
    if result.success {
        result
            .data
            .as_ref()
            .map(|d| serde_json::to_string(d).unwrap_or_default())
            .unwrap_or_else(|| "成功".into())
    } else {
        result.error.clone().unwrap_or_else(|| "执行失败".into())
    }
}
