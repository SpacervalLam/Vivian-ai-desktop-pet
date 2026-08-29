//! 视频动画演出层 — 命令 + 行为自动触发
//!
//! 素材（51 个透明动画 webm）暂未导入，本模块先完善架构：
//! - 目录（catalog）定义动画 id / 素材文件名 / 对应活动标签；素材放入
//!   `public/video-animations/` 后即自动生效（dev 由 vite 提供，prod 由 asset 协议提供）
//! - 命令 `play_video_animation` / `stop_video_animation` / `list_video_animations`
//! - 行为自动触发 `video_animation_auto_tick`：按当前活动标签匹配目录，带概率 + 冷却
//!
//! 前端 `VideoAnimationLayer` 监听 `video:animation` / `video:animation:stop` 事件播放/停止，
//! 缺失素材时优雅跳过（前端 error 事件兜底），不影响既有 Live2D 渲染。

use std::collections::HashMap;
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

use crate::state::AppState;

/// 单个动画定义
#[derive(Debug, Clone, Serialize)]
pub struct VideoAnimationDef {
    /// 唯一 id（命令与事件使用）
    pub id: &'static str,
    /// 素材文件名（`public/video-animations/<file>`）
    pub file: &'static str,
    /// 触发该动画的活动标签（匹配 ActivityJournal.activity_label，子串匹配）
    pub activity_labels: &'static [&'static str],
}

/// 动画目录。
///
/// 素材文件名约定为 `public/video-animations/*.webm`（导入时直接按名放置即可）。
/// `activity_labels` 为常见活动的匹配标签，素材未就位时 auto_tick 不会命中。
pub const CATALOG: &[VideoAnimationDef] = &[
    // ---- 待机 / 转向 ----
    VideoAnimationDef { id: "idle", file: "待机呼吸休闲.webm", activity_labels: &["idle", "桌面", "待机"] },
    VideoAnimationDef { id: "look_around", file: "东张西望.webm", activity_labels: &[] },
    // ---- 移动 ----
    VideoAnimationDef { id: "walk", file: "螃蟹走路.webm", activity_labels: &[] },
    VideoAnimationDef { id: "hover_step", file: "原地漂浮踏步.webm", activity_labels: &[] },
    VideoAnimationDef { id: "run", file: "原地左转奔跑.webm", activity_labels: &["运动", "跑步"] },
    // ---- 日常动作（与用户活动/角色行为相关）----
    VideoAnimationDef { id: "write_code", file: "写代码.webm", activity_labels: &["代码", "coding", "编程", "开发", "ide"] },
    VideoAnimationDef { id: "eat_rice", file: "吃白饭.webm", activity_labels: &["吃", "eat", "饮食", "进餐"] },
    VideoAnimationDef { id: "eat_snack", file: "大口吃零食.webm", activity_labels: &["零食", "零食"] },
    VideoAnimationDef { id: "play_game", file: "玩游戏气急败坏.webm", activity_labels: &["游戏", "game", "gaming", "steam"] },
    VideoAnimationDef { id: "hum_song", file: "悠闲哼歌.webm", activity_labels: &["音乐", "music", "听歌", "播放器"] },
    VideoAnimationDef { id: "play_violin", file: "小提琴演奏.webm", activity_labels: &["乐器", "小提琴"] },
    VideoAnimationDef { id: "read", file: "深度思考碎碎念.webm", activity_labels: &["阅读", "read", "文档", "book"] },
    VideoAnimationDef { id: "write_note", file: "轻快记录.webm", activity_labels: &["笔记", "写作", "write", "note"] },
    VideoAnimationDef { id: "dance", file: "可爱宅舞.webm", activity_labels: &["舞蹈", "dance"] },
    VideoAnimationDef { id: "mirror", file: "照镜子.webm", activity_labels: &["化妆", "试衣", "美妆"] },
    VideoAnimationDef { id: "water_gun", file: "玩水枪.webm", activity_labels: &[] },
    // ---- 点击 / 拖拽回应 ----
    VideoAnimationDef { id: "click_happy", file: "点击回应 - 开心跃动.webm", activity_labels: &[] },
    VideoAnimationDef { id: "click_shy", file: "点击回应 - 害羞惊讶.webm", activity_labels: &[] },
    VideoAnimationDef { id: "click_tsundere", file: "点击回应 - 傲娇生气（侧身展示）.webm", activity_labels: &[] },
    VideoAnimationDef { id: "drag_feedback", file: "被鼠标拖拽悬空反馈.webm", activity_labels: &[] },
    // ---- 其余动作素材（占位，行为暂不映射）----
    VideoAnimationDef { id: "stretch", file: "超大伸懒腰.webm", activity_labels: &["伸懒腰"] },
    VideoAnimationDef { id: "yawn", file: "哈欠连天.webm", activity_labels: &["困", "打哈欠"] },
    VideoAnimationDef { id: "sleep", file: "原地小憩沉眠.webm", activity_labels: &["睡眠", "sleep", "休息"] },
    VideoAnimationDef { id: "rubik", file: "原地专心玩魔方.webm", activity_labels: &["魔方"] },
    VideoAnimationDef { id: "desk_interact", file: "原地敲击桌面互动.webm", activity_labels: &[] },
    VideoAnimationDef { id: "squat", file: "原地重力下蹲压缩.webm", activity_labels: &[] },
    VideoAnimationDef { id: "toy_car", file: "原地蹲下玩玩具汽车.webm", activity_labels: &[] },
    VideoAnimationDef { id: "bubble", file: "鲸鱼吐泡泡特效.webm", activity_labels: &[] },
    VideoAnimationDef { id: "curtsy", file: "女仆屈膝礼仪.webm", activity_labels: &[] },
    VideoAnimationDef { id: "scared", file: "被吓一跳（炸毛）.webm", activity_labels: &[] },
    VideoAnimationDef { id: "jump_catch", file: "原地跳跃抓碎头顶物品.webm", activity_labels: &[] },
    VideoAnimationDef { id: "spin360", file: "小幅度原地 360 度旋转展示.webm", activity_labels: &[] },
    VideoAnimationDef { id: "snack_caught", file: "偷吃零食被抓住.webm", activity_labels: &[] },
    VideoAnimationDef { id: "tail_slap", file: "用鲸鱼尾巴拍打地面.webm", activity_labels: &[] },
    VideoAnimationDef { id: "woken_up", file: "打瞌睡被惊醒.webm", activity_labels: &["惊醒"] },
    VideoAnimationDef { id: "whale", file: "蓝鲸现世.webm", activity_labels: &[] },
    VideoAnimationDef { id: "dress_up", file: "整体换装试色.webm", activity_labels: &["换装"] },
    VideoAnimationDef { id: "balloon", file: "吹气球.webm", activity_labels: &["气球"] },
    VideoAnimationDef { id: "animals", file: "动物环绕.webm", activity_labels: &["宠物", "动物"] },
    VideoAnimationDef { id: "eat_token", file: "吃Token.webm", activity_labels: &["token"] },
    VideoAnimationDef { id: "eat_breakfast", file: "吃早餐.webm", activity_labels: &["早餐"] },
    VideoAnimationDef { id: "eat_lunch", file: "吃午餐.webm", activity_labels: &["午餐"] },
    VideoAnimationDef { id: "eat_dinner", file: "吃晚餐.webm", activity_labels: &["晚餐"] },
    VideoAnimationDef { id: "kite", file: "放风筝.webm", activity_labels: &["风筝"] },
    VideoAnimationDef { id: "fan", file: "摇扇纳凉.webm", activity_labels: &["扇子"] },
    VideoAnimationDef { id: "icecream", file: "吃冰淇淋融化.webm", activity_labels: &["冰淇淋"] },
    VideoAnimationDef { id: "leaves", file: "被落叶淹没.webm", activity_labels: &[] },
    VideoAnimationDef { id: "mooncake", file: "中秋赏月吃月饼.webm", activity_labels: &["中秋", "月饼"] },
    VideoAnimationDef { id: "snowman", file: "堆雪人.webm", activity_labels: &["雪人", "雪"] },
    VideoAnimationDef { id: "ballroom", file: "优雅女仆舞.webm", activity_labels: &[] },
    VideoAnimationDef { id: "swing", file: "轻快摇摆舞.webm", activity_labels: &[] },
    VideoAnimationDef { id: "flutter", file: "原地漂浮踏步.webm", activity_labels: &[] },
];

/// 按 id 或文件名（去 .webm 后缀）查找动画
pub fn find_by_name(name: &str) -> Option<&'static VideoAnimationDef> {
    let trimmed = name.trim_end_matches(".webm");
    CATALOG
        .iter()
        .find(|d| d.id == name || d.id == trimmed || d.file == name || d.file.trim_end_matches(".webm") == trimmed)
}

/// 按活动标签（子串匹配，双向）查找动画
fn find_by_activity(label: &str) -> Option<&'static VideoAnimationDef> {
    let l = label.to_lowercase();
    CATALOG.iter().find(|d| {
        d.activity_labels
            .iter()
            .any(|a| l.contains(&a.to_lowercase()) || a.to_lowercase().contains(&l))
    })
}

/// 播放视频动画
///
/// 校验目录后向目标角色窗口 emit `video:animation` 事件，由前端播放层消费。
#[tauri::command]
pub fn play_video_animation(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    name: String,
    character_id: Option<String>,
) -> Result<Value, String> {
    let def = find_by_name(&name).ok_or_else(|| format!("未知视频动画: {}", name))?;
    let char_id = character_id
        .clone()
        .unwrap_or_else(|| state.active_character_id.read().clone());
    let _ = app.emit_to(
        &char_id,
        "video:animation",
        json!({
            "character_id": &char_id,
            "name": def.id,
            "file": def.file,
        }),
    );
    tracing::info!("[video_animation] play {}({}) -> {}", def.id, def.file, char_id);
    Ok(json!({ "played": true, "name": def.id, "file": def.file }))
}

/// 停止当前视频动画（恢复 Live2D 本体）
#[tauri::command]
pub fn stop_video_animation(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    character_id: Option<String>,
) -> Result<(), String> {
    let char_id = character_id
        .unwrap_or_else(|| state.active_character_id.read().clone());
    let _ = app.emit_to(
        &char_id,
        "video:animation:stop",
        json!({ "character_id": &char_id }),
    );
    Ok(())
}

/// 列出全部可用视频动画（供调试/配置界面展示）
#[tauri::command]
pub fn list_video_animations() -> Result<Vec<VideoAnimationDef>, String> {
    Ok(CATALOG.to_vec())
}

/// 行为自动触发冷却表（按角色）
static LAST_TRIGGER: Lazy<RwLock<HashMap<String, i64>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// 行为自动触发 tick — 由前端主动 tick 循环周期调用。
///
/// 触发条件（全部满足）：
/// 1. 配置 `video_animations.enabled = true`
/// 2. 距上次触发超过冷却（`video_animations.cooldown_secs`，下限 60s）
/// 3. 当前活动标签命中目录映射（ActivityJournal.latest_classification）
/// 4. 概率门控（每次 tick 15%），避免过于频繁
///
/// 返回 `{ played, name, reason }`，供调试与前端日志。
#[tauri::command]
pub fn video_animation_auto_tick(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    character_id: Option<String>,
) -> Result<Value, String> {
    // 1. 配置开关
    let config = state.config.read().get_all();
    if !config.video_animations.enabled {
        return Ok(json!({ "played": false, "reason": "disabled" }));
    }

    let char_id = character_id
        .clone()
        .unwrap_or_else(|| state.active_character_id.read().clone());

    // 用户正在对话进行中：跳过，避免视频演出打断正式对话
    if crate::conversation::CONVERSATION_MANAGER.is_any_user_session_active() {
        return Ok(json!({ "played": false, "reason": "chatting" }));
    }

    // 2. 冷却
    let now = chrono::Utc::now().timestamp();
    let cooldown = config.video_animations.cooldown_secs.max(60) as i64;
    let last = LAST_TRIGGER.read().get(&char_id).copied().unwrap_or(0);
    if now - last < cooldown {
        return Ok(json!({ "played": false, "reason": "cooldown" }));
    }

    // 3. 当前活动标签
    let instance = match state.get_character(character_id.as_deref()) {
        Ok(inst) => inst,
        Err(_) => return Ok(json!({ "played": false, "reason": "no_character" })),
    };
    let activity_label = instance
        .brain
        .proactive
        .activity_journal()
        .latest_classification()
        .map(|(label, _)| label);
    let Some(label) = activity_label else {
        return Ok(json!({ "played": false, "reason": "no_activity" }));
    };

    // 4. 目录匹配 + 概率门控
    let Some(def) = find_by_activity(&label) else {
        return Ok(json!({ "played": false, "reason": "no_match" }));
    };
    if rand::random::<f64>() > 0.15 {
        return Ok(json!({ "played": false, "reason": "probability" }));
    }

    // 触发：记录时间戳 + 发事件
    LAST_TRIGGER.write().insert(char_id.clone(), now);
    let _ = app.emit_to(
        &char_id,
        "video:animation",
        json!({
            "character_id": &char_id,
            "name": def.id,
            "file": def.file,
        }),
    );
    tracing::info!(
        "[video_animation] auto trigger {}({}) for {} (activity={})",
        def.id,
        def.file,
        char_id,
        label
    );
    Ok(json!({ "played": true, "name": def.id, "file": def.file, "activity": label }))
}
