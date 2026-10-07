//! Shared task taxonomy and task-specific prompt boundaries for every router entry point.
use once_cell::sync::Lazy;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRouteSpec {
    pub id: String,
    pub group_key: String,
    pub label_key: String,
    pub help_key: String,
    pub concurrency: String,
    pub spoken: bool,
    pub fallback: Option<String>,
    pub instruction: String,
}

pub static TASK_ROUTES: Lazy<Vec<TaskRouteSpec>> = Lazy::new(|| {
    serde_json::from_str(include_str!("../../prompts/routing/task_catalog.json"))
        .expect("valid shared task route catalog")
});

pub fn find(task: &str) -> Option<&'static TaskRouteSpec> {
    TASK_ROUTES.iter().find(|spec| spec.id == task)
}

/// Logical task prompts remain unchanged when the provider falls back.
pub fn prompt(spec: &TaskRouteSpec) -> String {
    format!("[TASK CONTRACT: {}]\n{}\nThe caller's more specific output protocol and user constraints remain authoritative within this task.", spec.id, spec.instruction)
}

/// Inheritance is explicit and bounded. Unconfigured tasks ultimately use the main API.
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
    fn taxonomy_has_unique_prompts_valid_fallbacks_and_complete_defaults() {
        let mut ids = HashSet::new();
        let mut instructions = HashSet::new();
        let defaults = crate::config::manager::AppConfig::default();
        for spec in TASK_ROUTES.iter() {
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
        }
        assert_eq!(ids.len(), defaults.routing_matrix.len());
        assert_eq!(route_keys("pet_reaction"), ["pet_reaction", "companion"]);
        assert!(find("companion").unwrap().spoken);
        assert!(!find("reasoning").unwrap().spoken);
        assert!(!find("vision_describe").unwrap().spoken);
    }
}
