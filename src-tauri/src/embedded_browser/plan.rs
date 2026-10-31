//! Bounded browser plans and session-scoped, in-memory deduplication receipts.
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Plan {
    pub request_id: String,
    pub tab_id: String,
    pub steps: Vec<Step>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Step {
    pub action: String,
    #[serde(default = "empty_args")]
    pub args: Value,
    pub locator: Option<Value>,
}
fn empty_args() -> Value {
    json!({})
}
pub(crate) fn parse(args: &Value) -> Result<Plan, String> {
    let plan: Plan = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
    if plan.request_id.is_empty()
        || plan.request_id.len() > 100
        || plan.tab_id.is_empty()
        || !(1..=16).contains(&plan.steps.len())
    {
        return Err("Plan requires request_id (1..100 bytes), tab_id and 1..16 steps".into());
    }
    for step in &plan.steps {
        if !matches!(
            step.action.as_str(),
            "click"
                | "type"
                | "press"
                | "hover"
                | "select_option"
                | "scroll"
                | "wait"
                | "expect"
                | "get_text"
                | "snapshot"
        ) || !step.args.is_object()
        {
            return Err("Unsupported plan step or non-object args".into());
        }
        if step.args.get("tab_id").is_some() {
            return Err("A plan is bound to one tab_id".into());
        }
        validate_step(step)?;
        if matches!(
            step.action.as_str(),
            "click" | "type" | "hover" | "select_option"
        ) {
            let locator = step
                .locator
                .as_ref()
                .and_then(Value::as_object)
                .ok_or("This step requires a strict locator")?;
            if locator.is_empty()
                || locator
                    .keys()
                    .any(|k| !matches!(k.as_str(), "selector" | "role" | "name" | "scope"))
                || locator
                    .values()
                    .any(|v| v.as_str().is_none_or(|s| s.is_empty() || s.len() > 1000))
                || !["selector", "role", "name"]
                    .iter()
                    .any(|key| locator.contains_key(*key))
            {
                return Err("Locator requires selector, role or exact name; optional scope limits its region".into());
            }
        }
        if step.action == "expect"
            && step.args.get("selector").is_none()
            && step.args.get("text").is_none()
        {
            return Err("expect requires a selector or text condition".into());
        }
    }
    Ok(plan)
}

fn validate_step(step: &Step) -> Result<(), String> {
    let allowed: &[&str] = match step.action.as_str() {
        "click" => &["button", "click_count", "timeout_ms"],
        "type" => &["text", "replace", "timeout_ms"],
        "press" => &["key"],
        "hover" => &["timeout_ms"],
        "select_option" => &["value"],
        "scroll" => &["direction", "amount"],
        "wait" | "expect" => &["selector", "state", "text", "timeout_ms", "ms"],
        "get_text" => &["selector", "offset"],
        "snapshot" => &["delta", "region"],
        _ => return Err("Unsupported step".into()),
    };
    for (key, value) in step.args.as_object().ok_or("args must be an object")? {
        let valid = allowed.contains(&key.as_str())
            && match key.as_str() {
                "replace" | "delta" => value.is_boolean(),
                "timeout_ms" => value.as_u64().is_some_and(|v| {
                    v <= if matches!(step.action.as_str(), "wait" | "expect") {
                        10000
                    } else {
                        5000
                    }
                }),
                "click_count" => value.as_u64().is_some_and(|v| (1..=2).contains(&v)),
                "offset" => value.as_u64().is_some(),
                "amount" | "ms" => value.as_f64().is_some_and(|v| (0.0..=10000.0).contains(&v)),
                "button" => matches!(value.as_str(), Some("left" | "right" | "middle")),
                "direction" => matches!(value.as_str(), Some("up" | "down" | "top" | "bottom")),
                "state" => matches!(
                    value.as_str(),
                    Some("attached" | "detached" | "visible" | "hidden")
                ),
                "text" => value.as_str().is_some_and(|v| v.chars().count() <= 4000),
                "value" => value.as_str().is_some_and(|v| v.chars().count() <= 4000),
                _ => value
                    .as_str()
                    .is_some_and(|v| !v.is_empty() && v.chars().count() <= 1000),
            };
        if !valid {
            return Err(format!("Invalid {} step argument: {key}", step.action));
        }
    }
    if step.action == "type"
        && (step.args["text"].as_str().is_none()
            || (step.args["text"] == "" && step.args["replace"] != true))
    {
        return Err("type requires text; empty text requires replace=true".into());
    }
    if step.action == "press" {
        super::input::key_events(step.args["key"].as_str().ok_or("press requires key")?)?;
    }
    if step.action == "select_option" && !step.args["value"].is_string() {
        return Err("select_option requires value".into());
    }
    if step.action == "scroll" && step.args.get("direction").is_none() {
        return Err("scroll requires direction".into());
    }
    if matches!(step.action.as_str(), "wait" | "expect") {
        let condition = step.args.get("selector").is_some() || step.args.get("text").is_some();
        if step.args.get("state").is_some() && !condition {
            return Err("wait state requires a condition".into());
        }
        if step.args["text"] == ""
            || (step.args.get("text").is_some()
                && matches!(step.args["state"].as_str(), Some("hidden" | "detached")))
        {
            return Err("Text conditions must be non-empty and visible or attached".into());
        }
    }
    Ok(())
}

struct Entry {
    signature: Value,
    updated: Instant,
    receipt: Value,
}
#[derive(Default)]
pub(crate) struct Store {
    entries: HashMap<(String, String), Entry>,
}
pub(crate) static STORE: once_cell::sync::Lazy<Mutex<Store>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Store::default()));
impl Store {
    fn prune(&mut self) {
        self.entries
            .retain(|_, entry| entry.updated.elapsed() < Duration::from_secs(900));
    }
    pub fn begin(
        &mut self,
        session: &str,
        plan: &Plan,
        signature: &Value,
    ) -> Result<Option<Value>, String> {
        self.prune();
        let key = (session.into(), plan.request_id.clone());
        if let Some(entry) = self.entries.get(&key) {
            if entry.signature != *signature {
                return Err("request_id already belongs to another plan; query its receipt before deciding what to do".into());
            }
            return Ok(Some(entry.receipt.clone()));
        }
        if self.entries.len() >= 64 {
            return Err(
                "Receipt capacity reached; retained requests cannot be evicted safely yet".into(),
            );
        }
        self.entries.insert(key, Entry { signature: signature.clone(), updated: Instant::now(), receipt: json!({
            "request_id":plan.request_id,"tab_id":plan.tab_id,"status":"running","steps":[],
            "goal_verified":false,"mutation_state":"none","next_step":"Query run_result with the same request_id; do not replay input"
        }) });
        Ok(None)
    }
    pub fn lookup(&mut self, session: &str, request: &str) -> Result<Value, String> {
        self.prune();
        self.entries.get(&(session.into(), request.into())).map(|entry| entry.receipt.clone())
            .ok_or("Receipt unavailable (expired, another session, or runtime restarted). Inspect the page before any retry".into())
    }
    pub fn update(
        &mut self,
        session: &str,
        request: &str,
        status: &str,
        steps: &[Value],
        mutation: bool,
        error: Option<&str>,
    ) -> Value {
        let entry = self
            .entries
            .get_mut(&(session.into(), request.into()))
            .expect("active receipt");
        entry.updated = Instant::now();
        entry.receipt["status"] = json!(status);
        entry.receipt["steps"] = json!(steps);
        entry.receipt["mutation_state"] = json!(if mutation { "possible" } else { "none" });
        if let Some(error) = error {
            entry.receipt["error"] = json!(error);
        }
        entry.receipt.clone()
    }
}
pub(crate) struct Lease {
    pub session: String,
    pub request: String,
    pub steps: Vec<Value>,
    pub mutation: bool,
    pub finished: bool,
}
impl Lease {
    pub fn update(&self, status: &str, error: Option<&str>) -> Value {
        STORE.lock().update(
            &self.session,
            &self.request,
            status,
            &self.steps,
            self.mutation,
            error,
        )
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if !self.finished {
            self.update("interrupted", Some("Execution interrupted. Inspect completed steps and the page; do not replay mutations"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plan_preflight_rejects_invalid_later_input_before_execution() {
        let make = |args: Value| json!({"request_id":"preflight","tab_id":"t","steps":[
            {"action":"click","locator":{"selector":"#save"}},
            {"action":"type","locator":{"selector":"#entry"},"args":args}
        ]});
        assert!(parse(&make(json!({"text":"中文😀","replace":true}))).is_ok());
        for args in [json!({"text":"x","replace":"true"}),json!({"text":""}),json!({"text":"x","tab_id":"other"}),json!({"text":"x","code":"arbitrary"})] {
            assert!(parse(&make(args)).is_err());
        }
    }
    #[test]
    fn receipts_deduplicate_and_reject_changed_plans_and_other_sessions() {
        let args = json!({"request_id":"save-1","tab_id":"tab","steps":[{"action":"click","locator":{"role":"button","name":"Save","scope":"#dialog"}}]});
        let plan = parse(&args).unwrap();
        let mut store = Store::default();
        assert!(store.begin("session", &plan, &args).unwrap().is_none());
        store.update(
            "session",
            "save-1",
            "failed",
            &[json!({"step":0,"status":"failed"})],
            true,
            Some("Outcome unknown"),
        );
        assert_eq!(
            store.begin("session", &plan, &args).unwrap().unwrap()["status"],
            "failed"
        );
        let mut changed = args.clone();
        changed["steps"][0]["locator"]["name"] = json!("Delete");
        assert!(store
            .begin("session", &parse(&changed).unwrap(), &changed)
            .is_err());
        assert!(store.lookup("other", "save-1").is_err());
        assert!(parse(&json!({"request_id":"x","tab_id":"t","steps":[{"action":"eval","args":{"code":"document.cookie"}}]})).is_err());
        assert!(parse(
            &json!({"request_id":"x","tab_id":"t","steps":[{"action":"click","args":{"index":1}}]})
        )
        .is_err());
    }
    #[test]
    fn dropping_an_active_plan_preserves_partial_progress_and_uncertainty() {
        let args = json!({"request_id":"cancelled","tab_id":"t","steps":[{"action":"press","args":{"key":"Enter"}}]});
        let plan = parse(&args).unwrap();
        STORE.lock().begin("cancel-test", &plan, &args).unwrap();
        {
            let lease = Lease {
                session: "cancel-test".into(),
                request: plan.request_id,
                steps: vec![json!({"step":0,"status":"succeeded"})],
                mutation: true,
                finished: false,
            };
            lease.update("running", None);
        }
        let receipt = STORE.lock().lookup("cancel-test", "cancelled").unwrap();
        assert_eq!(receipt["status"], "interrupted");
        assert_eq!(receipt["mutation_state"], "possible");
        assert_eq!(receipt["steps"].as_array().unwrap().len(), 1);
        STORE
            .lock()
            .entries
            .remove(&("cancel-test".into(), "cancelled".into()));
    }
}
