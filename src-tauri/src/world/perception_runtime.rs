//! Optional read-only collectors run off the reply/scheduler path. No input or speech.
use super::perception::{
    accessible_context, browser_process, clean_text, PerceptionStore, Reading,
};
use crate::state::AppState;
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Arc};

struct CollectionGuard(Arc<PerceptionStore>);
impl Drop for CollectionGuard {
    fn drop(&mut self) {
        self.0.collecting.store(false, Ordering::Release);
    }
}

pub(crate) fn tick(agent: &Arc<super::environment_agent::EnvironmentAgent>, state: &Arc<AppState>) {
    let world = state.config.read().get_all().world;
    let mut config = world.perception.bounded();
    config.enabled &= world.enable;
    let store = agent.perception.clone();
    let epoch = store.configure(config.clone());
    if !config.enabled {
        return;
    }
    let now = chrono::Utc::now().timestamp();
    let previous = store.last_sample.load(Ordering::Acquire);
    if now - previous < 10
        || store
            .last_sample
            .compare_exchange(previous, now, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        || store.collecting.swap(true, Ordering::AcqRel)
    {
        return;
    }
    let state = state.clone();
    tauri::async_runtime::spawn(async move {
        let _guard = CollectionGuard(store.clone());
        if state.is_factory_reset_in_progress() || state.is_rebuild_in_progress() {
            return;
        }
        if config.system_resources {
            let (at, system) = state.world_provider.observed_system();
            let data=system.filter(|system|system.memory_total>0).map(|system|json!({"cpu_percent":system.cpu_usage,"memory_percent":system.memory_usage_pct,
                "memory_used_bytes":system.memory_used,"memory_total_bytes":system.memory_total,
                "download_bytes_per_second":system.net_download_bps,"upload_bytes_per_second":system.net_upload_bps})).unwrap_or(Value::Null);
            let events = state
                .config
                .read()
                .get_all()
                .proactive
                .enable_system_pressure_trigger;
            store.resource_report(epoch, at, now, data, events);
            let last = store.last_process.load(Ordering::Acquire);
            if store.memory_pressure_at(now)
                && now - last >= 600
                && store
                    .last_process
                    .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                let provider = state.world_provider.clone();
                if let Ok(processes) =
                    tokio::task::spawn_blocking(move || provider.top_memory_processes(5)).await
                {
                    store.report(epoch,Reading::new("resources:processes","observed",chrono::Utc::now().timestamp(),45,
                        json!({"source":"sysinfo_process_memory_aggregate","exclude_companion_process":true,"top_memory_processes":processes})),None,false);
                }
            }
        }
        if config.power_display {
            store.report(
                epoch,
                Reading::new("power_display", "observed", now, 35, power_display()),
                None,
                false,
            );
        }
        store.report(epoch,Reading::new("input_activity","observed",now,25,json!({
            "system_input_idle_seconds":crate::utils::get_system_idle_seconds(),
            "game_input_aggregate":state.world_provider.environment.game_input_activity(),
            "meaning":"inactivity is not proof of absence or emotion; no keystroke contents are collected"})),None,false);
        let foreground = super::foreground_window::get_current_foreground_window();
        let external = foreground.pid != 0
            && foreground.pid != std::process::id()
            && !foreground.process.trim().is_empty()
            && !config.excludes(&foreground.process);
        if external {
            store.foreground(epoch,json!({"pid":foreground.pid,"process":clean_text(&foreground.process,80),"title":clean_text(&foreground.title,240)}),now);
        } else {
            store.foreground_unavailable(
                epoch,
                now,
                if foreground.pid == std::process::id() {
                    "companion_has_focus"
                } else {
                    "foreground_unavailable_or_excluded"
                },
            );
        }
        let sequence = crate::desktop_clipboard::sequence();
        let mut clipboard = json!({"sequence":(sequence!=0).then_some(sequence),"text_enabled":config.clipboard_text,
            "text":Value::Null,"note":"a revision proves only a clipboard change, not what was copied"});
        if config.clipboard_text && external {
            match tokio::task::spawn_blocking(crate::desktop_clipboard::read).await {
                Ok(Ok((read_sequence, text))) if read_sequence == sequence => {
                    clipboard["text"] = json!(clean_text(&text, config.text_limit.min(1800)))
                }
                _ => clipboard["text_status"] = json!("unavailable_or_changed"),
            }
        }
        store.report(
            epoch,
            Reading::new(
                "clipboard",
                if sequence == 0 {
                    "unavailable"
                } else {
                    "observed"
                },
                now,
                35,
                clipboard,
            ),
            None,
            false,
        );
        let last = store.last_native.load(Ordering::Acquire);
        if !external || now - last < config.sample_seconds as i64 {
            return;
        }
        store.last_native.store(now, Ordering::Release);
        let request = crate::desktop_native::Request::Perception {
            expected_pid: foreground.pid,
            include_text: config.foreground_text
                || (config.browser_context && browser_process(&foreground.process)),
            include_windows: config.visible_windows,
        };
        let desktop = crate::desktop_contract::DesktopConfig {
            operation_timeout_ms: 2500,
            ..Default::default()
        };
        let observation =
            crate::desktop_native::request("environment-observer".into(), request, desktop).await;
        let observed_at = chrono::Utc::now().timestamp();
        let latest = super::foreground_window::get_current_foreground_window();
        if latest.pid != foreground.pid || latest.title != foreground.title {
            store.foreground_unavailable(epoch, observed_at, "foreground_changed_during_sample");
            return;
        }
        match observation {
            Ok(reply) => {
                let mut selection = config.clone();
                selection.browser_context &= browser_process(&foreground.process);
                let content = accessible_context(&reply.data, &selection);
                let status = content["status"]
                    .as_str()
                    .unwrap_or("unavailable")
                    .to_string();
                let ttl = config.sample_seconds as i64 + 10;
                store.report(epoch,Reading::new("accessibility",&status,observed_at,ttl,json!({"window":{
                    "pid":foreground.pid,"process":foreground.process,"title":clean_text(&foreground.title,240)},"content":content})),None,false);
                if config.visible_windows {
                    let windows=reply.data["visible_windows"].as_array().into_iter().flatten().take(16).map(|window|json!({
                        "title":window["title"].as_str().map(|title|clean_text(title,160)),"pid":window["pid"],"minimized":window["minimized"],"foreground":window["foreground"]})).collect::<Vec<_>>();
                    store.report(
                        epoch,
                        Reading::new(
                            "visible_windows",
                            "observed",
                            observed_at,
                            ttl,
                            json!(windows),
                        ),
                        None,
                        false,
                    );
                }
            }
            Err(error) => store.report(
                epoch,
                Reading::new(
                    "accessibility",
                    "unavailable",
                    observed_at,
                    30,
                    json!({"error":clean_text(&error,180)}),
                ),
                None,
                false,
            ),
        }
    });
}

pub(crate) fn visual_report(
    agent: &super::environment_agent::EnvironmentAgent,
    owner: &str,
    description: &str,
    source: &str,
) {
    if !agent.perception.enabled() || description.trim().is_empty() {
        return;
    }
    let now = chrono::Utc::now().timestamp();
    let epoch = agent.perception.epoch.load(Ordering::Acquire);
    let mut reading = Reading::new(
        &format!("vision:{owner}:{source}"),
        "model_interpretation",
        now,
        120,
        json!({
        "source":source,"description":clean_text(description,2000),"scope":"one supplied image, not continuous live vision"}),
    );
    reading.owner = Some(owner.into());
    agent.perception.report(epoch, reading, None, false);
}

fn power_display() -> Value {
    #[cfg(windows)]
    {
        use windows::Win32::{
            System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS},
            UI::WindowsAndMessaging::*,
        };
        let mut power = SYSTEM_POWER_STATUS::default();
        let status = unsafe { GetSystemPowerStatus(&mut power) };
        let battery = if status.is_ok() {
            Some(
                json!({"ac_connected":match power.ACLineStatus{0=>Some(false),1=>Some(true),_=>None},
            "battery_percent":(power.BatteryLifePercent<=100 && power.BatteryFlag!=128).then_some(power.BatteryLifePercent),
            "charging":(power.BatteryFlag!=255).then_some(power.BatteryFlag&8!=0),"battery_saver":power.SystemStatusFlag==1}),
            )
        } else {
            None
        };
        json!({"source":"win32_power_and_display","power":battery,"display_count":unsafe{GetSystemMetrics(SM_CMONITORS)},
            "remote_session":unsafe{GetSystemMetrics(SM_REMOTESESSION)}!=0,
            "desktop_bounds":{"left":unsafe{GetSystemMetrics(SM_XVIRTUALSCREEN)},"top":unsafe{GetSystemMetrics(SM_YVIRTUALSCREEN)},
                "width":unsafe{GetSystemMetrics(SM_CXVIRTUALSCREEN)},"height":unsafe{GetSystemMetrics(SM_CYVIRTUALSCREEN)}}})
    }
    #[cfg(not(windows))]
    {
        json!({"status":"unavailable_on_this_platform"})
    }
}
