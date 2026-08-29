use serde_json::{json, Value};

pub fn detect_format(model: &str) -> &'static str {
    if model.contains("o1") || model.contains("o3") || model.contains("o4") {
        "responses"
    } else {
        "chat"
    }
}

pub fn build_input(prompt: &str, format: &str) -> Value {
    match format {
        "responses" => json!({ "input": prompt }),
        _ => json!([{ "role": "user", "content": prompt }]),
    }
}

pub struct SmartRequestBuilder;

impl SmartRequestBuilder {
    pub fn detect_format(model: &str) -> &'static str {
        detect_format(model)
    }

    pub fn build_input(prompt: &str, format: &str) -> Value {
        build_input(prompt, format)
    }
}
