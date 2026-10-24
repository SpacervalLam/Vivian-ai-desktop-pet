//! Delivery reminder for an explicit reference-image review phase. Tool success
//! is not evidence that a usable image was found, viewed or shown to the user.
pub struct Observation<'a> {
    pub tool: &'a str,
    pub success: bool,
    pub has_pixels: bool,
}

pub fn delivery_hint(goal: &str, observations: &[Observation<'_>]) -> Option<String> {
    let goal = goal.to_lowercase();
    let visual = ["图片", "参考图", "形象", "图像", "image", "picture", "reference art", "画像", "参考絵"]
        .iter().any(|word| goal.contains(word));
    let review = ["确认", "先给", "先看", "confirm", "approve", "show me", "確認"]
        .iter().any(|word| goal.contains(word));
    if !visual || !review || observations.len() < 8 { return None; }
    if observations.iter().any(|o| o.success && o.tool == "send_image") { return None; }
    if observations.iter().any(|o| o.success && o.has_pixels) {
        return Some("[系统提示] 你已读取参考图的实际像素，但还没有通过 send_image 向用户展示。先展示已核对的候选图，附来源页和仍有疑问的部分；用户要求确认后继续时，在此等待确认。不要用脚本文件或文字设定代替图片交付。".into());
    }
    Some("[系统提示] 当前阶段要求先给用户看参考图，但多次工具调用后还没有读取或展示实际图片。脚本写入成功、命令退出码 0、HTTP 200 都不代表取得图片；零张下载、挑战页和超时仍是证据缺口。回到已有搜索结果，优先读取覆盖多个目标的合集、作者页或素材仓库；用 read_content 的 images[] 获取候选地址，再 read_content 图片 URL 看图、send_image(artifact.image_path) 展示并附出处。必要时补充定向 web_search。单站抓取失败后换来源，不继续调试同一站的 UA/接口/脚本；权限拒绝不可绕过。能确认多少先展示多少并说明缺口；若确实无法取图，如实报告。用户确认前不要进入后续制作。".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    const GOAL: &str = "制作游戏，先搜索二创形象，先给出图片，让我确认形象正确后再继续";
    fn observation(tool: &str, success: bool) -> Observation<'_> {
        Observation { tool, success, has_pixels: false }
    }
    #[test]
    fn successful_scraping_scripts_do_not_reset_missing_images() {
        // Replay the failure pattern: search -> text without images -> shell
        // failures interleaved with successful writes and empty downloads.
        let calls = vec![observation("web_search", true), observation("read_content", false),
            observation("read_content", true), observation("run_command", false),
            observation("run_command", true), observation("write_file", true),
            observation("run_command", true), observation("run_command", true)];
        let hint = delivery_hint(GOAL, &calls).unwrap();
        assert!(hint.contains("images[]") && hint.contains("换来源"));
    }
    #[test]
    fn viewed_is_not_presented_and_failed_send_is_not_delivery() {
        let mut calls: Vec<_> = (0..8).map(|_| observation("run_command", true)).collect();
        calls.push(Observation { tool: "read_content", success: true, has_pixels: true });
        assert!(delivery_hint(GOAL, &calls).unwrap().contains("已读取参考图"));
        calls.push(observation("send_image", false));
        assert!(delivery_hint(GOAL, &calls).is_some());
        calls.push(observation("send_image", true));
        assert!(delivery_hint(GOAL, &calls).is_none());
    }
    #[test]
    fn does_not_interrupt_early_work_or_other_tasks() {
        let calls: Vec<_> = (0..8).map(|_| observation("web_search", true)).collect();
        assert!(delivery_hint(GOAL, &calls[..7]).is_none());
        assert!(delivery_hint("研究图片格式的历史并写一份报告", &calls).is_none());
        assert!(delivery_hint("先给我确认数据库迁移计划", &calls).is_none());
        assert!(delivery_hint("Search reference images and show me before building", &calls).is_some());
    }
    #[test]
    fn recorded_failed_session_gets_a_reminder_before_more_scraper_variants() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!("../../../tests/fixtures/reference-research-regression.json")).unwrap();
        let observations: Vec<_> = fixture["observations"].as_array().unwrap().iter().map(|o| Observation {
            tool: o["tool"].as_str().unwrap(), success: o["success"].as_bool().unwrap(), has_pixels: o["has_pixels"].as_bool().unwrap(),
        }).collect();
        assert_eq!(observations.len(), 20);
        let goal = fixture["goal"].as_str().unwrap();
        let first = (1..=observations.len()).find(|n| delivery_hint(goal, &observations[..*n]).is_some());
        assert_eq!(first, Some(8));
        assert!(delivery_hint(goal, &observations).is_some());
    }
}
