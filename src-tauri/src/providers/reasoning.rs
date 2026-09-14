//! 推理控制层 —— 厂商无关的思考模式 / 档位到请求字段的映射。
//!
//! 职责链：用户偏好（mode + effort）→ 按模型查能力规则表 → 生成 effective
//! 偏好 → 按请求风格注入各家 wire 字段。
//!
//! 关键不变量：
//! - `Auto` 不增加任何字段（模型存在 `auto_effort` 显式映射的除外）
//! - 不支持关闭的模型（`supports_disable=false`）`Off` 折叠为 `On`，
//!   否则发送关闭字段会被服务端直接拒绝
//! - 各请求风格使用互斥字段，路径互不交叉
//!
//! 思考爆炸防护：部分强制思考模型（如 GLM-5.3）服务端默认档为 `max`，
//! `Auto` 不发字段等价于 `max`，多步工具任务的思考会吃穿输出预算；
//! 这些模型通过 `auto_effort` 把 `Auto` 显式映射为较轻档位。

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::providers::capabilities::{detect_vendor, VendorId};

/// 推理模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReasoningMode {
    /// 不干预，交由服务端默认
    Auto,
    /// 关闭思考
    Off,
    /// 开启思考（可带档位）
    On,
}

/// 推理档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    /// wire 字符串（小写，OpenAI effort 风格）。
    pub fn as_str(&self) -> &'static str {
        match self {
            ReasoningEffort::Minimal => "minimal",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
            ReasoningEffort::Xhigh => "xhigh",
            ReasoningEffort::Max => "max",
        }
    }
}

/// 用户 / 上层的推理偏好。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningPreference {
    pub mode: ReasoningMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<ReasoningEffort>,
}

impl ReasoningPreference {
    pub const AUTO: ReasoningPreference = ReasoningPreference {
        mode: ReasoningMode::Auto,
        effort: None,
    };

    pub fn on(effort: Option<ReasoningEffort>) -> Self {
        Self { mode: ReasoningMode::On, effort }
    }
}

/// 推理控制形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningControl {
    /// 无控制字段（服务端决定）
    None,
    /// 仅开关（thinking.type / enable_thinking）
    Toggle,
    /// 仅档位（reasoning_effort）
    Effort,
    /// 开关 + 档位
    ToggleEffort,
    /// 强制开启，不可关闭
    FixedOn,
}

/// 开启字段注入到请求体的风格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningRequestStyle {
    /// 顶层 `reasoning_effort`（OpenAI / Kimi K3）
    OpenaiEffort,
    /// `thinking: {type: enabled|disabled}`（DeepSeek V4 / GLM / MiMo / 豆包）
    ThinkingType,
    /// `thinking: {type: adaptive|disabled}`（Claude 新系 / MiniMax M3）
    AnthropicAdaptive,
    /// 顶层 `enable_thinking`（Qwen）
    QwenEnableThinking,
    /// 无字段
    None,
}

/// 一个模型族的推理能力描述。
#[derive(Debug, Clone, Copy)]
pub struct ReasoningCapability {
    pub control: ReasoningControl,
    pub supported_efforts: &'static [ReasoningEffort],
    pub default_effort: Option<ReasoningEffort>,
    pub request_style: ReasoningRequestStyle,
    /// 是否支持显式关闭（不支持时 Off 折叠为 On）
    pub supports_disable: bool,
    /// 仅 thinking-type 风格适用：开启且带工具时附加 `thinking.keep="all"`
    pub keep_on_tools: bool,
    /// `Auto` 档显式映射的档位（服务端默认档不可控 / 过重的模型）
    pub auto_effort: Option<ReasoningEffort>,
}

const fn cap(
    control: ReasoningControl,
    supported_efforts: &'static [ReasoningEffort],
    default_effort: Option<ReasoningEffort>,
    request_style: ReasoningRequestStyle,
    supports_disable: bool,
    keep_on_tools: bool,
    auto_effort: Option<ReasoningEffort>,
) -> ReasoningCapability {
    ReasoningCapability {
        control,
        supported_efforts,
        default_effort,
        request_style,
        supports_disable,
        keep_on_tools,
        auto_effort,
    }
}

/// 兜底能力：无控制字段（不干预）。
const UNKNOWN_CAP: ReasoningCapability = cap(
    ReasoningControl::None,
    &[],
    None,
    ReasoningRequestStyle::None,
    false,
    false,
    None,
);

/// 模型推理规则：厂商 + 模型名正则 → 能力。第一条命中生效。
///
/// 排序原则：具体型号在前，宽泛系列在后（如 `*-thinking` 必须在
/// `qwen3` 之前，`kimi-k2.7-code` 必须在通用 K2 系列之前）。
struct ModelReasoningRule {
    vendor: VendorId,
    pattern: &'static str,
    capability: ReasoningCapability,
}

static MODEL_REASONING_RULES: Lazy<Vec<(ModelReasoningRule, Regex)>> = Lazy::new(|| {
    let rules: Vec<ModelReasoningRule> = vec![
        // ── OpenAI ──
        ModelReasoningRule {
            vendor: VendorId::OpenAi,
            pattern: r"(?i)^gpt-5\.6",
            capability: cap(
                ReasoningControl::Effort,
                &[ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High, ReasoningEffort::Xhigh, ReasoningEffort::Max],
                Some(ReasoningEffort::Medium),
                ReasoningRequestStyle::OpenaiEffort,
                true, false, None,
            ),
        },
        ModelReasoningRule {
            vendor: VendorId::OpenAi,
            pattern: r"(?i)^gpt-5",
            capability: cap(
                ReasoningControl::Effort,
                &[ReasoningEffort::Minimal, ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High],
                Some(ReasoningEffort::Medium),
                ReasoningRequestStyle::OpenaiEffort,
                true, false, None,
            ),
        },
        ModelReasoningRule {
            vendor: VendorId::OpenAi,
            pattern: r"(?i)^o[134]",
            capability: cap(
                ReasoningControl::Effort,
                &[ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High],
                Some(ReasoningEffort::Medium),
                ReasoningRequestStyle::OpenaiEffort,
                true, false, None,
            ),
        },
        // ── Anthropic ──
        ModelReasoningRule {
            vendor: VendorId::Anthropic,
            pattern: r"(?i)^claude-(sonnet|opus|fable|haiku)-?[4-9]",
            capability: cap(
                ReasoningControl::ToggleEffort,
                &[ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High, ReasoningEffort::Xhigh, ReasoningEffort::Max],
                Some(ReasoningEffort::High),
                ReasoningRequestStyle::AnthropicAdaptive,
                true, false, None,
            ),
        },
        // ── DeepSeek ──
        ModelReasoningRule {
            vendor: VendorId::Deepseek,
            pattern: r"(?i)^deepseek-v[3-9]",
            capability: cap(
                ReasoningControl::ToggleEffort,
                &[ReasoningEffort::Low, ReasoningEffort::High, ReasoningEffort::Max],
                Some(ReasoningEffort::High),
                ReasoningRequestStyle::ThinkingType,
                true, false,
                // 服务端 auto 会对带工具的请求自动上 max，显式映射 high 防思考爆炸
                Some(ReasoningEffort::High),
            ),
        },
        // ── GLM（智谱）──
        ModelReasoningRule {
            // GLM-5.3 / 5.3-Flash：强制思考模型（thinking.type=disabled 服务端报错）
            vendor: VendorId::Glm,
            pattern: r"(?i)^glm-5\.3",
            capability: cap(
                ReasoningControl::ToggleEffort,
                &[ReasoningEffort::Low, ReasoningEffort::High, ReasoningEffort::Max],
                Some(ReasoningEffort::High),
                ReasoningRequestStyle::ThinkingType,
                false, false,
                // 服务端默认 effort=max，auto 不发字段等价于 max，多步任务思考会吃穿输出预算
                Some(ReasoningEffort::High),
            ),
        },
        ModelReasoningRule {
            vendor: VendorId::Glm,
            pattern: r"(?i)^glm-5\.2",
            capability: cap(
                ReasoningControl::ToggleEffort,
                &[ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High, ReasoningEffort::Xhigh, ReasoningEffort::Max],
                Some(ReasoningEffort::High),
                ReasoningRequestStyle::ThinkingType,
                true, false,
                Some(ReasoningEffort::High),
            ),
        },
        ModelReasoningRule {
            vendor: VendorId::Glm,
            pattern: r"(?i)^glm-(5|4)",
            capability: cap(
                ReasoningControl::Toggle,
                &[],
                None,
                ReasoningRequestStyle::ThinkingType,
                true, false, None,
            ),
        },
        // ── Kimi（月之暗面）──
        ModelReasoningRule {
            // K3 旗舰：思考常开，顶层 reasoning_effort 控档
            vendor: VendorId::Kimi,
            pattern: r"(?i)^kimi-k3",
            capability: cap(
                ReasoningControl::Effort,
                &[ReasoningEffort::Low, ReasoningEffort::High, ReasoningEffort::Max],
                Some(ReasoningEffort::High),
                ReasoningRequestStyle::OpenaiEffort,
                false, false,
                Some(ReasoningEffort::High),
            ),
        },
        ModelReasoningRule {
            // K2.7-Code 系列：思考常开，无控制字段
            vendor: VendorId::Kimi,
            pattern: r"(?i)^kimi-k2\.7",
            capability: cap(
                ReasoningControl::FixedOn,
                &[],
                None,
                ReasoningRequestStyle::None,
                false, false, None,
            ),
        },
        ModelReasoningRule {
            vendor: VendorId::Kimi,
            pattern: r"(?i)^kimi-k2\.6",
            capability: cap(
                ReasoningControl::Toggle,
                &[],
                None,
                ReasoningRequestStyle::ThinkingType,
                true, true, None,
            ),
        },
        ModelReasoningRule {
            vendor: VendorId::Kimi,
            pattern: r"(?i)^kimi-k2",
            capability: cap(
                ReasoningControl::Toggle,
                &[],
                None,
                ReasoningRequestStyle::ThinkingType,
                true, false, None,
            ),
        },
        // ── Qwen（通义千问）──
        ModelReasoningRule {
            // *-thinking 后缀：思考常开
            vendor: VendorId::Qwen,
            pattern: r"(?i)-thinking$",
            capability: cap(
                ReasoningControl::FixedOn,
                &[],
                None,
                ReasoningRequestStyle::None,
                false, false, None,
            ),
        },
        ModelReasoningRule {
            vendor: VendorId::Qwen,
            pattern: r"(?i)^qwen",
            capability: cap(
                ReasoningControl::Toggle,
                &[],
                None,
                ReasoningRequestStyle::QwenEnableThinking,
                true, false, None,
            ),
        },
        // ── 豆包 ──
        ModelReasoningRule {
            vendor: VendorId::Doubao,
            pattern: r"(?i)^doubao-seed",
            capability: cap(
                ReasoningControl::Toggle,
                &[],
                None,
                ReasoningRequestStyle::ThinkingType,
                true, false, None,
            ),
        },
        // ── MiniMax ──
        ModelReasoningRule {
            vendor: VendorId::MiniMax,
            pattern: r"(?i)^minimax-m3",
            capability: cap(
                ReasoningControl::Toggle,
                &[],
                None,
                ReasoningRequestStyle::AnthropicAdaptive,
                true, false, None,
            ),
        },
        ModelReasoningRule {
            vendor: VendorId::MiniMax,
            pattern: r"(?i)^minimax-m2",
            capability: cap(
                ReasoningControl::FixedOn,
                &[],
                None,
                ReasoningRequestStyle::None,
                false, false, None,
            ),
        },
        // ── MiMo（小米）──
        ModelReasoningRule {
            vendor: VendorId::Mimo,
            pattern: r"(?i)^mimo-v[2-9]",
            capability: cap(
                ReasoningControl::Toggle,
                &[],
                None,
                ReasoningRequestStyle::ThinkingType,
                true, false, None,
            ),
        },
    ];
    rules
        .into_iter()
        .filter_map(|r| Regex::new(r.pattern).ok().map(|re| (r, re)))
        .collect()
});

/// 按模型名解析推理能力。
///
/// 先按模型名前缀识别厂商，再在规则表中匹配（第一条命中生效）；
/// 未命中返回无控制字段的兜底能力（不干预服务端行为）。
pub fn resolve_reasoning_capability(model: &str) -> ReasoningCapability {
    let vendor = detect_vendor(model);
    for (rule, re) in MODEL_REASONING_RULES.iter() {
        if rule.vendor == vendor && re.is_match(model) {
            return rule.capability;
        }
    }
    UNKNOWN_CAP
}

/// 把用户偏好解析为实际生效的偏好。
///
/// 决策顺序：
/// 1. 无控制字段 → 强制 Auto（不干预）
/// 2. 强制开启 → 永远 On（不读偏好）
/// 3. `Auto` 且模型有 `auto_effort` → 视为 On + auto_effort
/// 4. `Off` 且不支持关闭 → 折叠为 On
/// 5. `On`：effort 不在支持列表 → 退回默认档；缺省 → 填默认档
pub fn resolve_effective_reasoning(
    pref: ReasoningPreference,
    capability: &ReasoningCapability,
) -> ReasoningPreference {
    // 1. 无控制 → 强制 Auto
    if capability.control == ReasoningControl::None {
        return ReasoningPreference::AUTO;
    }

    // 2. 强制开启 → 永远 On
    if capability.control == ReasoningControl::FixedOn {
        return ReasoningPreference::on(None);
    }

    // 3. Auto 档显式映射（防思考爆炸：服务端默认档不可控的模型）
    if pref.mode == ReasoningMode::Auto {
        if let Some(effort) = capability.auto_effort {
            return ReasoningPreference::on(Some(effort));
        }
        return ReasoningPreference::AUTO;
    }

    // 4. Off 折叠：不支持关闭的模型发关闭字段会被服务端拒绝
    if pref.mode == ReasoningMode::Off {
        if !capability.supports_disable {
            return ReasoningPreference::on(None);
        }
        return ReasoningPreference { mode: ReasoningMode::Off, effort: None };
    }

    // 5. On：档位校验与兜底
    let mut effort = pref.effort;
    if let Some(e) = effort {
        if !capability.supported_efforts.is_empty()
            && !capability.supported_efforts.contains(&e)
        {
            effort = capability.default_effort;
        }
    }
    if effort.is_none() {
        effort = capability.default_effort;
    }
    ReasoningPreference::on(effort)
}

/// 把生效偏好注入 Chat Completions 风格请求体（原地修改）。
///
/// 支持的注入风格：
/// - `OpenaiEffort`：顶层 `reasoning_effort`
/// - `ThinkingType`：`thinking: {type}`（+ `reasoning_effort` 当控制形态含档位）
/// - `AnthropicAdaptive`：`thinking: {type: adaptive|disabled}`（+ `output_config.effort`）
/// - `QwenEnableThinking`：顶层 `enable_thinking`
///
/// `keep_on_tools` 为 true 且请求带工具时，ThinkingType 风格附加
/// `thinking.keep="all"`（多轮工具调用保留思考上下文）。
pub fn apply_reasoning_preference(
    body: &mut Value,
    pref: ReasoningPreference,
    capability: &ReasoningCapability,
    has_tools: bool,
) {
    let effective = resolve_effective_reasoning(pref, capability);

    // 无控制 / Auto：不增加任何字段
    if capability.control == ReasoningControl::None
        || effective.mode == ReasoningMode::Auto
    {
        return;
    }

    // 强制开启：仅注入启用字段
    if capability.control == ReasoningControl::FixedOn {
        inject_enable(body, capability, None, has_tools);
        return;
    }

    if effective.mode == ReasoningMode::Off {
        inject_disable(body, capability);
        return;
    }

    // On：启用字段 + 档位（控制形态含档位时）
    let effort = if matches!(
        capability.control,
        ReasoningControl::Effort | ReasoningControl::ToggleEffort
    ) {
        effective.effort
    } else {
        None
    };
    inject_enable(body, capability, effort, has_tools);
}

/// Responses API 风格的推理注入（原地修改）。
///
/// Responses API 的推理参数为 `reasoning: {"effort": "..."}`，
/// 仅在生效偏好为 On 且带档位时注入；Off / Auto 不发字段
/// （Responses 端对关闭语义支持不一，保守省略交由服务端默认）。
pub fn apply_responses_reasoning(
    body: &mut Value,
    pref: ReasoningPreference,
    capability: &ReasoningCapability,
) {
    let effective = resolve_effective_reasoning(pref, capability);
    if capability.control == ReasoningControl::None {
        return;
    }
    if let ReasoningMode::On = effective.mode {
        if let Some(effort) = effective.effort.or(capability.default_effort) {
            body["reasoning"] = serde_json::json!({ "effort": effort.as_str() });
        }
    }
}

/// 注入启用字段（含档位与 keep_on_tools）。
fn inject_enable(
    body: &mut Value,
    capability: &ReasoningCapability,
    effort: Option<ReasoningEffort>,
    has_tools: bool,
) {
    match capability.request_style {
        ReasoningRequestStyle::OpenaiEffort => {
            if let Some(e) = effort {
                body["reasoning_effort"] = serde_json::json!(e.as_str());
            }
        }
        ReasoningRequestStyle::ThinkingType => {
            let keep = capability.keep_on_tools && has_tools;
            body["thinking"] = if keep {
                serde_json::json!({ "type": "enabled", "keep": "all" })
            } else {
                serde_json::json!({ "type": "enabled" })
            };
            if let Some(e) = effort {
                body["reasoning_effort"] = serde_json::json!(e.as_str());
            }
        }
        ReasoningRequestStyle::AnthropicAdaptive => {
            body["thinking"] = serde_json::json!({ "type": "adaptive" });
            if let Some(e) = effort {
                // 合并已有 output_config，不覆盖其他键
                let mut cfg = body
                    .get("output_config")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                cfg["effort"] = serde_json::json!(e.as_str());
                body["output_config"] = cfg;
            }
        }
        ReasoningRequestStyle::QwenEnableThinking => {
            body["enable_thinking"] = serde_json::json!(true);
        }
        ReasoningRequestStyle::None => {}
    }
}

/// 注入关闭字段。
fn inject_disable(body: &mut Value, capability: &ReasoningCapability) {
    match capability.request_style {
        ReasoningRequestStyle::OpenaiEffort => {
            if capability.supports_disable {
                body["reasoning_effort"] = serde_json::json!("none");
            }
        }
        ReasoningRequestStyle::ThinkingType
        | ReasoningRequestStyle::AnthropicAdaptive => {
            body["thinking"] = serde_json::json!({ "type": "disabled" });
        }
        ReasoningRequestStyle::QwenEnableThinking => {
            body["enable_thinking"] = serde_json::json!(false);
        }
        ReasoningRequestStyle::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn glm_53_auto_maps_to_high() {
        let cap = resolve_reasoning_capability("glm-5.3");
        assert_eq!(cap.control, ReasoningControl::ToggleEffort);
        assert!(!cap.supports_disable);
        let eff = resolve_effective_reasoning(ReasoningPreference::AUTO, &cap);
        assert_eq!(eff.mode, ReasoningMode::On);
        assert_eq!(eff.effort, Some(ReasoningEffort::High));
    }

    #[test]
    fn glm_53_off_folds_to_on() {
        let cap = resolve_reasoning_capability("GLM-5.3-Flash");
        let eff = resolve_effective_reasoning(
            ReasoningPreference { mode: ReasoningMode::Off, effort: None },
            &cap,
        );
        assert_eq!(eff.mode, ReasoningMode::On);
    }

    #[test]
    fn unknown_model_no_injection() {
        let cap = resolve_reasoning_capability("some-random-model");
        let mut body = json!({"model": "some-random-model"});
        apply_reasoning_preference(
            &mut body,
            ReasoningPreference::on(Some(ReasoningEffort::High)),
            &cap,
            false,
        );
        assert_eq!(body, json!({"model": "some-random-model"}));
    }

    #[test]
    fn deepseek_on_injects_thinking_and_effort() {
        let cap = resolve_reasoning_capability("deepseek-v4-pro");
        let mut body = json!({"model": "deepseek-v4-pro"});
        apply_reasoning_preference(
            &mut body,
            ReasoningPreference::on(None),
            &cap,
            false,
        );
        assert_eq!(body["thinking"], json!({"type": "enabled"}));
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn qwen_thinking_suffix_is_fixed_on() {
        let cap = resolve_reasoning_capability("qwen3-max-thinking");
        assert_eq!(cap.control, ReasoningControl::FixedOn);
    }

    #[test]
    fn qwen_toggle_uses_enable_thinking() {
        let cap = resolve_reasoning_capability("qwen3-235b");
        let mut body = json!({});
        apply_reasoning_preference(
            &mut body,
            ReasoningPreference { mode: ReasoningMode::Off, effort: None },
            &cap,
            false,
        );
        assert_eq!(body["enable_thinking"], json!(false));
    }

    #[test]
    fn effort_out_of_range_falls_back_to_default() {
        let cap = resolve_reasoning_capability("deepseek-v4");
        let eff = resolve_effective_reasoning(
            ReasoningPreference::on(Some(ReasoningEffort::Medium)),
            &cap,
        );
        assert_eq!(eff.effort, Some(ReasoningEffort::High));
    }
}
