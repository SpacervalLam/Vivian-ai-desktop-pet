//! Reuses Scheduler's one-second heartbeat and the shared cached world observations.
use std::sync::{Arc, LazyLock};
use std::sync::atomic::{AtomicBool, Ordering};
use chrono::Timelike;
use parking_lot::Mutex;
use serde_json::json;
use tauri::{Emitter, Manager};
use crate::companion_policy::{ClipboardChanges, HourlyClock, PressureState};
use crate::state::AppState;

#[derive(Default)]
struct FeedbackState {
    clock: HourlyClock, cpu: PressureState, memory: PressureState,
    network: Option<bool>, last_pressure_notice: Option<i64>, last_network_notice: Option<i64>,
    clipboard: ClipboardChanges, rain: Option<bool>, song: Option<String>, music_initialized: bool,
    last_weather_notice: Option<i64>, last_music_notice: Option<i64>,
}
static FEEDBACK: LazyLock<Mutex<FeedbackState>> = LazyLock::new(Default::default);
static RESOURCE_FEEDBACK: AtomicBool = AtomicBool::new(false);
static MUSIC_FEEDBACK: AtomicBool = AtomicBool::new(false);
pub fn resource_feedback_enabled() -> bool { RESOURCE_FEEDBACK.load(Ordering::Relaxed) }
pub fn music_feedback_enabled() -> bool { MUSIC_FEEDBACK.load(Ordering::Relaxed) }

pub fn quiet_reason(state: &AppState) -> Option<&'static str> {
    if crate::companion_quiet::active() { return Some("do_not_disturb"); }
    let config = state.config.read().get_all().companion;
    let (process, _, _) = state.world_provider.companion_observation();
    config.quiet_reason(chrono::Local::now().hour(), &process)
}

pub fn tick(app: &tauri::AppHandle) {
    let state = app.state::<Arc<AppState>>();
    if !state.initialized.load(Ordering::Relaxed) || state.is_factory_reset_in_progress() { return; }
    let cfg = state.config.read().get_all();
    RESOURCE_FEEDBACK.store(cfg.companion.resource_feedback && cfg.proactive.enable_system_pressure_trigger, Ordering::Relaxed);
    MUSIC_FEEDBACK.store(cfg.companion.music_feedback, Ordering::Relaxed);
    // Detect changes independently of the pet's online/busy state. Never read content here.
    {
        let mut feedback = FEEDBACK.lock();
        let sequence = crate::desktop_clipboard::sequence();
        let changed = feedback.clipboard.observe(sequence);
        if cfg.companion.clipboard_hint && changed {
            let _ = app.emit("companion:clipboard-hint", json!({"sequence":sequence}));
        }
    }
    let now = chrono::Local::now();
    let (_process, connected, metrics) = state.world_provider.companion_observation();
    let quiet = quiet_reason(&state);
    let active = state.active_character_id.read().clone();
    let character = {
        let characters = state.characters.read();
        characters.get(&active).filter(|c| *c.online.read()).cloned().or_else(||
            characters.values().filter(|c| *c.online.read()).min_by_key(|c| &c.id).cloned())
    };
    let Some(character) = character.filter(|c| *c.online.read()) else { return; };
    let busy = character.think_lock.try_lock().is_err() || state.playback_gate.is_playing();
    let mut feedback = FEEDBACK.lock();
    let hourly = feedback.clock.observe(now.timestamp(), now.minute(), now.second());
    let cpu_entered = feedback.cpu.sample(now.timestamp(), metrics.as_ref().map(|m| m.cpu_usage), 85.0, 70.0);
    let memory_entered = feedback.memory.sample(now.timestamp(), metrics.as_ref().map(|m| m.memory_usage_pct), 90.0, 80.0);
    let network_changed = connected.is_some() && feedback.network.is_some() && connected != feedback.network;
    if connected.is_some() { feedback.network = connected; }
    let eligible = quiet.is_none() && !busy && crate::utils::get_system_idle_seconds().is_some_and(|s| s < 300.0);
    let zh = cfg.base.language.starts_with("zh");
    let ja = cfg.base.language.starts_with("ja");
    let mut messages = Vec::new();
    let (weather, music) = state.world_provider.companion_scene();
    if let Some(weather) = weather.filter(|w| now.timestamp() - w.cached_at < 7200) {
        let changed = feedback.rain.is_some_and(|old| old != weather.is_precipitating);
        feedback.rain = Some(weather.is_precipitating);
        if eligible && cfg.companion.weather_feedback && changed
            && feedback.last_weather_notice.is_none_or(|last| now.timestamp() - last >= 3600) {
            feedback.last_weather_notice = Some(now.timestamp());
            let text = if weather.is_precipitating {
                if zh { "天气数据报告正在降水，出门记得带伞。" } else if ja { "雨の情報が届いたよ。傘を忘れずに。" } else { "The weather report shows precipitation. Take an umbrella if heading out." }
            } else if zh { "天气数据报告降水停了。" } else if ja { "雨がやんだようだよ。" } else { "The weather report says precipitation has stopped." };
            messages.push(("weather", text.into(), if weather.is_precipitating { "umbrella" } else { "remind" }));
        }
    }
    let song = music.filter(|m| m.status == crate::world::PlaybackStatus::Playing && !m.title.trim().is_empty())
        .map(|m| format!("{} · {}", m.title, m.artist));
    if song != feedback.song || !feedback.music_initialized {
        let initialized = feedback.music_initialized;
        feedback.music_initialized = true;
        feedback.song = song.clone();
        if eligible && cfg.companion.music_feedback && initialized
            && feedback.last_music_notice.is_none_or(|last| now.timestamp() - last >= 600) {
            if let Some(song) = song {
                feedback.last_music_notice = Some(now.timestamp());
                let text = if zh { format!("正在听《{song}》，我也陪你听一会儿。") } else if ja { format!("「{song}」を一緒に聴こう。") } else { format!("Listening to {song}. I'll keep you company.") };
                messages.push(("music", text, "music"));
            }
        }
    }
    if eligible && cfg.companion.hourly_chime && hourly {
        messages.push(("hourly", if zh { format!("现在是 {} 点整。", now.hour()) }
            else if ja { format!("{}時になったよ。", now.hour()) }
            else { format!("It's {}:00.", now.hour()) }, "remind"));
    }
    if eligible && resource_feedback_enabled() && (cpu_entered || memory_entered)
        && feedback.last_pressure_notice.is_none_or(|last| now.timestamp() - last >= 1800) {
        feedback.last_pressure_notice = Some(now.timestamp());
        let text = if memory_entered {
            if zh { "内存有点紧张，可以看看有没有暂时不用的应用。" } else if ja { "メモリが混み合っているね。使っていないアプリを確認してみよう。" } else { "Memory is running low. Check for apps you aren't using." }
        } else if zh { "电脑持续有点忙，CPU 占用比较高。" } else if ja { "CPUの負荷がしばらく高くなっているね。" } else { "The CPU has been working hard for a while." };
        messages.push(("pressure", text.to_string(), "tired"));
    }
    if eligible && cfg.companion.network_feedback && network_changed
        && feedback.last_network_notice.is_none_or(|last| now.timestamp() - last >= 60) {
        feedback.last_network_notice = Some(now.timestamp());
        let restored = connected == Some(true);
        let text = match (zh, ja, restored) {
            (true, _, true) => "网络连接恢复了。", (true, _, false) => "系统报告网络已断开，联网功能可能暂时不可用。",
            (_, true, true) => "ネット接続が戻ったよ。", (_, true, false) => "ネット接続が切れているようだよ。",
            (_, _, true) => "The internet connection is back.", _ => "The system reports that the internet connection is offline.",
        };
        messages.push(("network", text.into(), "remind"));
    }
    drop(feedback);
    // One presentation per heartbeat. Don't stack several simultaneous status bubbles.
    if let Some((kind, content, motion)) = messages.into_iter().next() {
        let _ = app.emit("companion:feedback", json!({"character_id":character.id,
            "kind":kind,"content":content,"motion":motion,"sound":cfg.companion.sound && kind == "hourly"}));
    }
}

#[tauri::command]
pub fn companion_status(state: tauri::State<'_, Arc<AppState>>) -> serde_json::Value {
    let feedback = FEEDBACK.lock();
    json!({"quiet_reason": quiet_reason(&state), "cpu_pressure":feedback.cpu.active,
        "memory_pressure":feedback.memory.active,"network_connected":feedback.network})
}
