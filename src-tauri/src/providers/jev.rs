//! TypeSafe Jev decision API. This endpoint returns typed choices, not chat text.
use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::manager::TaskRouteConfig;
use crate::error::{VivianError, VivianResult};

/// The System One response shape from TypeSafe's OpenAPI specification.
/// The discriminator keeps `choice`, `noul`, and `score` answers distinct.
#[derive(Debug, Deserialize)]
struct SystemOneResponse {
    model: String,
    answers: HashMap<String, JevAnswer>,
    usage: JevUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum JevAnswer {
    Choice {
        choice: String,
        confidence: f64,
        probabilities: HashMap<String, f64>,
    },
    Noul { noul: f64 },
    Score {
        score: f64,
        confidence: f64,
        legend: HashMap<String, Value>,
        probabilities: HashMap<String, f64>,
    },
}

#[derive(Debug, Deserialize)]
struct JevUsage {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, PartialEq)]
pub struct JevChoice {
    pub choice: String,
    pub confidence: f64,
    pub probabilities: HashMap<String, f64>,
}

fn parse_choice_response(body: Value, choices: &[(&str, &str)]) -> VivianResult<(JevChoice, String, JevUsage)> {
    let mut response: SystemOneResponse = serde_json::from_value(body)
        .map_err(|e| VivianError::Provider(format!("Jev response schema mismatch: {e}")))?;
    if response.model.trim().is_empty() {
        return Err(VivianError::Provider("Jev response has no model".into()));
    }
    let answer = response.answers.remove("decision")
        .ok_or_else(|| VivianError::Provider("Jev response has no decision answer".into()))?;
    let JevAnswer::Choice { choice, confidence, probabilities } = answer else {
        return Err(VivianError::Provider("Jev returned a non-choice answer".into()));
    };
    if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
        return Err(VivianError::Provider("Jev choice confidence is out of range".into()));
    }
    if probabilities.len() != choices.len()
        || choices.iter().any(|(name, _)| !probabilities.contains_key(*name))
        || probabilities.values().any(|p| !p.is_finite() || !(0.0..=1.0).contains(p))
    {
        return Err(VivianError::Provider("Jev choice probabilities do not match criteria".into()));
    }
    if !choices.iter().any(|(name, _)| *name == choice) {
        return Err(VivianError::Provider(format!("Jev returned unknown choice: {choice}")));
    }
    Ok((JevChoice { choice, confidence, probabilities }, response.model, response.usage))
}

#[derive(Clone)]
pub struct JevClient {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: String,
}

impl JevClient {
    pub fn new(route: &TaskRouteConfig) -> VivianResult<Self> {
        let endpoint = route.endpoint.trim().trim_end_matches('/');
        if !endpoint.starts_with("https://") {
            return Err(VivianError::Provider("Jev endpoint must use HTTPS".into()));
        }
        let endpoint = if endpoint.ends_with("/v1/systemone") {
            endpoint.to_owned()
        } else if endpoint.ends_with("/v1") {
            format!("{endpoint}/systemone")
        } else {
            format!("{endpoint}/v1/systemone")
        };
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| VivianError::Provider(e.to_string()))?;
        Ok(Self {
            client,
            endpoint,
            model: route.model.clone(),
            api_key: route.api_key.clone(),
        })
    }

    pub async fn choose(
        &self,
        state: Value,
        instructions: &str,
        choices: &[(&str, &str)],
    ) -> VivianResult<JevChoice> {
        if choices.len() < 2 {
            return Err(VivianError::Provider("Jev choice requires at least two options".into()));
        }
        let criteria: serde_json::Map<String, Value> = choices.iter()
            .map(|(name, meaning)| ((*name).to_owned(), json!(meaning)))
            .collect();
        let request = json!({
            "model": self.model,
            "state": state,
            "questions": {
                "decision": {
                    "type": "choice",
                    "instructions": instructions,
                    "criteria": criteria,
                }
            }
        });
        let response = self.client.post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send().await
            .map_err(|e| VivianError::Provider(format!("Jev request failed: {e}")))?;
        let status = response.status();
        let body: Value = response.json().await
            .map_err(|e| VivianError::Provider(format!("Jev response invalid: {e}")))?;
        if !status.is_success() {
            return Err(VivianError::Provider(format!("Jev HTTP {status}: {}", body.get("detail").unwrap_or(&body))));
        }
        let (decision, model, usage) = parse_choice_response(body, choices)?;
        crate::providers::usage_store::record_usage(
            &model, usage.input_tokens, usage.output_tokens, 0, 0,
        );
        Ok(decision)
    }

    /// Evaluate independent yes/no questions in one System One request.
    /// Each returned `noul` is the probability that its criterion is true.
    pub async fn noul_batch(
        &self,
        state: Value,
        questions: &[(&str, &str, &str, &str)],
    ) -> VivianResult<HashMap<String, f64>> {
        if questions.is_empty() {
            return Ok(HashMap::new());
        }
        let mut body_questions = serde_json::Map::new();
        for &(key, instructions, yes, no) in questions {
            if key.is_empty() || body_questions.contains_key(key) {
                return Err(VivianError::Provider("Jev question keys must be nonempty and unique".into()));
            }
            body_questions.insert(key.to_owned(), json!({
                "type": "noul", "instructions": instructions,
                "criteria": {"true": yes, "false": no}
            }));
        }
        let request = json!({
            "model": self.model, "state": state, "questions": body_questions
        });
        let response = self.client.post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send().await
            .map_err(|e| VivianError::Provider(format!("Jev request failed: {e}")))?;
        let status = response.status();
        let body: Value = response.json().await
            .map_err(|e| VivianError::Provider(format!("Jev response invalid: {e}")))?;
        if !status.is_success() {
            return Err(VivianError::Provider(format!("Jev HTTP {status}: {}", body.get("detail").unwrap_or(&body))));
        }
        let mut parsed: SystemOneResponse = serde_json::from_value(body)
            .map_err(|e| VivianError::Provider(format!("Jev response schema mismatch: {e}")))?;
        if parsed.model.trim().is_empty() {
            return Err(VivianError::Provider("Jev response has no model".into()));
        }
        let mut answers = HashMap::new();
        for &(key, _, _, _) in questions {
            let answer = parsed.answers.remove(key)
                .ok_or_else(|| VivianError::Provider(format!("Jev response missing {key}")))?;
            let JevAnswer::Noul { noul } = answer else {
                return Err(VivianError::Provider(format!("Jev response {key} is not noul")));
            };
            if !noul.is_finite() || !(0.0..=1.0).contains(&noul) {
                return Err(VivianError::Provider(format!("Jev noul {key} is out of range")));
            }
            answers.insert(key.to_owned(), noul);
        }
        crate::providers::usage_store::record_usage(
            &parsed.model, parsed.usage.input_tokens, parsed.usage.output_tokens, 0, 0,
        );
        Ok(answers)
    }

    /// Classify emotion and rate its intensity in one mixed-question request.
    pub async fn classify_emotion(&self, text: &str, labels: &[&str]) -> VivianResult<(String, f64)> {
        let criteria: serde_json::Map<String, Value> = labels.iter()
            .map(|label| ((*label).to_owned(), Value::String((*label).to_owned())))
            .collect();
        let request = json!({
            "model": self.model,
            "state": {"user_message": text},
            "questions": {
                "emotion": {"type": "choice", "instructions": "Which single emotion best describes the user's message?", "criteria": criteria},
                "intensity": {"type": "score", "instructions": "How strongly does the message express its emotion?", "criteria": [
                    "No discernible emotion", "Faint emotion", "Moderate emotion", "Strong emotion", "Overwhelming emotion"
                ]}
            }
        });
        let response = self.client.post(&self.endpoint).bearer_auth(&self.api_key)
            .json(&request).send().await
            .map_err(|e| VivianError::Provider(format!("Jev request failed: {e}")))?;
        let status = response.status();
        let body: Value = response.json().await
            .map_err(|e| VivianError::Provider(format!("Jev response invalid: {e}")))?;
        if !status.is_success() {
            return Err(VivianError::Provider(format!("Jev HTTP {status}: {}", body.get("detail").unwrap_or(&body))));
        }
        let mut parsed: SystemOneResponse = serde_json::from_value(body)
            .map_err(|e| VivianError::Provider(format!("Jev response schema mismatch: {e}")))?;
        let emotion = match parsed.answers.remove("emotion") {
            Some(JevAnswer::Choice { choice, confidence, probabilities })
                if (0.0..=1.0).contains(&confidence)
                    && labels.contains(&choice.as_str())
                    && probabilities.len() == labels.len()
                    && labels.iter().all(|label| probabilities.contains_key(*label)) => choice,
            _ => return Err(VivianError::Provider("Jev emotion choice invalid".into())),
        };
        let intensity = match parsed.answers.remove("intensity") {
            Some(JevAnswer::Score { score, confidence, legend, probabilities })
                if score.is_finite() && (0.0..=4.0).contains(&score)
                    && confidence.is_finite() && (0.0..=1.0).contains(&confidence)
                    && legend.len() == 5 && probabilities.len() == 5
                    && (0..5).all(|level| legend.contains_key(&level.to_string())
                        && probabilities.get(&level.to_string()).is_some_and(|p| p.is_finite() && (0.0..=1.0).contains(p)))
                    => score / 4.0,
            _ => return Err(VivianError::Provider("Jev emotion intensity score invalid".into())),
        };
        crate::providers::usage_store::record_usage(
            &parsed.model, parsed.usage.input_tokens, parsed.usage.output_tokens, 0, 0,
        );
        Ok((emotion, intensity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTIONS: &[(&str, &str)] = &[("direct", "desktop bubble"), ("wechat", "chat message")];

    #[test]
    fn parses_documented_choice_shape() {
        let body = json!({
            "model": "jev-latest",
            "answers": {"decision": {
                "type": "choice", "choice": "wechat", "confidence": 0.9,
                "probabilities": {"direct": 0.1, "wechat": 0.9}
            }},
            "usage": {"input_tokens": 120, "output_tokens": 12}
        });
        let (decision, model, usage) = parse_choice_response(body, OPTIONS).unwrap();
        assert_eq!(decision.choice, "wechat");
        assert_eq!(decision.confidence, 0.9);
        assert_eq!(decision.probabilities["direct"], 0.1);
        assert_eq!(model, "jev-latest");
        assert_eq!(usage.input_tokens, 120);
    }

    #[test]
    fn rejects_wrong_answer_type_and_missing_probabilities() {
        let base = json!({
            "model": "jev-latest",
            "answers": {"decision": {"type": "noul", "noul": 0.9}},
            "usage": {"input_tokens": 3, "output_tokens": 1}
        });
        assert!(parse_choice_response(base.clone(), OPTIONS).is_err());
        let mut missing = base;
        missing["answers"]["decision"] = json!({
            "type": "choice", "choice": "direct", "confidence": 0.9
        });
        assert!(parse_choice_response(missing, OPTIONS).is_err());
    }

    #[test]
    fn parses_mixed_noul_and_score_answers() {
        let body = json!({
            "model": "jev-latest",
            "answers": {
                "relevant": {"type": "noul", "noul": 0.82},
                "intensity": {
                    "type": "score", "score": 1.7, "confidence": 0.9,
                    "legend": {"0": "none", "1": "mild", "2": "strong"},
                    "probabilities": {"0": 0.1, "1": 0.2, "2": 0.7}
                }
            },
            "usage": {"input_tokens": 10, "output_tokens": 2}
        });
        let response: SystemOneResponse = serde_json::from_value(body).unwrap();
        assert!(matches!(response.answers["relevant"], JevAnswer::Noul { noul } if noul == 0.82));
        assert!(matches!(response.answers["intensity"], JevAnswer::Score { score, .. } if score == 1.7));
    }

    #[test]
    fn rejects_unknown_choice_and_invalid_confidence() {
        let mut body = json!({
            "model": "jev-latest",
            "answers": {"decision": {
                "type": "choice", "choice": "unexpected", "confidence": 0.8,
                "probabilities": {"direct": 0.2, "wechat": 0.8}
            }},
            "usage": {"input_tokens": 3, "output_tokens": 1}
        });
        assert!(parse_choice_response(body.clone(), OPTIONS).is_err());
        body["answers"]["decision"]["choice"] = json!("wechat");
        body["answers"]["decision"]["confidence"] = json!(1.2);
        assert!(parse_choice_response(body, OPTIONS).is_err());
    }
}
