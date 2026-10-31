//! Bounded, character-local task continuity. Only tool receipts establish outcomes.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const TTL: f64 = 900.0;
const MAX_TURNS: u32 = 6;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Phase { Pending, AwaitingSelection, Ready, Running, Blocked, Completed, Cancelled }

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub source_tool: String,
    pub label: String,
    pub identifiers: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct Task {
    pub goal: String,
    pub phase: Phase,
    pub candidates: Vec<Candidate>,
    /// Indexes into candidates, ordered by their appearance in the actual reply.
    pub offered: Vec<usize>,
    pub selected: Option<usize>,
    pub last_outcomes: Vec<Value>,
    #[serde(skip_serializing)]
    updated_at: f64,
    #[serde(skip_serializing)]
    idle_turns: u32,
}

#[derive(Default)]
pub struct TaskTracker { revision: u64, task: Option<Task>, turn_attached: bool }

#[derive(Clone, Debug, Serialize)]
pub struct Continuation {
    pub task: Task,
    pub needs_clarification: bool,
    pub retry: bool,
}

/// Interpretation of the latest utterance in the complete available conversation.
/// This is neither a tool receipt nor a replacement for permission checks.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolvedTurn {
    pub action: bool,
    pub continues_task: bool,
    pub needs_clarification: bool,
    pub task: String,
    /// Semantic assignment, not a name/keyword match in the execution layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_actor: Option<String>,
}

impl ResolvedTurn {
    pub fn for_actor(mut self, channel: &str, current: &str) -> Self {
        if channel == "broadcast" {
            match self.execution_actor.as_deref() {
                Some(actor) if actor != current => {
                    self.action = false;
                    self.continues_task = false;
                    self.needs_clarification = false;
                }
                None if self.action || self.continues_task => {
                    self.action = false;
                    self.continues_task = false;
                    self.needs_clarification = true;
                }
                _ => {}
            }
        }
        self.sanitized()
    }

    /// 解释器是模型，字段之间会互相矛盾——最常见的是 `action=true` 配
    /// `needs_clarification=true`：模型想表达"这是操作请求，但目标还不明确"，
    /// 这是它最自然的说法，却违反 `RESOLVE_SYSTEM` 里"模糊时 action=false"的约定。
    ///
    /// 下游只认两种形状：一条**自足**的执行目标，或一次澄清。其余组合一律折进澄清，
    /// 而不是让整轮失败。代价不对称——误澄清只多问一句，误执行是真的动了用户的机器。
    ///
    /// 判为不执行时清空 `task`：这个 JSON 会原样注入生成侧 system 段，留着目标字符串
    /// 只会诱导模型对着一个并未获授权的目标发工具调用。
    pub fn sanitized(mut self) -> Self {
        self.needs_clarification = self.needs_clarification || (self.action && self.task.trim().is_empty());
        self.action = self.action && !self.needs_clarification;
        if !self.action {
            self.task.clear();
        }
        self
    }
}

pub const RESOLVE_SYSTEM: &str = include_str!("../../prompts/execution/intent-resolve-system.en.md");

pub const ASSIGNMENT_SYSTEM: &str = include_str!("../../prompts/execution/intent-assignment.en.md");

fn normalized(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn compact(text: &str, limit: usize) -> String { text.chars().take(limit).collect() }

fn running(receipt: &Value) -> bool {
    matches!(receipt["status"].as_str(), Some("pending" | "non_blocking"))
}

fn completed(receipt: &Value) -> bool {
    receipt["success"] == true && receipt["goal_completed"] == true && !running(receipt)
}

fn ordinal(input: &str) -> Option<usize> {
    let input = input.trim_start_matches("就").trim_start_matches("用").trim_start_matches("选");
    let rest = input.strip_prefix('第')?;
    let number: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let suffix_ok = |suffix: &str| ["", "个", "张", "项", "份", "个吧", "张吧", "项吧", "个就好", "张就好", "个就行", "张就行"].contains(&suffix);
    if !number.is_empty() {
        return suffix_ok(&rest[number.len()..]).then(|| number.parse::<usize>().ok()?.checked_sub(1)).flatten();
    }
    let values = ["一", "二", "三", "四", "五", "六", "七", "八", "九", "十"];
    values.iter().position(|value| rest.strip_prefix(*value).is_some_and(suffix_ok))
}

fn cancellation(input: &str) -> bool {
    ["算了", "取消", "先不换", "不要换", "别换", "不用了", "不弄了", "cancel", "never mind", "やめて"]
        .iter().any(|word| input.to_lowercase().starts_with(word))
}

impl TaskTracker {
    /// Whole-request settlement overrides individual tools' step-completion markers.
    pub(crate) fn observe_settlement(&mut self, revision: u64, outcome: &Value) {
        if revision != self.revision { return; }
        let Some(task) = self.task.as_mut() else { return; };
        task.phase = match outcome["task"]["settlement"]["status"].as_str() {
            Some("completed") => Phase::Completed,
            Some("waiting") => Phase::Running,
            Some("blocked") => Phase::Blocked,
            Some("partial") => Phase::Ready,
            _ => return,
        };
    }

    /// Every user turn owns a revision; late results cannot overwrite a newer turn.
    pub fn begin(&mut self, input: &str, new_request: bool, empty_history: bool, now: f64) -> (u64, Option<Continuation>) {
        self.begin_resolved(input, new_request, empty_history, now, None)
    }

    pub fn snapshot(&self, now: f64) -> Option<Value> {
        self.task.as_ref().filter(|task| now - task.updated_at <= TTL && task.idle_turns < MAX_TURNS)
            .and_then(|task| serde_json::to_value(task).ok())
    }

    pub fn begin_resolved(&mut self, input: &str, new_request: bool, empty_history: bool, now: f64,
        resolved: Option<&ResolvedTurn>) -> (u64, Option<Continuation>) {
        self.revision += 1;
        self.turn_attached = false;
        if empty_history || self.task.as_ref().is_some_and(|task| now - task.updated_at > TTL || task.idle_turns >= MAX_TURNS) {
            self.task = None;
        }
        let input = input.trim().trim_end_matches(['。', '！', '!', '.', '？', '?']);
        if cancellation(input) && resolved.is_none_or(|turn| turn.continues_task) {
            if let Some(task) = &mut self.task { task.phase = Phase::Cancelled; task.updated_at = now; }
            self.turn_attached = true;
            return (self.revision, self.task.clone().map(|task| Continuation { task, needs_clarification: false, retry: false }));
        }
        let retry = ["再试一次", "再试试", "重试", "retry", "try again", "もう一度"].contains(&input);
        let resume = ["继续执行", "继续刚才的任务", "开始吧", "执行吧", "go ahead", "continue the task"].contains(&input);
        let choice = ordinal(input);
        let deictic = ["就这张", "就那张", "就这个", "就那个", "这张吧", "那张吧", "这个吧", "那个吧", "用这张", "用那张", "that one", "this one", "それで"].contains(&input);
        let selection = input.trim_start_matches("那就").trim_start_matches("就").trim_start_matches("现在").trim_start_matches("请");
        let direct_selection = ["换那", "换这", "用那", "用这", "选那", "选这", "设置为这", "设置为那", "设为这", "设为那"]
            .iter().any(|prefix| selection.starts_with(prefix));
        let reference = resolved.map(|turn| turn.continues_task)
            .unwrap_or(choice.is_some() || deictic || retry || resume || direct_selection);
        if reference && resolved.is_some_and(|turn| !turn.action && !turn.needs_clarification) {
            self.turn_attached = true;
            return (self.revision, self.task.clone().map(|task| Continuation { task, needs_clarification: false, retry: false }));
        }
        let mut context = None;
        if let Some(task) = &mut self.task {
            if !matches!(task.phase, Phase::Completed | Phase::Cancelled | Phase::Running) && reference {
                let named: Vec<usize> = task.candidates.iter().enumerate()
                    .filter(|(_, candidate)| normalized(input).contains(&normalized(&candidate.label)))
                    .map(|(index, _)| index).collect();
                let selected = if let Some(index) = choice { task.offered.get(index).copied() }
                    else if named.len() == 1 { named.first().copied() }
                    else if retry || resume { task.selected }
                    else if deictic && task.selected.is_some() { task.selected }
                    else if deictic && task.offered.len() == 1 { task.offered.first().copied() }
                    else { None };
                // A bare choice after an unrelated exchange must not revive the old list.
                let stale_reference = resolved.is_none() && task.idle_turns > 0 && (choice.is_some() || deictic || retry);
                let duplicate_label = selected.is_some_and(|index| task.candidates.iter()
                    .filter(|candidate| normalized(&candidate.label) == normalized(&task.candidates[index].label)).count() > 1);
                let needs_clarification = resolved.is_some_and(|turn| turn.needs_clarification)
                    || stale_reference || duplicate_label || named.len() > 1
                    || (resume && !task.candidates.is_empty() && selected.is_none())
                    || (!retry && selected.is_none() && (choice.is_some() || deictic));
                let selected = if needs_clarification { None } else { selected };
                task.selected = selected;
                task.phase = if needs_clarification { Phase::AwaitingSelection } else { Phase::Ready };
                task.updated_at = now;
                task.idle_turns = 0;
                context = Some(Continuation { task: task.clone(), needs_clarification, retry });
            } else if reference && matches!(task.phase, Phase::Completed | Phase::Cancelled | Phase::Running) {
                context = Some(Continuation { task: task.clone(), needs_clarification: true, retry: false });
            } else { task.idle_turns += 1; }
        }
        if context.is_none() && new_request {
            let goal = resolved.map(|turn| turn.task.as_str()).filter(|goal| !goal.trim().is_empty()).unwrap_or(input);
            self.task = Some(Task { goal: compact(goal, 2000), phase: Phase::Pending, candidates: vec![], offered: vec![],
                selected: None, last_outcomes: vec![], updated_at: now, idle_turns: 0 });
        }
        self.turn_attached = context.is_some() || new_request;
        (self.revision, context)
    }

    pub fn observe(&mut self, revision: u64, input: &str, is_request: bool, reply: &str, receipts: &[Value], now: f64) {
        if revision != self.revision { return; }
        if !self.turn_attached && !is_request { return; }
        if (self.task.is_none() || !self.turn_attached) && is_request {
            self.task = Some(Task { goal: compact(input, 2000), phase: Phase::Pending, candidates: vec![], offered: vec![],
                selected: None, last_outcomes: vec![], updated_at: now, idle_turns: 0 });
        }
        let Some(task) = self.task.as_mut().filter(|task| !matches!(task.phase, Phase::Completed | Phase::Cancelled)) else { return; };
        if receipts.is_empty() { return; } // A promise is never an execution outcome.
        task.updated_at = now;
        task.last_outcomes = receipts.iter().rev().take(4).map(|receipt| json!({
            "tool": receipt["tool_name"], "success": receipt["success"], "status": receipt["status"],
            "goal_completed": receipt["goal_completed"], "error": receipt.get("error").and_then(Value::as_str).map(|s| compact(s, 500)),
        })).collect();
        for receipt in receipts {
            if receipt["success"] != true { task.phase = Phase::Blocked; continue; }
            if running(receipt) { task.phase = Phase::Running; continue; }
            if completed(receipt) { task.phase = Phase::Completed; continue; }
            let result = &receipt["result"];
            let data = result.get("data").unwrap_or(result);
            let list = ["wallpapers", "items", "results", "files", "options", "candidates"].iter()
                .find_map(|key| data.get(key).and_then(Value::as_array));
            let Some(list) = list else { continue; };
            let candidates: Vec<_> = list.iter().take(64).filter_map(|item| {
                let label = item.get("title").or_else(|| item.get("name")).and_then(Value::as_str)?;
                if normalized(label).is_empty() { return None; }
                let identifiers: serde_json::Map<String, Value> = ["workshop_id", "id", "path", "folder_path", "url"]
                    .into_iter().filter_map(|key| item.get(key).filter(|v| v.is_string() || v.is_number())
                        .filter(|v| v.to_string().len() <= 1024).map(|v| (key.to_string(), v.clone()))).take(1).collect();
                if identifiers.is_empty() { return None; }
                Some(Candidate { source_tool: receipt["tool_name"].as_str().unwrap_or_default().to_string(),
                    label: compact(label, 300), identifiers: Value::Object(identifiers) })
            }).collect();
            if !candidates.is_empty() {
                let reply = normalized(reply);
                let mut offered: Vec<_> = candidates.iter().enumerate().filter_map(|(index, candidate)| {
                    let label = normalized(&candidate.label);
                    reply.find(&label).map(|position| (position, position + label.len(), index))
                }).collect();
                // Prefer the full title over another candidate whose title is a substring.
                offered.sort_by_key(|(start, end, _)| (*start, std::cmp::Reverse(end - start)));
                let mut end_of_title = 0;
                task.candidates = candidates;
                task.offered = offered.into_iter().filter_map(|(start, end, index)| {
                    if start < end_of_title { return None; }
                    end_of_title = end;
                    Some(index)
                }).collect();
                task.selected = None;
                task.phase = Phase::AwaitingSelection;
            }
        }
        // A successful action wins over preparatory lists in the same batch.
        if receipts.iter().any(completed) { task.phase = Phase::Completed; }
    }
}

pub fn prompt(context: &Value) -> String {
    let mut focused = context.clone();
    let selected = context["task"]["selected"].as_u64()
        .and_then(|index| context["task"]["candidates"].as_array()?.get(index as usize)).cloned();
    if let Some(selected) = selected {
        if let Some(task) = focused["task"].as_object_mut() {
            task.insert("selected_candidate".into(), selected);
            task.remove("candidates"); task.remove("offered"); task.remove("selected");
        }
    }
    format!(include_str!("../../prompts/execution/task-continuity.en.md"), focused)
}

pub fn reply_only(context: &Value) -> bool {
    context["needs_clarification"] == true
        || matches!(context["task"]["phase"].as_str(), Some("completed" | "cancelled" | "running"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn whole_request_settlement_overrides_step_completion_and_rejects_stale_revisions() {
        let mut tracker = TaskTracker::default();
        let (revision, _) = tracker.begin("查找并应用壁纸", true, true, 100.0);
        tracker.observe(revision, "查找并应用壁纸", true, "", &[json!({
            "tool_name":"wallpaper_list","success":true,"goal_completed":true,"status":"success"
        })], 101.0);
        tracker.observe_settlement(revision, &json!({"task":{"settlement":{"status":"partial"}}}));
        assert_eq!(tracker.task.as_ref().unwrap().phase, Phase::Ready);
        tracker.observe_settlement(revision, &json!({"task":{"settlement":{"status":"waiting"}}}));
        assert_eq!(tracker.task.as_ref().unwrap().phase, Phase::Running);
        tracker.observe_settlement(revision + 1, &json!({"task":{"settlement":{"status":"completed"}}}));
        assert_eq!(tracker.task.as_ref().unwrap().phase, Phase::Running);
    }

    #[test]
    fn identical_utterances_follow_resolved_context_not_a_phrase_whitelist() {
        for goal in ["从已安装的壁纸里选一张并更换", "从歌单挑一首播放", "把已有文档中的一份导出为 PDF"] {
            let mut tracker = TaskTracker::default();
            tracker.begin(goal, true, true, 100.0);
            let resolved = ResolvedTurn { action: true, continues_task: true, needs_clarification: false, task: goal.into(), execution_actor: None };
            let continuation = tracker.begin_resolved("那你挑", true, false, 101.0, Some(&resolved)).1.unwrap();
            assert_eq!(continuation.task.goal, goal);
            assert!(continuation.task.selected.is_none());
            assert!(continuation.task.last_outcomes.is_empty());
        }
        let mut tracker = pending();
        let chat = ResolvedTurn { action: false, continues_task: false, needs_clarification: false, task: String::new(), execution_actor: None };
        assert!(tracker.begin_resolved("那你挑", false, false, 102.0, Some(&chat)).1.is_none());
        assert_eq!(tracker.task.as_ref().unwrap().phase, Phase::AwaitingSelection);
        assert!(pending().begin("那你挑", false, false, 102.0).1.is_none(), "the phrase itself must not authorize anything");
        let new_task = ResolvedTurn { action: true, continues_task: false, needs_clarification: false, task: "播放一首歌".into(), execution_actor: None };
        tracker.begin_resolved("那你挑", true, false, 103.0, Some(&new_task));
        assert_eq!(tracker.task.as_ref().unwrap().goal, "播放一首歌");
        assert!(tracker.task.as_ref().unwrap().candidates.is_empty());
    }
    use super::*;
    #[test]
    fn broadcast_execution_obeys_semantic_assignment_even_when_action_is_true() {
        let resolved: ResolvedTurn = serde_json::from_value(json!({
            "action":true,"continues_task":true,"needs_clarification":false,
            "task":"更换指定壁纸","execution_actor":"vivian"})).unwrap();
        assert!(resolved.clone().for_actor("broadcast", "vivian").action);
        let listener = resolved.for_actor("broadcast", "nana");
        assert!(!listener.action);
        assert!(!listener.continues_task);
        assert!(!listener.needs_clarification, "a listener must not ask to take over");
        assert!(listener.task.is_empty());
        assert_eq!(listener.execution_actor.as_deref(), Some("vivian"));
        let mut tracker = pending();
        assert!(tracker.begin_resolved("同一句广播", listener.action, false, 102.0, Some(&listener)).1.is_none());
    }

    #[test]
    fn unresolved_broadcast_assignment_cannot_authorize_execution() {
        let resolved: ResolvedTurn = serde_json::from_value(json!({
            "action":true,"continues_task":true,"needs_clarification":false,
            "task":"更换壁纸"})).unwrap();
        let broadcast = resolved.clone().for_actor("broadcast", "nana");
        assert!(!broadcast.action && !broadcast.continues_task && broadcast.needs_clarification);
        assert!(broadcast.task.is_empty());
        assert!(resolved.for_actor("direct", "nana").action, "existing direct callers remain compatible");
    }
    /// 模型实际返回过 `action=true` + `needs_clarification=true`（LLM-IO 日志反推出的
    /// 82B / 75B 两种排版），旧实现把这种矛盾当致命错误，整轮对话没有任何回复。
    #[test]
    fn contradictory_model_fields_fold_into_clarification_instead_of_failing() {
        let ambiguous: ResolvedTurn = serde_json::from_value(json!({
            "action": true, "continues_task": false, "needs_clarification": true, "task": ""})).unwrap();
        let normalized = ambiguous.sanitized();
        assert!(!normalized.action, "asking and executing cannot be the same turn");
        assert!(normalized.needs_clarification);
        assert!(normalized.task.is_empty());
        assert!(!normalized.continues_task, "interpreting ambiguity must not revive a host task");

        // 想执行但给不出自足目标，同样只能问一句。
        let empty_goal: ResolvedTurn = serde_json::from_value(json!({
            "action": true, "continues_task": false, "needs_clarification": false, "task": "   "})).unwrap();
        let empty_goal = empty_goal.sanitized();
        assert!(!empty_goal.action);
        assert!(empty_goal.needs_clarification);

        // 矛盾且带目标：误执行的代价高于误澄清，目标一并丢弃，避免注入生成侧后诱导工具调用。
        let both: ResolvedTurn = serde_json::from_value(json!({
            "action": true, "continues_task": false, "needs_clarification": true, "task": "换一张壁纸"})).unwrap();
        let both = both.sanitized();
        assert!(!both.action && both.needs_clarification);
        assert!(both.task.is_empty());

        // 契约内输出一律不被改写。
        for (action, clarification, task) in [(true, false, "从已安装的壁纸里换一张"), (false, true, ""), (false, false, "")] {
            let turn: ResolvedTurn = serde_json::from_value(json!({"action": action,
                "continues_task": false, "needs_clarification": clarification, "task": task})).unwrap();
            let turn = turn.sanitized();
            assert_eq!((turn.action, turn.needs_clarification), (action, clarification));
            assert_eq!(turn.task, task);
        }
    }
    fn list() -> Value { json!({"tool_name":"wallpaper_list","success":true,"goal_completed":false,
        "result":{"data":{"wallpapers":[
            {"title":"窗旁の伊蕾娜","workshop_id":"elaina","folder_path":"do-not-repeat-path"},
            {"title":"Blue Sky","workshop_id":"sky"},
            {"title":"Chill Time","workshop_id":"chill"}
        ]}}}) }
    fn pending() -> TaskTracker {
        let mut tracker = TaskTracker::default();
        let (revision, _) = tracker.begin("帮我换一张壁纸", true, true, 100.0);
        tracker.observe(revision, "帮我换一张壁纸", true, "可以选 Blue Sky 或窗旁の伊蕾娜。", &[list()], 101.0);
        tracker
    }
    #[test]
    fn choice_follows_display_order_and_preserves_real_identifier() {
        let mut tracker = pending();
        let (_, context) = tracker.begin("第二个吧", false, false, 102.0);
        let context = context.unwrap();
        assert!(!context.needs_clarification);
        assert_eq!(context.task.phase, Phase::Ready);
        let selected = &context.task.candidates[context.task.selected.unwrap()];
        assert_eq!(selected.identifiers, json!({"workshop_id":"elaina"}));
        assert_eq!(context.task.goal, "帮我换一张壁纸");
        assert_eq!(context.task.last_outcomes[0]["tool"], "wallpaper_list");
        let focused = prompt(&json!(context));
        assert!(focused.contains("elaina"));
        assert!(!focused.contains("Blue Sky"), "resolved tasks avoid flooding the model with the unselected list");
    }
    #[test]
    fn deictic_and_out_of_range_choices_require_clarification() {
        for input in ["就这张", "就这个", "第三个吧", "第99个"] {
            let (_, context) = pending().begin(input, false, false, 102.0);
            let context = context.unwrap();
            assert!(context.needs_clarification, "{input}");
            assert!(context.task.selected.is_none());
        }
        let mut tracker = pending();
        tracker.task.as_mut().unwrap().offered = vec![1];
        let (_, context) = tracker.begin("就这张", false, false, 102.0);
        assert_eq!(context.unwrap().task.selected, Some(1));
    }
    #[test]
    fn mentions_and_explanations_do_not_become_selections() {
        for input in ["这个天气真好", "第二个是什么意思", "那张很好看", "你没换啊", "好的"] {
            assert!(pending().begin(input, false, false, 102.0).1.is_none(), "{input}");
        }
        let mut tracker = pending();
        tracker.begin("这个天气真好", false, false, 102.0);
        assert!(tracker.begin("第二个吧", false, false, 103.0).1.unwrap().needs_clarification);
    }
    #[test]
    fn cancelled_or_completed_tasks_do_not_reexecute() {
        let mut tracker = pending();
        tracker.begin("不用了", false, false, 102.0);
        let (_, context) = tracker.begin("第二个吧", false, false, 103.0);
        let context = context.unwrap();
        assert_eq!(context.task.phase, Phase::Cancelled);
        assert!(context.needs_clarification);
        let mut tracker = pending();
        let (revision, _) = tracker.begin("第二个吧", false, false, 102.0);
        tracker.observe(revision, "第二个吧", true, "完成了", &[json!({"tool_name":"wallpaper_set","success":true,"goal_completed":true})], 103.0);
        let (_, context) = tracker.begin("再试一次", false, false, 104.0);
        assert_eq!(context.unwrap().task.phase, Phase::Completed);
    }
    #[test]
    fn failure_and_explicit_retry_keep_the_selection_but_not_success() {
        let mut tracker = pending();
        let (revision, _) = tracker.begin("第二个吧", false, false, 102.0);
        tracker.observe(revision, "第二个吧", true, "好了", &[json!({"tool_name":"wallpaper_set","success":false,
            "goal_completed":false,"error":"Wallpaper engine unavailable"})], 103.0);
        assert_eq!(tracker.task.as_ref().unwrap().phase, Phase::Blocked);
        let (_, context) = tracker.begin("再试一次", false, false, 104.0);
        let context = context.unwrap();
        assert!(context.retry);
        assert_eq!(context.task.selected, Some(0));
        assert_eq!(context.task.last_outcomes[0]["success"], false);
    }
    #[test]
    fn a_promise_has_no_completion_authority() {
        let mut tracker = pending();
        let (revision, _) = tracker.begin("第二个吧", false, false, 102.0);
        tracker.observe(revision, "第二个吧", true, "已经换好了", &[], 103.0);
        assert_eq!(tracker.task.as_ref().unwrap().phase, Phase::Ready);
    }
    #[test]
    fn new_task_and_revision_isolate_late_results() {
        let mut tracker = pending();
        let (old, _) = tracker.begin("第二个吧", false, false, 102.0);
        tracker.begin("帮我查询天气", true, false, 103.0);
        tracker.observe(old, "第二个吧", true, "好了", &[list()], 104.0);
        let task = tracker.task.as_ref().unwrap();
        assert_eq!(task.goal, "帮我查询天气");
        assert!(task.candidates.is_empty());
        let (revision, _) = tracker.begin("今日ニュースをまとめて", false, false, 105.0);
        tracker.observe(revision, "今日ニュースをまとめて", true, "調べます", &[], 106.0);
        assert_eq!(tracker.task.as_ref().unwrap().goal, "今日ニュースをまとめて", "intent discovered in the pipeline replaces the old task");
        let (_, context) = tracker.begin("帮我分析这个文件", true, false, 107.0);
        assert!(context.is_none(), "a new request containing a pronoun is not automatically a selection");
        assert_eq!(tracker.task.as_ref().unwrap().goal, "帮我分析这个文件");
    }
    #[test]
    fn history_clear_expiration_and_idle_turns_drop_old_choices() {
        assert!(pending().begin("第二个吧", false, true, 102.0).1.is_none());
        assert!(pending().begin("第二个吧", false, false, 1002.0).1.is_none());
        let mut tracker = pending();
        for turn in 0..6 { tracker.begin("聊点别的", false, false, 102.0 + f64::from(turn)); }
        assert!(tracker.begin("第二个吧", false, false, 110.0).1.is_none());
    }
    #[test]
    fn offered_choices_come_only_from_successful_receipts_and_actual_reply() {
        let mut tracker = pending();
        let (revision, _) = tracker.begin("换一批", true, false, 102.0);
        tracker.observe(revision, "换一批", true, "我推荐一个不存在的选项", &[list()], 103.0);
        let (_, context) = tracker.begin("第二个吧", false, false, 104.0);
        assert!(context.unwrap().needs_clarification);
        let mut tracker = TaskTracker::default();
        let (revision, _) = tracker.begin("换壁纸", true, true, 100.0);
        let mut failed = list(); failed["success"] = json!(false);
        tracker.observe(revision, "换壁纸", true, "Blue Sky", &[failed], 101.0);
        assert!(tracker.task.as_ref().unwrap().candidates.is_empty());
    }
    #[test]
    fn duplicate_names_do_not_resolve_an_ambiguous_reference() {
        let mut tracker = pending();
        let duplicate = tracker.task.as_ref().unwrap().candidates[0].clone();
        tracker.task.as_mut().unwrap().candidates.push(duplicate);
        let (_, context) = tracker.begin("换那张窗旁の伊蕾娜的吧", true, false, 102.0);
        assert!(context.unwrap().needs_clarification);
    }
    #[test]
    fn explicit_resume_keeps_the_original_goal_without_inventing_a_selection() {
        let mut tracker = TaskTracker::default();
        tracker.begin("帮我整理文件", true, true, 100.0);
        let context = tracker.begin("继续执行", false, false, 101.0).1.unwrap();
        assert_eq!(context.task.goal, "帮我整理文件");
        assert!(!context.needs_clarification);
        assert!(pending().begin("开始吧", false, false, 102.0).1.unwrap().needs_clarification);
    }
    #[test]
    fn a_title_substring_is_not_an_extra_offered_option() {
        let mut tracker = TaskTracker::default();
        let (revision, _) = tracker.begin("换壁纸", true, true, 100.0);
        tracker.observe(revision, "换壁纸", true, "Chill Time 或 Blue Sky", &[
            json!({"tool_name":"wallpaper_list","success":true,"result":{"data":{"wallpapers":[
                {"title":"Chill","id":"short"}, {"title":"Chill Time","id":"full"}, {"title":"Blue Sky","id":"sky"}
            ]}}})
        ], 101.0);
        let context = tracker.begin("第二个吧", false, false, 102.0).1.unwrap();
        assert_eq!(context.task.candidates[context.task.selected.unwrap()].identifiers, json!({"id":"sky"}));
    }
    #[test]
    fn unrelated_tool_receipts_cannot_overwrite_the_pending_task() {
        let mut tracker = pending();
        let (revision, _) = tracker.begin("天气真好", false, false, 102.0);
        tracker.observe(revision, "天气真好", false, "晴天", &[
            json!({"tool_name":"get_weather","success":true,"goal_completed":true})
        ], 103.0);
        assert_eq!(tracker.task.as_ref().unwrap().phase, Phase::AwaitingSelection);
        assert_eq!(tracker.task.as_ref().unwrap().last_outcomes[0]["tool"], "wallpaper_list");
    }
    #[test]
    fn background_start_is_running_not_completed_and_cannot_be_started_twice() {
        let mut tracker = TaskTracker::default();
        let (revision, _) = tracker.begin("帮我生成文档", true, true, 100.0);
        tracker.observe(revision, "帮我生成文档", true, "正在处理", &[
            json!({"tool_name":"delegate_to_work_agent","success":true,"goal_completed":true,"status":"non_blocking"})
        ], 101.0);
        assert_eq!(tracker.task.as_ref().unwrap().phase, Phase::Running);
        let context = tracker.begin("再试一次", false, false, 102.0).1.unwrap();
        assert!(reply_only(&json!(context)));
    }
}
