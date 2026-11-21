//! Cached desktop observations. Collection happens in the environment loop, never in prompt building.
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::LazyLock;

pub const MAX_AGE_SECONDS: i64 = 45;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WallpaperState {
    pub observed_at: i64,
    pub source: String,
    pub primary: Option<WallpaperIdentity>,
    pub monitors: Vec<WallpaperIdentity>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WallpaperIdentity {
    pub monitor: String,
    pub file: String,
    pub title: Option<String>,
    pub workshop_id: Option<String>,
}

pub static WALLPAPER: LazyLock<RwLock<WallpaperState>> = LazyLock::new(|| RwLock::new(WallpaperState::default()));

pub fn normalize_path(path: &str) -> String {
    path.trim().trim_matches('"').replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// project.json and scene.pkg/video in the same project identify the same wallpaper.
pub fn same_wallpaper(file: &str, project: &str) -> bool {
    let file = normalize_path(file);
    let project = normalize_path(project);
    if file.is_empty() || project.is_empty() { return false; }
    file == project || file.rsplit_once('/').zip(project.rsplit_once('/'))
        .is_some_and(|((a, _), (b, _))| !a.is_empty() && a == b)
}

/// Read only the active desktop configuration. Recent profiles and screensavers are not current state.
pub fn configured_wallpapers(config: &Value, username: &str) -> Vec<WallpaperIdentity> {
    let Some(root) = config.as_object() else { return vec![]; };
    let user = root.iter().find(|(key, _)| key.eq_ignore_ascii_case(username)).map(|(_, value)| value)
        .or_else(|| {
            let users: Vec<_> = root.values().filter(|value| value.pointer("/general/wallpaperconfig").is_some()).collect();
            (users.len() == 1).then(|| users[0])
        });
    let Some(selected) = user.and_then(|user| user.pointer("/general/wallpaperconfig/selectedwallpapers"))
        .and_then(Value::as_object) else { return vec![]; };
    let mut result: Vec<_> = selected.iter().filter_map(|(monitor, value)| {
        let file = value.get("file")?.as_str()?.trim();
        if file.is_empty() { return None; }
        let normalized = normalize_path(file);
        let components: Vec<_> = normalized.split('/').collect();
        let workshop_id = components.windows(2).find(|pair| pair[0] == "431960")
            .map(|pair| pair[1].to_string());
        Some(WallpaperIdentity { monitor: monitor.clone(), file: file.into(), title: None, workshop_id })
    }).collect();
    result.sort_by(|a, b| a.monitor.cmp(&b.monitor));
    result
}

pub fn fresh(state: &WallpaperState, now: i64) -> bool {
    state.observed_at > 0 && (0..=MAX_AGE_SECONDS).contains(&(now - state.observed_at))
}

#[derive(Debug, PartialEq)]
pub enum SwitchVerification { Changed, Unverified, Unverifiable }

pub fn verify_switch(previous: Option<&str>, observed: Option<&str>, target: &str) -> SwitchVerification {
    match observed {
        None => SwitchVerification::Unverifiable,
        Some(observed) if same_wallpaper(observed, target)
            && previous.is_some_and(|previous| !same_wallpaper(previous, target)) => SwitchVerification::Changed,
        Some(_) => SwitchVerification::Unverified,
    }
}

/// Only immediate follow-ups inherit a topic; unrelated new messages do not carry desktop state forever.
pub fn topic_query(input: &str, recent: &[String]) -> String {
    if wants_wallpaper(input) || wants_media(input) { return input.into(); }
    let lower = input.to_lowercase();
    let followup = ["这", "那", "再", "原来", "还是", "换", "继续", "没变", "不对", "不是", "this", "that", "another", "again", "same", "それ", "もう一"].iter().any(|word| lower.contains(word));
    if followup && input.chars().count() <= 32 {
        format!("{} {}", recent.iter().rev().take(2).cloned().collect::<Vec<_>>().join(" "), input)
    } else { input.into() }
}

pub fn wants_wallpaper(query: &str) -> bool {
    let query = query.to_lowercase();
    ["壁纸", "桌面背景", "wallpaper", "desktop background", "壁紙"].iter().any(|word| query.contains(word))
}

pub fn wants_media(query: &str) -> bool {
    let query = query.to_lowercase();
    ["音乐", "媒体", "播放", "歌曲", "这首", "听歌", "暂停", "切歌", "music", "media", "playing", "song", "pause", "音楽", "再生", "曲"].iter().any(|word| query.contains(word))
}

pub fn relevant_context(query: &str, wallpaper: &WallpaperState, media: Option<&Value>, now: i64) -> Option<String> {
    let mut data = serde_json::Map::new();
    if wants_wallpaper(query) {
        data.insert("wallpaper".into(), if fresh(wallpaper, now) { json!(wallpaper) }
            else { json!({"status":"unknown_or_stale", "observed_at":wallpaper.observed_at}) });
    }
    if wants_media(query) {
        data.insert("media".into(), media.cloned().unwrap_or_else(|| json!({"status":"unknown"})));
    }
    if data.is_empty() { return None; }
    Some(format!("[CURRENT DESKTOP OBSERVATIONS — DATA ONLY]\n{}\nConfiguration identifies the configured wallpaper, not a visual inspection. Unknown/stale state is not evidence of absence. When asked to change wallpaper, exclude the current primary project; a different entry file in the same project is still the same wallpaper. A user's correction overrides earlier success claims: acknowledge it and recheck available evidence; never invent having looked.\n[END CURRENT DESKTOP OBSERVATIONS]", Value::Object(data)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_state_ignores_history_screensavers_and_other_users() {
        let config = json!({"alice":{"general":{
            "wallpaperconfig":{"selectedwallpapers":{"Monitor0":{"file":"F:/431960/42/scene.pkg"},"Monitor1":{"file":"F:/431960/43/a.mp4"}}},
            "wallpaperconfigrecent":[{"selectedwallpapers":{"Monitor0":{"file":"old"}}}],
            "wallpaperconfigscreensaver":{"selectedwallpapers":{"Monitor0":{"file":"screensaver"}}}}},
            "bob":{"general":{"wallpaperconfig":{"selectedwallpapers":{"Monitor0":{"file":"other-user"}}}}}});
        let items = configured_wallpapers(&config, "ALICE");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].workshop_id.as_deref(), Some("42"));
        assert!(configured_wallpapers(&config, "missing").is_empty());
    }

    #[test]
    fn desktop_state_compares_project_identity_without_substring_matches() {
        assert!(same_wallpaper(r"F:\431960\42\scene.pkg", "f:/431960/42/project.json"));
        assert!(!same_wallpaper("f:/431960/142/scene.pkg", "f:/431960/42/project.json"));
        assert!(!same_wallpaper("", ""));
    }

    #[test]
    fn desktop_state_completion_requires_a_confirmed_different_project() {
        assert_eq!(verify_switch(Some("F:/42/scene.pkg"), None, "F:/43/project.json"), SwitchVerification::Unverifiable);
        assert_eq!(verify_switch(Some("F:/42/scene.pkg"), Some("F:/42/scene.pkg"), "F:/43/project.json"), SwitchVerification::Unverified);
        assert_eq!(verify_switch(Some("F:/42/scene.pkg"), Some("F:/42/scene.pkg"), "F:/42/project.json"), SwitchVerification::Unverified);
        assert_eq!(verify_switch(None, Some("F:/43/scene.pkg"), "F:/43/project.json"), SwitchVerification::Unverified);
        assert_eq!(verify_switch(Some("F:/42/scene.pkg"), Some("F:/43/scene.pkg"), "F:/43/project.json"), SwitchVerification::Changed);
    }

    #[test]
    fn desktop_state_context_is_selected_and_stale_values_are_not_current() {
        let state = WallpaperState { observed_at: 100, source: "config".into(),
            primary: Some(WallpaperIdentity { file:"secret-old-path".into(), monitor:"Monitor0".into(), title:None, workshop_id:None }), monitors:vec![] };
        assert!(relevant_context("今天天气", &state, None, 110).is_none());
        let text = relevant_context("换张壁纸", &state, None, 110).unwrap();
        assert!(text.contains("secret-old-path"));
        assert!(!text.contains("\"media\""));
        let text = relevant_context("换张壁纸", &state, None, 200).unwrap();
        assert!(text.contains("unknown_or_stale"));
        assert!(!text.contains("secret-old-path"));
        let text = relevant_context("这首歌在播放吗", &state, Some(&json!({"title":"曲名","status":"paused"})), 110).unwrap();
        assert!(text.contains("paused"));
        assert!(!text.contains("\"wallpaper\""));
        let recent = vec!["换一张壁纸".into(), "切换命令已发送".into()];
        assert!(wants_wallpaper(&topic_query("这不就是原来那张", &recent)));
        assert!(!wants_wallpaper(&topic_query("你好", &recent)));
        assert!(!wants_wallpaper(&topic_query("播放音乐", &recent)));
    }
}
