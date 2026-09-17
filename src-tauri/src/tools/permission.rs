//! 权限系统 - 工具权限检查、文件路径权限和工作目录限制
//!
//! ## 确认档位（四档策略）
//!
//! 执行前要不要停下来问用户，由 [`confirmation_tier`] 判定，四档由宽到严：
//!
//! | 档位 | 语义 |
//! |---|---|
//! | [`ToolConfirmationTier::NotRequired`] | 直接做 |
//! | [`ToolConfirmationTier::PreApprovalAllowed`] | 本轮用户消息里明确授权过 → 直接做；否则问 |
//! | [`ToolConfirmationTier::ConfirmAtAction`] | 动手前必须确认 |
//! | [`ToolConfirmationTier::HandOff`] | agent 不许执行，必须把控制权交回用户 |
//!
//! ## ⚠️ 工具名必须与真实注册名一致
//!
//! 下面几张表填的是 `Tool::name()` 的返回值。**名字写错不会编译报错、不会测试失败**，
//! 只表现为"该问的没问"或"该拦的没拦"。有前科：本表曾写
//! `list_directory` / `search_files` / `grep` / `cancel_scheduled` / `delete_todo`，
//! 而真实注册名是 `list_dir` / `grep_search` / `manage_scheduled` / `manage_todo`
//! —— 五个工具因此**从未进入确认流程**（`list_dir`、`grep_search` 尤其严重：
//! 它们能遍历、搜索用户全盘文件）。
//! [`tests::confirmation_list_names_all_registered`] 单测守住这条不变量。

use serde_json::Value;

use super::types::{
    is_path_within, policy_for, AgentAccessLevel, PermissionContext, PermissionMode,
    PermissionResult, Tool, ToolUseContext,
};

// ============================================================================
// 一、确认档位
// ============================================================================

/// 工具确认档位 —— 执行前要不要停下来问用户。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolConfirmationTier {
    /// 直接做，不问。
    NotRequired,
    /// 本轮用户消息里明确授权过目标 → 直接做；否则问。
    ///
    /// 用于"用户已经说清楚了要干什么"的低风险动作：说了"换成这张壁纸"就不该
    /// 再弹一次窗问"要换壁纸吗"。
    PreApprovalAllowed,
    /// 动手前必须确认（预授权也要再问）。
    ConfirmAtAction,
    /// agent 不许执行，必须把控制权交回用户自己完成。
    ///
    /// 仅用于"agent 做这一步本身就不合适"的场景（在支付/银行/凭据页面执行任意
    /// JS）。这一档**不受 Bypass 模式影响**——它不是"要不要问"，而是"能不能做"。
    HandOff,
}

/// 必须确认的工具（**真实注册名**）。
///
/// 这些工具涉及隐私敏感或系统状态变更操作，默认必须经前端用户确认后才能执行。
/// 用户仍可通过会话级放行（确认 toast 的"本次运行允许"）或 Bypass 模式覆盖。
const CONFIRM_AT_ACTION_TOOLS: &[&str] = &[
    // 文件读写（coding 工具集）
    "read_file",
    "write_file",
    "edit_file",
    "list_dir",
    "grep_search",
    // 屏幕感知
    "take_screenshot",
    "screenshot_analyze",
    // 破坏性卸载
    "delete_plugin",
    // 系统状态变更
    "media_control",
    "wallpaper_set",
    "wallpaper_control",
    // 浏览器任意脚本：在已登录会话执行任意 JS，等同代码执行。
    // 此处填的是**扩展线名**；该工具在模型侧叫 `mcp__browser__eval_js`，
    // 匹配由 [`canonical_tool_name`] 归一化
    "browser_eval_js",
];

/// 分发器工具：只有**非破坏性** action 才免确认。
///
/// `manage_todo` / `manage_scheduled` 是"一个工具干几件事"的分发器，
/// 把整个工具塞进确认名单会让 `list` 这类只读调用也弹窗。
///
/// 刻意用**白名单**（列出安全的 action）而不是黑名单（列出破坏性的）：
/// 模型给出未知 action、拼写错误或干脆不给 action 时，白名单会落到
/// `ConfirmAtAction` —— 拿不到"这是安全操作"的证据就不放行。
/// （旧表里写死的 `delete_todo` / `cancel_scheduled` 从来不是真实工具名，
/// 等于这两条确认一直不存在。）
const DISPATCHER_SAFE_ACTIONS: &[(&str, &[&str])] = &[
    ("manage_todo", &["list", "get", "update"]),
    ("manage_scheduled", &["list", "get", "pause", "resume"]),
];

/// 允许预授权的工具：本轮用户消息点到具体目标时免确认。
///
/// 判据是"用户已经把这件事说清楚了"，不是"这个工具很安全"——
/// 用户没说清楚时仍然照常询问。
const PRE_APPROVAL_ALLOWED_TOOLS: &[&str] = &[
    "wallpaper_set",
    "wallpaper_control",
    "media_control",
    "open_application",
    "close_application",
    "take_screenshot",
    "screenshot_analyze",
];

/// 动作关键词 → 工具。用户本轮消息里出现这类词，视为对该工具目标的明确授权。
///
/// 三语齐备：命中判定是「消息包含关键词」，所以中英日都要列，否则英文/日文用户
/// 永远拿不到预授权（这是"英文反例约束不了中文输出"的镜像问题）。
const ACTION_KEYWORDS: &[(&str, &[&str])] = &[
    (
        "wallpaper_set",
        &["壁纸", "桌面背景", "换张图", "wallpaper", "壁紙"],
    ),
    (
        "wallpaper_control",
        &["壁纸", "桌面背景", "wallpaper", "壁紙"],
    ),
    (
        "media_control",
        &[
            "播放", "暂停", "下一首", "上一首", "切歌", "音量", "静音", "调小", "调大",
            "play", "pause", "next track", "previous track", "volume", "mute",
            "再生", "一時停止", "次の曲", "ミュート",
        ],
    ),
    (
        "open_application",
        &["打开", "启动", "开一下", "帮我开", "open ", "launch", "開いて", "起動"],
    ),
    (
        "close_application",
        &["关闭", "关掉", "退出", "close ", "quit", "閉じて"],
    ),
    (
        "take_screenshot",
        &[
            "截图", "看看我的屏幕", "看我屏幕", "看看我在干嘛", "看我在干嘛", "我屏幕上",
            "screenshot", "look at my screen", "what am i doing",
            "スクリーンショット", "画面を見て",
        ],
    ),
    (
        "screenshot_analyze",
        &[
            "截图", "看看我的屏幕", "看我屏幕", "看看我在干嘛", "看我在干嘛", "我屏幕上",
            "screenshot", "look at my screen", "what am i doing",
            "スクリーンショット", "画面を見て",
        ],
    ),
];

/// 敏感域主机标记：agent 一律不许在这些站点执行脚本，必须交回用户。
///
/// 按**主机名子串**匹配（不做全文匹配——`display` 里含 `play` 这类假阳性会让
/// 正常浏览也被拦）。
const SENSITIVE_HOST_MARKERS: &[&str] = &[
    // 支付 / 银行 / 钱包
    "alipay.com",
    "paypal.com",
    "unionpay",
    "wechatpay",
    "tenpay",
    "bank",
    "icbc",
    "ccb.com",
    "abchina",
    "cmbchina",
    "boc.cn",
    "stripe.com",
    "checkout.",
    // 政务 / 票务 / 医疗
    "gov.cn",
    "12306",
    "chinatax",
    "hospital",
    // 券商 / 交易
    "eastmoney",
    "xueqiu",
    "htsc",
    "gtja",
];

/// 敏感域路径标记：主机看不出来时再看路径段。
const SENSITIVE_PATH_MARKERS: &[&str] = &[
    "/pay",
    "/payment",
    "/checkout",
    "/transfer",
    "/withdraw",
    "/recharge",
    "/password",
    "/passwd",
    "/security",
    "/recovery",
    "/bindcard",
    "/wallet",
    "/account/close",
];

/// 等价能力簇 —— 同一簇内的工具能达成同样的效果。
///
/// `always_deny` 只按工具名匹配，模型可以换一个同簇工具绕过拒绝
/// （用户拒绝 `write_file` → 模型改用 `edit_file`，或改用 `run_command` 重定向写盘）。
/// 判定逻辑见 [`equivalence_conflict`]：命中任一成员被拒时，同簇其它成员
/// **强制降级为询问**（而不是直接拒绝——用户拒的是那个工具名，不一定是整簇，
/// 所以给一次明确的确认机会，而不是静默拦住）。
const EQUIVALENCE_CLUSTERS: &[&[&str]] = &[
    // 写盘
    &["write_file", "edit_file", "run_command"],
    // 读盘
    &["read_file", "grep_search", "list_dir"],
    // 屏幕 / 前台感知
    &[
        "take_screenshot",
        "screenshot_analyze",
        "get_active_window",
        "get_foreground_app_context",
    ],
    // 浏览器任意脚本（两个工具都能在已登录会话里跑 JS）
    &["browser_eval_js", "browser_task_tab"],
];

/// 把工具名归一为"线名"口径，让 `mcp__browser__eval_js` 与 `browser_eval_js`
/// 能落到同一张表上比较。
fn canonical_tool_name(tool_name: &str) -> String {
    match crate::browser_bridge::tools::wire_name(tool_name) {
        Some(wire) => wire.to_string(),
        None => tool_name.to_string(),
    }
}

/// 工具是否需要用户确认（**按名字判定**，看不到参数）。
///
/// 这是给"工具描述里要不要标 `[需确认]`"用的近似判定：分发器只要**有**
/// 破坏性 action 就返回 true（宁可多标，不可漏标）。
/// 真正的执行前判定走 [`confirmation_tier`]，那里能看到 action 参数。
pub fn is_confirmation_required_tool(tool_name: &str) -> bool {
    let name = canonical_tool_name(tool_name);
    if CONFIRM_AT_ACTION_TOOLS.contains(&name.as_str()) {
        return true;
    }
    DISPATCHER_SAFE_ACTIONS
        .iter()
        .any(|(dispatcher, _)| *dispatcher == name)
}

/// 工具在本次调用下的确认档位（能看到参数，是精确判定）。
pub fn confirmation_tier(tool_name: &str, args: &Value) -> ToolConfirmationTier {
    let name = canonical_tool_name(tool_name);

    // Hand-off：敏感域里的脚本执行，agent 不许碰。
    if name == "browser_eval_js" {
        if let Some(url) = eval_js_target_url(args) {
            if is_sensitive_url(&url) {
                return ToolConfirmationTier::HandOff;
            }
        }
    }

    // 分发器：白名单外的 action 一律确认（含缺参数 / 拼写错误）
    for (dispatcher, safe) in DISPATCHER_SAFE_ACTIONS {
        if *dispatcher == name {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            return if safe.contains(&action.as_str()) {
                ToolConfirmationTier::NotRequired
            } else {
                ToolConfirmationTier::ConfirmAtAction
            };
        }
    }

    if CONFIRM_AT_ACTION_TOOLS.contains(&name.as_str()) {
        // 名单里同时允许预授权的工具（壁纸 / 媒体键 / 屏幕感知）走预授权档，
        // 其余一律"动手前必问"。
        return if PRE_APPROVAL_ALLOWED_TOOLS.contains(&name.as_str()) {
            ToolConfirmationTier::PreApprovalAllowed
        } else {
            ToolConfirmationTier::ConfirmAtAction
        };
    }

    if PRE_APPROVAL_ALLOWED_TOOLS.contains(&name.as_str()) {
        return ToolConfirmationTier::PreApprovalAllowed;
    }

    ToolConfirmationTier::NotRequired
}

/// 从 `browser_eval_js` 参数里取目标 URL（不同扩展动作的字段名不同）。
fn eval_js_target_url(args: &Value) -> Option<String> {
    for key in ["url", "href", "target_url", "page_url"] {
        if let Some(v) = args.get(key).and_then(Value::as_str) {
            if !v.trim().is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// URL 是否落在敏感域（支付 / 银行 / 政务 / 券商 / 凭据页）。
pub fn is_sensitive_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    // 抽出主机部分：跳过 scheme，取到第一个 '/' '?' '#' 之前
    let after_scheme = lower.split("://").nth(1).unwrap_or(&lower);
    let host = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    if SENSITIVE_HOST_MARKERS.iter().any(|m| host.contains(m)) {
        return true;
    }
    // 主机看不出来时再看路径
    let path = after_scheme.strip_prefix(host).unwrap_or("");
    SENSITIVE_PATH_MARKERS.iter().any(|m| path.contains(m))
}

/// 本轮用户消息里是否明确点到了这个工具的目标。
///
/// 判据（任一成立即可）：
/// 1. 消息里出现了该工具的动作关键词（"换壁纸" "播放" "看看我的屏幕"）
/// 2. 消息里出现了工具某个字符串参数的值或其文件名（用户直接给了路径/名称）
///
/// 刻意用"字符串包含"而不是 LLM 判定：这是热路径（每次工具调用都跑），
/// 且误判的代价可控（多问一次 vs 少问一次，前者只是烦，后者才是事故）。
fn user_authorized_target(tool_name: &str, args: &Value, user_input: &str) -> bool {
    let input = user_input.trim().to_ascii_lowercase();
    if input.is_empty() {
        return false;
    }
    let name = canonical_tool_name(tool_name);

    // 判据 1：动作关键词
    for (tool, keywords) in ACTION_KEYWORDS {
        if *tool == name && keywords.iter().any(|k| input.contains(&k.to_ascii_lowercase())) {
            return true;
        }
    }

    // 判据 2：参数值 / 文件名
    for value in string_arg_values(args) {
        let v = value.trim().to_ascii_lowercase();
        // 太短的值（"a"、"1"）会在任意消息里误命中
        if v.chars().count() < 3 {
            continue;
        }
        if input.contains(&v) {
            return true;
        }
        let file_name = v.rsplit(['/', '\\']).next().unwrap_or("").trim().to_string();
        if file_name.chars().count() >= 3 && input.contains(&file_name) {
            return true;
        }
    }
    false
}

/// 收集参数里所有字符串值（含数组内元素）。
fn string_arg_values(args: &Value) -> Vec<String> {
    let mut out = Vec::new();
    match args {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => {
            for item in items {
                out.extend(string_arg_values(item));
            }
        }
        Value::Object(map) => {
            for value in map.values() {
                out.extend(string_arg_values(value));
            }
        }
        _ => {}
    }
    out
}

/// 当前工具是否与"已被拒绝的同簇工具"构成等价绕过。
///
/// 返回被拒绝的那个工具名（用于提示文案）。
fn equivalence_conflict(tool_name: &str, context: &PermissionContext) -> Option<&'static str> {
    let name = canonical_tool_name(tool_name);
    let cluster = EQUIVALENCE_CLUSTERS
        .iter()
        .find(|cluster| cluster.iter().any(|m| *m == name))?;

    for member in cluster.iter() {
        if *member == name {
            continue;
        }
        if matches_pattern(member, &context.always_deny) {
            return Some(member);
        }
    }
    None
}

/// 文件工具 → 操作类型映射
fn file_operation_for(tool_name: &str) -> Option<&'static str> {
    match canonical_tool_name(tool_name).as_str() {
        "read_file" | "list_dir" | "grep_search" => Some("read"),
        "write_file" | "edit_file" => Some("write"),
        _ => None,
    }
}

/// 从工具参数中提取文件路径
fn extract_file_paths(tool_name: &str, args: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    if matches!(tool_name, "copy_file" | "move_file") {
        for key in &["source", "destination"] {
            if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
                paths.push(v.to_string());
            }
        }
        return paths;
    }
    if tool_name == "list_directory" {
        if let Some(v) = args.get("directory").and_then(|v| v.as_str()) {
            paths.push(v.to_string());
        }
        return paths;
    }
    for key in &["file_path", "path"] {
        if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
            paths.push(v.to_string());
        }
    }
    paths
}

/// 检查工具权限
///
/// 综合考虑：
/// - 确认档位（Hand-off / 必问 / 可预授权 / 免问）
/// - 权限模式（Bypass 直接放行，Ask 一律询问）
/// - always_allow / always_deny / always_ask 规则
/// - 等价能力簇（防换工具绕过拒绝）
/// - 文件路径的工作目录与只读限制
pub async fn check_tool_permission(
    tool: &dyn Tool,
    args: &Value,
    context: &ToolUseContext,
    permission_context: &PermissionContext,
) -> PermissionResult {
    let tool_name = tool.name();
    let tier = confirmation_tier(tool_name, args);

    // 0. 显式拒绝规则：优先级最高，置于 Bypass 之前，保证 always_deny 不被绕过。
    if matches_tool_pattern(tool_name, &permission_context.always_deny) {
        return PermissionResult::deny(format!("工具 '{}' 被规则拒绝", tool_name));
    }

    // 0.5 Hand-off 档：这一档问的不是"要不要问"，而是"能不能做"，因此**置于 Bypass 之前**。
    //     典型场景：在支付/银行/凭据页面执行任意 JS —— 交给用户自己操作才是正确答案。
    if tier == ToolConfirmationTier::HandOff {
        return PermissionResult::deny(format!(
            "工具 '{}' 在敏感页面（支付/银行/凭据）被禁止执行：这类操作必须由你本人完成，\
             我不会代你操作。请自己在浏览器里完成这一步。",
            tool_name
        ));
    }

    // 1. Bypass 模式直接放行
    if permission_context.is_bypass_mode() {
        return PermissionResult::allow();
    }

    // 1.2 等价能力簇：用户拒绝过同簇工具时，换一个工具达到同样效果必须重新确认。
    //     只按工具名匹配的 always_deny 挡不住「拒 write_file → 改用 edit_file /
    //     run_command 重定向写盘」这类变通执行。
    if let Some(denied_sibling) = equivalence_conflict(tool_name, permission_context) {
        return PermissionResult::ask(format!(
            "工具 '{}' 与已被拒绝的 '{}' 具备等价能力，需要你再次确认",
            tool_name, denied_sibling
        ));
    }

    // 1.5 权限网关矩阵：access_level × risk 决定 allow/ask/deny
    let access = permission_context.access_level;
    let risk = tool.risk();
    let matrix_behavior = policy_for(access, risk);
    if matches!(
        matrix_behavior,
        super::types::PermissionBehavior::Deny
    ) {
        let hint = if risk == super::types::ToolRiskTier::InputControl {
            "。该操作需要输入控制权限，请在设置中将访问级别提升至 FullControl（完全控制）后重试"
        } else {
            "。可在设置中提升访问级别以解锁更高权限的工具"
        };
        return PermissionResult::deny(format!(
            "工具 '{}' 风险等级 {} 超出当前访问级别 {} 的允许范围{}",
            tool_name,
            risk.as_str(),
            access.as_str(),
            hint,
        ));
    }
    let matrix_ask = matches!(
        matrix_behavior,
        super::types::PermissionBehavior::Ask
    );

    // 2. 文件路径权限检查
    if let Some(operation) = file_operation_for(tool_name) {
        for fp in extract_file_paths(tool_name, args) {
            let result = check_file_permission(&fp, operation, permission_context);
            if result.is_denied() {
                return result;
            }
            if result.requires_confirmation() {
                return result;
            }
        }
    }

    // 3. always_ask 规则
    if matches_tool_pattern(tool_name, &permission_context.always_ask) {
        return PermissionResult::ask(format!(
            "工具 '{}' 需要用户确认",
            tool_name
        ));
    }

    // 4. always_allow 规则
    if matches_tool_pattern(tool_name, &permission_context.always_allow) {
        return PermissionResult::allow();
    }

    // 4.5 浏览器可信来源：导航到高信任白名单站点免确认，收敛信任边界。
    //     其余浏览器改动（点击/输入/导航他站等）仍走下方矩阵/确认流程。
    //     工具名在模型侧是 MCP 命名空间名，这里按动作名判定，两种写法都认。
    if crate::browser_bridge::tools::action_of(tool_name) == Some("navigate") {
        if let Some(url) = args.get("url").and_then(Value::as_str) {
            if super::trusted_origins::is_trusted_url(url) {
                return PermissionResult::allow();
            }
        }
    }

    // 5. Ask 模式：一律询问
    if permission_context.is_ask_mode() {
        return PermissionResult::ask(format!(
            "工具 '{}' 在 Ask 模式下需要确认",
            tool_name
        ));
    }

    // 5.5 矩阵判定 Ask：风险等级超出访问级别直接允许范围，向用户请求确认
    //     （always_allow 显式规则已在步骤 4 返回，优先级高于矩阵判定）
    if matrix_ask {
        return PermissionResult::ask(format!(
            "工具 '{}' 风险等级 {} 在当前访问级别 {} 下需要用户确认",
            tool_name,
            risk.as_str(),
            access.as_str(),
        ));
    }

    // 6. 确认档位判定（精确档，能看到参数）
    //
    //    这里是隐私敏感工具（文件 5 + 屏幕 2 + 壁纸 2 + 媒体键 + 插件卸载）
    //    的真正闸门：不再依赖各工具自己 check_permissions 的实现细节。
    match tier {
        // 已在步骤 0.5 处理，此处不可达；保留分支让新增档位时编译器提醒。
        ToolConfirmationTier::HandOff => PermissionResult::deny(format!(
            "工具 '{}' 属于禁止 agent 执行的档位",
            tool_name
        )),
        ToolConfirmationTier::ConfirmAtAction => PermissionResult::ask(format!(
            "工具 '{}' 需要用户确认后执行",
            tool_name
        )),
        ToolConfirmationTier::PreApprovalAllowed => {
            // 用户本轮已经把这件事说清楚了（"换成这张壁纸" / "看看我的屏幕"）
            // → 不再重复问一遍。没说清楚时照常询问。
            let authorized = permission_context
                .current_user_input
                .as_deref()
                .is_some_and(|input| user_authorized_target(tool_name, args, input));
            if authorized {
                PermissionResult::allow()
            } else {
                PermissionResult::ask(format!(
                    "工具 '{}' 需要用户确认后执行",
                    tool_name
                ))
            }
        }
        // 免问档：仍委托给工具自身的权限检查（个别工具有自己的附加规则）
        ToolConfirmationTier::NotRequired => tool.check_permissions(args, context).await,
    }
}

/// 工具名规则匹配：同时按模型可见名与归一化线名匹配。
///
/// `always_deny` 这类规则由用户/配置填写，写 `browser_eval_js`（线名）或
/// `mcp__browser__eval_js`（模型名）都应该生效——只认一种会让规则静默失效。
fn matches_tool_pattern(tool_name: &str, patterns: &[String]) -> bool {
    if matches_pattern(tool_name, patterns) {
        return true;
    }
    let canonical = canonical_tool_name(tool_name);
    canonical != tool_name && matches_pattern(&canonical, patterns)
}

// ============================================================================
// 名单自检（防"名字写错导致确认静默失效"）
// ============================================================================

/// 确认名单里**没有对应已注册工具**的名字。
///
/// 空表示名单全部有效。非空时说明有人改了工具名却没同步这张表——
/// 后果是"该问的没问"（安全回归），且编译器与常规测试都不会报。
///
/// 由 `tools::builtin::register_builtin_tools` 在注册完成后调用一次并打 error 日志；
/// 不 panic 是因为名字表可能暂时领先于实现（工具尚未注册），
/// 但**必须留下显眼的日志**，否则这个 bug 会像前科那样潜伏很久。
pub fn unresolved_confirmation_names(
    is_registered: impl Fn(&str) -> bool,
) -> Vec<&'static str> {
    let mut missing: Vec<&'static str> = Vec::new();

    for name in CONFIRM_AT_ACTION_TOOLS {
        // 浏览器桥工具在注册表里是模型可见名，名单里填的是线名——两种都认。
        let ok = is_registered(name) || is_registered(&to_mcp_style(name));
        if !ok {
            missing.push(name);
        }
    }
    for (dispatcher, _) in DISPATCHER_SAFE_ACTIONS {
        if !is_registered(dispatcher) {
            missing.push(dispatcher);
        }
    }
    missing
}

/// 线名 → 模型可见名（仅浏览器桥工具会发生这种转换，其余原样返回）。
fn to_mcp_style(name: &str) -> String {
    crate::browser_bridge::tools::to_mcp_name(name).unwrap_or_else(|| name.to_string())
}

/// 检查文件路径权限
pub fn check_file_permission(
    file_path: &str,
    operation: &str,
    context: &PermissionContext,
) -> PermissionResult {
    if context.is_bypass_mode() {
        return PermissionResult::allow();
    }

    // 多个工作区可以互相嵌套（附加目录落在主工作区内）。这里必须取**最长匹配**（最具体的
    // 那个）而不是第一个匹配：HashMap 迭代顺序不确定，否则「主工作区可写 + 其中某个子目录
    // 只读」这种配置下，同一路径的结论会随机在 allow 与 deny 之间跳。
    if let Some(wd) = context
        .additional_working_directories
        .values()
        .filter(|wd| is_path_within(file_path, &wd.path))
        .max_by_key(|wd| wd.path.len())
    {
        if wd.is_read_only && (operation == "write" || operation == "delete") {
            return PermissionResult::deny(format!(
                "目录 '{}' 是只读的，不允许 {} 操作",
                wd.path, operation
            ));
        }

        if !wd.permissions.iter().any(|p| p == operation || p == "*") {
            return PermissionResult::ask(format!(
                "操作 '{}' 在目录 '{}' 中不被显式允许",
                operation, wd.path
            ));
        }

        return PermissionResult::allow();
    }

    // 路径不在任何工作目录中：写入/删除操作需要询问
    if operation == "write" || operation == "delete" {
        return PermissionResult::ask(format!(
            "路径 '{}' 不在已授权的工作目录中，需要确认",
            file_path
        ));
    }

    PermissionResult::allow()
}

/// 通配符匹配
fn matches_pattern(name: &str, patterns: &[String]) -> bool {
    for pattern in patterns {
        if pattern_match(pattern, name) {
            return true;
        }
    }
    false
}

/// 单个 pattern 匹配（支持 `*`、`?`、`regex:` 前缀）
fn pattern_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(regex_str) = pattern.strip_prefix("regex:") {
        if let Ok(re) = regex::Regex::new(regex_str) {
            return re.is_match(value);
        }
        return false;
    }
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        return glob_match(pattern, value);
    }
    pattern == value
}

/// 简单的 glob 匹配（支持 `*` 和 `?`）
fn glob_match(pattern: &str, value: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();
    glob_match_inner(&p, &v)
}

fn glob_match_inner(pattern: &[char], value: &[char]) -> bool {
    let mut pi = 0;
    let mut vi = 0;
    let mut star_pi = None;
    let mut star_vi = 0;

    while vi < value.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == value[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star_pi = Some(pi);
            star_vi = vi;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_vi += 1;
            vi = star_vi;
        } else {
            return false;
        }
    }

    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }

    pi == pattern.len()
}

/// 创建权限上下文构建器
pub struct PermissionContextBuilder {
    context: PermissionContext,
}

impl PermissionContextBuilder {
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            context: PermissionContext::new(mode),
        }
    }

    pub fn with_access_level(mut self, level: AgentAccessLevel) -> Self {
        self.context.access_level = level;
        self
    }

    pub fn with_working_directory(mut self, path: impl Into<String>, read_only: bool) -> Self {
        self.context.add_working_directory(path, read_only);
        self
    }

    /// 注入本轮用户消息原文，供「预授权」档判定（用户已说清目标则免确认）。
    pub fn with_user_input(mut self, user_input: impl Into<String>) -> Self {
        self.context.current_user_input = Some(user_input.into());
        self
    }

    pub fn allow(mut self, tool: impl Into<String>) -> Self {
        self.context.always_allow.push(tool.into());
        self
    }

    pub fn deny(mut self, tool: impl Into<String>) -> Self {
        self.context.always_deny.push(tool.into());
        self
    }

    pub fn ask(mut self, tool: impl Into<String>) -> Self {
        self.context.always_ask.push(tool.into());
        self
    }

    pub fn build(self) -> PermissionContext {
        self.context
    }
}

/// 工具是否需要权限确认（是否需要进入 [`check_tool_permission`] 流程）
///
/// 这是**闸门**：返回 false 表示"直接执行，不必问"。因此判定必须偏保守——
/// 只在能确定"不会有 ask/deny"时才返回 false。
pub fn requires_permission(
    tool: &dyn Tool,
    args: &Value,
    context: &PermissionContext,
) -> bool {
    // 显式拒绝优先级最高：置于 Bypass 判定之前，保证 always_deny 不被绕过。
    // 该分支已覆盖下方矩阵结果，故此处命中后无需再次匹配。
    if matches_tool_pattern(tool.name(), &context.always_deny) {
        return true;
    }

    if context.is_bypass_mode() {
        return false;
    }

    // 权限网关矩阵：Deny / Ask 都需要进入权限检查流程
    let behavior = policy_for(context.access_level, tool.risk());
    if matches!(
        behavior,
        super::types::PermissionBehavior::Deny | super::types::PermissionBehavior::Ask
    ) {
        return true;
    }
    if matches_tool_pattern(tool.name(), &context.always_ask) {
        return true;
    }
    if matches_tool_pattern(tool.name(), &context.always_allow) {
        return false;
    }
    if context.is_ask_mode() {
        return true;
    }

    // 等价能力簇：与已拒绝的同簇工具构成绕过时，必须进流程（走 ask 分支）
    if equivalence_conflict(tool.name(), context).is_some() {
        return true;
    }

    // 确认档位
    match confirmation_tier(tool.name(), args) {
        // Hand-off / 动手前必问：都要进流程才能给出 deny / ask
        ToolConfirmationTier::HandOff | ToolConfirmationTier::ConfirmAtAction => return true,
        // 预授权档：用户本轮已说清目标 → 不必进流程；没说清 → 进流程（要问）
        ToolConfirmationTier::PreApprovalAllowed => {
            let authorized = context
                .current_user_input
                .as_deref()
                .is_some_and(|input| user_authorized_target(tool.name(), args, input));
            return !authorized;
        }
        // 免问档：继续走下面的通用判定
        ToolConfirmationTier::NotRequired => {}
    }

    // 文件写入路径不在工作目录内时也需要确认
    if let Some(op) = file_operation_for(tool.name()) {
        if op == "write" {
            for fp in extract_file_paths(tool.name(), args) {
                if !context.is_path_in_working_directory(&fp) {
                    return true;
                }
            }
        }
    }
    !tool.is_read_only()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    use crate::tools::types::{ToolCategory, ToolResult, ToolRiskTier, ValidationResult};

    struct TestTool;

    #[async_trait]
    impl Tool for TestTool {
        fn name(&self) -> &str {
            "test_tool"
        }

        fn description(&self) -> &str {
            "permission test tool"
        }

        fn parameters_schema(&self) -> Value {
            json!({"type": "object"})
        }

        async fn validate_input(
            &self,
            _input: &Value,
            _context: &ToolUseContext,
        ) -> ValidationResult {
            ValidationResult::success(None)
        }

        async fn check_permissions(
            &self,
            _input: &Value,
            _context: &ToolUseContext,
        ) -> PermissionResult {
            PermissionResult::allow()
        }

        async fn call(&self, _args: Value, _context: &ToolUseContext) -> ToolResult {
            ToolResult::success(json!({}))
        }

        fn is_read_only(&self) -> bool {
            true
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::System
        }

        fn risk(&self) -> ToolRiskTier {
            ToolRiskTier::Safe
        }
    }

    #[tokio::test]
    async fn explicit_deny_cannot_be_bypassed() {
        let permissions = PermissionContextBuilder::new(PermissionMode::Bypass)
            .deny("test_tool")
            .build();
        let result = check_tool_permission(
            &TestTool,
            &json!({}),
            &ToolUseContext::default(),
            &permissions,
        )
        .await;

        assert!(result.is_denied());
        assert!(requires_permission(&TestTool, &json!({}), &permissions));
    }

    #[test]
    fn sibling_directory_does_not_inherit_workspace_permission() {
        let permissions = PermissionContextBuilder::new(PermissionMode::Default)
            .with_working_directory("C:/work", false)
            .build();

        assert!(check_file_permission("C:/work/file.txt", "write", &permissions).is_allowed());
        assert!(check_file_permission("C:/work-evil/file.txt", "write", &permissions)
            .requires_confirmation());
        assert!(check_file_permission("C:/work/../outside/file.txt", "write", &permissions)
            .requires_confirmation());
    }

    /// 归属判定只有一个口径：`is_path_in_working_directory` 与
    /// `check_file_permission` 必须对同一路径给出同样的「在内/在外」结论，
    /// 否则确认闸门与决策引擎会互相矛盾。
    #[test]
    fn working_directory_membership_matches_file_permission() {
        let permissions = PermissionContextBuilder::new(PermissionMode::Default)
            .with_working_directory("C:/work", false)
            .build();

        for path in ["C:/work/file.txt", "C:/work-evil/file.txt", "C:/work/../outside/file.txt"] {
            let membership = permissions.is_path_in_working_directory(path);
            let checked = check_file_permission(path, "write", &permissions);
            assert_eq!(
                membership,
                checked.is_allowed(),
                "路径 '{path}' 的归属判定与文件权限判定不一致"
            );
        }

        assert!(permissions.get_working_directory_permissions("C:/work/file.txt").is_some());
        assert!(permissions.get_working_directory_permissions("C:/work-evil/file.txt").is_none());
    }

    // ========================================================================
    // 名单自检：防"名字写错导致确认静默失效"
    // ========================================================================

    /// 这 5 个名字曾出现在确认名单里，但**从来不是真实工具名**——
    /// 于是 list_dir / grep_search / manage_scheduled / manage_todo
    /// 这些工具从未进入确认流程。此测试是回归守卫。
    #[test]
    fn confirmation_list_has_no_stale_names() {
        let stale = [
            "list_directory",
            "search_files",
            "grep",
            "cancel_scheduled",
            "delete_todo",
        ];
        for name in stale {
            assert!(
                !CONFIRM_AT_ACTION_TOOLS.contains(&name),
                "确认名单里又出现了不存在的工具名 '{name}'；\
                 它对应的真实注册名是 list_dir / grep_search / manage_scheduled / manage_todo。\
                 写错名字不会编译报错，只会让这条确认静默失效。"
            );
            assert!(
                !DISPATCHER_SAFE_ACTIONS
                    .iter()
                    .any(|(dispatcher, _)| *dispatcher == name),
                "分发器表里出现了不存在的工具名 '{name}'"
            );
        }
    }

    /// 关键词表里的每个工具名、等价簇里的每个成员，都必须是"我们真的认识"的名字。
    ///
    /// 用一张显式的真实名清单做锚点：改工具名时这里会红，逼人回来同步。
    #[test]
    fn keyword_and_cluster_tables_reference_known_tools() {
        // 与本仓库 `tools/builtin/**` 里 `fn name()` 的返回值对齐
        const KNOWN: &[&str] = &[
            "read_file",
            "write_file",
            "edit_file",
            "list_dir",
            "grep_search",
            "run_command",
            "take_screenshot",
            "screenshot_analyze",
            "get_active_window",
            "get_foreground_app_context",
            "media_control",
            "wallpaper_set",
            "wallpaper_control",
            "open_application",
            "close_application",
            "delete_plugin",
            "manage_todo",
            "manage_scheduled",
            "browser_eval_js",
            "browser_task_tab",
        ];
        for (tool, _) in ACTION_KEYWORDS {
            assert!(KNOWN.contains(tool), "关键词表引用了未知工具 '{tool}'");
        }
        for cluster in EQUIVALENCE_CLUSTERS {
            for member in cluster.iter() {
                assert!(KNOWN.contains(member), "等价簇引用了未知工具 '{member}'");
            }
        }
        for name in PRE_APPROVAL_ALLOWED_TOOLS {
            assert!(KNOWN.contains(name), "预授权表引用了未知工具 '{name}'");
        }
    }

    /// 确认名单里的名字都必须能在真实注册表里找到（用夹具表模拟注册表）。
    #[test]
    fn unresolved_names_detects_missing_tools() {
        const REGISTERED: &[&str] = &[
            "read_file",
            "write_file",
            "edit_file",
            "list_dir",
            "grep_search",
            "take_screenshot",
            "screenshot_analyze",
            "delete_plugin",
            "media_control",
            "wallpaper_set",
            "wallpaper_control",
            "manage_todo",
            "manage_scheduled",
            "mcp__browser__eval_js",
        ];
        let missing = unresolved_confirmation_names(|n| REGISTERED.contains(&n));
        assert!(
            missing.is_empty(),
            "确认名单里这些名字在夹具注册表中不存在：{missing:?}"
        );

        // 反向：注册表缺了 manage_todo 时必须被检出
        let missing = unresolved_confirmation_names(|n| n != "manage_todo");
        assert!(missing.contains(&"manage_todo"));
    }

    // ========================================================================
    // 确认档位
    // ========================================================================

    #[test]
    fn dispatcher_tier_uses_a_safe_action_allowlist() {
        // 白名单内的 action 不该弹窗
        assert_eq!(
            confirmation_tier("manage_todo", &json!({"action": "update"})),
            ToolConfirmationTier::NotRequired
        );
        assert_eq!(
            confirmation_tier("manage_todo", &json!({"action": "list"})),
            ToolConfirmationTier::NotRequired
        );
        assert_eq!(
            confirmation_tier("manage_scheduled", &json!({"action": "list"})),
            ToolConfirmationTier::NotRequired
        );
        assert_eq!(
            confirmation_tier("manage_scheduled", &json!({"action": "pause"})),
            ToolConfirmationTier::NotRequired
        );
        // 破坏性 action 必须确认
        assert_eq!(
            confirmation_tier("manage_todo", &json!({"action": "delete"})),
            ToolConfirmationTier::ConfirmAtAction
        );
        assert_eq!(
            confirmation_tier("manage_scheduled", &json!({"action": "cancel"})),
            ToolConfirmationTier::ConfirmAtAction
        );
        // 白名单式判定：缺 action / 未知 action 一律落到"必须确认"，
        // 而不是被当成安全操作放行
        assert_eq!(
            confirmation_tier("manage_todo", &json!({})),
            ToolConfirmationTier::ConfirmAtAction
        );
        assert_eq!(
            confirmation_tier("manage_todo", &json!({"action": "wipe"})),
            ToolConfirmationTier::ConfirmAtAction
        );
        assert_eq!(
            confirmation_tier("manage_todo", &json!({"action": "DELETE"})),
            ToolConfirmationTier::ConfirmAtAction
        );
    }

    #[test]
    fn screen_and_wallpaper_are_pre_approval_not_always_confirm() {
        // 屏幕感知是桌面宠物的核心能力，不该每次都必须弹窗
        assert_eq!(
            confirmation_tier("take_screenshot", &json!({})),
            ToolConfirmationTier::PreApprovalAllowed
        );
        assert_eq!(
            confirmation_tier("wallpaper_set", &json!({})),
            ToolConfirmationTier::PreApprovalAllowed
        );
        // 写文件这类没有"用户已说清目标"语义的，仍是动手前必问
        assert_eq!(
            confirmation_tier("write_file", &json!({})),
            ToolConfirmationTier::ConfirmAtAction
        );
        // 未列入任何表的工具免问
        assert_eq!(
            confirmation_tier("web_search", &json!({})),
            ToolConfirmationTier::NotRequired
        );
    }

    #[test]
    fn mcp_browser_name_and_wire_name_land_on_same_tier() {
        // 名单里填线名，模型侧传 MCP 名，两者必须得出同一档位
        assert_eq!(
            confirmation_tier("browser_eval_js", &json!({})),
            confirmation_tier("mcp__browser__eval_js", &json!({}))
        );
        assert!(is_confirmation_required_tool("mcp__browser__eval_js"));
        assert!(is_confirmation_required_tool("browser_eval_js"));
    }

    // ========================================================================
    // Hand-off：敏感域不许 agent 执行
    // ========================================================================

    #[test]
    fn eval_js_on_payment_page_is_hand_off() {
        for url in [
            "https://www.alipay.com/account",
            "https://paypal.com/checkout",
            "https://www.icbc.com.cn/icbc/",
            "https://www.12306.cn/index/",
            "https://example.com/account/password",
            "https://shop.example.com/checkout/step2",
        ] {
            assert_eq!(
                confirmation_tier("mcp__browser__eval_js", &json!({ "url": url })),
                ToolConfirmationTier::HandOff,
                "{url} 应判定为敏感域 Hand-off"
            );
        }
    }

    /// 敏感域判定不能把正常站点也拦下来——`display` 里含 `play`、
    /// `github.com/bank-notes` 这类假阳性会让功能变得不可用。
    #[test]
    fn sensitive_url_has_no_obvious_false_positives() {
        for url in [
            "https://display.example.com/gallery",
            "https://github.com/rust-lang/rust",
            "https://www.bilibili.com/video/BV1xx",
            "https://play.google.com/store",
            "https://en.wikipedia.org/wiki/Bank",
            "https://doc.rust-lang.org/std/",
        ] {
            assert!(
                !is_sensitive_url(url),
                "{url} 被误判为敏感域（假阳性会让正常浏览也被拦）"
            );
        }
        assert!(!is_sensitive_url(""));
    }

    // ========================================================================
    // 预授权：用户说清目标才免问
    // ========================================================================

    #[test]
    fn pre_approval_requires_explicit_user_authorization() {
        // 用户说了具体目标 → 授权成立
        assert!(user_authorized_target(
            "wallpaper_set",
            &json!({"path": "D:\\pics\\cat.png"}),
            "帮我把壁纸换成这张"
        ));
        assert!(user_authorized_target(
            "take_screenshot",
            &json!({}),
            "看看我在干嘛"
        ));
        assert!(user_authorized_target(
            "media_control",
            &json!({"action": "pause"}),
            "暂停一下"
        ));
        // 英文用户同样拿得到授权（三语关键词都要在表里）
        assert!(user_authorized_target(
            "wallpaper_set",
            &json!({}),
            "change my wallpaper to this one"
        ));
        // 用户没提这件事 → 不授权
        assert!(!user_authorized_target(
            "wallpaper_set",
            &json!({}),
            "今天天气不错啊"
        ));
        // 空消息（主动触发、后台任务）→ 不授权
        assert!(!user_authorized_target("take_screenshot", &json!({}), ""));
        // 用户直接给了路径 → 授权成立
        assert!(user_authorized_target(
            "wallpaper_set",
            &json!({"path": "D:\\pics\\cat.png"}),
            "用 D:\\pics\\cat.png 这张"
        ));
        // 过短的参数值不得造成误命中
        assert!(!user_authorized_target(
            "wallpaper_set",
            &json!({"path": "a"}),
            "随便说点什么 a 都行"
        ));
    }

    #[tokio::test]
    async fn authorized_pre_approval_skips_the_gate_entirely() {
        let authorized = PermissionContextBuilder::new(PermissionMode::Default)
            .with_user_input("帮我换个壁纸")
            .build();
        // 免问 ⇒ 不进权限流程
        assert!(!requires_permission(
            &WallpaperLikeTool,
            &json!({}),
            &authorized
        ));

        let unauthorized = PermissionContextBuilder::new(PermissionMode::Default)
            .with_user_input("今天好累")
            .build();
        assert!(requires_permission(
            &WallpaperLikeTool,
            &json!({}),
            &unauthorized
        ));

        // 没拿到用户原话（主动触发/后台任务）→ 一律照常询问
        let no_input = PermissionContextBuilder::new(PermissionMode::Default).build();
        assert!(requires_permission(
            &WallpaperLikeTool,
            &json!({}),
            &no_input
        ));
    }

    // ========================================================================
    // 等价能力簇：防换工具绕过拒绝
    // ========================================================================

    #[test]
    fn equivalence_cluster_catches_indirect_bypass() {
        // 用户拒绝 write_file 后，模型改用 edit_file / run_command 写同一份文件
        let denied = PermissionContextBuilder::new(PermissionMode::Default)
            .deny("write_file")
            .build();
        assert_eq!(equivalence_conflict("edit_file", &denied), Some("write_file"));
        assert_eq!(
            equivalence_conflict("run_command", &denied),
            Some("write_file")
        );
        // 同簇内被拒的那个自己不算"冲突"（它走 always_deny 分支直接拒绝）
        assert_eq!(equivalence_conflict("write_file", &denied), None);
        // 不同簇不受影响
        assert_eq!(equivalence_conflict("read_file", &denied), None);

        // 读盘簇
        let denied = PermissionContextBuilder::new(PermissionMode::Default)
            .deny("read_file")
            .build();
        assert_eq!(equivalence_conflict("grep_search", &denied), Some("read_file"));
        assert_eq!(equivalence_conflict("list_dir", &denied), Some("read_file"));
    }

    #[tokio::test]
    async fn bypassing_denied_cluster_escalates_to_ask() {
        let denied = PermissionContextBuilder::new(PermissionMode::Default)
            .deny("write_file")
            .build();
        assert!(requires_permission(&EditFileLikeTool, &json!({}), &denied));

        let result = check_tool_permission(
            &EditFileLikeTool,
            &json!({}),
            &ToolUseContext::default(),
            &denied,
        )
        .await;
        assert!(
            result.requires_confirmation(),
            "换用同簇工具必须重新确认，而不是静默放行"
        );
    }

    /// 夹具：名字是 `wallpaper_set` 的"预授权档"工具（避免为测试引入真实工具依赖）
    struct WallpaperLikeTool;

    #[async_trait]
    impl Tool for WallpaperLikeTool {
        fn name(&self) -> &str {
            "wallpaper_set"
        }

        fn description(&self) -> &str {
            "wallpaper test tool"
        }

        fn parameters_schema(&self) -> Value {
            json!({"type": "object"})
        }

        async fn validate_input(
            &self,
            _input: &Value,
            _context: &ToolUseContext,
        ) -> ValidationResult {
            ValidationResult::success(None)
        }

        async fn check_permissions(
            &self,
            _input: &Value,
            _context: &ToolUseContext,
        ) -> PermissionResult {
            // 真实工具自身也会 ask —— 预授权必须能越过这一层
            PermissionResult::ask("wallpaper tool asks")
        }

        async fn call(&self, _args: Value, _context: &ToolUseContext) -> ToolResult {
            ToolResult::success(json!({}))
        }

        fn is_read_only(&self) -> bool {
            false
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::System
        }

        fn risk(&self) -> ToolRiskTier {
            ToolRiskTier::Safe
        }
    }

    /// 夹具：`edit_file`，与 `write_file` 同属"写盘"等价簇
    struct EditFileLikeTool;

    #[async_trait]
    impl Tool for EditFileLikeTool {
        fn name(&self) -> &str {
            "edit_file"
        }

        fn description(&self) -> &str {
            "edit file test tool"
        }

        fn parameters_schema(&self) -> Value {
            json!({"type": "object"})
        }

        async fn validate_input(
            &self,
            _input: &Value,
            _context: &ToolUseContext,
        ) -> ValidationResult {
            ValidationResult::success(None)
        }

        async fn check_permissions(
            &self,
            _input: &Value,
            _context: &ToolUseContext,
        ) -> PermissionResult {
            PermissionResult::allow()
        }

        async fn call(&self, _args: Value, _context: &ToolUseContext) -> ToolResult {
            ToolResult::success(json!({}))
        }

        fn is_read_only(&self) -> bool {
            false
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::System
        }

        fn risk(&self) -> ToolRiskTier {
            ToolRiskTier::Safe
        }
    }
}
