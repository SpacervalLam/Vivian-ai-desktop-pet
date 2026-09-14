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
use super::types::ToolUseContext;

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
