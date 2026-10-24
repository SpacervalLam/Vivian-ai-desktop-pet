//! Shared task taxonomy and task-specific prompt boundaries for every router entry point.
//!
//! 两层结构：`tier`（模型档位，用户实际选择的粒度）与 `task`（任务契约，代码实际
//! 使用的粒度）。一个 task 归属某个 tier；tier 只决定用哪个模型，task 决定注入哪份
//! 契约、进哪个并发组、走哪条回退链，以及它自己的采样参数。
use once_cell::sync::Lazy;
use serde::Deserialize;

/// 用户可见的模型档位。只有这里会被渲染成一排可配置槽位。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskTierSpec {
    pub id: String,
    pub label_key: String,
    pub help_key: String,
    /// 档位职责说明；仅用于设置页提示，不注入请求。
    pub instruction: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRouteSpec {
    pub id: String,
    /// 所属档位。`None` 表示非 LLM 任务（如图片生成），不参与档位解析。
    #[serde(default)]
    pub tier: Option<String>,
    /// `llm`（默认）或 `image`。非 LLM 任务由各自的专用客户端发起请求。
    #[serde(default = "default_kind")]
    pub kind: String,
    pub group_key: String,
    pub label_key: String,
    pub help_key: String,
    pub concurrency: String,
    pub spoken: bool,
    pub fallback: Option<String>,
    /// 代码定义的采样温度。调用方显式指定时以调用方为准。
    #[serde(default)]
    pub temperature: Option<f64>,
    /// 代码定义的输出上限。调用方显式指定时以调用方为准。
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// 不放进设置窗口的路由矩阵列表（专属界面配置或内部任务）。
    ///
    /// 与"内部任务"不同：hidden 的路由依然是一条完整可配置路由，只是通用设置页
    /// 不渲染它；用量统计、路由连通性测试等其余入口照常可见。
    #[serde(default)]
    pub hidden: bool,
    pub instruction: String,
}

fn default_kind() -> String {
    "llm".to_string()
}

#[derive(Debug, Deserialize)]
struct Catalog {
    tiers: Vec<TaskTierSpec>,
    tasks: Vec<TaskRouteSpec>,
}

static CATALOG: Lazy<Catalog> = Lazy::new(|| {
    serde_json::from_str(include_str!("../../prompts/routing/task_catalog.json"))
        .expect("valid shared task route catalog")
});

pub fn tiers() -> &'static [TaskTierSpec] {
    &CATALOG.tiers
}

pub fn tasks() -> &'static [TaskRouteSpec] {
    &CATALOG.tasks
}

pub fn find(task: &str) -> Option<&'static TaskRouteSpec> {
    CATALOG.tasks.iter().find(|spec| spec.id == task)
}

pub fn find_tier(tier: &str) -> Option<&'static TaskTierSpec> {
    CATALOG.tiers.iter().find(|spec| spec.id == tier)
}

/// 任务所属档位名；非 LLM 任务或未声明档位时返回 None。
pub fn tier_of(task: &str) -> Option<&'static str> {
    find(task).and_then(|spec| spec.tier.as_deref())
}

/// Logical task prompts remain unchanged when the provider falls back.
pub fn prompt(spec: &TaskRouteSpec) -> String {
    format!("[TASK CONTRACT: {}]\n{}\nThe caller's more specific output protocol and user constraints remain authoritative within this task.", spec.id, spec.instruction)
}

/// Inheritance is explicit and bounded. Unconfigured tasks ultimately use the main API.
///
/// 只沿 `fallback` 链走；档位继承由 `ModelRouter` 在任务链之后单独处理，因为档位
/// 属于"模型选择"，而 `fallback` 属于"任务契约更宽泛的邻居"。
pub fn route_keys(task: &str) -> Vec<&str> {
    let mut keys = vec![task];
    let mut current = task;
    while let Some(parent) = find(current).and_then(|spec| spec.fallback.as_deref()) {
        if keys.contains(&parent) { break; }
        keys.push(parent);
        current = parent;
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn companion_contract_does_not_turn_ordinary_speech_into_execution() {
        let contract = prompt(find("companion").unwrap());
        assert!(!contract.contains("Speak only after verified results"));
        assert!(!contract.contains("delegate_to_work_agent"));
        assert!(!contract.contains("always-visible tool_search"));
        let operation = include_str!("../../prompts/framework/companion_operations.en.md");
        assert!(operation.contains("delegate_to_work_agent"));
        assert!(operation.contains("Starting work is not finishing it"));
    }

    #[test]
    fn taxonomy_has_unique_prompts_valid_fallbacks_and_complete_defaults() {
        let mut ids = HashSet::new();
        let mut instructions = HashSet::new();
        let defaults = crate::config::manager::AppConfig::default();
        for spec in tasks().iter() {
            assert!(ids.insert(&spec.id), "duplicate task {}", spec.id);
            assert!(instructions.insert(&spec.instruction), "identical task prompt {}", spec.id);
            assert!(spec.instruction.len() > 150);
            assert!(spec.id.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
            assert!(spec.group_key.starts_with("config.routing_group_"));
            assert_eq!(spec.label_key, format!("config.routing_{}", spec.id));
            assert_eq!(spec.help_key, format!("config.routing_{}_help", spec.id));
            assert!(defaults.routing_matrix.contains_key(&spec.id));
            if let Some(parent) = &spec.fallback {
                assert!(find(parent).is_some());
                assert!(!route_keys(parent).contains(&spec.id.as_str()), "fallback cycle");
            }
            match spec.kind.as_str() {
                "llm" => {
                    let tier = spec.tier.as_deref().expect("llm tasks declare a tier");
                    assert!(find_tier(tier).is_some(), "unknown tier {tier} on {}", spec.id);
                }
                "image" => assert!(spec.tier.is_none(), "non-LLM task {} must not claim a tier", spec.id),
                other => panic!("unknown task kind {other} on {}", spec.id),
            }
        }
        assert_eq!(ids.len(), defaults.routing_matrix.len());
        assert_eq!(tiers().len(), defaults.routing_tiers.len());
        assert_eq!(route_keys("pet_reaction"), ["pet_reaction", "companion"]);
        assert!(find("companion").unwrap().spoken);
        assert!(!find("reasoning").unwrap().spoken);
        assert!(!find("vision_describe").unwrap().spoken);
        assert_eq!(route_keys("persona_evolution"), ["persona_evolution", "reasoning"]);
        assert!(!find("persona_evolution").unwrap().spoken);
        assert_eq!(tier_of("companion"), Some("dialogue"));
        assert_eq!(tier_of("persona_evolution"), Some("cognition"));
        assert_eq!(tier_of("simple_judge"), Some("judge"));
        assert_eq!(tier_of("image_gen"), None);
    }

    #[test]
    fn every_tier_is_populated_and_only_dialogue_may_speak() {
        let mut used = HashSet::new();
        for spec in tasks().iter().filter(|spec| spec.kind == "llm") {
            used.insert(spec.tier.clone().unwrap());
            if spec.spoken {
                assert_eq!(spec.tier.as_deref(), Some("dialogue"), "{} speaks outside the dialogue tier", spec.id);
            }
        }
        for tier in tiers().iter() {
            assert!(used.contains(&tier.id), "empty tier {}", tier.id);
            assert_eq!(tier.label_key, format!("config.routing_tier_{}", tier.id));
            assert_eq!(tier.help_key, format!("config.routing_tier_{}_help", tier.id));
            assert!(!tier.instruction.trim().is_empty());
        }
        // 陪伴与工作两条产品线必须各自独占档位，不允许共用。
        let work_members: Vec<&str> = tasks().iter()
            .filter(|spec| spec.tier.as_deref() == Some("work"))
            .map(|spec| spec.id.as_str())
            .collect();
        assert!(work_members.contains(&"work_agent"));
        assert!(work_members.iter().all(|id| *id == "work_agent" || *id == "work_memory"));
        assert!(!tasks().iter().any(|spec| spec.id != "work_agent"
            && spec.id != "work_memory"
            && spec.concurrency == "work"));
    }
}
