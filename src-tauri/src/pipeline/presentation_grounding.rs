//! A reference to wallpaper choices must contain real choices, not stage directions.
use serde_json::Value;

pub fn ground_wallpaper_outcome(text: &str, receipts: &[Value]) -> String {
    let Some(receipt) = receipts.iter().rev().find(|receipt|
        matches!(receipt.get("tool_name").or_else(|| receipt.get("tool")).and_then(Value::as_str), Some("wallpaper_set" | "wallpaper_control"))) else { return text.into(); };
    let result = &receipt["result"];
    let data = result.get("data").unwrap_or(result);
    let unsupported = data.get("changed") == Some(&Value::Bool(false))
        || matches!(data["verification_status"].as_str(), Some("unverified" | "unverifiable"));
    let same = receipt["error"].as_str() == Some("AlreadyCurrentWallpaper")
        || result["error"].as_str() == Some("AlreadyCurrentWallpaper");
    let lower = text.to_lowercase();
    let claims_success = ["搞定", "换好", "换上了", "已切换", "切换成功", "done", "changed your wallpaper", "switched"]
        .iter().any(|claim| lower.contains(claim));
    if !((unsupported || same) && claims_success) { return text.into(); }
    // Remove only the sentences that assert the unverified change; keep the rest of
    // the character's wording so a failed tool call does not erase her voice.
    let claims = ["搞定", "换好", "换上了", "已切换", "切换成功", "done", "changed your wallpaper", "switched"];
    let kept: String = text
        .split_inclusive(['。', '！', '？', '!', '?', '\n'])
        .filter(|sentence| {
            let sentence = sentence.to_lowercase();
            !claims.iter().any(|claim| sentence.contains(claim))
        })
        .collect();
    let kept = kept.trim();
    if kept.is_empty() {
        if same { "还是原来那张。" } else { "这边还没确认到真的换上了。" }.into()
    } else {
        kept.to_string()
    }
}

pub fn ground_wallpaper_choices(text: &str, receipts: &[Value], current_project: Option<&str>) -> String {
    static STAGE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(
        r"[（(][^（）()\n]*(?:呈现|展示|递给|拿出)[^（）()\n]*(?:壁纸|列表|图片|候选)[^（）()\n]*[）)]|[（(][^（）()\n]*(?:壁纸|列表|图片|候选)[^（）()\n]*(?:呈现|展示|递给)[^（）()\n]*[）)]"
    ).unwrap());
    let cleaned = STAGE.replace_all(text, "").trim().to_string();
    let wallpaper_topic = text.contains("壁纸") || text.contains("壁紙") || text.to_lowercase().contains("wallpaper")
        || receipts.iter().any(|receipt| receipt.get("tool_name").or_else(|| receipt.get("tool")).and_then(Value::as_str) == Some("wallpaper_list"));
    if !wallpaper_topic { return cleaned; }
    let references = ["这几张", "这几款", "下面几张", "以下几张", "these wallpapers", "these options", "この壁紙たち"];
    if !references.iter().any(|reference| cleaned.contains(reference)) { return cleaned; }
    // The most recent successful list is the available evidence, never model-authored tool text.
    let list = receipts.iter().rev().filter(|receipt| receipt["success"] == true
        && receipt.get("tool_name").or_else(|| receipt.get("tool")).and_then(Value::as_str) == Some("wallpaper_list"))
        .find_map(|receipt| {
            let result = receipt.get("result")?;
            result.pointer("/data/wallpapers").or_else(|| result.get("wallpapers")).and_then(Value::as_array)
        });
    let candidates: Vec<_> = list.into_iter().flatten().filter(|item| item["is_current"] != true)
        .filter(|item| !current_project.is_some_and(|project| item.get("folder_path").and_then(Value::as_str)
            .is_some_and(|folder| same_project_folder(project, folder)) || item.get("workshop_id").and_then(Value::as_str)
                .is_some_and(|id| project.replace('\\', "/").rsplit_once('/').is_some_and(|(folder, _)| folder.ends_with(&format!("/431960/{id}"))))))
        .filter_map(|item| item.get("title").and_then(Value::as_str)).filter(|title| !title.trim().is_empty()).collect();
    if candidates.iter().any(|title| cleaned.contains(*title)) { return cleaned; }
    if candidates.is_empty() {
        // No evidence: remove the unsupported invitation instead of inventing options.
        return cleaned.split_inclusive(['。', '！', '？', '!', '?', '\n']).filter(|sentence|
            !references.iter().any(|reference| sentence.contains(reference))).collect::<String>().trim().to_string()
            + "我还没有展示可选壁纸。";
    }
    let names = candidates.iter().take(3).map(|title| format!("《{}》", title.replace(['\n', '\r'], " ")))
        .collect::<Vec<_>>().join("、");
    let mut result = cleaned;
    for reference in references { result = result.replace(reference, &names); }
    result
}

fn same_project_folder(project: &str, folder: &str) -> bool {
    let project = project.replace('\\', "/").to_lowercase();
    let folder = folder.replace('\\', "/").trim_end_matches('/').to_lowercase();
    project.rsplit_once('/').is_some_and(|(parent, _)| parent == folder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn wallpaper_offer_contains_real_names_and_excludes_current() {
        let receipts = vec![json!({"tool_name":"wallpaper_list","success":true,"result":{"data":{"wallpapers":[
            {"title":"原壁纸","folder_path":"F:/42"}, {"title":"海边","folder_path":"F:/43"}, {"title":"星空","folder_path":"F:/44"}]}}})];
        let reply = ground_wallpaper_choices("比如这几张，你有喜欢的吗？（将壁纸列表呈现在你面前）", &receipts, Some("f:/42/scene.pkg"));
        assert_eq!(reply, "比如《海边》、《星空》，你有喜欢的吗？");
    }
    #[test]
    fn wallpaper_offer_cannot_invent_evidence_from_failed_list() {
        let receipts = vec![json!({"tool_name":"wallpaper_list","success":false,"result":{"wallpapers":[{"title":"不可信"}]}})];
        let reply = ground_wallpaper_choices("这几张喜欢吗？（展示壁纸列表）", &receipts, None);
        assert!(!reply.contains("不可信"));
        assert!(!reply.contains("这几张"));
        assert!(!reply.contains("（"));
        assert!(reply.contains("还没有展示"));
    }
    #[test]
    fn real_named_offer_and_ordinary_conversation_stay_intact() {
        assert_eq!(ground_wallpaper_choices("我喜欢海边。", &[], None), "我喜欢海边。");
        assert_eq!(ground_wallpaper_choices("这几张照片真好看。", &[], None), "这几张照片真好看。");
        let receipts = vec![json!({"tool_name":"wallpaper_list","success":true,"result":{"wallpapers":[{"title":"海边"}]}})];
        assert_eq!(ground_wallpaper_choices("这几张里《海边》怎么样？", &receipts, None), "这几张里《海边》怎么样？");
    }

    #[test]
    fn sent_command_and_same_wallpaper_do_not_support_a_success_claim() {
        let pending = vec![json!({"tool_name":"wallpaper_set","success":true,"result":{"data":{"changed":false,"verification_status":"unverifiable"}}})];
        assert_eq!(ground_wallpaper_outcome("搞定啦！已经换好了。", &pending), "这边还没确认到真的换上了。");
        // Only the claim is removed; the character keeps the rest of her line.
        assert_eq!(ground_wallpaper_outcome("我试着切了。搞定了。", &pending), "我试着切了。");
        let same = vec![json!({"tool_name":"wallpaper_set","success":false,"error":"AlreadyCurrentWallpaper"})];
        assert_eq!(ground_wallpaper_outcome("切换成功。", &same), "还是原来那张。");
        let verified = vec![json!({"tool_name":"wallpaper_set","success":true,"result":{"data":{"changed":true,"verification_status":"verified_configuration"}}})];
        assert_eq!(ground_wallpaper_outcome("已经换好了。", &verified), "已经换好了。");
    }
}
