//! 工具链 - 顺序编排多个工具调用，支持输入/输出变换、条件、失败策略
//!
//! - `ToolChain` / `ChainStep` / `ChainBuilder`：声明式工具链
//! - `IntentRecognizer`：基于正则的意图识别，自动选择链模板
//! - 链式执行：上一步输出可作为下一步输入

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::registry::ToolSystem;
use super::tool_call_manager::ToolCallManager;
use super::types::{ToolResult, ToolUseContext};

/// 失败策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailurePolicy {
    /// 失败时停止整条链
    Stop,
    /// 失败时跳过当前步骤继续
    Skip,
    /// 失败时把错误作为输入继续
    Continue,
}

impl Default for FailurePolicy {
    fn default() -> Self {
        FailurePolicy::Stop
    }
}

/// 链中的单个步骤
#[derive(Clone)]
pub struct ChainStep {
    /// 工具名
    pub tool_name: String,
    /// 输入变换：接收上一步输出 + 原始输入，返回当前步骤输入
    pub input_transform: Arc<dyn Fn(&Value, &Value) -> Value + Send + Sync>,
    /// 失败策略
    pub on_failure: FailurePolicy,
    /// 超时秒数（0 表示使用默认）
    pub timeout_secs: u64,
}

impl std::fmt::Debug for ChainStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainStep")
            .field("tool_name", &self.tool_name)
            .field("on_failure", &self.on_failure)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

/// 单步骤执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainStepResult {
    pub tool_name: String,
    pub success: bool,
    pub output: Value,
    pub error: Option<String>,
    pub duration_ms: f64,
}

/// 链执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolChainResult {
    pub success: bool,
    pub steps: Vec<ChainStepResult>,
    pub final_output: Value,
    pub error: Option<String>,
}

impl ToolChainResult {
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            success: false,
            steps: Vec::new(),
            final_output: Value::Null,
            error: Some(message.into()),
        }
    }
}

/// 工具链
pub struct ToolChain {
    /// 链名
    pub name: String,
    /// 步骤序列
    pub steps: Vec<ChainStep>,
}

impl ToolChain {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            steps: Vec::new(),
        }
    }

    /// 添加一步：工具名 + 输入变换
    pub fn then(
        mut self,
        tool_name: impl Into<String>,
        input_transform: impl Fn(&Value, &Value) -> Value + Send + Sync + 'static,
    ) -> Self {
        self.steps.push(ChainStep {
            tool_name: tool_name.into(),
            input_transform: Arc::new(input_transform),
            on_failure: FailurePolicy::default(),
            timeout_secs: 0,
        });
        self
    }

    /// 设置最后一步的失败策略
    pub fn with_failure_policy(mut self, policy: FailurePolicy) -> Self {
        if let Some(last) = self.steps.last_mut() {
            last.on_failure = policy;
        }
        self
    }

    /// 设置最后一步的超时
    pub fn with_timeout(mut self, secs: u64) -> Self {
        if let Some(last) = self.steps.last_mut() {
            last.timeout_secs = secs;
        }
        self
    }

    /// 执行链
    pub async fn execute(
        &self,
        initial_input: Value,
        tool_system: &ToolSystem,
        context: &ToolUseContext,
    ) -> ToolChainResult {
        let mut current_input = initial_input.clone();
        let mut step_results = Vec::with_capacity(self.steps.len());
        let mut last_output: Value = Value::Null;

        for step in &self.steps {
            let step_input = (step.input_transform)(&current_input, &last_output);

            let tool = match tool_system.find_tool(&step.tool_name) {
                Some(t) => t,
                None => {
                    let err = format!("工具 {} 不存在", step.tool_name);
                    step_results.push(ChainStepResult {
                        tool_name: step.tool_name.clone(),
                        success: false,
                        output: Value::Null,
                        error: Some(err.clone()),
                        duration_ms: 0.0,
                    });
                    if step.on_failure == FailurePolicy::Stop {
                        return ToolChainResult {
                            success: false,
                            steps: step_results,
                            final_output: Value::Null,
                            error: Some(err),
                        };
                    }
                    continue;
                }
            };

            let start = std::time::Instant::now();
            let result = tool.call(step_input.clone(), context).await;
            let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

            let step_result = ChainStepResult {
                tool_name: step.tool_name.clone(),
                success: result.success,
                output: result.data.clone().unwrap_or(Value::Null),
                error: result.error.clone(),
                duration_ms,
            };

            let success = result.success;
            last_output = result.data.unwrap_or(Value::Null);
            current_input = step_input;
            step_results.push(step_result);

            if !success {
                match step.on_failure {
                    FailurePolicy::Stop => {
                        return ToolChainResult {
                            success: false,
                            steps: step_results,
                            final_output: last_output,
                            error: Some(format!("步骤 {} 失败", step.tool_name)),
                        };
                    }
                    FailurePolicy::Skip => {
                        // 跳过本步，下一步的 current_input 不变
                        continue;
                    }
                    FailurePolicy::Continue => {
                        // 错误作为下一步输入
                        // current_input 已是 step_input，last_output 是错误数据
                    }
                }
            }
        }

        ToolChainResult {
            success: step_results.iter().all(|s| s.success),
            steps: step_results,
            final_output: last_output,
            error: None,
        }
    }
}

/// 意图类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentType {
    DesktopContext,
    FileOperation,
    SystemInfo,
    AppManagement,
    WebResearch,
    Unknown,
}

/// 意图识别器：基于正则模式匹配
pub struct IntentRecognizer {
    patterns: Vec<(IntentType, Vec<Regex>)>,
}

impl Default for IntentRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

impl IntentRecognizer {
    pub fn new() -> Self {
        let mut patterns: Vec<(IntentType, Vec<Regex>)> = Vec::new();

        let mk = |pats: &[&str]| {
            pats.iter()
                .filter_map(|p| Regex::new(p).ok())
                .collect::<Vec<_>>()
        };

        patterns.push((
            IntentType::DesktopContext,
            mk(&[
                r"(?i)桌[面上]?(?:有什么|有什么应用|在跑什么)",
                r"(?i)(?:当前|现在)窗口",
                r"(?i)屏幕(?:内容|文字|上)",
                r"(?i)前台(?:应用|窗口|是什么)",
            ]),
        ));
        patterns.push((
            IntentType::FileOperation,
            mk(&[
                r"(?i)(?:读|写|打开|编辑|删除|复制|移动|新建).{0,10}文件",
                r"(?i)file\s*(?:read|write|edit|delete|copy|move)",
                r"(?i)(?:读取|查看).{0,10}(?:\.|内容)",
            ]),
        ));
        patterns.push((
            IntentType::SystemInfo,
            mk(&[
                r"(?i)系统(?:信息|状态|资源)",
                r"(?i)(?:CPU|内存|磁盘|进程)(?:占用|使用|信息)?",
                r"(?i)system\s*info",
            ]),
        ));
        patterns.push((
            IntentType::AppManagement,
            mk(&[
                r"(?i)(?:打开|启动|关闭|结束).{0,10}(?:应用|程序|软件|app)",
                r"(?i)(?:open|launch|close|kill)\s*\w+\.exe",
            ]),
        ));
        patterns.push((
            IntentType::WebResearch,
            mk(&[
                r"(?i)(?:搜索|查一下|查查|google一下|百度)",
                r"(?i)(?:web|网络)\s*(?:search|fetch|搜索|抓取)",
            ]),
        ));

        Self { patterns }
    }

    /// 识别意图
    pub fn recognize(&self, text: &str) -> IntentType {
        for (intent, regexes) in &self.patterns {
            for re in regexes {
                if re.is_match(text) {
                    return *intent;
                }
            }
        }
        IntentType::Unknown
    }
}

/// 工具链管理器：维护意图 → 工具链的映射
pub struct ToolChainer {
    recognizer: IntentRecognizer,
    /// 意图 → 链模板（链名，步骤工具名序列）
    templates: RwLock<HashMap<IntentType, Vec<String>>>,
}

impl Default for ToolChainer {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolChainer {
    pub fn new() -> Self {
        let mut templates: HashMap<IntentType, Vec<String>> = HashMap::new();
        templates.insert(
            IntentType::DesktopContext,
            vec!["get_active_window".to_string(), "take_screenshot".to_string()],
        );
        templates.insert(
            IntentType::FileOperation,
            vec!["read_file".to_string()],
        );
        templates.insert(
            IntentType::AppManagement,
            vec!["open_application".to_string()],
        );
        templates.insert(
            IntentType::WebResearch,
            vec!["web_search".to_string()],
        );

        Self {
            recognizer: IntentRecognizer::new(),
            templates: RwLock::new(templates),
        }
    }

    /// 识别意图并返回对应的工具链模板
    pub fn plan_chain(&self, text: &str) -> (IntentType, Vec<String>) {
        let intent = self.recognizer.recognize(text);
        let steps = self
            .templates
            .read()
            .get(&intent)
            .cloned()
            .unwrap_or_default();
        (intent, steps)
    }

    /// 注册/覆盖意图 → 链模板
    pub fn register_template(&self, intent: IntentType, steps: Vec<String>) {
        self.templates.write().insert(intent, steps);
    }

    /// 按预定义链模板执行：依次调用工具，把上一步的输出作为下一步的输入
    pub async fn execute_chain(
        &self,
        text: &str,
        initial_input: Value,
        tool_system: &ToolSystem,
        context: &ToolUseContext,
    ) -> ToolChainResult {
        let (_, steps) = self.plan_chain(text);
        if steps.is_empty() {
            return ToolChainResult::failed("未识别到意图，无可执行链模板");
        }

        let mut chain = ToolChain::new("intent_chain");
        for name in steps {
            chain = chain.then(name, |prev, _last| prev.clone());
        }
        chain.execute(initial_input, tool_system, context).await
    }
}

/// 构建一个简单的直通链：每步用上一步输出作为输入
pub fn build_sequential_chain(name: impl Into<String>, tool_names: Vec<String>) -> ToolChain {
    let mut chain = ToolChain::new(name);
    for n in tool_names {
        chain = chain.then(n, |prev, _last| prev.clone());
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_recognizer_works() {
        let r = IntentRecognizer::new();
        assert_eq!(r.recognize("帮我读一下文件"), IntentType::FileOperation);
        assert_eq!(r.recognize("搜索一下天气"), IntentType::WebResearch);
        assert_eq!(r.recognize("看看系统信息"), IntentType::SystemInfo);
    }
}


// ====================================================================
// 多步循环执行器：LLM 返回 tool_calls → 逐个执行 → 收集结果 → 反馈给 LLM 再决策 → 循环
// ====================================================================

/// 多步循环终止原因
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopTerminationReason {
    /// LLM 不再返回工具调用，任务完成
    Completed,
    /// 达到最大迭代次数
    MaxIterationsReached,
    /// 检测到连续重复调用（连续 2 次相同指纹）
    RepeatDetected,
    /// LLM 生成失败（无响应）
    GenerationFailed,
}

impl Default for LoopTerminationReason {
    fn default() -> Self {
        LoopTerminationReason::MaxIterationsReached
    }
}

/// 多步循环中单次工具调用记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiStepCallRecord {
    /// 工具名
    pub tool_name: String,
    /// 调用参数
    pub arguments: Value,
    /// 是否成功
    pub success: bool,
    /// 工具返回数据
    pub output: Value,
    /// 错误信息
    pub error: Option<String>,
    /// 执行耗时（毫秒）
    pub duration_ms: f64,
    /// 是否因重复检测被跳过
    pub skipped_duplicate: bool,
}

/// 多步循环执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiStepLoopResult {
    /// 整体是否成功
    pub success: bool,
    /// 完成的 LLM 轮数
    pub iterations_used: usize,
    /// 所有工具调用记录
    pub calls: Vec<MultiStepCallRecord>,
    /// 最终输出（LLM 最后一次响应或工具结果）
    pub final_output: Value,
    /// 终止原因
    pub termination: LoopTerminationReason,
    /// LLM 最后一次原始响应
    pub last_response: Option<String>,
}

impl MultiStepLoopResult {
    pub fn failed(reason: LoopTerminationReason, message: impl Into<String>) -> Self {
        Self {
            success: false,
            iterations_used: 0,
            calls: Vec::new(),
            final_output: Value::Null,
            termination: reason,
            last_response: Some(message.into()),
        }
    }
}
/// 多步循环执行器
///
/// 通过指纹（工具名 + 参数 JSON 哈希）检测重复调用，避免 LLM 在循环中
/// 反复触发同一工具+同一参数。循环终止条件：LLM 不再返回 tool_calls、
/// 达到 max_iterations、或检测到连续 2 次相同指纹。
pub struct MultiStepExecutor {
    /// 最大迭代次数（默认 5）
    max_iterations: usize,
    /// 已执行调用的指纹（工具名 + 参数哈希），用于重复检测
    executed_calls: Vec<(String, String)>,
    /// 工具系统
    tool_system: Arc<ToolSystem>,
    /// 工具调用上下文
    context: ToolUseContext,
}

impl MultiStepExecutor {
    /// 创建执行器，默认最大迭代次数 5
    pub fn new(tool_system: Arc<ToolSystem>, context: ToolUseContext) -> Self {
        Self {
            max_iterations: 5,
            executed_calls: Vec::new(),
            tool_system,
            context,
        }
    }

    /// 设置最大迭代次数（最小为 1）
    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max.max(1);
        self
    }

    /// 已执行调用指纹列表（只读访问，便于调试/观测）
    pub fn executed_calls(&self) -> &[(String, String)] {
        &self.executed_calls
    }

    /// 计算工具调用指纹：工具名 + 参数 JSON 规范化后的哈希
    ///
    /// 规范化（递归按键名排序）确保相同内容、不同键序的参数产生相同指纹。
    fn fingerprint(tool_name: &str, arguments: &Value) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let canonical = canonicalize_json(arguments);
        let mut hasher = DefaultHasher::new();
        tool_name.hash(&mut hasher);
        canonical.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// 多步循环主流程
    ///
    /// `ai_generate` 回调接收当前提示词，返回 LLM 响应（None 视为生成失败）。
    /// 每轮：解析 tool_calls → 重复检测 → 执行 → 构建下一轮提示词。
    pub async fn execute_loop<F, Fut>(
        &mut self,
        initial_prompt: &str,
        mut ai_generate: F,
    ) -> MultiStepLoopResult
    where
        F: FnMut(String) -> Fut,
        Fut: std::future::Future<Output = Option<String>>,
    {
        let mut current_prompt = initial_prompt.to_string();
        let mut calls: Vec<MultiStepCallRecord> = Vec::new();
        let mut last_response: Option<String> = None;
        let mut last_fingerprint: Option<String> = None;
        let mut final_output: Value = Value::Null;
        let mut termination = LoopTerminationReason::MaxIterationsReached;
        let mut iterations_used = 0usize;

        'outer: for step in 0..self.max_iterations {
            // 1. 调用 LLM 生成响应
            let ai_response = match ai_generate(current_prompt.clone()).await {
                Some(resp) => resp,
                None => {
                    termination = LoopTerminationReason::GenerationFailed;
                    break 'outer;
                }
            };
            last_response = Some(ai_response.clone());

            // 2. 解析工具调用（复用 ToolCallManager 的解析逻辑）
            let tool_calls = ToolCallManager::parse_tool_calls(&ai_response);

            // LLM 不再返回工具调用 → 任务完成
            if tool_calls.is_empty() {
                termination = LoopTerminationReason::Completed;
                final_output = Value::String(ai_response);
                iterations_used = step + 1;
                break 'outer;
            }

            // 3. 逐个执行工具调用（重复检测 + 执行）
            for tc in tool_calls {
                let tool_name = tc.tool.clone();
                let arguments = tc.arguments.clone();
                let fp = Self::fingerprint(&tool_name, &arguments);

                let is_duplicate = self
                    .executed_calls
                    .iter()
                    .any(|(n, h)| *n == tool_name && *h == fp);
                let is_consecutive = last_fingerprint.as_deref() == Some(fp.as_str());

                // 连续 2 次相同指纹 → 终止循环
                if is_consecutive {
                    tracing::warn!(
                        "[MultiStepExecutor] 检测到连续重复调用: {} (终止循环)",
                        tool_name
                    );
                    termination = LoopTerminationReason::RepeatDetected;
                    break 'outer;
                }

                // 已执行过的相同调用 → 跳过并记录警告
                if is_duplicate {
                    tracing::warn!(
                        "[MultiStepExecutor] 跳过重复调用: {} (相同参数已执行)",
                        tool_name
                    );
                    calls.push(MultiStepCallRecord {
                        tool_name,
                        arguments,
                        success: true,
                        output: Value::Null,
                        error: None,
                        duration_ms: 0.0,
                        skipped_duplicate: true,
                    });
                    last_fingerprint = Some(fp);
                    continue;
                }

                // 新调用：记录指纹并执行
                self.executed_calls.push((tool_name.clone(), fp.clone()));
                last_fingerprint = Some(fp);

                let start = std::time::Instant::now();
                let result = match self.tool_system.find_tool(&tool_name) {
                    Some(tool) => tool.call(arguments.clone(), &self.context).await,
                    None => ToolResult::error(format!("工具 {} 不存在", tool_name)),
                };
                let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
                let output = result.data.clone().unwrap_or(Value::Null);

                calls.push(MultiStepCallRecord {
                    tool_name,
                    arguments,
                    success: result.success,
                    output,
                    error: result.error,
                    duration_ms,
                    skipped_duplicate: false,
                });
            }

            // 4. 本轮结束，记录轮数并构建下一轮提示词（反馈工具结果给 LLM）
            iterations_used = step + 1;
            current_prompt = build_continue_prompt(&calls, &ai_response);
        }

        let success = matches!(termination, LoopTerminationReason::Completed)
            || (!calls.is_empty() && calls.iter().any(|c| c.success));

        MultiStepLoopResult {
            success,
            iterations_used,
            calls,
            final_output,
            termination,
            last_response,
        }
    }
}
/// 构建下一轮 LLM 提示词：汇总本轮工具执行结果，引导 LLM 决定下一步
fn build_continue_prompt(calls: &[MultiStepCallRecord], ai_response: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push("## 工具执行结果".to_string());
    lines.push(String::new());
    for call in calls {
        let status = if call.skipped_duplicate {
            "SKIPPED_DUPLICATE"
        } else if call.success {
            "SUCCESS"
        } else {
            "FAILED"
        };
        lines.push(format!("### {} [{}]", call.tool_name, status));
        if call.skipped_duplicate {
            lines.push("已跳过：与之前执行过的调用完全相同。".to_string());
        } else if call.success {
            lines.push("```json".to_string());
            lines.push(serde_json::to_string_pretty(&call.output).unwrap_or_default());
            lines.push("```".to_string());
        } else if let Some(err) = &call.error {
            lines.push(format!("Error: {}", err));
        }
        lines.push(String::new());
    }
    lines.push(format!("上一轮 AI 响应摘要：{}", ai_response));
    lines.push(String::new());
    lines.push("请根据以上工具执行结果决定下一步：".to_string());
    lines.push("- 若任务已完成，请直接给出最终回复（不要再调用工具）；".to_string());
    lines.push("- 若需要继续，请输出工具调用 JSON。".to_string());
    lines.join("\n")
}

/// 规范化 JSON：递归按键名排序后序列化，保证相同内容不同键序产生相同输出
fn canonicalize_json(v: &Value) -> String {
    let mut sorted = v.clone();
    sort_object_keys(&mut sorted);
    serde_json::to_string(&sorted).unwrap_or_default()
}

/// 递归排序 JSON 对象的键
fn sort_object_keys(v: &mut Value) {
    match v {
        Value::Object(map) => {
            let mut sorted: std::collections::BTreeMap<String, Value> =
                std::collections::BTreeMap::new();
            for (k, mut val) in std::mem::take(map) {
                sort_object_keys(&mut val);
                sorted.insert(k, val);
            }
            for (k, val) in sorted {
                map.insert(k, val);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                sort_object_keys(item);
            }
        }
        _ => {}
    }
}
